# AGENTS.md — arazzo-cli

Instructions for AI coding agents working on this repository.

## What This Is

A standalone CLI and Rust workspace for executing Arazzo 1.0 workflow specifications at runtime (no code generation).

## Build & Verify

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Run all three before committing.

## Project Layout

| Path | Purpose |
|------|---------|
| `crates/arazzo-cli/src/main.rs` | CLI entry point (`run`, `validate`, `list`, `catalog`, `show`) |
| `crates/arazzo-spec/src/lib.rs` | Arazzo spec model types |
| `crates/arazzo-validate/src/lib.rs` | YAML parsing + structural validation |
| `crates/arazzo-expr/src/lib.rs` | Expression parsing/evaluation |
| `crates/arazzo-runtime/src/lib.rs` | Runtime execution engine |
| `crates/arazzo-cli/tests/cli_integration.rs` | CLI integration tests |
| `examples/` | Working specs for smoke testing |
| `testdata/` | Shared fixtures |

## Rules

- **No domain-specific logic.** Keep this a generic Arazzo executor.
- **Every CLI command must support `--json`.** Machine-parseable output is required.
- **Keep tests hermetic.** No external API/network dependencies in tests.
- **Prefer strong typing.** Encode invariants in types and validation where possible.
- **Safe concurrency by default.** Avoid shared mutable state unless synchronization is explicit and justified.
- **No `unsafe`.** Workspace forbids unsafe code.

## Runtime Overview

1. `arazzo_validate::parse()` loads YAML into typed spec structures.
2. `arazzo_runtime::Engine::new()` builds workflow/step indexes.
3. `engine.execute(workflow_id, inputs)` executes workflow steps, evaluates criteria, applies actions (`end`, `goto`, `retry`), and returns workflow outputs.
4. Supports dry-run capture and trace hooks.

## Expression Language

- `$inputs.name` -> workflow input
- `$steps.<id>.outputs.<name>` -> previous step output
- `$env.VAR_NAME` -> environment variable
- `$statusCode` -> response status
- `$response.header.Name` -> header extraction
- `$response.body.path.to.field` -> JSON extraction
- `//xpath/expression` -> XML extraction

## Adding CLI Behavior

1. Update command definitions in `crates/arazzo-cli/src/main.rs`.
2. Ensure `--json` output remains available and stable.
3. Add/adjust integration tests in `crates/arazzo-cli/tests/cli_integration.rs`.
4. Re-run fmt/clippy/tests.

## Adding Runtime Features

1. Extend types in `crates/arazzo-spec/src/lib.rs` when required.
2. Enforce structure in `crates/arazzo-validate/src/lib.rs`.
3. Implement runtime behavior in `crates/arazzo-runtime/src/lib.rs`.
4. Add focused tests in the relevant crate.
5. Re-run fmt/clippy/tests.
