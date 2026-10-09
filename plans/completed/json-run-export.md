---
status: completed
epic: ac-bd06d
created: 2026-10-09
completed: 2026-10-09
---

# JSON run export

**Seed:** Steve asked: “You are the orchestrator for implementing JSON workflow output with --export-run using sub-agents for implementation.” The preceding discussion selected declared step outputs, automatic HTTP metadata, ordered execution history, and JSON as the first presentation surface. Steve also requested production timestamps and a cumulative project structure assessment.

## Outcome and non-goals

`run --export-run <path>` writes a versioned JSON run artifact for inspection and future rendering. It preserves the JSON types of declared outputs and every completed step attempt, including retries and revisits. It does not change Arazzo syntax, introduce vendor extensions, build a UI, change workflow variable semantics, or retain undeclared response bodies in the artifact.

## User-visible workflows

- Successful runs export run identity, tool version, source path, selected workflow/step, inputs, final workflow outputs, and workflows keyed by ID containing steps keyed by ID with ordered `executions` arrays.
- Each execution carries a global record sequence, a per-workflow/step attempt ordinal, outcome, duration, declared outputs, optional body-free request/response metadata, and error/decision information. Arrays are authoritative; latest means the last entry. No duplicated `latest` payload or invented expression parser.
- Failed or timed-out runs export the completed/settled attempts and preserve the command's failing outcome. The artifact distinguishes a run error from an attempt failure. Unexecuted steps are absent. Repeated workflow invocations with the same ID are merged in record order; v1 does not claim invocation-tree identity.
- Dry runs produce an explicitly marked dry-run artifact, with simulated attempts identified rather than represented as real responses. `--step` exports only steps actually executed.
- Run metadata contains RFC3339 UTC `startedAt`, `finishedAt`, `exportedAt`, plus elapsed `durationMs`. These describe execution and artifact production, not API forecast timestamps.
- Artifact status/error describe runtime execution, not subsequent CLI expression-diagnostic policy or trace/export persistence failures. Ordinary stdout and `--json` retain existing contracts. Export failure is a command failure with a structured error under `--json`; combined failures retain each cause. Trace and export may coexist at distinct paths; identical destinations and export aliases of input documents are rejected before execution. Setup/parse errors before an execution begins do not promise an artifact.

## Decision-relevant facts

- `runtime_core/state.rs::VarStore` retains named step outputs; `engine_impl.rs::build_outputs` only returns declared workflow outputs.
- `events.rs::ExecutionResult` already collects streamed events; `EngineEvent` is non-exhaustive. `step_attempt.rs` owns attempt completion for sequential, single-step, and parallel execution. Use this shared boundary instead of new scheduler-specific collectors.
- `trace.rs::redact_trace_file` truncates response bodies and handles trace persistence; trace is a debugging/replay format, not the run-data contract. Enabling trace solely for export would retain undeclared response body text in collected events.
- Runtime redaction helpers are public and shared by the CLI. Existing ordinary run JSON is owned by `output.rs::emit_run_outputs` and the schema drift guards.

## Architecture

The dependency direction remains CLI → runtime → spec/expressions. No new crate or dependency is needed.

1. Runtime owns an opt-in, body-free `RunStepRecord` event and builder configuration independent of tracing. Related public metadata types and capture behavior live in a focused `runtime_core/run_record.rs` owner. `ExecutionResult` exposes filtered records. The existing attempt protocol delegates capture; it does not acquire JSON export policy. Capture records outputs and metadata without accumulating raw request or response bodies, except bodies intentionally retained as declared outputs. Existing trace contracts remain unchanged.
2. CLI owns the `run.v1` persisted envelope, grouping, shared-policy redaction, serialization, and atomic file replacement in a focused `run_export` module. Request metadata is method/URL/headers; response metadata is status code/headers/content type/body byte count. No implicit body or body preview fields. Error text and URL/header/output values are sanitized with existing applicable redaction helpers. JSON receives existing recursive sensitive-key redaction plus existing text-pattern redaction on string leaves. Output values are not subject to the trace preview size limit. Explicitly document that redaction is policy based, not a guarantee that arbitrary data contains no secrets. Early transport errors may lack request metadata under the current settlement protocol; failed criteria preserve current empty-output semantics. The body-free guarantee concerns retained export events/artifacts, not removal of current transient trace construction.
3. The run command moves from the broad `handlers.rs` into a focused `run.rs` owner as a bounded extraction. `handlers.rs` delegates; `main.rs` only wires options and modules. Export domain decisions stay in `run_export`, not `run.rs`, `main.rs`, `handlers.rs`, or `output.rs`. The extraction preserves current run behavior and tests.
4. A new export schema is discoverable through `schema export-run` and checked by the existing schema drift mechanism. User documentation owns the serialized contract. Generated schemas remain tracked because the repository distributes and verifies them as public contracts.
5. Atomic writing must not leave partial JSON or silently swallow errors. Reuse an existing narrow writer if accessible without coupling export to trace; otherwise keep a feature-specific atomic writer rather than an unsolicited persistence refactor. Distinct trace/export paths must be checked, including obvious filesystem aliases where feasible.
6. Export-local URL sanitation also redacts URL userinfo. The existing shared helper handles query values only; the broader trace/MCP repair remains separate under [ac-d95a9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d95a9). Do not expand this initiative into that global repair.

## Required order

Implement and independently review runtime capture first; then implement CLI export and its contract/docs. One implementer writes at a time in the current checkout. A fresh frontier review binds each candidate to its retained ticket contract. No pushes, releases, branches, or worktrees.

## Verification and operational risks

Focused hermetic HTTP tests establish success, failure, retries/revisits, subworkflows, parallel and single-step capture, capture-off behavior, and absence of undeclared body capture. CLI integration establishes usable JSON, UTC timestamps, typed/nested/full-body declared outputs, redaction, partial failures/timeouts, dry-run semantics, trace coexistence, destination errors, and unchanged stdout. Workspace fmt, clippy with all targets/features, and tests are required before candidate commits. Existing dirty debugger/expression files remain outside this initiative; test evidence must disclose the dirty baseline.

Metadata and outputs can be large or sensitive. V1 keeps every retained output per execution as authorized, so memory scales with that data; no unapproved fallback, truncation, or retention limit is introduced. Wall clocks can adjust; elapsed duration uses the monotonic clock.

## System success condition

Run a hermetic workflow with multiple steps and a retry, inspect one exported JSON file, address each step's typed outputs and response status, observe all attempts and production timestamps, and demonstrate partial export after a failure. Prove undeclared body text is absent, ordinary JSON stdout is unchanged, and schema validation/drift checks pass. Leave a cumulative assessment under `plans/assessments/` with dated baseline, additions, ownership boundaries, measured scope, verification, and remaining concerns.

**Disposition:** Completed. Both implementation tickets in [epic:ac-bd06d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd06d) closed after fresh independent candidate review and required verification. System success was independently observed after closure at **2026-10-09T22:45:43Z** against source candidate `8dd873273c4731797113102ca037662400d8cc5f`: five steps, six attempts including a retry, 300 complete declared rows, partial failure history, UTC timestamps, unchanged ordinary stdout, three schema-valid artifacts and exclusion of undeclared bodies. The [cumulative structure assessment](../assessments/project-structure.md) owns implementation measurements, evidence pointers and continuation guidance. No push or release was performed.
