# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-1.82+-000000?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**Execute multi-step API workflows from a YAML spec — no code generation, no glue scripts.**

arazzo-cli is a standalone executor for [Arazzo](https://spec.openapis.org/arazzo/latest.html), the OpenAPI Initiative's spec for describing sequences of API calls as declarative workflows. Define your steps, parameters, success criteria, and control flow in YAML, then run them directly from the command line or step through them in VS Code.

## Why?

Testing a sequence of API calls today means writing imperative scripts, maintaining Postman collections, or building custom test harnesses. The Arazzo spec (part of the OpenAPI ecosystem) lets you describe these sequences declaratively — but without a runtime, the spec is just documentation.

arazzo-cli makes Arazzo specs executable: validate them, run them, trace them, and debug them interactively.

## Quick Start

```bash
git clone https://github.com/strefethen/arazzo-cli.git
cd arazzo-cli
cargo run -p arazzo-cli -- validate examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- run examples/httpbin-get.arazzo.yaml get-origin
```

Or install it:

```bash
cargo install --path ./crates/arazzo-cli --locked
arazzo validate examples/httpbin-get.arazzo.yaml
arazzo run examples/httpbin-get.arazzo.yaml get-origin
```

## Features

| Feature | What it does |
|---|---|
| **Run workflows** | Execute HTTP steps, resolve expressions, evaluate success criteria, route control flow |
| **Validate specs** | Parse and structurally validate Arazzo YAML before running |
| **Parallel execution** | Run independent steps concurrently with `--parallel` |
| **Dry-run mode** | Resolve all requests without sending them (`--dry-run`) |
| **Execution traces** | Write detailed `trace.v1` JSON artifacts with automatic sensitive value redaction |
| **Sub-workflows** | Call workflows from workflows with input/output passing |
| **VS Code debugger** | Set breakpoints, step through workflows, inspect variables, evaluate expressions |
| **JSON output** | `--json` on every command for scripting and CI integration |
| **Expression language** | `$inputs`, `$steps`, `$response`, `$env`, XPath, JSON Pointer, interpolation |
| **Multiple API sources** | Route steps to different APIs via `sourceDescriptions` |

## Contents

- [CLI Commands](#cli-commands)
- [Examples](#examples)
- [Execution Traces](#execution-traces)
- [VS Code Debugger](#vs-code-debugger)
- [Expression Language](#expression-language)
- [Repository Layout](#repository-layout)
- [Building from Source](#building-from-source)
- [Contributing](#contributing)

## CLI Commands

```
arazzo run <spec> <workflow-id>     Execute a workflow
arazzo validate <spec>              Parse and validate a spec
arazzo list <spec>                  List workflows in a spec
arazzo catalog <dir>                Discover specs across a directory tree
arazzo show <workflow-id> --dir <dir>  Display workflow details
arazzo schema [command]             Print JSON Schema for a command's --json output
```

Global flags:

- `--json` — structured JSON output (all commands)
- `--verbose` — step-by-step execution details

`run` flags:

- `--input key=value` — workflow input (repeatable)
- `--input-json key=<json>` — JSON-typed input (repeatable)
- `--header Name=value` — HTTP header applied to all requests (repeatable)
- `--http-timeout <duration>` — per-request timeout (default `30s`)
- `--execution-timeout <duration>` — overall workflow deadline (default `5m`)
- `--parallel` — execute independent steps concurrently
- `--dry-run` — resolve requests without sending
- `--openapi <path>` — operationId source spec (repeatable)
- `--expr-diagnostics <off|warn|error>` — expression warning level (default `off`)
- `--trace <path>` — write a trace.v1 execution artifact
- `--trace-max-body-bytes <n>` — max body size in trace (default `2048`)

## Examples

The `examples/` directory contains 14 runnable specs using httpbin.org:

| Spec | Demonstrates |
|---|---|
| `httpbin-get.arazzo.yaml` | Basic GET, headers, status codes, inputs |
| `httpbin-methods.arazzo.yaml` | POST, PUT, PATCH, DELETE with JSON bodies |
| `httpbin-auth.arazzo.yaml` | Basic auth, bearer tokens, auth failure handling |
| `httpbin-conditions.arazzo.yaml` | Comparison operators, contains, compound conditions |
| `httpbin-data-flow.arazzo.yaml` | Output chaining, interpolation, cookies, sub-workflows |
| `httpbin-error-handling.arazzo.yaml` | Retry, criteria-based goto, workflow-level failure actions |
| `httpbin-parallel.arazzo.yaml` | Parallel execution, diamond dependencies |
| `httpbin-response-headers.arazzo.yaml` | Reading and forwarding response headers |
| `httpbin-components.arazzo.yaml` | Reusable parameters and actions via components |
| `httpbin-chained-posts.arazzo.yaml` | Multi-step POST body chaining, onSuccess goto |

Try them:

```bash
# Validate
arazzo validate examples/httpbin-auth.arazzo.yaml

# Run with inputs
arazzo run examples/httpbin-get.arazzo.yaml status-check --input code=200

# Dry-run (no network calls)
arazzo run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429

# Verbose output with step details
arazzo run examples/httpbin-parallel.arazzo.yaml independent-steps --parallel --verbose

# Write a trace file
arazzo run examples/httpbin-get.arazzo.yaml status-check --input code=429 --trace ./trace.json
```

## Execution Traces

`run --trace <path>` writes a `trace.v1` JSON artifact capturing every step's request, response, criteria evaluation, and routing decision.

Sensitive values are automatically redacted:
- Headers: `Authorization`, `Cookie`, `X-API-Key`
- URL query params: `token`, `password`, `session`
- JSON body fields with sensitive keys

```json
{
  "schemaVersion": "trace.v1",
  "tool": { "name": "arazzo", "version": "0.1.0" },
  "run": {
    "workflowId": "status-check",
    "status": "success",
    "durationMs": 12
  },
  "steps": [
    {
      "seq": 1,
      "workflowId": "status-check",
      "stepId": "check-status",
      "decision": { "path": "next" }
    }
  ]
}
```

Schema reference: `docs/trace-schema-v1.md` | `docs/schemas/trace-v1.schema.json`

## VS Code Debugger

The project includes a full Debug Adapter Protocol (DAP) implementation with a VS Code extension for interactive workflow debugging.

**Capabilities:** breakpoints on steps/criteria/actions, conditional breakpoints, Step Over / Step In / Step Out / Continue / Pause, variable inspection (Locals, Request, Response, Inputs, Steps scopes), watch expressions, call stack with sub-workflow depth tracking.

### Setup

1. Build the debug adapter and extension:
   ```bash
   cargo build --release -p arazzo-debug-adapter
   cd vscode-arazzo-debug && npm install && npm run build && node scripts/copy-binary.js
   ```
2. In VS Code, press **F5** to launch the Extension Development Host
3. Create `.vscode/launch.json` in the new window:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "arazzo",
      "request": "launch",
      "name": "Debug Workflow",
      "spec": "${workspaceFolder}/examples/httpbin-get.arazzo.yaml",
      "workflowId": "get-origin",
      "stopOnEntry": true
    }
  ]
}
```

4. Open the Arazzo YAML file, set breakpoints on step lines, and press **F5**

### Launch Configuration

| Property | Type | Required | Description |
|---|---|---|---|
| `spec` | string | yes | Path to the `.arazzo.yaml` workflow spec |
| `workflowId` | string | yes | Workflow to execute |
| `inputs` | object | no | Key-value map passed as workflow inputs |
| `stopOnEntry` | boolean | no | Pause before the first step (default: `false`) |
| `runtimeExecutable` | string | no | Command to launch the debug adapter (default: `cargo`) |
| `runtimeArgs` | string[] | no | Arguments for adapter launch |
| `runtimeCwd` | string | no | Working directory for adapter launch |

### Breakpoints

Set breakpoints on any meaningful line in a YAML spec. The debugger maps source lines to internal checkpoints:

- **Step lines** (`- stepId: fetch-data`) — pause before execution
- **Success criteria** (`- condition: $statusCode == 200`) — pause at evaluation
- **onSuccess / onFailure actions** — pause at dispatch
- **Output lines** (`title: //item[1]/title`) — pause at extraction

Conditional breakpoints are supported — right-click a breakpoint and add an expression:

```
$statusCode == 429
$steps.fetch-auth.outputs.token != null
```

### Stepping

| Control | Behavior |
|---|---|
| **Continue** (F5) | Run to next breakpoint or end |
| **Step Over** (F10) | Next checkpoint at current workflow depth |
| **Step In** (F11) | Descend into sub-workflow calls |
| **Step Out** (Shift+F11) | Run until returning to parent workflow |
| **Pause** (F6) | Pause at next checkpoint |

### Variable Inspection

When paused, the Variables panel shows:

- **Locals** — `workflowId`, `stepId`, `checkpoint`
- **Request** — `method`, `url`, `headers`, `body`
- **Response** — `statusCode`, `contentType`, `headers`, `bodyPreview`
- **Inputs** — workflow input parameters
- **Steps** — completed step outputs as a nested tree

### Watch Expressions

Add expressions to the Watch panel or hover in the editor:

- `$inputs.name` — workflow input
- `$steps.fetch-data.outputs.token` — step output
- `$statusCode` — HTTP status code
- `$response.header.Content-Type` — response header
- `$response.body.data.origin` — JSON body path
- `//item[1]/title` — XPath query

### Debugger Architecture

Three-thread coordinator design prevents deadlocks during slow HTTP requests:

```
stdin ──> [Reader Thread] ──cmd_tx──> [Coordinator] ──> stdout
                                           ^
          [Engine Monitor] ──event_tx──────┘
                |
          [Runtime Engine]
```

Neither channel blocks the other. A slow HTTP request does not prevent processing VS Code commands (pause, disconnect, etc.).

## Expression Language

| Expression | Resolves to |
|---|---|
| `$inputs.name` | Workflow input parameter |
| `$steps.<id>.outputs.<name>` | Previous step output |
| `$outputs.name` | Workflow outputs map (inside `workflow.outputs`) |
| `$env.VAR_NAME` | Environment variable (`.env` auto-loaded) |
| `$statusCode` | HTTP response status code |
| `$method` | HTTP method (GET, POST, etc.) |
| `$url` | Fully constructed request URL |
| `$response.header.Name` | Response header (case-insensitive) |
| `$response.body.path` | JSON dot-path body extraction |
| `$response.body#/pointer` | RFC 6901 JSON Pointer body access |
| `$request.header.Name` | Request header |
| `$request.query.Name` | Request query parameter |
| `$request.path.Name` | Request path parameter |
| `$request.body` | Request body (dot-path or JSON Pointer) |
| `$sourceDescriptions.<name>.url` | Source description URL |
| `//xpath/expression` | XML/HTML extraction |

**String interpolation:** `{$expr}` embeds any expression in a string value (e.g., `"Bearer {$steps.auth.outputs.token}"`)

**Multi-source routing:** `{sourceName}./path` selects a source description's base URL

**Condition operators:** `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `contains`, `matches`, `in`

## Repository Layout

```text
crates/
  arazzo-spec            Arazzo domain model types
  arazzo-validate        YAML parser + structural validation
  arazzo-expr            Expression parser/evaluator
  arazzo-runtime         Execution engine + debug controller
  arazzo-cli             CLI binary
  arazzo-debug-adapter   DAP server (Debug Adapter Protocol)
vscode-arazzo-debug/     VS Code debugger extension (TypeScript)
examples/                14 runnable workflow specs
testdata/                Test fixtures
```

## Building from Source

**Prerequisites:** Rust 1.82+ (`rustup` will handle this automatically via `rust-toolchain.toml`)

```bash
git clone https://github.com/strefethen/arazzo-cli.git
cd arazzo-cli
cargo build --workspace
cargo test --workspace
```

Quality gates (run by CI on every push):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

CI also runs `cargo audit`, MSRV verification (Rust 1.82), and cross-platform builds (Linux, macOS, Windows).

## Contributing

Issues, bug reports, and feature requests are welcome.

This project accepts PRs to demonstrate a fix or approach, though the maintainer may independently implement changes after review. See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## License

[MIT](LICENSE)
