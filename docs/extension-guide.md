# Extension Guide

This guide covers the safest way to add features without regressing CLI contracts.

## Add a New CLI Command

1. Add the command shape in `crates/arazzo-cli/src/cli.rs`.
2. Route it in `crates/arazzo-cli/src/main.rs`.
3. Implement logic in `crates/arazzo-cli/src/handlers.rs`.
4. Add output serializers/formatters in `crates/arazzo-cli/src/output.rs`.
5. Add integration tests in `crates/arazzo-cli/tests/cli_integration.rs`.
6. If JSON output changes, update snapshot tests.

## Add Run-time Flags

For `run`, add options to `RunOptions` and thread them through `RunContext`.

Guideline:

- Parse in `cli.rs`
- Store in `run_context.rs`
- Consume in `handlers.rs`
- Keep formatter behavior in `output.rs`

## Add Runtime Trace/Replay Features

1. Extend `ExecutionEvent` and/or `TraceStepRecord` in `arazzo-runtime`.
2. Preserve deterministic sequencing semantics.
3. Update `arazzo_runtime::api_v1` only if the change is part of the frozen contract.
4. Update CLI trace redaction/writer only in `trace.rs`.
5. Add runtime tests first, then CLI integration tests.

## Add Debugger Features

1. Extend runtime debug types under `crates/arazzo-runtime/src/debug/`.
2. Keep step gate ordering deterministic and covered by tests.
3. Extend adapter protocol handling in `crates/arazzo-debug-adapter/src/dap.rs`.
4. Add transcript tests in `crates/arazzo-debug-adapter/tests/`.
5. Keep CLI debug transport (`debug-stdio`) behavior stable while evolving DAP.
6. Update `vscode-arazzo-debug/` only after runtime/adapter contracts are stable.

## Add Expression Capabilities

1. Implement in `arazzo-expr`.
2. Add deterministic tests and property tests.
3. Validate no panic paths with fuzz-style generated inputs.

## Required Verification Before Commit

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Common Pitfalls

- Mixing business logic with output formatting
- Emitting hook/events from worker threads in non-deterministic order
- Changing trace schema fields without versioning
- Adding parser behavior without property tests
- Letting extension assumptions drift from adapter/runtime contracts
