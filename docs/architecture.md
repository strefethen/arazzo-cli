# Arazzo CLI Architecture

This document owns the detailed runtime and CLI flow. Start with the
[system map](system-architecture.md) for the abstraction ladder, ownership and
contract boundaries. The [agent operability plan](../plans/current/agent-operability.md)
describes proposed improvements; it does not establish current functionality.

## Implementation navigation

These are entry points into the current implementation, not independent engines
or a linear stack. Runtime owns execution meaning; adapters own presentation.

1. CLI shell (`crates/arazzo-cli/src/main.rs`)
2. Command parsing (`crates/arazzo-cli/src/cli.rs`)
3. Command handlers (`crates/arazzo-cli/src/handlers.rs`)
4. Run orchestration (`crates/arazzo-cli/src/run.rs`)
5. Output formatters (`crates/arazzo-cli/src/output.rs`)
6. Run export contract and persistence (`crates/arazzo-cli/src/run_export.rs`)
7. Trace artifact plumbing (`crates/arazzo-cli/src/trace.rs`)
8. Execution runtime (`crates/arazzo-runtime`)
9. Debug adapter (`crates/arazzo-debug-adapter`)
10. Spec model + validation (`crates/arazzo-spec`, `crates/arazzo-validate`)
11. Expression evaluator (`crates/arazzo-expr`)
12. VSCode extension (`vscode-arazzo-debug`)
13. MCP adapter (`crates/arazzo-mcp`)
14. OpenAPI workflow generation (`crates/arazzo-generate`)

## CLI Command Flow

1. Parse flags and subcommands with clap in `cli.rs`.
2. Convert global flags into `GlobalOptions`.
3. Build a `RunContext` for `run` requests (`run_context.rs`).
4. Dispatch to handler functions in `handlers.rs`; the run handler delegates to `run.rs`.
5. Render output via `output.rs` (JSON or human text).
6. For `run --trace`, build/redact/write trace files in `trace.rs`.
7. For `run --export-run`, enable body-free runtime capture, then build/redact/write the versioned artifact through `run_export.rs`.

Ordinary stdout formatting remains in `output.rs`. Export serialization and
filesystem policy belong to the export owner, not the command dispatcher or
runtime. See the [run export contract](run-export-v1.md) for fields and semantics.
Within that owner, private `run_export/paths.rs` handles destination identity,
prospective paths and filesystem naming equivalence. Artifact types and
conversion remain in `run_export.rs`; path checks do not depend on the run model.

## Run Context

`RunContext` is the central object for run execution. It includes:

- Global output settings (`json`, `verbose`)
- Run settings (`workflow`, timeout, headers, parallel, dry-run, trace and export paths)

Clap declarations and shell wiring pass these settings to the run owner;
execution and artifact policy stay outside the shell and dispatcher.

## Runtime Event/Trace Model

The CLI runs on Tokio. `Engine::execute` returns an `ExecutionHandle` carrying
cancellation and separate event/result channels. The current public collection
API retains events; the private receivers are not an already-available public
streaming subscription. A new adapter needing bounded streaming must settle
that API at the runtime event owner, as the
[explorer draft](../plans/current/arazzo-workflow-explorer.md) requires.

The runtime exposes three complementary views:

- `ExecutionEvent` stream (`BeforeStep`, `AfterStep`) with deterministic sequence numbers
- `TraceStepRecord` attempt-level execution records (request/response/criteria/decision)
- `RunStepRecord` attempt-level retained outputs and body-free HTTP metadata

Parallel execution guarantees deterministic ordering for these views:

- Level ordering from dependency graph
- Stable per-level step index ordering
- Each step's attempts, retries included, recorded together in attempt order
- No ordering dependence on thread completion timing

`EngineBuilder::capture_run` enables run records independently of tracing.
The shared attempt protocol delegates capture to `runtime_core/run_record.rs`,
and `ExecutionResult::run_steps` exposes the collected records. Sequential,
parallel and single-step execution use this same capture boundary. Runtime
records contain execution facts; CLI code owns the persisted `run.v1` envelope,
grouping, redaction and atomic writing. Undeclared request/response bodies are
absent from run records; a body explicitly selected as an output remains data.

Beside the HTTP executor (`runtime_core/engine_http.rs`) sits the message
executor (`runtime_core/engine_message.rs`): it arms every Kafka receive step
of an invocation before the first step runs (connecting through the
`runtime_core/kafka/` client module and snapshotting partition end offsets),
owns the per-invocation cursors keyed by connection identity and topic, and
turns the next record into the step's `$message` context, criteria, outputs,
and the optional `message` object on its trace record. Document admission
(`asyncapi_receive.rs`) decides, before any connection, which AsyncAPI forms
the executor will run; the CLI's `message_transport.rs` converts the Kafka
flags into the engine's options. The user-facing contract is
[kafka-receive.md](kafka-receive.md).

The [cumulative structure assessment](../plans/assessments/project-structure.md)
records dated measurements, ownership guidance and remaining structural debt.

## Debugger Surfaces

1. `arazzo-runtime` exposes debug controller APIs for breakpoints, stepping, and paused scopes.
2. `arazzo-debug-adapter` exposes the DAP loop (`run_dap_stdio`).
3. `vscode-arazzo-debug/` is the editor integration.

Runtime owns breakpoints, paused scopes, evaluation and stepping; DAP owns
session/protocol projection. Watch evaluation uses the runtime/expression
owners. Debugger pause/resume is local execution control, not durable recovery
or rollback of already-sent operations. Client loss must release local control;
a missing final observation cannot establish that remote effects were undone.
New source/run/invocation/attempt correlation requires versioned contracts;
existing DAP responses cannot be assumed to contain it. The
[debugger guide](debugger-user-guide.md) covers current use.

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
dot-path traversal remains a separate code path in `arazzo-expr` and is not part
of this owner. Author standalone body/payload selection with a bare reference
or JSON Pointer, such as `$response.body` or `$response.body#/items/0/id`.
Pointer-only standalone syntax is the accepted target; the legacy consumer
still runs until the coordinated evaluator cutover. Simple-condition `.member`
and `[0]` operators remain separate from standalone expression parsing. See
the [authoring guidance](../README.md#expression-language) for nested versus
literal property names, token escaping, and field-specific cutover behavior.

## Stability Notes

Frozen v1 internal APIs are declared in:

- `arazzo_runtime::INTERNAL_RUNTIME_API_VERSION`
- `arazzo_runtime::api_v1::*`
- `trace::INTERNAL_TRACE_PIPELINE_VERSION` (CLI layer)

Changes to these contracts should be treated as versioned internal API changes.
