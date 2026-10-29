import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn, ChildProcessWithoutNullStreams } from "node:child_process";
import * as fs from "node:fs";
import * as http from "node:http";
import * as os from "node:os";
import * as path from "node:path";

const PHASE_TIMEOUT = 5_000;
const BUILD_COMMAND = "cargo build -p arazzo-debug-adapter --locked";
type ObjectValue = Record<string, unknown>;
interface DapMessage {
  type: string;
  command?: string;
  request_seq?: number;
  success?: boolean;
  event?: string;
  body?: ObjectValue;
}

function object(value: unknown): ObjectValue {
  assert.ok(value !== null && typeof value === "object" && !Array.isArray(value));
  return value as ObjectValue;
}

function objects(value: unknown): ObjectValue[] {
  assert.ok(Array.isArray(value));
  return value.map(object);
}

function numeric(value: unknown): number {
  assert.equal(typeof value, "number");
  return value as number;
}

// This ledger belongs only to the native transcript below. A waiter first checks
// recorded messages, so an event delivered alongside a response is never lost.
class NativeTranscript {
  readonly child: ChildProcessWithoutNullStreams;
  readonly messages: DapMessage[] = [];
  private buffer: Buffer = Buffer.alloc(0);
  private readonly consumedEvents = new Set<DapMessage>();
  private readonly listeners = new Set<() => void>();
  private seq = 0;
  private stderr = "";
  private failure?: Error;
  private closed?: { code: number | null; signal: NodeJS.Signals | null };
  private expectClose = false;

  constructor(binary: string) {
    this.child = spawn(binary, [], { stdio: "pipe" });
    this.child.stderr.on("data", (chunk: Buffer) => {
      this.stderr = (this.stderr + chunk.toString("utf8")).slice(-65_536);
    });
    this.child.stdout.on("data", (chunk: Buffer) => {
      try {
        this.buffer = Buffer.concat([this.buffer, chunk]);
        while (true) {
          const headerEnd = this.buffer.indexOf("\r\n\r\n");
          if (headerEnd < 0) { break; }
          const header = this.buffer.subarray(0, headerEnd).toString("ascii");
          const match = /^Content-Length:\s*(\d+)\s*$/im.exec(header);
          assert.ok(match, `Missing Content-Length in ${header}`);
          const length = Number(match[1]);
          const payloadStart = headerEnd + 4;
          if (this.buffer.length < payloadStart + length) { break; }
          const payload = this.buffer.subarray(payloadStart, payloadStart + length);
          this.buffer = this.buffer.subarray(payloadStart + length);
          const message = object(JSON.parse(payload.toString("utf8")));
          assert.ok(message.type === "response" || message.type === "event");
          this.messages.push(message as unknown as DapMessage);
        }
        this.notify();
      } catch (error) {
        this.fail(error);
      }
    });
    this.child.on("error", (error) => this.fail(new Error(`${error.message}; run '${BUILD_COMMAND}' first`)));
    this.child.stdin.on("error", (error) => this.fail(error));
    this.child.on("close", (code, signal) => {
      this.closed = { code, signal };
      if (!this.expectClose) { this.fail(new Error(`Adapter closed unexpectedly: ${code ?? signal}`)); }
      this.notify();
    });
  }

  private notify() {
    for (const listener of this.listeners) { listener(); }
  }

  private fail(error: unknown) {
    this.failure ??= error instanceof Error ? error : new Error(String(error));
    this.notify();
  }

  diagnostics(): string {
    return `stderr:\n${this.stderr}\nRecent DAP messages:\n${JSON.stringify(this.messages.slice(-15), null, 2)}`;
  }

  private wait<T>(label: string, find: () => T | undefined, closing = false): Promise<T> {
    return new Promise((resolve, reject) => {
      const finish = (error?: Error, value?: T) => {
        clearTimeout(timer);
        this.listeners.delete(check);
        if (error) { reject(new Error(`${label}: ${error.message}\n${this.diagnostics()}`)); }
        else { resolve(value as T); }
      };
      const check = () => {
        const value = find();
        if (value !== undefined) { finish(undefined, value); }
        else if (this.failure && !closing) { finish(this.failure); }
        else if (this.closed && !closing) { finish(new Error("Adapter closed before the expected message")); }
      };
      const timer = setTimeout(() => finish(new Error(`Timed out after ${PHASE_TIMEOUT}ms`)), PHASE_TIMEOUT);
      this.listeners.add(check);
      check();
    });
  }

  async request(command: string, args: ObjectValue = {}): Promise<ObjectValue> {
    const seq = ++this.seq;
    const payload = Buffer.from(JSON.stringify({ seq, type: "request", command, arguments: args }), "utf8");
    this.child.stdin.write(Buffer.concat([Buffer.from(`Content-Length: ${payload.length}\r\n\r\n`), payload]));
    const response = await this.wait(`${command} response #${seq}`, () => this.messages.find((message) =>
      message.type === "response" && message.request_seq === seq && message.command === command));
    assert.equal(response.success, true, `${command} failed\n${this.diagnostics()}`);
    return object(response.body);
  }

  async event(name: string): Promise<ObjectValue> {
    const message = await this.wait(`${name} event`, () => this.messages.find((candidate) =>
      candidate.type === "event" && candidate.event === name && !this.consumedEvents.has(candidate)));
    this.consumedEvents.add(message);
    return object(message.body ?? {});
  }

  async disconnect() {
    this.expectClose = true;
    await this.request("disconnect");
    const closed = await this.wait("adapter close", () => this.closed, true);
    assert.deepEqual(closed, { code: 0, signal: null }, this.diagnostics());
    assert.equal(this.buffer.length, 0, "Adapter closed with an incomplete DAP frame");
  }

  async cleanup() {
    this.expectClose = true;
    if (!this.closed) {
      this.child.kill("SIGTERM");
      try { await this.wait("SIGTERM cleanup", () => this.closed, true); }
      catch {
        this.child.kill("SIGKILL");
        await this.wait("SIGKILL cleanup", () => this.closed, true);
      }
    }
    this.child.stdin.destroy();
    this.child.stdout.destroy();
    this.child.stderr.destroy();
    this.child.removeAllListeners();
    this.listeners.clear();
  }
}

test("native DAP executes HTTP between entry and second-step breakpoint, then exits naturally", { timeout: 30_000 }, async (t) => {
  let root: string | undefined;
  let transcript: NativeTranscript | undefined;
  let server: http.Server | undefined;
  t.after(async () => {
    try {
      await transcript?.cleanup();
    } finally {
      try {
        if (server?.listening) {
          const closingServer = server;
          await new Promise<void>((resolve, reject) => {
            const timer = setTimeout(() => reject(new Error("HTTP fixture cleanup timed out")), PHASE_TIMEOUT);
            closingServer.close((error) => {
              clearTimeout(timer);
              if (error) { reject(error); } else { resolve(); }
            });
            closingServer.closeAllConnections();
          });
        }
      } finally {
        if (root) { fs.rmSync(root, { recursive: true, force: true }); }
      }
    }
  });

  const binary = path.resolve(__dirname, "../../../target/debug", `arazzo-debug-adapter${os.platform() === "win32" ? ".exe" : ""}`);
  assert.doesNotThrow(() => fs.accessSync(binary, fs.constants.X_OK),
    `Native adapter is missing or unexecutable at ${binary}; run '${BUILD_COMMAND}' from the repository root`);
  root = fs.mkdtempSync(path.join(os.tmpdir(), "arazzo-dap-"));
  const received: string[] = [];
  server = http.createServer((request, response) => {
    received.push(`${request.method} ${request.url}`);
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(JSON.stringify({ step: request.url, label: "café" }));
  });
  await new Promise<void>((resolve, reject) => {
    const fixture = server as http.Server;
    const timer = setTimeout(() => { fixture.close(); reject(new Error("HTTP fixture listen timed out")); }, PHASE_TIMEOUT);
    const onError = (error: Error) => { clearTimeout(timer); reject(error); };
    fixture.once("error", onError);
    fixture.listen(0, "127.0.0.1", () => {
      clearTimeout(timer);
      fixture.off("error", onError);
      resolve();
    });
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  fs.writeFileSync(path.join(root, "openapi.json"), JSON.stringify({
    openapi: "3.0.3", info: { title: "DAP transcript fixture", version: "1.0.0" },
    servers: [{ url: `http://127.0.0.1:${address.port}` }],
    paths: Object.fromEntries(["first", "second"].map((id) => [`/${id}`, {
      get: { operationId: id, responses: { "200": { description: "Success" } } },
    }])),
  }));
  const yaml = [
    "arazzo: 1.1.0",
    "info:", "  title: Native DAP transcript", "  version: 1.0.0",
    "sourceDescriptions:", "  - name: local", "    url: ./openapi.json", "    type: openapi",
    "workflows:", "  - workflowId: transcript",
    "    inputs:", "      type: object", "      properties:", "        label:", "          type: string",
    "    steps:", "      - stepId: first-step", "        operationId: first",
    "      - stepId: second-step", "        operationId: second", "",
  ].join("\n");
  const spec = path.join(root, "workflow.arazzo.yaml");
  fs.writeFileSync(spec, yaml);
  const breakpointLine = yaml.split("\n").findIndex((line) => line.trim() === "- stepId: second-step") + 1;
  assert.ok(breakpointLine > 0);
  transcript = new NativeTranscript(binary);
  try {
    const capabilities = await transcript.request("initialize", { clientID: "arazzo-node-transcript", adapterID: "arazzo", linesStartAt1: true, columnsStartAt1: true });
    assert.equal(capabilities.supportsConfigurationDoneRequest, true);
    await transcript.event("initialized");
    await transcript.request("launch", { spec, workflowId: "transcript", stopOnEntry: true, inputs: { label: "transcript ✓" } });
    const breakpoints = objects((await transcript.request("setBreakpoints", {
      source: { path: spec }, breakpoints: [{ line: breakpointLine }],
    })).breakpoints);
    assert.equal(breakpoints.length, 1);
    assert.equal(breakpoints[0].verified, true);
    assert.equal(breakpoints[0].line, breakpointLine);
    await transcript.request("configurationDone");
    const entry = await transcript.event("stopped");
    assert.equal(entry.reason, "pause");
    assert.deepEqual(received, []);

    await transcript.request("continue", { threadId: numeric(entry.threadId) });
    const breakpoint = await transcript.event("stopped");
    assert.equal(breakpoint.reason, "breakpoint");
    assert.deepEqual(received, ["GET /first"]);
    const stack = await transcript.request("stackTrace", { threadId: numeric(breakpoint.threadId) });
    const frames = objects(stack.stackFrames);
    assert.ok(frames.length > 0);
    const frame = frames[0];
    assert.equal(object(frame.source).path, spec);
    assert.equal(frame.line, breakpointLine);
    assert.equal(frame.name, "transcript::second-step");
    const scopes = objects((await transcript.request("scopes", { frameId: numeric(frame.id) })).scopes);
    async function variablesFor(name: string) {
      const scope = scopes.find((candidate) => candidate.name === name);
      assert.ok(scope, `Missing ${name} scope`);
      const variablesReference = numeric(scope.variablesReference);
      assert.ok(variablesReference > 0);
      return objects((await transcript!.request("variables", { variablesReference })).variables);
    }
    const locals = await variablesFor("Locals");
    assert.equal(locals.find((variable) => variable.name === "workflowId")?.value, "transcript");
    assert.equal(locals.find((variable) => variable.name === "stepId")?.value, "second-step");
    const inputs = await variablesFor("Inputs");
    assert.equal(inputs.find((variable) => variable.name === "label")?.value, "transcript ✓");

    await transcript.request("continue", { threadId: numeric(breakpoint.threadId) });
    assert.equal((await transcript.event("exited")).exitCode, 0);
    await transcript.event("terminated");
    assert.deepEqual(received, ["GET /first", "GET /second"]);
    await transcript.disconnect();
    assert.deepEqual(transcript.messages
      .filter((message) => message.type === "event" && (message.event === "exited" || message.event === "terminated"))
      .map((message) => message.event), ["exited", "terminated"]);
  } catch (error) {
    throw new Error(`${error instanceof Error ? error.stack : String(error)}\n${transcript.diagnostics()}`);
  }
});
