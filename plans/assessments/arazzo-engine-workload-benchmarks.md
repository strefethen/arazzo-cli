# Engine workload benchmark assessment

**Disposition: proposed contract, awaiting acceptance.** Prepared 2026-10-29 for
[ac-5c27d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5c27d), against
`2bd383f086e9f7eb56551b488019bb538f1621af`. This assessment selects a measurement
approach for review; it neither authorizes implementation nor establishes a
performance baseline, speedup, resource guarantee, or regression threshold.

Recommend a small runtime workload target using existing public execution
surfaces, complemented by existing Criterion benchmarks and a separate process
resource launcher. This distinguishes costs developers can act on without
introducing a production API, a new crate, or a general benchmark framework.
No dependency is approved. The six families below are all required; fixture
sizes and sample counts are proposed experimental bounds, not product limits.

## Existing boundaries and metric ownership

| Metric | Sole owner and boundary |
| --- | --- |
| CLI smoke process latency | Existing [perf-baseline.sh](../../scripts/ci/perf-baseline.sh), `measure`/`summarize`: validate/list/dry-run process invocations, including startup and harness overhead. The workload target must not duplicate these series. |
| Isolated expression latency | Existing [expression.rs](../../crates/arazzo-expr/benches/expression.rs), `bench_evaluate`/`bench_path_depth_scaling`, under Criterion. |
| Synthetic criterion/clone/library costs | Existing [criteria.rs](../../crates/arazzo-runtime/benches/criteria.rs). These diagnostic loops are not production workflow throughput. Legacy dotted response expressions there are not fixture authority. |
| Root document parse/validation latency | Proposed `arazzo-validate` bench, calling `parse_bytes_with_diagnostics` from [lib.rs](../../crates/arazzo-validate/src/lib.rs), which already validates; no source-document indexing or second validation pass included. |
| Source loading/build and workflow completion latency; cancellation completion | Proposed `arazzo-runtime` workload target, using [builder.rs](../../crates/arazzo-runtime/src/runtime_core/builder.rs), `EngineBuilder::build`, [engine_impl.rs](../../crates/arazzo-runtime/src/runtime_core/engine_impl.rs), `Engine::execute`, and [events.rs](../../crates/arazzo-runtime/src/runtime_core/events.rs), `ExecutionHandle`. |
| Worker process CPU and maximum RSS | Proposed OS resource launcher around one prebuilt workload worker. It measures process lifetime, not an engine-only subinterval. |

The corrected process baseline already builds before timing, discovers Cargo's
executable, hashes it, and records three untimed warmups plus twenty measured
invocations. A failed invocation fails the baseline even though summaries use
successful samples. Its [CI consumer](../../.github/workflows/ci.yml) uploads
evidence; it does not set a regression budget.

Current [deps.rs](../../crates/arazzo-runtime/src/runtime_core/deps.rs),
`parallel_blocker`/`build_levels`, allows plain failure retries and rejects
parallel eligibility for control-flow actions, retry references, and subworkflow
steps. [engine_parallel.rs](../../crates/arazzo-runtime/src/runtime_core/engine_parallel.rs),
`execute_parallel`, spawns a whole dependency level and joins it before advancing.
These are current implementation facts, not performance promises. Benchmark
fixtures must not assume the proposed future scheduler is delivered.

## Six bounded workload families

All fixtures use synthetic data, deterministic identifiers, local source files
and loopback HTTP only. Generate and hash fixture bytes before measurement;
validate each positive Arazzo 1.1.0 fixture and its referenced OpenAPI document
before sampling. Use unique operation IDs and valid local document references.
The [conformance audit](arazzo-spec-conformance-audit.md) identifies historical
deviations; neither it nor existing examples grants syntax authority.

The vendored [Arazzo tool behavior](../../spec/arazzo/v1.1.0.html#tool-behavior)
requires: “Tools MUST respect all declared dependsOn relationships.” Its runtime
expression grammar and failure-action definitions also govern fixtures. Use
JSON-pointer body access, not an inferred dotted-path extension. Unsupported
features are explicitly rejected or omitted from the fixture manifest, never
silently converted into successful benchmark cases.

**1. Document loading.** Propose root documents with 10/100/1,000 steps, capped
at 2 MiB serialized bytes, and local OpenAPI source sets of 1/8 documents with
10/100 operations each, capped at 4 MiB aggregate. Avoid a full cross-product:
hold source size fixed for root scaling, then root size fixed for source scaling.
The validate owner times root bytes → validated model/diagnostics;
fixture reads occur outside that interval. The runtime owner separately times
builder entry → built engine, with a prepared parsed root and native local source
reads/index construction inside. Do not add these measurements into a purported
single engine-overhead number. Expect exact workflow/operation identities,
diagnostic counts, and one request to a selected indexed operation in an untimed
correctness run. Missing and malformed source files must retain their errors;
malformed root bytes must fail parsing. Source loading or validation failure
invalidates the positive series. Anchors:
[source_document_loader.rs](../../crates/arazzo-runtime/tests/source_document_loader.rs),
`native_relative_document_indexes_operations_and_servers`,
`injected_bytes_drive_index_and_server_with_exact_single_resolved_call`, and
`native_malformed_document_preserves_exact_parse_error`.

**2. Expression-heavy execution.** Propose 8/64 sequential HTTP steps, each
returning the same 1 KiB JSON shape, with 1/16/64 simple criteria per step and
eight scalar outputs. Independently vary criterion count at eight steps, then
step count at sixteen criteria; cap pointer depth at twelve. Time execute →
completed result with parsing/build excluded. The exact output map, all expected
criterion outcomes, and request count are correctness oracles; one deliberately
false criterion must produce the expected failure, never a fast success. Check
fixture expressions against `ExpressionEvaluator` through the existing
`bench_evaluate` cases, then use the actual engine criterion path. The runtime
[parallel_event_drain.rs](../../crates/arazzo-runtime/tests/parallel_event_drain.rs)
tests `parallel_step_drains_seventy_criterion_events_while_executing` and
`parallel_step_drains_high_volume_criterion_events_while_executing` establish the
nearby event-volume hazard. Isolated evaluator timing remains with Criterion;
this family includes HTTP, state updates and event handling.

**3. Wide DAGs.** Propose widths 1/8/40/128 in two levels, at most 256 steps,
plus a four-step diamond control. Use 1 KiB immediate responses and run identical
documents sequentially and in parallel. Compare exact final outputs, request
counts, dependency ordering and source-ordered collected events. The parallel
series must emit no sequential fallback. A cyclic dependency control must fail
before HTTP; a control-flow-action control must report the expected sequential
fallback and must not enter the eligible-parallel series. Time execute → result,
including dependency planning. Fix the Tokio worker count and explicitly record
rate/burst settings: propose a disabled rate limiter for scheduler experiments
and a separately named default-policy series, never mix them. Current
[client.rs](../../crates/arazzo-runtime/src/runtime_core/client.rs),
`RateLimitConfig`/`RateLimiterState`, defaults to 10 starts/second, burst 20;
those waits can dominate wide workloads. Correctness anchors in
[engine_parallel.rs tests](../../crates/arazzo-runtime/tests/engine_parallel.rs):
`execute_parallel_respects_explicit_local_depends_on`,
`execute_parallel_detects_explicit_depends_on_cycle_before_http`, and
`execute_parallel_outputs_preserved_and_diamond_dependency`.

**4. Large HTTP bodies.** Propose exact serialized payloads of 1 KiB/1 MiB/8 MiB,
one step initially, then eight parallel steps at 1 MiB. Keep a deterministic JSON
shape and declared output policy fixed; compare scalar-only output and full-body
retention as separately named cases. Record payload bytes and node counts.
Expect exact declared scalar values or full parsed body equality, complete byte
delivery, and correct request count. Measure execute → result plus fresh-worker
RSS/CPU. Run `result_only` and `collect` as distinct series; trace and capture
settings are explicit. Use fixed/chunked transfer controls and an over-cap
negative control against `EngineBuilder::max_response_bytes`; record the actual
configured cap. The production streaming cap belongs to `client.rs`; the
[http_cancellation.rs](../../crates/arazzo-runtime/tests/http_cancellation.rs)
test `uncancelled_chunked_response_still_succeeds` checks normal chunk completion.
A `ResponseTooLarge` error is correctness evidence, not throughput success.

**5. Retry paths.** Propose one and eight independent steps, retry limits 0/1/3,
with deterministic per-path service counters returning 503 for the selected
number of attempts, then 200. Plain retries alone qualify for parallel cases.
Expect exactly failures-plus-one attempts on recovery; exhaustion must return
the expected failure and preserve settled sibling results. Reset counters before
every invocation. Separate zero-delay cases from 100 ms configured retry waits;
report the latter as inclusive latency. Never subtract requested sleep durations
and call the remainder exact engine overhead. Anchors in the parallel tests:
`execute_parallel_retries_a_failed_step_inside_its_level` and
`execute_parallel_retry_exhausts_its_budget_and_still_records_later_siblings`.
Measure execute → result and process resources; retained attempts are part of
the explicitly selected event/trace mode.

**6. Cancelled slow I/O.** Propose one/eight requests stalled before headers or
between body chunks, with a dependent next level. A server readiness barrier
confirms requests reached the intended stall; observer notifications or sleeps
alone do not prove pending transport. Clone `handle.cancel_token()`, timestamp
immediately before `cancel()`, and end when `collect()` or `result_only()`
resolves, keeping modes separate. This is cancellation-signal-to-API-completion
latency: the public handle does not expose a task join primitive. Require
`ExecutionCancelled`, no next-level request, and bounded server/worker cleanup;
use separate watchdog controls expecting `ExecutionTimeout`. Existing
`external_cancel_interrupts_pending_headers_via_execute`,
`external_cancel_interrupts_pending_body_via_execute_step`, and
`parallel_external_cancel_records_the_level_without_completing_the_workflow`
in `http_cancellation.rs` supply the semantic controls. Retain their 900 ms
completion control against a 5-second HTTP deadline as a fixture check, not a
host SLA. Deadline breaches and wrong terminal classifications fail the series;
they are not latency samples demonstrating success.

## Measurement and comparison contract

Use a separate service process so its allocations and CPU do not enter worker
resource accounting. The service binds an ephemeral loopback port, announces
readiness, serves predetermined bytes, counts requests, and has bounded teardown.
No external network, build, fixture generation, source validation, or service
startup occurs within a workflow interval. Recreate the engine per invocation
outside that interval to reset limiter state and connection-pool history. This
defines a fresh-engine workload; do not describe it as warm-connection throughput.

For each workload/configuration, propose three untimed warmups and twenty
measured invocations in a recorded order. Retain every outcome and raw monotonic
duration, plus requested/successful/failed counts. Check correctness after the
timer stops, but invalidate the series if any positive invocation fails its
oracle. Expected-negative controls are separately named. Report median, minimum,
maximum, and nearest-rank p95 (sorted element `ceil(0.95*n)`; median averages the
middle pair for even `n`). With twenty samples p95 is only the nineteenth sample,
not reliable tail characterization. No outlier removal or pooled failed runs.
Criterion retains its own sampling/estimator and must not be relabelled as this
twenty-invocation protocol.

For peak memory, run each measured invocation in a fresh worker process under
the OS resource launcher. Maximum RSS includes worker setup, execution and
teardown; it is neither heap allocation volume nor execution-only memory. Keep
an empty-worker control, but do not subtract maxima to manufacture engine RSS.
Process user+system CPU is reported separately from wall time; CPU is not wall
time minus upstream delay. Immediate-loopback runs still include transport and
service scheduling. Delayed controls expose wait sensitivity without claiming
exact engine-cost subtraction.

On Darwin, propose `/usr/bin/time -l` and preserve the raw maximum-resident-set
counter in bytes, distinct from peak memory footprint. The
[Apple getrusage source](https://github.com/apple/darwin-xnu/blob/main/bsd/man/man2/getrusage.2)
defines the unit. On Linux, propose GNU time `%M`, `%U`, `%S`, `%e`; preserve raw
units/provider and normalize maximum RSS from KiB to bytes after a platform
probe. [Linux getrusage](https://www.man7.org/linux/man-pages/man2/getrusage.2.html)
defines KiB and warns that child accounting is the largest child's peak, not
aggregate process-tree peak; [GNU time](https://www.gnu.org/software/time/manual/time.html)
defines its resource fields. Workers must not spawn measured descendants.

The current Mac successfully ran `/usr/bin/time -l /usr/bin/true` only after
approved execution outside the initial sandbox restriction (`sysctl
kern.clockrate` was denied). That establishes tool access, not workload evidence.
Linux execution was unavailable here. Future runners must validate platform
units, parsing and permissions; denied/missing counters are unavailable, never
zero. Do not silently replace high-water RSS with polling. Optional sampled RSS
must be a separate approximate diagnostic, acknowledging missed short peaks.

Retain source commit and dirty-tree status, lockfile/fixture/binary hashes,
Cargo-reported executable, rustc/profile/target/features, allocator, harness/tool
versions, OS/kernel/CPU/architecture/RAM, power state and concurrent load.
Record Tokio workers, rate/burst, response cap, timeouts, tracing/capture/event
mode, sizes, run order and all commands. Store build logs separately. Compare
paired revisions on the same host/configuration, alternating order to expose
drift; do not pool machines, architectures, operating systems or different
resource providers. Shared CI host samples remain advisory.

## Resource policy, implementation slices, and acceptance

The accepted [runtime resource contract](runtime-resource-contract.md) owns the
future bounded-session and charged-retention policy. Charged bytes and OS RSS
answer different questions; this suite must not become a second enforcement
ledger. Its historical sampled RSS and launch-to-timeout overrun are calibration,
not an exact high-water or token-cancellation baseline. Open
[ac-38c60](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-38c60) owns loading/graph
resource admission and [ac-7a6ba](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-7a6ba)
owns synchronous evaluation resource/cancellation guarantees. These experiments
do not settle those guarantees or claim future bounded APIs are delivered.

After Steve explicitly accepts this recommendation, partition work into Tier 2
slices: (1) `arazzo-validate` root-load
fixtures/bench; (2) `arazzo-runtime` public-path workload worker and focused
correctness targets for source-build and execution families; (3) repository
benchmark tooling for platform CPU/RSS launch and evidence reporting; then
(4) CI artifact integration after local methods pass. Existing expression and
criteria benches remain their metric owners. Exact file placement and any
dependency need require reviewed unit-scoped tickets; avoid growing existing
large engine/test files or exposing private methods merely for benchmarking.

First collect valid baselines and repeated same-host control runs, assess noise,
then propose metric-specific absolute/relative regression budgets with evidence
and explicit later acceptance. A bounded smoke subset may follow measured cost;
it cannot replace the six-family baseline. No numerical budget is selected here.
Independent review must establish statistical definitions, accounting boundaries,
semantic oracles and host comparability before this design ticket is complete.
