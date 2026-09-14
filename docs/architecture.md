# Arazzo CLI Architecture

This document describes the runtime and CLI architecture.

## High-level Layers

1. CLI shell (`crates/arazzo-cli/src/main.rs`)
2. Command parsing (`crates/arazzo-cli/src/cli.rs`)
3. Command handlers (`crates/arazzo-cli/src/handlers.rs`)
4. Output formatters (`crates/arazzo-cli/src/output.rs`)
5. Trace artifact plumbing (`crates/arazzo-cli/src/trace.rs`)
6. Execution runtime (`crates/arazzo-runtime`)
7. Debug adapter (`crates/arazzo-debug-adapter`)
8. Spec model + validation (`crates/arazzo-spec`, `crates/arazzo-validate`)
9. Expression evaluator (`crates/arazzo-expr`)
10. VSCode extension scaffold (`vscode-arazzo-debug`)

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
2. `arazzo-debug-adapter` exposes the DAP loop (`run_dap_stdio`).
3. `vscode-arazzo-debug/` is the editor integration scaffold.

## Typed Query Ownership

Typed JSONPath has exactly one owner: `arazzo_expr::jsonpath`, published as
`JsonPathQuery` / `JsonPathMatch` / `JsonPathSelection` / `JsonPathError`.
It wraps `serde_json_path` 0.7.2 (RFC 9535) plus a private I-Regexp callback
adapter for `match()` and `search()`; the upstream parser, query and node
types stay private, so no consumer can reach the backend directly.

Three production entry points consume it, and there is no fourth:

1. `runtime_core::criteria` — `type: jsonpath` success criteria, decided by
   nodelist cardinality in `runtime_core::jsonpath`.
2. `runtime_core::payload::resolve_selector_checked` — Selector Objects in
   parameters, payload values, and step/workflow outputs.
3. `runtime_core::payload` replacement targets — `targetSelectorType: jsonpath`,
   applied through the existing JSON Pointer mutator.

Dependency direction is `arazzo-runtime` → `arazzo-expr`; nothing flows back.
Both handwritten typed-JSONPath implementations that preceded this owner are
retired, and there is no engine-selection flag, alias, or fallback.

`JsonPathQuery::parse` admits a declared version (`None` or `rfc9535`) and the
query byte/structural budgets, then validates the complete expression, all
before any context is resolved. `JsonPathQuery::query` checks the context
nesting budget, then runs one *located* query so a selected value and its
RFC 6901 pointer always come from the same walk. An operational failure inside
a regex callback is recorded in a scoped thread-local frame and invalidates the
whole query rather than degrading to `false`.

The upstream patch is narrow and documented: `[patch.crates-io]` in the root
`Cargo.toml` points `serde_json_path_core` at `vendor/serde_json_path_core`,
whose `PATCHES.md` records the provenance, checksum, and removal criterion for
the one recursive numeric-equality repair.

JSON Pointer and XPath selectors keep their own arms in
`runtime_core::payload` and `runtime_core::xpath`; the legacy GJSON-flavored
dot-path traversal used by `$response.body...` runtime expressions remains a
separate code path in `arazzo-expr` and is not part of this owner.

## Stability Notes

Frozen v1 internal APIs are declared in:

- `arazzo_runtime::INTERNAL_RUNTIME_API_VERSION`
- `arazzo_runtime::api_v1::*`
- `trace::INTERNAL_TRACE_PIPELINE_VERSION` (CLI layer)

Changes to these contracts should be treated as versioned internal API changes.
