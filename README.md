# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A standalone CLI, Rust library workspace, and VS Code debugger for executing and debugging [Arazzo 1.0.1](https://spec.openapis.org/arazzo/latest.html) workflow specifications without code generation.

## What It Does

`arazzo-cli` parses Arazzo YAML specs and executes workflows at runtime:

- Builds and sends HTTP requests from `operationPath` (or sub-workflow calls via `workflowId`)
- Resolves expressions (`$inputs`, `$steps`, `$env`, `$statusCode`, `$response.*`)
- Evaluates success criteria and routes control flow (`onSuccess`, `onFailure`)
- Extracts step outputs and returns workflow outputs
- Supports `--json` on all CLI commands for machine-readable output
- Ships a full **VS Code debug extension** for interactive step-through debugging of Arazzo workflows

## Repository Layout

```text
arazzo-cli/
  crates/
    arazzo-spec            # Arazzo domain model types
    arazzo-validate        # parser + structural validation
    arazzo-expr            # expression parser/evaluator
    arazzo-runtime         # execution engine + debug controller
    arazzo-cli             # command-line binary
    arazzo-debug-adapter   # DAP server (Debug Adapter Protocol)
    arazzo-debug-protocol  # DAP message types
  vscode-arazzo-debug/     # VS Code debugger extension (TypeScript)
  examples/                # sample specs
  testdata/                # shared fixtures
```

## Prerequisites

- Rust stable toolchain
- `rustfmt`, `clippy` components

Toolchain is pinned in `rust-toolchain.toml`.

## Compatibility Policy

- **MSRV:** Rust `1.82`
- CI validates stable plus an explicit MSRV job on Rust `1.82`

## Build And Verify

Run from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Run The CLI

Run from repository root and use `examples/...` paths (not `../examples/...`):

```bash
cargo run -p arazzo-cli -- --json validate examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- --json list examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml get-origin
```

If running from `crates/arazzo-cli/`, use `../../examples/...` paths instead.

Optional install:

```bash
cargo install --path ./crates/arazzo-cli --locked
```

This project is private-release oriented right now: crates.io publishing is disabled across the workspace (`publish = false` in all manifests).

## Internal Distribution (Private Repo)

- Build and run directly from source:
  - `cargo run -p arazzo-cli -- --json validate examples/httpbin-get.arazzo.yaml`
- Install locally from a checked-out repo:
  - `cargo install --path ./crates/arazzo-cli --locked`
- For tagged internal releases, prefer release artifacts from your private GitHub release flow.
- Release playbook: `docs/internal-release.md`
- Tag helper: `bash scripts/release/cut-tag.sh <tag> [--push] [remote]`
- Post-release validator: `bash scripts/release/verify-downloaded-release.sh <tag>`

## CLI Commands

- `run <spec> <workflow-id>`
- `validate <spec>`
- `list <spec>`
- `catalog <dir>`
- `show <workflow-id> --dir <dir>`
- `schema [command]` — print JSON Schema for a command's `--json` output

Global flags:

- `--json` for structured output
- `--verbose` for additional diagnostics

`run` flags:

- `--input key=value` (repeatable)
- `--header Name=value` (repeatable)
- `--http-timeout <duration>` (per-request HTTP timeout; default `30s`)
- `--execution-timeout <duration>` (overall workflow deadline; default `5m`)
- `--parallel`
- `--dry-run`
- `--openapi <path>` (repeatable operationId source specs)
- `--input-json key=<json>` (repeatable JSON-typed inputs)
- `--expr-diagnostics <off|warn|error>` (default `off`)
- `--trace <path>` (write a `trace.v1` execution artifact)
- `--trace-max-body-bytes <n>` (default `2048`)

## Examples

Scenario catalog: see `examples/README.md` for intent-driven examples (`auth-flow`, `error-handling-retry`, `sub-workflow`, `multi-api-orchestration`).

Validate a spec:

```bash
cargo run -p arazzo-cli -- --json validate examples/httpbin-get.arazzo.yaml
```

List workflows:

```bash
cargo run -p arazzo-cli -- --json list examples/httpbin-get.arazzo.yaml
```

Execute workflow with inputs:

```bash
cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml status-check --input code=200
```

Dry-run request planning (no network calls):

```bash
cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429
```

Write a trace file while executing:

```bash
cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml status-check --input code=429 --trace ./tmp/run-trace.json
```

## Execution Traces

`run --trace <path>` writes a `trace.v1` JSON artifact for both successful and failed runs.

- Normal stdout behavior is unchanged (`--json` output contract remains the same).
- Sensitive values are redacted as `"[REDACTED]"`.
- Redaction applies to:
  - headers such as `Authorization`, `Cookie`, `X-API-Key`
  - URL query params with sensitive names (for example `token`, `password`, `session`)
  - JSON fields in inputs/request bodies/outputs with sensitive keys

Reference docs:

- `docs/trace-schema-v1.md`
- `docs/trace-schema-changelog.md`
- `docs/schemas/trace-v1.schema.json`

Minimal trace example:

```json
{
  "schemaVersion": "trace.v1",
  "tool": {
    "name": "arazzo",
    "version": "0.1.0"
  },
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
      "decision": {
        "path": "next"
      }
    }
  ]
}
```

## VS Code Debugger

The project includes a full-featured VS Code debug extension that lets you set breakpoints, step through workflows, inspect variables, and evaluate expressions — all from the standard VS Code debug UI.

### Quick Start

1. Open this repository in VS Code
2. Run `npm install && npm run build` in `vscode-arazzo-debug/`
3. Press **F5** to launch the Extension Development Host
4. In the new window, create a `.vscode/launch.json`:

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

5. Open the Arazzo YAML file, set breakpoints on step lines, and press **F5**

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

Set breakpoints on any meaningful line in a YAML spec. The debugger maps source lines to internal checkpoints using YAML-aware parsing.

**Where breakpoints can be set:**

- **Step lines** (`- stepId: fetch-data`) — pause before the step executes
- **Success criteria** (`- condition: $statusCode == 200`) — pause at each criterion evaluation
- **onSuccess / onFailure actions** — pause at action dispatch, criterion checks, retry selection, and retry delays
- **Output lines** (`title: //item[1]/title`) — pause at output extraction

The debugger resolves breakpoints to the nearest valid checkpoint within 10 lines. The verified line and a descriptive message are returned to VS Code (visible in the Breakpoints panel).

**Conditional breakpoints** are supported. Right-click a breakpoint and add a condition using any expression the runtime understands:

```
$statusCode == 429
$steps.fetch-auth.outputs.token != null
```

### Stepping

All standard VS Code stepping controls work:

| Control | Behavior |
|---|---|
| **Continue** (F5) | Run to next breakpoint or end |
| **Step Over** (F10) | Execute the next checkpoint at the current workflow depth |
| **Step In** (F11) | Descend into sub-workflow calls |
| **Step Out** (Shift+F11) | Run until returning to the parent workflow |
| **Pause** (F6) | Request pause at the next checkpoint |

Step depth tracking is fully sub-workflow aware. Stepping over a step that triggers a sub-workflow executes the entire sub-workflow and stops at the next step in the calling workflow.

### Variable Inspection

When paused, the Variables panel shows five scope categories:

**Locals** — current step context:
- `workflowId`, `stepId`, `checkpoint` (human-readable name like `onSuccess[0]`)

**Request** — the HTTP request that was (or will be) sent:
- `method`, `url`, `headers`, `body`

**Response** — the HTTP response received:
- `statusCode`, `contentType`, `headers`, `bodyPreview`

**Inputs** — all workflow input parameters as key-value pairs

**Steps** — completed step outputs as a nested tree:
```
steps/
  fetch-auth/
    token: "eyJ..."
  get-data/
    items: "[{...}]"
```

All variables are expandable where the underlying value is an object or array.

### Watch Expressions

Add expressions to the Watch panel or hover over identifiers in the editor. The debugger evaluates against the current runtime state:

**Runtime expressions:**
- `$inputs.name` — workflow input
- `$steps.fetch-data.outputs.token` — step output
- `$statusCode` — HTTP status code
- `$response.header.Content-Type` — response header
- `$response.body.data.origin` — JSON body path

**XPath expressions:**
- `//item[1]/title` — query XML/HTML response bodies

**Shorthand names:**
- `origin` — resolves to a matching local variable or step output automatically

### Call Stack

The Call Stack panel shows the current workflow execution depth. When a step triggers a sub-workflow, the stack grows:

```
get-data          (sub-workflow, current)
main-workflow     (caller)
```

Each frame shows the workflow ID, step ID, and source line. Clicking a frame navigates to that location in the spec.

### Architecture

The debugger uses a three-thread coordinator design to prevent deadlocks during slow HTTP requests:

```
stdin ──> [Reader Thread] ──cmd_tx──> [Coordinator] ──> stdout
                                           ^
          [Engine Monitor] ──event_tx──────┘
                |
          [Runtime Engine]
```

- **Reader thread** — reads DAP commands from stdin, forwards via channel
- **Coordinator** (main thread) — multiplexes commands and engine events; owns all DAP I/O
- **Engine monitor** — watches for runtime stop events and completion; forwards via channel

Neither channel blocks the other. A slow HTTP request in the engine does not prevent the coordinator from processing VS Code commands (pause, disconnect, etc.).

## Expression Language

Runtime expressions:

- `$inputs.name` -> workflow input
- `$steps.<id>.outputs.<name>` -> previous step output
- `$outputs.name` -> workflow outputs map (inside `workflow.outputs`)
- `$env.VAR_NAME` -> environment variable (`.env` is auto-loaded)
- `$statusCode` -> response status code
- `$method` -> HTTP method (GET, POST, etc.)
- `$url` -> fully constructed request URL (post-request only)
- `$response.header.Name` -> response header (case-insensitive)
- `$response.body.path.to.field` -> JSON dot-path body extraction
- `$response.body#/json/pointer` -> RFC 6901 JSON Pointer body access
- `$request.header.Name` -> request header introspection
- `$request.query.Name` -> request query parameter
- `$request.path.Name` -> request path parameter (substituted value)
- `$request.body` / `$request.body.path` / `$request.body#/pointer` -> request body access
- `$sourceDescriptions.<name>.url` -> source description URL lookup
- `//xpath/expression` -> XML/HTML extraction

String interpolation:

- `{$expr}` -> embed any expression in a string value (e.g., `"Bearer {$steps.auth.outputs.token}"`)

Multi-source routing:

- `{sourceName}./path` -> operationPath prefix selects a source description's base URL

Condition operators:

- `==`, `!=`, `>`, `<`, `>=`, `<=`
- `&&`, `||`
- `contains`, `matches`, `in`

## Development Notes

- This project is a generic Arazzo executor; avoid domain-specific behavior.
- Keep CLI output machine-friendly; every command must continue supporting `--json`.
- Tests should stay hermetic (local test servers/fixtures), with no external API dependencies.
- The VS Code debugger (`arazzo-debug-adapter` + `vscode-arazzo-debug`) is a separate debug surface; existing CLI command UX remains stable.
- Architecture and extension docs:
  - `docs/architecture.md`
  - `docs/extension-guide.md`
  - `docs/internal-api-v1.md`
  - `docs/debugger-architecture.md`
  - `docs/debugger-user-guide.md`
  - `docs/debugger-troubleshooting.md`

## Contributions

This is a personal project maintained for focus and velocity. External code contributions are not accepted for direct merge.

- Issues and bug reports are welcome.
- PRs can be opened to demonstrate a fix or approach, but may be closed without merge.
- The maintainer may independently implement similar changes after review, including AI-assisted review workflows.
- See `CONTRIBUTING.md` for details.

## License

MIT
