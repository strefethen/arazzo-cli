import assert from "node:assert/strict";
import { test } from "node:test";
import * as fs from "node:fs";
import * as path from "node:path";
import * as vm from "node:vm";
import type * as vscode from "vscode";

function setup(state = new Map<string, unknown>(), answer?: string) {
  const commands = new Map<string, () => Promise<void>>();
  const prompts: string[][] = [];
  const urls: string[] = [];
  const errors: string[] = [];
  let terminate: (session: vscode.DebugSession) => void = () => {};
  const api = {
    version: "1.100.0",
    commands: { registerCommand: (name: string, callback: () => Promise<void>) => {
      commands.set(name, callback);
      return { dispose() {} };
    } },
    debug: { onDidTerminateDebugSession: (callback: typeof terminate) => {
      terminate = callback;
      return { dispose() {} };
    } },
    env: { openExternal: async (uri: string) => { urls.push(uri); return true; } },
    Uri: { parse: (uri: string) => uri },
    window: {
      showInformationMessage: async (...args: string[]) => { prompts.push(args); return answer; },
      showErrorMessage: async (message: string) => { errors.push(message); },
    },
  };
  const context = {
    extension: { packageJSON: { version: "0.0.9" } },
    subscriptions: [],
    globalState: {
      get: (key: string, fallback: unknown) => state.has(key) ? state.get(key) : fallback,
      update: async (key: string, value: unknown) => { state.set(key, value); },
    },
  } as unknown as vscode.ExtensionContext;
  const exports = {} as typeof import("../feedback");
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, "../feedback.js"), "utf8"), {
    exports, URLSearchParams,
    require: (id: string) => id === "vscode" ? api : require(id),
  });
  exports.registerFeedback(context);
  return {
    commands, prompts, urls, errors, api, context, state,
    end: (id: string, type = "arazzo", noDebug = false) => terminate({
      id, type, configuration: { noDebug, spec: "/private/workflow.yaml", inputs: { token: "secret" } },
    } as unknown as vscode.DebugSession),
    settle: () => new Promise<void>((resolve) => setImmediate(resolve)),
  };
}

test("both commands open distinct reviewable issue drafts containing only environment details", async () => {
  const env = setup();
  for (const name of ["arazzo.sendFeedback", "arazzo.reportBug"]) {
    await env.commands.get(name)!();
  }
  assert.equal(env.urls.length, 2);
  for (const [index, uri] of env.urls.entries()) {
    const url = new URL(uri);
    assert.equal(url.origin + url.pathname, "https://github.com/strefethen/arazzo-cli/issues/new");
    assert.match(url.searchParams.get("title")!, index === 0 ? /feedback/ : /bug/);
    const body = url.searchParams.get("body")!;
    assert.match(body, /Arazzo Debugger: 0.0.9/);
    assert.match(body, /VS Code: 1.100.0/);
    assert.match(body, /OS: .+/);
    assert.doesNotMatch(body, /secret|private|workflow.yaml/);
    if (index === 1) assert.match(body, /Steps to reproduce/);
  }
  assert.equal(env.prompts.length, 0);
});

test("third distinct debug session invites once, including across extension reloads", async () => {
  const env = setup();
  env.end("other", "python");
  env.end("run", "arazzo", true);
  env.end("one");
  env.end("one");
  env.end("two");
  await env.settle();
  assert.equal(env.prompts.length, 0);
  assert.equal(env.state.get("feedback.completedSessions"), 2);
  const reloaded = setup(env.state);
  reloaded.end("three");
  reloaded.end("four");
  await reloaded.settle();
  assert.equal(reloaded.prompts.length, 1);
  assert.equal(reloaded.prompts[0][1], "Send Feedback");
  assert.equal(reloaded.prompts[0][2], "Don’t Ask Again");
  assert.equal(reloaded.urls.length, 0);
  const again = setup(env.state);
  again.end("five");
  await again.settle();
  assert.equal(again.prompts.length, 0);
});

test("sending feedback and opting out both permanently dismiss the invitation", async () => {
  for (const answer of ["Send Feedback", "Don’t Ask Again"]) {
    const env = setup(new Map([["feedback.completedSessions", 2]]), answer);
    env.end("three");
    await env.settle();
    assert.equal(env.urls.length, answer === "Send Feedback" ? 1 : 0);
    env.end("four");
    await env.settle();
    assert.equal(env.prompts.length, 1);
    await env.commands.get("arazzo.reportBug")!();
    assert.equal(env.urls.length, answer === "Send Feedback" ? 2 : 1);
  }
});

test("failed browser launches report an actionable error", async () => {
  const env = setup();
  env.api.env.openExternal = async () => false;
  await env.commands.get("arazzo.sendFeedback")!();
  assert.match(env.errors[0], /Could not open GitHub.*https:\/\/github.com/);
});

test("storage failure reports an error and does not poison subsequent session handling", async () => {
  const env = setup();
  const update = env.context.globalState.update;
  env.context.globalState.update = async () => { throw new Error("Storage unavailable"); };
  env.end("one");
  await env.settle();
  assert.match(env.errors[0], /Storage unavailable/);
  assert.equal(env.prompts.length, 0);
  env.context.globalState.update = update;
  env.end("two");
  await env.settle();
  assert.equal(env.state.get("feedback.completedSessions"), 1);
});
