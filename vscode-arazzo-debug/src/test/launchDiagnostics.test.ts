import assert from "node:assert/strict";
import { test } from "node:test";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vm from "node:vm";
import type * as vscode from "vscode";

// Load the compiled extension modules with just the VS Code UI API supplied.
// Filesystem checks use real temporary files; no editor or server is started.
function setup() {
  const lines: string[] = [];
  const popups: string[] = [];
  const channel = {
    appendLine: (line: string) => lines.push(line), show: () => {},
  } as unknown as vscode.OutputChannel;
  const api = {
    window: { showErrorMessage: (message: string) => popups.push(message) },
    DebugAdapterExecutable: class {
      constructor(public command: string) {}
    },
  };
  function load<T>(name: string): T {
    const exports = {};
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, `../${name}.js`), "utf8"), {
      exports, process,
      require: (id: string) => id === "vscode" ? api : require(id),
    });
    return exports as T;
  }
  const { ArazzoAdapterDescriptorFactory } = load<typeof import("../adapterClient")>("adapterClient");
  const { ArazzoDebugConfigurationProvider } = load<typeof import("../debugConfigProvider")>("debugConfigProvider");
  return { lines, popups, channel, ArazzoAdapterDescriptorFactory, ArazzoDebugConfigurationProvider };
}

test("missing override explains the exact executable and how to build it", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "arazzo-launch-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = setup();
  const executable = path.join(root, "target/debug/arazzo-debug-adapter");
  const result = new env.ArazzoAdapterDescriptorFactory(root, env.channel).createDebugAdapterDescriptor({
    name: "NWS", configuration: { runtimeExecutable: executable },
  } as unknown as vscode.DebugSession);
  assert.equal(result, undefined);
  assert.ok(env.lines[0].includes(executable));
  assert.match(env.lines[0], /Cannot start "NWS".*cargo build -p arazzo-debug-adapter/);
  assert.equal(env.popups[0], env.lines[0]);
});

test("invalid working directory points to runtimeCwd", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "arazzo-launch-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = setup();
  const result = new env.ArazzoAdapterDescriptorFactory(root, env.channel).createDebugAdapterDescriptor({
    name: "NWS", configuration: { runtimeExecutable: "cargo", runtimeCwd: path.join(root, "missing") },
  } as unknown as vscode.DebugSession);
  assert.equal(result, undefined);
  assert.match(env.lines[0], /runtimeCwd.*existing accessible directory/);
});

test("missing bundled binary recommends reinstall and identifies its path", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "arazzo-launch-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const env = setup();
  new env.ArazzoAdapterDescriptorFactory(root, env.channel).createDebugAdapterDescriptor({
    name: "NWS", configuration: {},
  } as unknown as vscode.DebugSession);
  assert.match(env.lines[0], /bundled debug adapter is missing.*Reinstall/);
  assert.ok(env.lines[0].includes(path.join(root, "bin")));
});

test("bare PATH commands remain supported", () => {
  const env = setup();
  const result = new env.ArazzoAdapterDescriptorFactory("/unused", env.channel).createDebugAdapterDescriptor({
    name: "NWS", configuration: { runtimeExecutable: "cargo" },
  } as unknown as vscode.DebugSession);
  assert.ok(result);
  assert.equal(env.popups.length, 0);
});

test("missing spec is explained in both output and popup", () => {
  const env = setup();
  const result = new env.ArazzoDebugConfigurationProvider(env.channel)
    .resolveDebugConfiguration(undefined, { type: "arazzo", request: "launch", name: "NWS" });
  assert.equal(result, undefined);
  assert.match(env.lines[0], /Cannot start "NWS".*Open an .arazzo.yaml file.*spec.*launch.json/);
  assert.equal(env.popups[0], env.lines[0]);
});
