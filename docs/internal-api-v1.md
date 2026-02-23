# Internal API v1 Freeze

This repository now treats the following surfaces as frozen internal v1 contracts for trace/replay/debugger work.

## Runtime Version Marker

- `arazzo_runtime::INTERNAL_RUNTIME_API_VERSION == "v1"`

## Runtime Frozen Types (`arazzo_runtime::api_v1`)

- `ExecutionEvent`
- `ExecutionEventKind`
- `TraceStepRecord`
- `TraceDecision`
- `TraceDecisionPath`
- `TraceCriterionResult`
- `TraceRequest`
- `TraceResponse`
- `RuntimeError`
- `RuntimeErrorKind`

## CLI Trace Pipeline Version Marker

- `trace::INTERNAL_TRACE_PIPELINE_VERSION == "v1"`

## Change Policy

1. Backward-compatible additions are allowed.
2. Renames/removals/type-shape changes require a version bump and migration note.
3. Deterministic event ordering is part of the contract.
4. Trace redaction behavior is part of the contract.
