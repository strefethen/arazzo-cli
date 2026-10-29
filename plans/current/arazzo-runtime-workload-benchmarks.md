---
status: decomposed
epic: ac-8ba62
created: 2026-10-29
---

# Runtime source-build and workflow workload benchmark

**Seed:** Planning is tracked by
[ac-9731e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9731e). Steve's “Accept design and planning” decision is retained in
[ac-0be12](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0be12). The accepted
[engine workload assessment](../assessments/arazzo-engine-workload-benchmarks.md)
owns the six families and measurement boundaries. This is its second unit,
following the completed [root-load unit](../completed/arazzo-root-load-benchmark.md).
The current planning authorization does not itself authorize implementation.

## Outcome and non-goals

Give maintainers a development-only `arazzo-runtime` workload executable that
prepares bounded synthetic fixtures, serves them from a separate loopback process,
and measures exactly one public runtime invocation per worker process. It covers
source loading/build plus expression-heavy execution, wide DAGs, large bodies,
retries and cancelled slow I/O. Every result must pass an independent semantic
oracle before its raw duration can support a positive series.

This unit adds no production API, crate, dependency edge, package/version change,
scheduler change, resource enforcement or performance threshold. CLI process,
root parser, expression Criterion and synthetic runtime Criterion series retain
their existing owners. OS CPU/RSS launching, repeated-series orchestration,
statistical summaries and CI are subsequent repository-tooling units. Kafka and
AsyncAPI work and the paused generator/MCP candidate are outside this plan.

## Maintainer workflows

Build the existing CLI and new Cargo example before measurement. Start the
service as a sibling process, obtaining an ephemeral loopback endpoint. Prepare
fixtures for that endpoint in a fresh evidence directory, validate every positive
root using the prebuilt CLI's strict JSON validation path, verify the restricted
OpenAPI fixture shapes, and hash the exact input files with existing host tools.
Record the preparation and validation receipts before launching any worker.

Invoke the prebuilt worker for one case, invocation ID and event mode. It loads
and checks the prepared inputs outside the measured interval, begins a fresh
service session, builds a fresh engine and performs the selected public call.
It emits one machine-readable raw outcome and exits. Separate verification
invocations exercise negative controls. No warmups, repetitions or percentiles
are hidden in the worker. A bounded local proof recipe runs all cases; the
future launcher owns three warmups, twenty measured invocations, series validity,
run order and the assessment's statistical definitions.

An unknown case/configuration, invalid fixture, failed service handshake, wrong
result, timeout or teardown failure produces a nonzero exit and an explicit
failure record when output remains possible. Missing/truncated records also
invalidate an invocation. Expected-negative controls succeed only when their
specified error and side-effect oracle matches; they are never positive samples.
Partial evidence is retained under that failed invocation, never reused as a
successful baseline.

## Decision-relevant facts

- [`EngineBuilder::build`](../../crates/arazzo-runtime/src/runtime_core/builder.rs)
  creates the HTTP client, binds local sources through `DocumentSet`, reads and
  parses source bytes, derives server bases and indexes operations. A prepared
  root plus `source_base_dir` reaches native file loading without injecting a
  loader or preloaded documents. This measures the whole public build boundary,
  including client construction; it is not isolated indexing time.
- [`Engine::execute`](../../crates/arazzo-runtime/src/runtime_core/engine_impl.rs)
  spawns execution. [`ExecutionHandle`](../../crates/arazzo-runtime/src/runtime_core/events.rs)
  exposes `collect`, `result_only`, and `cancel_token`, but no join primitive.
  `collect` retains drained events; `result_only` closes the event receiver.
  API completion and task-join completion must not be conflated.
- [`parallel_blocker`/`build_levels`](../../crates/arazzo-runtime/src/runtime_core/deps.rs)
  and [`execute_parallel`](../../crates/arazzo-runtime/src/runtime_core/engine_parallel.rs)
  permit plain retries, reject control-flow eligibility, and execute complete
  dependency levels before advancing. Collected attempts are replayed in source
  order. That ordering cannot prove real transport or completion ordering.
- [`RateLimitConfig`/`RateLimiterState`](../../crates/arazzo-runtime/src/runtime_core/client.rs)
  default to 10 starts/second and burst 20; nonpositive requests/second disables
  waiting. `ClientConfig` owns HTTP timeout and the builder owns response caps.
- The [runtime manifest](../../crates/arazzo-runtime/Cargo.toml) already supplies
  serde/JSON/YAML, Tokio, URL handling and development-only multithread Tokio and
  `tiny_http`; standard-library sockets/process primitives cover fixture control.
  It has no validator dependency. The existing CLI's `handlers::validate_spec`
  calls the public validator and promotes warnings with `--strict`. Reuse that
  prebuilt executable during setup; do not introduce a runtime→validator edge.
- [`source_document_loader`](../../crates/arazzo-runtime/tests/source_document_loader.rs),
  [`engine_parallel`](../../crates/arazzo-runtime/tests/engine_parallel.rs),
  [`parallel_event_drain`](../../crates/arazzo-runtime/tests/parallel_event_drain.rs),
  and [`http_cancellation`](../../crates/arazzo-runtime/tests/http_cancellation.rs)
  establish current public result/error and event behavior. They are semantic
  anchors, not normative fixture syntax or benchmark support to copy wholesale.
  Known ownership constraints in [god-files.md](../../god-files.md) prohibit
  growing engine HTTP orchestration or the existing execution-test facade for
  this independent responsibility.

## Fixture authority and preflight

Use Arazzo 1.1.0, required metadata, uniquely named workflows/steps/sources,
local OpenAPI 3.2.0 files and explicit GET operations with documented responses.
The [conformance audit](../assessments/arazzo-spec-conformance-audit.md), including
F1/F4/F5, F7, F12/F13 and F15/F17, is debt context; its old examples grant no
syntax authority. Selected fixture forms follow these vendored requirements:

- [Source Description](../../spec/arazzo/v1.1.0.html#source-description-object):
  “A URL to a source description to be used by a workflow.” Source URLs name
  actual local documents; OpenAPI `servers` supplies the loopback API endpoint.
- [Step](../../spec/arazzo/v1.1.0.html#step-object): operation IDs identify “an
  existing, resolvable operation”. With multiple non-Arazzo sources, the ID
  “MUST be specified using a Runtime Expression”; use
  `$sourceDescriptions.source_0.operation_0`, including in the eight-source case.
  “All assertions MUST be satisfied for the step to be deemed successful.”
  Every positive step declares nonempty success criteria.
- [Runtime Expressions](../../spec/arazzo/v1.1.0.html#runtime-expressions):
  `body-reference = "body" ["#" json-pointer ]` and
  `steps-reference = step-id ".outputs." output-name [ "#" json-pointer ]`.
  Use pointer body access, bare `$response.body` only for full retention, and
  declared workflow outputs referencing step outputs. Simple conditions use
  supported comparison/Boolean operators, never `contains`, `matches` or `in`.
- [Tool Behavior](../../spec/arazzo/v1.1.0.html#tool-behavior): “Tools MUST respect
  all declared dependsOn relationships.” Use local step IDs for DAG edges.
  Explicit synchronous DAG edges are a deliberate benchmark construction;
  ordinary sequential fixtures follow the recommended ordered step array.
- [Failure Action](../../spec/arazzo/v1.1.0.html#failure-action-object):
  `retryAfter` is “A non-negative decimal indicating the seconds to delay”.
  `retryLimit` is “A non-negative integer indicating how many attempts to retry
  the step MAY be attempted before failing the overall step.” Supply a literal
  name and type, `retryAfter: 0` or `0.1`, and no retry-reference target.
- [OpenAPI Operation](../../spec/oas/v3.2.0.html#operation-object): “The id MUST
  be unique among all operations described in the API.”
  [Responses](../../spec/oas/v3.2.0.html#responses-object): “The Responses Object
  MUST contain at least one response code”. Supply required Info metadata,
  concrete `paths`, and response descriptions; no parameter serialization,
  schema expansion or external references are needed.

Preparation generates deterministic bytes from a fixed case catalog. The bound
endpoint is recorded as the only host-dependent fixture field. Cap root bytes
at 2 MiB and aggregate source bytes at 4 MiB. Generated documents and payloads
are evidence artifacts, not committed generated churn. The catalog and the
small independent expected-value definitions are tracked.

Setup validates roots through the existing CLI, retaining version, workflow and
source counts and zero-warning success. A benchmark-specific source checker
asserts the complete minimal OpenAPI shape, all operation IDs/methods/paths,
servers and responses against independent expected values; it is not a general
OpenAPI validator. Negative malformed/cyclic controls have separately expected
preflight failures and are never admitted through the positive path. For the
runtime cycle control, deserialize its explicit typed fixture and exercise the
public runtime directly after recording the validator rejection.

Workers recheck exact catalog dimensions, input identities and semantic shape.
The setup caller verifies retained SHA-256 receipts against files immediately
before each launch; inputs remain immutable during a series. Hash collection
and CLI validation are setup responsibilities, never measured descendants.

## Architecture and measurement contract

Own this in the `arazzo-runtime` Cargo example target `runtime_workload`, with
private benchmark support under its example directory. Its thin command surface
selects `prepare`, `service`, `worker` or `empty-worker`. Separate cohesive
support owners hold case generation/expected values, loopback service/session
control, invocation timing/oracles and raw record encoding. Focused integration
targets reuse that private support. Do not add benchmark responsibilities to
production `src/`, existing Criterion targets, or large legacy test targets.
The dependency direction is example/tests → published runtime/spec APIs;
production never depends on the example. CLI validation is a setup command,
not a Rust package dependency. No new code comments are introduced.

The worker accepts one catalog case and fully recorded configuration, prepared
fixture directory, service/control endpoints and invocation ID. Its raw record
contains a format version, case/manifest identity, invocation role (verification,
warmup, measured or empty), settings, metric name, monotonic duration in integer
nanoseconds, actual output/error classification, oracle outcomes and service
counters. Keep large actual values in separately identified evidence files.
The future launcher attaches binary/fixture hashes, source revision and dirty
state, build/host provenance, exit status, logs and process-resource counters.
The record is a development harness contract, not the CLI JSON contract or a
new runtime API. Encode no median, percentile, outlier policy or RSS estimate.

Start the source-build clock immediately before calling `build` on an already
configured builder holding the prepared parsed root; stop immediately on return.
Root reads/deserialization, builder configuration and all result/oracle work
are excluded. Native source reads, parsing, index/client creation and internal
temporaries remain included. After timing, check `Engine::workflows`, resolve
all expected IDs through public `resolve_operation_id`, and execute the selected
probe workflow against the service. This untimed probe proves actual routing.
Worker lifetime resource measurements will include that oracle execution; they
must not be labelled build-only memory or CPU.

For ordinary execution, read/deserialize roots and build a fresh engine first.
Start immediately before `Engine::execute`; stop when the selected handle method
resolves. Include dispatch, dependency planning, HTTP, retries, waits, expression
work and the chosen event behavior. Checks, serialization, destruction and
service snapshot queries occur afterward. No interval includes fixture
creation, CLI validation, service startup or compilation. Engine freshness
resets limiter/pool history; these are not warm-connection throughput claims.

For cancellation, start execution and immediately drive the selected completion
future while awaiting a service barrier proving every intended request has
reached its configured header/body stall. Clone the token first; timestamp
immediately before `cancel()` and stop at handle-method completion. Record
signal-to-API-completion, not launch-to-completion or task join. The separately
named watchdog control uses `execute_with_timeout` and checks `ExecutionTimeout`.
Never subtract intentional waits or control-process timings from a duration.

Fix Tokio at two worker threads, HTTP timeout at five seconds, channel capacity
at 1024 and response cap at 10 MiB for the ordinary catalog. Scheduler cases
use requests/second 0 and burst 20; the separately named default-policy DAG
series uses 10 and 20. Case identity includes parallel, trace, capture and
handle mode. Expression cases use collect with trace enabled to check every
criterion result; DAG/retry cases use collect with capture enabled and trace
disabled. Body cases cross collect/result_only with both capture and trace off.
Cancellation uses both handle modes with trace/capture off. No observer or
trace hook is installed. Changes to these settings create distinct series.

## Bounded cases and correct-result oracles

- **Source build:** four source sets, 1/8 files × 10/100 operations, with a fixed
  ten-step root per case. Step targets are source-qualified; the independent
  catalog enumerates every expected operation in the complete source set. Resolve every expected method/path; the probe's ten
  request paths and final output map are exact, including a last-index operation.
  Missing and malformed-source controls require `SourceDescriptionLoad` and
  `SourceDescriptionParse` respectively and zero requests.
- **Expressions:** eight sequential steps at 1/16/64 criteria, then 64 steps at
  sixteen criteria, all with exact 1 KiB JSON and eight scalar step outputs;
  workflow outputs expose all step results. Fix pointer depth at twelve and
  vary no other dimension. Check the entire expected output map, every trace
  criterion's identity/result and exactly one request per step. A false criterion
  at the first step requires `SuccessCriteriaFailed` and no later request.
- **DAG:** two levels of width 1/8/40/128, each second-level step depending on all
  first-level steps, plus a four-step diamond. Run identical documents sequentially
  and in parallel, with 1 KiB replies. Match all outputs and exactly one request
  per step. Service final-response-release/request-start logs check that no
  dependent request precedes any prerequisite release; exact predecessor-derived
  outputs and captured lifecycle projections complete the dependency oracle.
  Capture separately proves source-ordered replay.
  Eligible collect runs must contain no `SequentialFallback`. Cycles require
  `DependencyCycle` before HTTP; a named `onSuccess: end` control requires the
  precise control-flow fallback and its deliberately shortened request/output set.
- **Bodies:** one-step exact serialized 1 KiB/1 MiB/8 MiB JSON, plus eight parallel
  steps at 1 MiB, each with scalar-only and full-body output policies. Record
  bytes and node counts; assert exact scalars or complete parsed-body equality,
  path counts and service delivery lengths. Normal fixed-length and chunked
  controls must both succeed. Over-cap cases require `ResponseTooLarge` for both
  transfers with a recorded cap; partial delivery in that control is expected,
  and must never satisfy the successful complete-delivery oracle.
- **Retries:** one/eight independent steps at limits 0/1/3 with predetermined
  per-path 503 counts equal to the limit, then 200; run sequential and eligible
  parallel variants. Compare full final outputs and exactly limit-plus-one
  attempts per path, including zero-limit immediate success. Separate zero-delay
  and 0.1-second cases. Exhaustion controls remain 503, require
  `RetryLimitExceeded`, exact attempt records and preservation of settled sibling
  records in parallel mode. Reset service session counters before every invocation.
- **Cancellation:** one/eight parallel stalled requests at headers or between
  chunks, followed by a dependent level. Hold responses until cancellation has
  completed; require `ExecutionCancelled`, exact arrived request set and zero
  next-level requests. Both modes must complete within the existing 900 ms fixture
  bound against the five-second HTTP timeout, as a fixture control rather than
  a host SLA. Watchdog controls separately require
  `ExecutionTimeout`. A missing barrier is a fixture failure, never a cancellation
  measurement. Uncancelled chunked controls deliver the complete expected body.

These are bounded experimental fixtures, not product resource limits. Exact
expected maps and normalized event projections omit unstable timestamps but
retain IDs, counts, attempts, statuses and ordering. Tests perturb expected
successes (wrong values, omitted steps/criteria, altered counts/order and wrong
terminal errors) to prove the oracle cannot pass merely because an API returned.

## Service lifetime, verification and slice order

Service work stays in a sibling process, never a measured worker descendant.
Use ephemeral loopback data/control endpoints, predetermined response bytes,
a single active invocation and bounded per-connection I/O. Session begin/reset
requires quiescence; stale invocation IDs and overlapping begin requests fail.
The service supports begin, stall-readiness, final snapshot, release and stop.
Begin echoes the catalog version, case, invocation ID and response policy; the
worker rejects any mismatch before timing. A fixed benchmark-only HTTP header
set through `ClientConfig::default_headers` binds data requests to that invocation;
it is transport configuration, not new Arazzo syntax. Reject stale/missing IDs.
Snapshots echo that same identity, and release closes all session sockets before
reset is acknowledged. Record this header policy with the measured configuration.
It records monotonic request-header arrival and final-response release in its
own clock domain, timestamped before final bytes are written. These are causal
service markers, not client receipt or step completion times; do not compare
absolute timestamps with worker clocks or use post-write logging susceptible to
a response/next-request scheduling race.

Limit outstanding data connections to the catalog's maximum width, bound request
headers and route counts, and reject unknown paths. Control remains responsive
while data sockets stall. Use explicit readiness and join deadlines, not sleeps
as transport evidence. Bound setup/readiness at five seconds, ordinary execution
at sixty seconds (including the largest default-policy case), cancellation API
completion at 900 ms, and release/teardown at five seconds. A service session has
an independent finite deadline and service startup requires an explicit finite
lifetime budget. The documented local proof caller owns service process start/stop/reap and
terminates it on deadline; the future launcher assumes that same ownership.
An independent worker watchdog bounds the whole process at seventy-five seconds,
including synchronous build/setup, and exits nonzero if cleanup cannot finish;
workers never spawn children. The watchdog emits no fabricated valid record.
Shutdown failures
invalidate evidence. Bounded runtime shutdown cannot certify private task joins.

Implement in dependency order: (1) fixture catalog, preparation/source oracle and
service/session protocol with focused proof; (2) worker/raw-record contract and
source/expression/DAG paths; (3) body/retry/cancellation cases and adverse-lifetime
controls; (4) integrated release-example proof and maintainer instructions.
Each implemented slice must remain buildable and have an owned callable surface;
no slice claims whole-unit completion. Required fresh review checks actual
public-call timing, independent oracles, fixture legality and lifetime bounds.

System verification runs every positive catalog case and every named negative
control through the prebuilt executable, proves record/exit agreement and cleanup,
and retains same-host repeated raw observations without a threshold claim.
Focused tests cover malformed protocol, unavailable service, failed readiness,
wrong outputs/counters, worker failure and service teardown. Repository build
checks cover affected consumers; no changes to existing benchmark semantics or
production behavior are expected. OS-provider probes and CI artifacts remain
future proof, not inferred from this local exercise.

## System success condition and disposition

The independently reviewed example produces a fresh evidence bundle covering
all runtime-owned families with exact oracles and negative controls, clear
per-invocation boundaries, complete configuration/provenance and bounded
worker/service exits. A caller can launch one prebuilt worker, retain its raw
record and classify success without importing private Rust support or requiring
a resource/statistics runner. Existing benchmark series remain distinct.

**Disposition:** Decomposed under
[epic:ac-8ba62](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8ba62), 2026-10-29.
The Chief of Staff accepts this unit and directs its next bounded decomposition
under Steve's standing [maintainer mandate](../roadmap.md#standing-delegation-and-continuity)
and explicit unit-by-unit planning acceptance in
[ac-0be12](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0be12).
The private example layout and existing API/dependency use stay within that
accepted direction; no reserved dependency, production contract or policy
choice is being made. No implementation ticket is started by this disposition.

Independent fresh-context frontier review passed round 1 for
[f78863c](https://github.com/strefethen/arazzo-cli/commit/f78863c22f52e2bf3b766692421cff67225f3451)
(local; not pushed), with no blocking findings or unresolved authority gate.
[ac-9731e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9731e) retains the
contract and review evidence. This acceptance changes authority metadata only;
the reviewed design is unchanged.

[ac-10ce6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-10ce6) transfers the
accepted design into four serial implementation tickets:

1. [ac-a42f8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a42f8): fixtures,
   preparation and bounded sibling service.
2. [ac-4a221](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4a221): worker/raw
   records and source, expression and DAG workloads.
3. [ac-a34d5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a34d5): body, retry
   and cancellation workloads and adverse lifetime controls.
4. [ac-0d779](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0d779): integrated
   prebuilt-executable proof and maintainer instructions.

Fresh-context frontier transfer audit passed round 1 with no material gaps.
The epic retains bidirectional coverage and exact reviewed ticket revisions.
Lint and implementation readiness pass for all four; only the foundation is
queue-eligible. Source start clearance remains a kickoff check. No source ticket
has started, and system verification remains unobserved. The design above is
frozen; OS-resource tooling, statistics, CI and numerical budgets remain outside
this unit.
