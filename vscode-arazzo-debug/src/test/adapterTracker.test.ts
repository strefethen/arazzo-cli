import assert from "node:assert/strict";
import { test } from "node:test";
import type * as vscode from "vscode";
import { ArazzoAdapterTrackerFactory } from "../adapterTracker";

function createTracker() {
  const lines: string[] = [];
  const channel = {
    appendLine: (line: string) => lines.push(line),
    append: (line: string) => lines.push(line),
    show: () => {},
  } as unknown as vscode.OutputChannel;
  const tracker = new ArazzoAdapterTrackerFactory(channel)
    .createDebugAdapterTracker({ name: "NWS", configuration: { spec: "/nws.yaml", workflowId: "address-forecast" } } as unknown as vscode.DebugSession) as vscode.DebugAdapterTracker;
  return { tracker, lines };
}

test("normal termination does not report stdout close as an adapter failure", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  tracker.onError?.(new Error("read error"));
  assert.equal(lines.length, 1);
  assert.match(lines[0], /workflow ended; adapter did not report an exit status/);
});

test("user stop does not report stdout close as an adapter failure", () => {
  const { tracker, lines } = createTracker();
  tracker.onWillStopSession?.();
  tracker.onError?.(new Error("read error"));
  assert.equal(lines.length, 1);
  assert.match(lines[0], /session ended/);
});

test("successful completion and disconnect produce one unambiguous outcome", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({ type: "event", event: "exited", body: { exitCode: 0 } });
  tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  tracker.onWillReceiveMessage?.({ type: "request", command: "disconnect" });
  tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  tracker.onError?.(new Error("read error"));
  assert.equal(lines.length, 1);
  assert.match(lines[0], /workflow completed successfully \(exit code 0\)/);
});

test("unresolved outputs are visible and qualify the completion summary", () => {
  const { tracker, lines } = createTracker();
  const diagnostic = 'unresolved output value: "first_alert_event" expression: "$response.body#/features/0/properties/event" in workflow: "forecast", step: "check-active-alerts".\n';
  tracker.onDidSendMessage?.({ type: "event", event: "output", body: {
    category: "console", output: diagnostic, data: { kind: "unresolvedOutput" },
  } });
  tracker.onDidSendMessage?.({ type: "event", event: "exited", body: { exitCode: 0 } });
  tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  assert.equal(lines.length, 2);
  assert.ok(lines[0].includes(diagnostic));
  assert.match(lines[1], /completed with 1 unresolved output/);
  assert.ok(!lines.some((line) => line.includes("completed successfully")));
});

test("failed completion and requested stop are not reported as success", () => {
  const failed = createTracker();
  failed.tracker.onDidSendMessage?.({ type: "event", event: "exited", body: { exitCode: 1 } });
  failed.tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  assert.equal(failed.lines.length, 1);
  assert.match(failed.lines[0], /workflow failed \(exit code 1\)/);
  const stopped = createTracker();
  stopped.tracker.onWillReceiveMessage?.({ type: "request", command: "disconnect" });
  stopped.tracker.onDidSendMessage?.({ type: "event", event: "terminated" });
  assert.match(stopped.lines[0], /debug session stopped by request/);
});

test("unexpected stream close includes last request and source location", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({
    type: "response", command: "stackTrace", success: true,
    body: { stackFrames: [{ name: "address-forecast::check-active-alerts", line: 155, source: { path: "/spec.yaml" } }] },
  });
  tracker.onWillReceiveMessage?.({ type: "request", command: "evaluate" });
  tracker.onError?.(new Error("read error"));
  assert.match(lines[0], /output stream closed unexpectedly/);
  assert.match(lines[0], /last request: evaluate/);
  assert.match(lines[0], /spec: \/nws.yaml; workflow: address-forecast/);
  assert.match(lines[0], /check-active-alerts.*\/spec.yaml:155/);
});

test("DAP error details preserve substituted diagnostics", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({
    type: "response", command: "launch", success: false, message: "cancelled",
    body: { error: { format: "Cannot open {file}: {reason}", variables: { file: "/nws.yaml", reason: "permission denied" } } },
  });
  assert.match(lines[0], /launch failed: Cannot open \/nws.yaml: permission denied/);
});

test("expected stop signals are lifecycle messages", () => {
  const { tracker, lines } = createTracker();
  tracker.onWillReceiveMessage?.({ type: "request", command: "disconnect" });
  tracker.onExit?.(undefined, "SIGTERM");
  tracker.onError?.(new Error("read error"));
  assert.equal(lines.length, 1);
  assert.match(lines[0], /adapter stopped by request/);
});

test("exit zero without termination is still diagnosed", () => {
  const { tracker, lines } = createTracker();
  tracker.onExit?.(0, undefined);
  assert.match(lines[0], /without a workflow termination event/);
});

test("runtime error output retains its cause and session context", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({ type: "event", event: "output", body: { category: "stderr", output: "workflow failed [RUNTIME_TEST]: step fetch failed\n" } });
  assert.match(lines[0], /RUNTIME_TEST.*step fetch failed/);
  assert.match(lines[1], /failure context: spec: \/nws.yaml; workflow: address-forecast/);
});

test("failed requests and signal exits retain their diagnostics after termination", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({ type: "response", command: "next", success: false, message: "engine already terminated" });
  tracker.onWillStopSession?.();
  tracker.onExit?.(undefined, "SIGABRT");
  tracker.onError?.(new Error("write failed"));
  assert.match(lines[0], /next failed: engine already terminated/);
  assert.match(lines[2], /SIGABRT/);
  assert.match(lines[3], /write failed/);
});

test("unverified breakpoint reasons reach the output channel", () => {
  const { tracker, lines } = createTracker();
  tracker.onDidSendMessage?.({
    type: "response", command: "setBreakpoints", success: true,
    body: { breakpoints: [
      { verified: true, line: 10 },
      { verified: false, line: 20, message: 'invalid breakpoint condition: "$inputs.alerts ==" at /nws.yaml:20: expected operand' },
    ] },
  });
  assert.equal(lines.length, 1);
  assert.match(lines[0], /breakpoint not installed/);
  assert.match(lines[0], /\$inputs.alerts ==/);
  assert.match(lines[0], /\/nws.yaml:20: expected operand/);
});
