# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A standalone CLI and Rust library workspace for executing [Arazzo 1.0](https://spec.openapis.org/arazzo/latest.html) workflow specifications without code generation.

## What It Does

`arazzo-cli` parses Arazzo YAML specs and executes workflows at runtime:

- Builds and sends HTTP requests from `operationPath` (or sub-workflow calls via `workflowId`)
- Resolves expressions (`$inputs`, `$steps`, `$env`, `$statusCode`, `$response.*`)
- Evaluates success criteria and routes control flow (`onSuccess`, `onFailure`)
- Extracts step outputs and returns workflow outputs
- Supports `--json` on all CLI commands for machine-readable output

## Repository Layout

```text
arazzo-cli/
  crates/
    arazzo-spec      # Arazzo domain model types
    arazzo-validate  # parser + structural validation
    arazzo-expr      # expression parser/evaluator
    arazzo-runtime   # execution engine
    arazzo-cli       # command-line binary
  examples/            # sample specs
  testdata/            # shared fixtures
```

## Prerequisites

- Rust stable toolchain
- `rustfmt`, `clippy` components

Toolchain is pinned in `rust-toolchain.toml`.

## Build And Verify

Run from repository root (`/Users/stevetrefethen/github/arazzo-cli`):

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

## CLI Commands

- `run <spec> <workflow-id>`
- `validate <spec>`
- `list <spec>`
- `catalog <dir>`
- `show <workflow-id> --dir <dir>`

Global flags:

- `--json` for structured output
- `--verbose` for additional diagnostics

`run` flags:

- `--input key=value` (repeatable)
- `--header Name=value` (repeatable)
- `--timeout <duration>` (for example `30`, `30s`, `500ms`, `2m`)
- `--parallel`
- `--dry-run`
- `--trace <path>` (write a `trace.v1` execution artifact)
- `--trace-max-body-bytes <n>` (default `2048`)

## Examples

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

## Expression Language

- `$inputs.name` -> workflow input
- `$steps.<id>.outputs.<name>` -> previous step output
- `$env.VAR_NAME` -> environment variable (`.env` is auto-loaded)
- `$statusCode` -> response status code
- `$response.header.Name` -> response header
- `$response.body.path.to.field` -> JSON body extraction
- `//xpath/expression` -> XML extraction

Condition operators supported:

- `==`, `!=`, `>`, `<`, `>=`, `<=`
- `&&`, `||`
- `contains`, `matches`, `in`

## Development Notes

- This project is a generic Arazzo executor; avoid domain-specific behavior.
- Keep CLI output machine-friendly; every command must continue supporting `--json`.
- Tests should stay hermetic (local test servers/fixtures), with no external API dependencies.
- Debugger work is in progress in-repo with a separate debug surface (`arazzo-debug-protocol`, `arazzo-debug-adapter`, and `vscode-arazzo-debug`), so existing CLI command UX remains stable.
- Architecture and extension docs:
  - `docs/architecture.md`
  - `docs/extension-guide.md`
  - `docs/internal-api-v1.md`
  - `docs/debugger-architecture.md`
  - `docs/debugger-protocol-v1.md`

## Contributions

This is a personal project maintained for focus and velocity. External code contributions are not accepted for direct merge.

- Issues and bug reports are welcome.
- PRs can be opened to demonstrate a fix or approach, but may be closed without merge.
- The maintainer may independently implement similar changes after review, including AI-assisted review workflows.
- See `CONTRIBUTING.md` for details.

## License

MIT
