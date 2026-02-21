# Arazzo CLI Architecture

This document captures the current runtime and CLI architecture after the Week 5/6 hardening pass.

## High-level Layers

1. CLI shell (`crates/arazzo-cli/src/main.rs`)
2. Command parsing (`crates/arazzo-cli/src/cli.rs`)
3. Command handlers (`crates/arazzo-cli/src/handlers.rs`)
4. Output formatters (`crates/arazzo-cli/src/output.rs`)
5. Trace artifact plumbing (`crates/arazzo-cli/src/trace.rs`)
6. Execution runtime (`crates/arazzo-runtime`)
7. Debug protocol models (`crates/arazzo-debug-protocol`)
8. Debug adapters (`crates/arazzo-debug-adapter`)
9. Spec model + validation (`crates/arazzo-spec`, `crates/arazzo-validate`)
10. Expression evaluator (`crates/arazzo-expr`)
11. VSCode extension scaffold (`vscode-arazzo-debug`)

## CLI Command Flow

1. Parse flags and subcommands with clap in `cli.rs`.
2. Convert global flags into `GlobalOptions`.
3. Build a `RunContext` for `run` requests (`run_context.rs`).
4. Dispatch to handler functions in `handlers.rs`.
5. Render output via `output.rs` (JSON or human text).
6. For `run --trace`, build/redact/write trace files in `trace.rs`.

The command UX is intentionally unchanged by this split.

## Run Context

`RunContext` is the central object for run execution. It includes:

- Global output settings (`json`, `verbose`)
- Run settings (`workflow`, timeout, headers, parallel, dry-run, trace flags)
- Reserved `DebugFlags` for upcoming replay/debugger behavior

This keeps run-time feature growth out of clap structs and handlers.

## Runtime Event/Trace Model

The runtime exposes two complementary views:

- `ExecutionEvent` stream (`BeforeStep`, `AfterStep`) with deterministic sequence numbers
- `TraceStepRecord` attempt-level execution records (request/response/criteria/decision)

Parallel execution guarantees deterministic ordering for both:

- Level ordering from dependency graph
- Stable per-level step index ordering
- No ordering dependence on thread completion timing

## Debugger Surfaces

1. `arazzo-runtime` exposes debug controller APIs for breakpoints, stepping, and paused scopes.
2. `arazzo-debug-adapter` exposes:
   - newline protocol loop (`run_stdio`)
   - DAP loop (`run_dap_stdio`)
3. `arazzo-cli debug-stdio` keeps a stable internal adapter bootstrap path.
4. `vscode-arazzo-debug/` is the editor integration scaffold.

## Stability Notes

Frozen v1 internal APIs are declared in:

- `arazzo_runtime::INTERNAL_RUNTIME_API_VERSION`
- `arazzo_runtime::api_v1::*`
- `trace::INTERNAL_TRACE_PIPELINE_VERSION` (CLI layer)

Changes to these contracts should be treated as versioned internal API changes.
