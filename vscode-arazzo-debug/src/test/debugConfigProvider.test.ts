import assert from "node:assert/strict";
import { test } from "node:test";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vm from "node:vm";
import type * as vscode from "vscode";

function setup(editorFile?: string) {
  const lines: string[] = [];
  const popups: string[] = [];
  let shown = false;
  const channel = {
    appendLine: (line: string) => lines.push(line),
    show: () => { shown = true; },
  } as unknown as vscode.OutputChannel;
  const api = {
    window: {
      activeTextEditor: editorFile ? { document: { fileName: editorFile } } : undefined,
      showErrorMessage: (message: string) => popups.push(message),
    },
  };
  const exports = {};
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../debugConfigProvider.js"), "utf8"), {
    exports,
    require: (id: string) => id === "vscode" ? api : require(id),
  });
  const { ArazzoDebugConfigurationProvider } = exports as typeof import("../debugConfigProvider");
  const provider = new ArazzoDebugConfigurationProvider(channel);
  function resolve(config: Record<string, unknown>) {
    const result = provider.resolveDebugConfiguration(undefined, config as vscode.DebugConfiguration);
    return result === undefined ? undefined : JSON.parse(JSON.stringify(result)) as Record<string, unknown>;
  }
  return { resolve, lines, popups, shown: () => shown };
}

test("provider fills launch defaults without mutating the supplied configuration", () => {
  const env = setup();
  const config = { spec: "/workflow.arazzo.yaml" };
  assert.deepEqual(env.resolve(config), {
    ...config, type: "arazzo", request: "launch", name: "Debug Arazzo Workflow",
    inputs: {}, stopOnEntry: false,
  });
  assert.deepEqual(config, { spec: "/workflow.arazzo.yaml" });
  assert.deepEqual(env.lines, []);
  assert.deepEqual(env.popups, []);
});

test("provider uses the active editor only when spec is missing", () => {
  const env = setup("/editor.arazzo.yaml");
  assert.equal(env.resolve({})?.spec, "/editor.arazzo.yaml");
  assert.equal(env.resolve({ spec: "/explicit.arazzo.yaml" })?.spec, "/explicit.arazzo.yaml");
});

test("provider preserves explicit fields and both boolean stopOnEntry values", () => {
  for (const stopOnEntry of [true, false]) {
    const env = setup();
    const config = {
      type: "custom", request: "attach", name: "Selected workflow", spec: "/chosen.yaml",
      inputs: { label: "chosen", count: 2 }, stopOnEntry,
    };
    const original = structuredClone(config);
    assert.deepEqual(env.resolve(config), original);
    assert.deepEqual(config, original);
  }
});

test("provider coerces nonboolean stopOnEntry to false", () => {
  for (const stopOnEntry of ["true", 1, null, {}, undefined]) {
    const config = { spec: "/chosen.yaml", stopOnEntry };
    assert.equal(setup().resolve(config)?.stopOnEntry, false);
    assert.equal(config.stopOnEntry, stopOnEntry);
  }
});

test("provider rejects missing spec with an actionable output error and popup", () => {
  const env = setup();
  assert.equal(env.resolve({ name: "Missing workflow" }), undefined);
  assert.equal(env.lines.length, 1);
  assert.match(env.lines[0], /Cannot start "Missing workflow".*Open an .arazzo.yaml file.*spec.*launch.json/);
  assert.deepEqual(env.popups, env.lines);
  assert.equal(env.shown(), true);
});
