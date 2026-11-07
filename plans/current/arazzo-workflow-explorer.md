---
status: draft
epic:
created: 2026-11-05
---

# Workflow explorer as an execution-system view

**Seed:** The original zoomable catalog → document → workflow → step explorer
proposal is revised under Steve's 2026-11-05 request for a coherent,
agent-intuitive system, retained in the [umbrella plan](agent-operability.md).

## Outcome and boundaries

A human can navigate the same identities, requirements, preview limitations and
execution evidence available to an agent. Moving from an agent diagnosis into
an interactive view does not change the meaning of a workflow or lose its source
and attempt context. The proposed TUI is a separate binary adapter, not a second
workflow engine. It is an optional consumer, not a prerequisite for agent tools.

No TUI crate exists today. Framework/package selection remains open; the old
dependency versions and comparative framework claims are not adoption evidence.
Retain the zoomable navigation concept; do not retain its obsolete synchronous
runtime bridge, LOC estimates or assumption that every response body is captured.

## User-visible flow

Catalog → document → workflow → step provides progressive disclosure with a
breadcrumb containing qualified identity. Detail panes show authored structure,
validation, supported capability and configuration gaps separately. Duplicate
workflow names remain distinct. A source refresh invalidates stale selections
and previews instead of silently retaining a run target.

Input editing preserves JSON types and distinguishes omitted values from null.
Preview explains known effects, missing data and response-dependent unknowns.
Execution is an explicit user action using the actual selected inputs and
configuration. The UI does not infer authorization from a successful preview.

During execution, show phase, attempts, warnings, effective limits and terminal
result. Cancellation is a request followed by an observed outcome, not an
immediate success badge. An ambiguous send remains visibly ambiguous. Leaving
a view must not accidentally restart, detach or duplicate execution. A retained
artifact can be opened for inspection; opening it cannot trigger a live run.

Response/evidence views distinguish captured, omitted, redacted and unavailable
data. Do not promise a body from body-free run exports. Long values are fetched
or explicitly projected within budget, not silently clipped into false data.
Terminal control characters and document-provided descriptions render as data.

## Ownership and runtime integration

The [discovery plan](agent-discovery-and-inspection.md) owns shared read models
and qualified identity. The explorer consumes that surface instead of scanning
arbitrary YAML files or rebuilding validation. Runtime owns target admission,
execution, cancellation, debug checkpoints and evidence. TUI owns navigation,
input presentation, selected run state and terminal rendering.

The current runtime starts asynchronous execution and returns an
`ExecutionHandle`. Its events/result receivers are private, and `collect()`
retains events. A usable bounded public streaming subscription must be settled
and independently verified before a live explorer is decomposed; the existence
of internal channels alone is not a consumable API.

The proposed subscription belongs to runtime's event owner and is reusable by
adapters that need it. It must define queue capacity, consumer-loss behavior,
cancellation propagation, shutdown and delivery of the terminal result. UI
updates may coalesce presentation-only progress; required execution evidence
must not disappear silently. Retention limits and stream backpressure are
separate decisions. Slow rendering must neither grow memory without bound nor
leave execution blocked forever after the UI exits.

The UI event loop must not block on network I/O. Its async bridge holds the
execution handle for the run's lifetime and observes settlement before declaring
it stopped. Closing an active session requests cancellation and bounded cleanup;
if settlement cannot be observed, report that limitation without implying remote
rollback. Do not revive the old AtomicBool/background-thread execution recipe.

## Delivery and proof

First settle shared inspection and a framework choice compatible with the
workspace/toolchain and dependency policy. Deliver read-only navigation using
real inspection results. Then settle the runtime subscription API and add
explicit preview/run/control; finally add artifact inspection and handoff.
Exact key bindings and view decomposition belong to the accepted UI slice.

Verify the same workflow/input/attempt facts across CLI and explorer. Exercise
duplicate identities, stale sources, typed inputs, unsupported capability,
preview unknowns, success, cancellation during slow I/O, consumer loss,
backpressure, missing capture and artifact failure. Use hermetic endpoints and
observed sends. Terminal resizing and hostile text must not alter execution.

**System success:** an operator finds the same failed attempt from a CLI
artifact or a live view, sees the same bounded explanation and controls the
run without unintended sends, lost terminal outcomes or unbounded queues.
Opening a catalog or artifact sends no requests. Independent surface proof is
required once the UI exists; source review is insufficient.

**Disposition — 2026-11-05:** Draft. Shared inspection, the bounded runtime
subscription contract and framework selection are prerequisites. This revision
does not approve dependencies, create a new crate or authorize implementation.
