# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A standalone CLI and Rust library workspace for executing [Arazzo 1.0](https://spec.openapis.org/arazzo/latest.html) workflow specifications without code generation.

## Status

Rust cutover completed on 2026-02-19.

- Rust is the only supported implementation on `main`.
- Go runtime and CLI were removed as part of migration completion.

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
  rust/
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

Toolchain is pinned in `rust/rust-toolchain.toml`.

## Build And Verify

From `rust/`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Run The CLI

From `rust/`:

```bash
cargo run -p arazzo-cli -- --json validate ../examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- --json list ../examples/httpbin-get.arazzo.yaml
cargo run -p arazzo-cli -- --json run ../examples/httpbin-get.arazzo.yaml get-origin
```

Optional install:

```bash
cargo install --path ./rust/crates/arazzo-cli --locked
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
- `--timeout <seconds>`
- `--parallel`
- `--dry-run`

## Examples

Validate a spec:

```bash
cargo run -p arazzo-cli -- --json validate ../examples/httpbin-get.arazzo.yaml
```

List workflows:

```bash
cargo run -p arazzo-cli -- --json list ../examples/httpbin-get.arazzo.yaml
```

Execute workflow with inputs:

```bash
cargo run -p arazzo-cli -- --json run ../examples/httpbin-get.arazzo.yaml status-check --input code=200
```

Dry-run request planning (no network calls):

```bash
cargo run -p arazzo-cli -- --json run ../examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429
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

## License

MIT
