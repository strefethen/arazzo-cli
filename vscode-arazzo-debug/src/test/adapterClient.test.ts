import assert from "node:assert/strict";
import { test, TestContext } from "node:test";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vm from "node:vm";
import type * as vscode from "vscode";

function setup(t: TestContext, workspacePath?: string) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "arazzo-descriptor-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const lines: string[] = [];
  const popups: string[] = [];
  const channel = {
    appendLine: (line: string) => lines.push(line), show: () => {},
  } as unknown as vscode.OutputChannel;
  class Executable {
    constructor(
      public command: string,
      public args: string[],
      public options: vscode.DebugAdapterExecutableOptions,
    ) {}
  }
  const api = {
    workspace: { workspaceFolders: workspacePath ? [{ uri: { fsPath: workspacePath } }] : undefined },
    window: { showErrorMessage: (message: string) => popups.push(message) },
    DebugAdapterExecutable: Executable,
  };
  const exports = {};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../adapterClient.js"), "utf8"), {
    exports, process,
    require: (id: string) => id === "vscode" ? api : require(id),
  });
  const { ArazzoAdapterDescriptorFactory } = exports as typeof import("../adapterClient");
  const factory = new ArazzoAdapterDescriptorFactory(root, channel);
  function descriptor(configuration: Record<string, unknown>) {
    const result = factory.createDebugAdapterDescriptor({
      name: "Test workflow", configuration,
    } as unknown as vscode.DebugSession);
    assert.ok(result instanceof Executable, `Expected executable descriptor: ${lines.join("\n")}`);
    // Values cross a VM boundary; compare every field with host-realm containers.
    return { command: result.command, args: Array.from(result.args), options: { cwd: result.options.cwd } };
  }
  function executable(relativePath: string) {
    const target = path.join(root, relativePath);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, "test executable fixture\n", { mode: 0o755 });
    return target;
  }
  return { root, descriptor, executable, popups };
}

test("factory preserves override command and cwd while filtering mixed arguments", (t) => {
  const env = setup(t);
  const command = env.executable("custom-adapter");
  assert.deepEqual(env.descriptor({
    runtimeExecutable: command, runtimeArgs: ["--stdio", 42, null, "", "chosen"], runtimeCwd: env.root,
  }), { command, args: ["--stdio", "", "chosen"], options: { cwd: env.root } });
  assert.deepEqual(env.popups, []);
});

test("factory allows bare PATH overrides and normalizes absent or unusable arguments", (t) => {
  const env = setup(t);
  for (const runtimeArgs of [undefined, "--stdio", [], [null, 2], {}]) {
    assert.deepEqual(env.descriptor({ runtimeExecutable: "arazzo-on-PATH", runtimeArgs }), {
      command: "arazzo-on-PATH", args: [], options: { cwd: undefined },
    });
  }
});

test("factory validates relative override paths against runtimeCwd and preserves the command", (t) => {
  const env = setup(t);
  env.executable("tools/adapter");
  const command = path.join("tools", "adapter");
  assert.deepEqual(env.descriptor({ runtimeExecutable: command, runtimeCwd: env.root, runtimeArgs: ["--stdio"] }), {
    command, args: ["--stdio"], options: { cwd: env.root },
  });
});

test("factory selects the platform bundled executable and workspace cwd for invalid overrides", (t) => {
  const workspacePath = path.join(os.tmpdir(), "arazzo-workspace");
  const env = setup(t, workspacePath);
  const command = env.executable(`bin/arazzo-debug-adapter${os.platform() === "win32" ? ".exe" : ""}`);
  for (const runtimeExecutable of [undefined, "", 12, null, {}]) {
    assert.deepEqual(env.descriptor({ runtimeExecutable, spec: "/spec/selected.yaml", runtimeArgs: ["ignored"] }), {
      command, args: [], options: { cwd: workspacePath },
    });
  }
});

test("factory uses the spec directory without workspace folders and leaves absent cwd undefined", (t) => {
  const env = setup(t);
  const command = env.executable(`bin/arazzo-debug-adapter${os.platform() === "win32" ? ".exe" : ""}`);
  const spec = path.join(env.root, "specs", "workflow.yaml");
  assert.deepEqual(env.descriptor({ spec }), {
    command, args: [], options: { cwd: path.dirname(spec) },
  });
  assert.deepEqual(env.descriptor({}), { command, args: [], options: { cwd: undefined } });
});
