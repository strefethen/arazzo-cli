# CLAUDE.md — arazzo-cli

Standalone Arazzo 1.0 workflow executor (Rust-only on `main`).

## Build & Test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo audit
```

## Workspace Structure

```text
crates/
  arazzo-spec            Typed Arazzo model
  arazzo-validate        YAML parse + validation
  arazzo-expr            Expression parser/evaluator
  arazzo-runtime         Engine, HTTP execution, control flow
  arazzo-cli             CLI commands
  arazzo-mcp             MCP server (Model Context Protocol) for AI agents
  arazzo-debug-protocol  Internal JSON-line debug protocol types
  arazzo-debug-adapter   Full DAP (Debug Adapter Protocol) implementation
vscode-arazzo-debug/     VS Code extension for Arazzo debugging
  src/                   TypeScript extension source
  bin/                   Bundled debug adapter binary (copied from target/release)
  dist/                  Compiled extension JS
  package.json           Extension manifest + debugger contribution
examples/                Sample Arazzo specs
testdata/                Fixtures used by tests
```

## VS Code Extension

The debugger extension lives at `vscode-arazzo-debug/`. To rebuild and install:

```bash
cargo build --release -p arazzo-debug-adapter   # Build the adapter binary
cd vscode-arazzo-debug && npm run build          # Compile TypeScript
npx @vscode/vsce package --no-dependencies       # Package VSIX (copies binary automatically)
code --install-extension arazzo-debug-0.0.1.vsix --force  # Install into VS Code
```

After installing, reload VS Code. The extension registers the `arazzo` debug type.
Debug adapter diagnostics go to stderr (visible in VS Code's Output > Arazzo Debug panel).

## Architecture Notes

- Runtime interpretation only (no codegen)
- Typed expression evaluation across `$inputs`, `$steps`, and response context
- Spec-driven control flow via `onSuccess` / `onFailure`
- Optional dry-run request planning and trace hooks
- Unsafe code is forbidden across the workspace

## CLI Principles

- Every command supports `--json`
- Human-readable output stays available by default
- Structured error JSON when `--json` is set
- Commands: `run`, `replay`, `validate`, `list`, `steps`, `catalog`, `show`, `generate`, `schema`, `serve`

## Conventions

- Keep behavior generic (no app-specific logic)
- Keep tests hermetic and deterministic
- Prefer compile-time checks and explicit types over dynamic behavior

## Expression Surface

- `$inputs.name`
- `$steps.<id>.outputs.<name>`
- `$env.VAR_NAME`
- `$statusCode`
- `$method`
- `$response.header.Name`
- `$response.body.path`
- `$response.body#/json/pointer` (RFC 6901)
- `$outputs.name` (workflow outputs map, inside `workflow.outputs` only)
- `$outputs.name#/json/pointer`
- `$url`
- `$request.header.Name`
- `$request.query.Name`
- `$request.path.Name`
- `$request.body` / `$request.body.path` / `$request.body#/pointer`
- `$sourceDescriptions.{name}.url`
- `{$expr}` interpolation in string values
- `{sourceName}./path` operationPath routing (multiple source descriptions)
- `//xpath`

## Quick Smoke

```bash
cargo run -p arazzo-cli -- validate examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- run examples/httpbin-get.arazzo.yaml get-origin
cargo run -p arazzo-cli -- --json catalog examples
```
