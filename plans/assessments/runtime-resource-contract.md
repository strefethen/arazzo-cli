# Runtime resource contract

**Disposition: accepted authority for implementation planning.** Prepared 2026-09-08
against `f1ab35f978330a86db57e025a5b78a48839076a9` for
[ac-e8235](https://sonos.scapedeck.com/docs/ac-tickets/ac-e8235), retained revision
`93eeefbc66e1f2646f23ed9e029bac8959044f73f08243225c8af043000d7c06`.
The retained JSON is `/private/tmp/ac-e8235-contract-93eeefbc.json`, SHA-256
`59e51f748dfcda3d545e744b4beafdb30a94c85d971f33cb1f72076cb97b0e99`.
Live requirements match that snapshot. Warning-level lint fails solely with
`plan-not-set`; Steve authorized this planning assessment despite that warning.
That exception does not make the future implementation ready.

**Acceptance recorded 2026-09-08:** Steve selected the reviewed contract at
`95abd56f624a9da7fcb8b91b7b45d3afcbd0a6c7`, accepting all five choices:
concurrency/rate/response limits; aggregate charged-memory and artifact limits;
the versioned bounded API and CLI migration with legacy compatibility; terminal
cancellation and no-partial-publication semantics; and logical-retention scope
with the separately tracked residual-resource work. The direction below is
selected, not pending approval. Implementation is not delivered; resulting
tickets still require reconciliation, warning-level lint and transfer review.

## Selected direction

Introduce an explicitly versioned bounded execution session, using the existing
engine, with two cooperating controls: bounded admission of independent work
and one charged-memory ledger for retained execution data and evidence. Move CLI
execution to that surface during implementation. Preserve the frozen runtime v1
surface through explicit adapters; do not retrofit new error meanings into it.

An accepted [recording/replay decision](replay-fidelity-decision.md) already
requires complete candidate evidence to remain in bounded memory for the whole
run. A cap failure must terminate bounded execution and reject publication; it
cannot truncate evidence, spill to disk, emit a partial artifact, or silently
switch modes. This contract selects resource policy, not Arazzo syntax or new
scheduler eligibility.

## Current evidence and owners

| Owner / anchor | Verified behavior and implication |
|---|---|
| [builder.rs](../../crates/arazzo-runtime/src/runtime_core/builder.rs), `EngineBuilder`; [client.rs](../../crates/arazzo-runtime/src/runtime_core/client.rs), `RateLimiterState`, `HttpClient::request` | Sequential mode by default; 10 logical request starts/second, burst 20, shared across an engine's clones. Redirects share their logical request's token. A 10 MiB default response-body cap checks Content-Length and each received chunk before appending. There is no in-flight cap; response JSON parsing and retained copies are additional memory. |
| [engine_parallel.rs](../../crates/arazzo-runtime/src/runtime_core/engine_parallel.rs), `execute_parallel`, `execute_parallel_step` | The whole level is spawned, with cloned variables per task. Local event vectors and completed results survive until all siblings join, then results/events are processed by source index. Limiting HTTP alone leaves queued tasks, clones and completed results unbounded. |
| [events.rs](../../crates/arazzo-runtime/src/runtime_core/events.rs), `collect`, `result_only` | The outer channel defaults to 1024 items; a parallel step's local channel holds 64. Both are item counts, not byte caps. `collect` drains into an unlimited vector; `result_only` drops its receiver but does not remove task-local retention or accumulated workflow outputs. |
| [state.rs](../../crates/arazzo-runtime/src/runtime_core/state.rs), `VarStore`; [engine_trace.rs](../../crates/arazzo-runtime/src/runtime_core/engine_trace.rs), `build_trace_response`; [handlers.rs](../../crates/arazzo-cli/src/handlers.rs), `run_workflow`; [trace.rs](../../crates/arazzo-cli/src/trace.rs), `prepare_trace_for_write` | Parsed values, raw response bytes, trace strings, variables and outputs coexist. CLI collects, clones trace records, clones them into the file, then applies the v1 2048-byte diagnostic policy. Final file size is not peak retention. |
| [control.rs](../../crates/arazzo-runtime/src/runtime_core/control.rs), `sleep_with_cancel`; client `await_transport`; [http_cancellation.rs](../../crates/arazzo-runtime/tests/http_cancellation.rs) | Token waits, sends and chunk reads support cancellation. Existing integration tests use a 900 ms completion bound against a 5-second HTTP budget. This does not establish a hard deadline for synchronous parsing, callbacks or OS scheduling. |
| [MCP handlers](../../crates/arazzo-mcp/src/handlers.rs), `run_workflow`; [protocol.rs](../../crates/arazzo-mcp/src/protocol.rs), `serve` | Serial dispatch, unbounded decoded input queue, collected runtime events and multi-stage JSON/result serialization. Default HTTP/execution timeouts are 30/300 seconds. Output framing does not bound the already-built result. |

The scheduler owner is
[ac-c1a40](https://sonos.scapedeck.com/docs/ac-tickets/ac-c1a40), live revision
`ed54f7954db92428107684fd9718c7c78cbc074f70fb4f704f38db2b338f32f6`, open and
blocked by its existing prerequisite. It owns source-stable topological
execution from one dependency plan. Today
[deps.rs](../../crates/arazzo-runtime/src/runtime_core/deps.rs) still owns the
older level builder and eligibility check. Do not treat the desired scheduler
as delivered. The normative constraint remains: “Tools MUST respect all declared
dependsOn relationships” and sequential tools “MUST execute steps in an order
that satisfies both explicit (dependsOn) and implicit (output reference)
dependencies” ([vendored Arazzo §5.8.5.2.4](../../spec/arazzo/v1.1.0.html#tool-behavior)).

[ac-950a8](https://sonos.scapedeck.com/docs/ac-tickets/ac-950a8), revision
`88b24cb07648185a950dd7747bea2345d9042cbb1451aab7e9478a7085f380c9`, remains
the open MCP security/framing/queue/result-memory/timeout decision owner. Its
referenced `plans/current/mcp-security-hardening.md` does not exist; it supplies
no accepted ceilings. [ac-5c27d](https://sonos.scapedeck.com/docs/ac-tickets/ac-5c27d),
revision `4ea766211f248431c61cabccbf98215dc2fb991045a66282062f6fe9edebe0da`,
owns the later six-family benchmark methodology and regression budgets. This
assessment provides calibration, not a competing benchmark framework.

## Measurements and selected envelope

The corrected loopback probe uses validated Arazzo 1.1.0 documents, unique
`operationId` values in local OpenAPI 3.1.0 documents, text/plain bodies, and a
threaded server with accept backlog 128. Every execution follows successful CLI
validation. Three fresh-process repetitions per case used the release CLI,
unchanged default rate settings, a 2048-byte trace policy for tiny bodies and a
1 MiB trace policy for large bodies. Compilation and fixture creation were
outside timed intervals. These are exploratory calibration samples, not a
statistical performance baseline.

| Workload | Median wall time / completed requests per second | Server peak active requests | Median sampled peak RSS | v1 file bytes |
|---|---:|---:|---:|---:|
| Sequential 8 × 1 KiB, 100 ms upstream delay | 878.0 ms / 9.1 | 1–2* | 6.2 MiB | 24,400 |
| Parallel 8 × 1 KiB, 100 ms | 130.4 ms / 61.4 | 8 | 6.8 MiB | 24,401 |
| Parallel 40 × 1 KiB, 500 ms | 2563.1 ms / 15.6 | 24–25 | 9.2 MiB | 120,130 |
| Parallel 8 × 1 MiB, 100 ms, trace | 156.0 ms / 51.3 | 8 | 78.2 MiB | 8,413,079 |
| Same, without trace | 129.5 ms / 61.8 | 8 | 49.1 MiB | 0 |
| Parallel 20 × 1 MiB, 100 ms, trace | 184.3 ms / 108.5 | 20 | 130.2 MiB | 21,031,991 |
| Parallel 40, 3-second stalled headers, 750 ms execution timeout | 776.6 ms / no completed requests | 27 | 7.8 MiB | 804 |

The cancellation case started 27 requests and exited 20.2–32.4 ms after the
configured timeout measured from process launch. This overrun includes startup,
reporting and polling error; it is not token-cancel-to-join latency. The v1 file
does not prove complete failed-attempt evidence. Peak active counts cover server
handler entry through write completion, an approximation of transport in-flight
work. *One sequential sample counted two handlers during server write cleanup;
it does not demonstrate two overlapping client requests. `/bin/ps -o rss= -p
<pid>` samples about every 10 ms plus process-launch cost and misses short peaks
(untraced large-body samples ranged 23.5–55.2 MiB). The 20-body case ranged
120.8–134.2 MiB. These measurements combine engine cost
with deliberate upstream delay and CLI serialization; do not subtract delay and
call the remainder precise engine overhead. No v2 recorder exists to measure.

The inspected-script independent check repeated the eight-request tiny case:
exit 0, peak 8, 155.4 ms, sampled RSS 6.8 MiB. The slower one-off reinforces the
host/sampling limitation. All probes use synthetic data; the earlier bare-path
fixture and small-backlog exploratory run are excluded from this evidence.

An inspected `/private/tmp` Rust driver called the public runtime API with
`requests_per_second=0`, burst 1, parallel and trace enabled, and `.collect()`;
all other client defaults remained unchanged. With rate limiting disabled, 64
tiny requests reached peak 64 (529.8 ms, 10.7 MiB RSS); 64 × 256 KiB reached
peak 64 (154.8 ms, 413.4 requests/s, 83.8 MiB RSS, 513 retained events and
16,945,272 serialized trace-step bytes). A 750 ms timeout cancelled 64 stalled
requests with 17.4–22.8 ms process overrun. Those results demonstrate the missing
concurrency bound independently of the default rate limiter.

Content shape matters more than file length: eight JSON responses of 1,048,565
bytes each, containing about 95,324 short strings apiece, retained 65 events and
9,937,625 serialized trace-step bytes but reached median 179.2 MiB RSS (351.4 ms,
22.8 requests/s, peak 8). Serializing trace steps in this driver is itself part
of its measured memory; that byte count is a representation-size measurement,
not an allocator census. The future 128 MiB ledger may reject this workload;
raw body bytes and artifact size do not establish admission safety.

Build/host: `cargo build --release -p arazzo-cli --locked --offline`, Apple M1 Max,
10 cores/64 GB, Darwin 24.5.0, Rust/Cargo 1.98.0. CLI SHA-256:
`ae059b04651c02cb69aa48d8ed497ed61a9225207ad460a350ee3e2db0616de1`;
workspace lock: `eba9d9f12d50ec27cb30a1d51ee9b11e277fc7005bf2f55a1c45feb17588bfe1`.
The external driver used the workspace lock as its seed, then `cargo build
--release --offline` in `/private/tmp/arazzo-resource-driver`; no repository
dependency changed. Driver binary SHA-256:
`4d918c4f551e9dd1027e3b99c3699867285a14f9092b904c093fe93f2ac9b7d5`;
`src/main.rs`: `d70320944cd318a4be08df6bcdd706982125ebd0f0934de73d8d0445927c7cd1`;
driver lock: `100ed342bfc7fdad63f193ea5fab8037e16558a87d51d5e35fa38e9a2b6e4138`.

Reproduction commands (fixtures, raw stdout/stderr, per-run commands and hashes
are retained in the named results directories):

```sh
python3 /private/tmp/arazzo-resource-envelope-probe.py --binary /Users/stevetrefethen/github/arazzo-cli/target/release/arazzo-cli --out /private/tmp/arazzo-resource-envelope-f1ab35f-v3
python3 /private/tmp/arazzo-resource-envelope-driver-probe.py
python3 /private/tmp/arazzo-resource-envelope-json-probe.py
```

The shared probe SHA-256 is
`c544a9448f9287f9fb5bc91166176f3f66fde7b3be493f0203a8b7f9a42ea657`;
driver runner `885a421abe23909da3a8203a0e452a768785fce832b462da9a7f3f31bb4b570a`;
JSON runner `af46ee1f4789b9351215a8adb3bd3727bfeab849dc09710a96c4c75023457f06`.
All three final manifests bind that same shared probe:

| `/private/tmp/` evidence file | SHA-256 |
|---|---|
| `arazzo-resource-envelope-f1ab35f-v3/results.json` | `f4c413f563d7e351cba606c0a3ab765f412e2f227d7b74fe1c6e827ff526dbd0` |
| `arazzo-resource-envelope-f1ab35f-driver/results.json` | `0c0a5acee74a990f329efae16805d09e2a3883bedb777e4fad4bb6aac0763cb2` |
| `arazzo-resource-envelope-f1ab35f-json/results.json` | `2854e9c7e034732c987a651d6f311f641a9749778ea30f513b048547d0c28c77` |
| `arazzo-resource-envelope-independent/result.json` (single independent check) | `d357dd5ff4129b88d17ba61129bff575d3fa940dba6d759be741d66d958a6041` |

These local evidence files are ephemeral, not production artifacts. The matrix
above retains the decision-relevant observations in the repository. This is one
host, three samples per main case, with no warmup, allocator instrumentation or
implemented-cap comparison; it cannot select statistical regression budgets.

| Selected bounded-session control | Default | Hard maximum / application |
|---|---:|---|
| Admitted HTTP requests and launched-but-unretired parallel steps | 8 | 32 per engine HTTP pool; the same configured count limits each invocation's window. Valid overrides 1–32. |
| Aggregate charged retained execution/evidence and recorder working memory | 128 MiB (134,217,728 bytes) | 512 MiB (536,870,912 bytes) per root invocation, shared by its nested work and publication. |
| Complete serialized v2 artifact, including newline | 32 MiB (33,554,432 bytes) | 128 MiB (134,217,728 bytes), additionally constrained by the aggregate budget. |
| Transport response-body bytes | Existing 10 MiB | Keep 10 MiB as the first bounded surface's supported ceiling; library callers may lower it. Legacy builder behavior remains separately compatible. |

Eight retains the measured small-level concurrency benefit while curbing the
25–27 live requests observed under the current rate limiter. Thirty-two permits
an explicit increase above that observed envelope but forbids unlimited fanout;
it is a conservative policy choice, not a measured optimum. A 64 MiB aggregate
would already be tight around the eight-body trace case before additional v2
classification/encoding/report work; 128 MiB supplies initial headroom. The
32 MiB artifact ceiling accommodates the measured 20 MiB v1 artifact size while
requiring whole-run admission of its larger v2 representation. The 4× memory
and artifact overrides are explicit trusted-user headroom, not a promise of
proportional throughput or proof that maximum-sized runs fit. The aggregate cap
always wins: choosing all maxima together may fail. Actual v2 allocation tests
must validate the charges and establish usable workload capacity before release.

Keep rate defaults at 10 logical starts/second and burst 20, and the existing
1024/64 channel item capacities as batching choices, with their allocated storage
charged. Rate controls start frequency; concurrency controls outstanding work;
neither bounds accumulated evidence. Channel capacity and zero-body records
cannot bypass the aggregate byte reservation. No unlimited setting is introduced.

## Admission, ordering and cancellation

The runtime resource owner holds a shared-per-engine HTTP permit pool. Every
live HTTP attempt, including execute-step, retry and nested-workflow requests,
uses it. Acquire before waiting for the existing rate token, check cancellation
again before send, and retain the permit through all redirect hops, final body
read and cleanup. Release it before retry delay. A permit includes admitted
requests waiting for a token, so it conservatively bounds physical in-flight
requests. Sequential execution stays sequential. Independent engines are
independent budgets; callers own admission across engines and root invocations.

Within an eligible parallel level, admit at most the configured concurrency
count of **launched but not yet retired** steps. Retirement is source-index
FIFO, so a slow first sibling deliberately applies backpressure to later work.
Spawn lazily; do not allocate the whole level's tasks and make them wait on a
semaphore. All admitted siblings read the same immutable level snapshot. Retire
and release a completed result before admitting its successor; the result's
evidence reservation transfers to its retained destination rather than vanishing.
Never start a dependent level before the current level completes.

Preserve source-order logical BeforeStep, event, trace and result sequences.
BeforeStep remains a scheduling marker, not a promise that a network request
has started; today's RequestSent also precedes the rate wait. Physical-start
telemetry belongs at the transport send boundary. Callback wall-clock timing
will change under bounded admission and is not claimed isomorphic. Preserve
ordinary step-failure behavior: finish current-level sibling attempts, select
the same source-order failure, suppress later legacy result publications as
today, then stop dependent levels. Complete v2 evidence still accounts for every
attempt made. Resource failure or cancellation instead immediately stops new
admission, redirects and retries, cancels active work and drains/joins owned tasks.

Queue-full waits must be cancellation-aware and must run concurrently with
draining; retain the repair proved by
[parallel_event_drain.rs](../../crates/arazzo-runtime/tests/parallel_event_drain.rs).
A memory reservation that cannot fit fails immediately; waiting for a collector
that intentionally retains data until completion would deadlock. Resource
failure is a latched terminal cause, never a retryable upstream failure or an
action-routing input. A later timeout cannot overwrite it. Already-performed
remote effects are not undone; cleanup never re-executes the workflow.

Effective-mode reporting comes from the runtime's existing eligibility owner,
using its canonical effective-action view when the scheduler ticket lands.
Report requested mode, effective sequential/parallel mode and fixed reasons:
not requested, debug controller, effective control-flow action, workflow target.
Do not add parallel eligibility for actionful workflows or subworkflows. Preserve
execute-step dependency ownership. CLI/MCP must not copy the predicate or add
an Arazzo field. An ineligible parallel request retains today's sequential
behavior; this adds visibility to an existing decision, not a new fallback.

## What the byte contract counts

The aggregate limit bounds **charged execution-owned retention and recorder
working memory**, not process RSS. Its lifetime starts before bounded-session
input/evidence admission and ends after the final result and publication buffers
are released. Charge every owned raw/request/response buffer, decoded value and
container, variable/output clone, task-local event, channel slot, ordered result,
collected event, candidate manifest/evidence field and diagnostic/report buffer.
Fixed record and container overhead is charged even for empty payloads, so tiny
events cannot evade the byte limit. Shared immutable backing is charged once
while held; deep clones receive new reservations. Moves carry reservations.

Reserve before allocation or growth with checked arithmetic, including spare
capacity and conservative container/node storage. Accounting tables for each
owned representation must state their allocator-independent charge and be
validated by the implementation's focused tests. Length-only accounting and
checking a finished `serde_json::Value` are insufficient. The shared decoder's
bounded parsing path must reserve nodes/strings/containers as they are created;
the resource layer supplies reservations, not a second JSON parser. A required
parser or classifier without bounded scratch admission makes the recording
unclassifiable; do not run it unbounded and check afterward.

Whole-run recording includes preflight source/configuration inspection, all
candidate fields, classifier traversal stacks and pattern-matching scratch,
JSON parsing, base64/escaping expansion, exact serialized-size counting and
stdout/stderr/verbose report buffers. Classify original borrowed data without
copying when possible; charge any copy or parse owned by recording. Large source
documents can be inspected in bounded chunks only where the canonical classifier
can preserve its full rules across boundaries; otherwise reject. No new weaker
classifier, truncation or uninspected suffix is permitted.

Before opening a publication destination, complete the publication gate and
perform bounded in-memory serialization with checked exact length, including
encoding expansion and the final newline. The complete serialized artifact,
still-live evidence and reports must fit the same aggregate ledger. If necessary,
reject an otherwise successful run; the serialized-size ceiling is additional
to the aggregate cap. Never infer allocation cost from the final JSON length or
use a fixed unproven response-size multiplier.

Record/replay rejection is monotonic. A classified secret takes the accepted
secret rejection route; a missing classifier takes the unclassifiable route;
resource exhaustion takes the fixed resource route below. Any of them permanently
prevents publication, even if earlier data was eligible. No streaming, spool,
checkpoint, temporary evidence file, sidecar, projected trace or v1 fallback is
allowed. Only after the complete gate passes may atomic publication write the
already-validated artifact; a failed publication leaves an existing destination
byte-identical and reports failure. The existing v1 temporary writer is not a
permitted pre-gate v2 sink.

Immutable loaded documents/indexes, caller-owned values before admission,
arbitrary user callbacks, general evaluator transients, allocator overhead,
Tokio/HTTP/TLS/kernel buffers and other engines are outside this ledger. RSS
measurements include some of these costs and cannot prove the logical invariant.
Recording's classifier/parser/serialization allocations are explicitly inside,
even though general pre-session document parsing and evaluation are outside.
Do not advertise the contract as a hostile-input process-memory sandbox.

## Public configuration and safe reports

Use one runtime-owned `ResourceLimitsV1` contract and versioned bounded
session/handle/result/report types. Validate policy at engine construction and
keep it immutable for that engine; each root invocation receives its own ledger
while engine clones share the same HTTP permit pool. They call existing owners;
there is no second engine, resolver, decoder or scheduler. The frozen
[api_v1](../../crates/arazzo-runtime/src/lib.rs) models retain their field and
error meanings. Legacy methods remain explicitly outside the new retention
guarantee; conversion to v1 is deliberate and cannot convert a resource failure
to success or publish partial recording evidence. Deprecating legacy unbounded
collection can be a later compatibility decision, not an accidental break.

Selected CLI options are `--max-concurrency`, `--execution-memory-bytes` and
`--record-max-bytes`; the last applies only to `--record`. Decimal integer bytes
avoid unit ambiguity. Reject zero, negative, fractional, malformed, overflowing
or above-ceiling explicit values before workflow side effects; never clamp or
interpret zero as unlimited. For recording, the serialized cap must not exceed
the aggregate cap; reject that combination before execution. The record-size
option requires record mode.
Neither concurrency nor a larger memory cap turns parallel mode on. Preserve
the existing transport/rate defaults and `--trace-max-body-bytes` v1 policy;
transport bounds are independent from retained evidence bounds.

CLI `run`, execute-step, test and replay adapters consume the bounded surface;
authoritative record/replay require it. Record/replay preflight and all reports
participate in the same publication gate. For ordinary bounded runs, return a
fixed resource failure and optional bounded effective-policy/high-water counters;
never include rejected values or upstream messages. Selected fixed reports are:

| Surface | Failure contract |
|---|---|
| Ordinary bounded runtime/CLI | New versioned `ResourceLimitExceeded`; `kind: error`, `code: RUNTIME_RESOURCE_LIMIT`, `error: Execution resource limit exceeded`; CLI exit 1. |
| `--record` | Only `kind: error`, `code: RECORD_RESOURCE_LIMIT`, `error: Recording rejected: resource limit exceeded`; exit 1. |
| Replay resource preflight/execution | Only `kind: error`, `code: REPLAY_RESOURCE_LIMIT`, `error: Replay rejected: resource limit exceeded`; exit 1. |

Recording/replay rejection reports contain no user-derived path, identifier,
count, length, digest, mode explanation, partial output or exception detail.
Suppress previously buffered reports on rejection. On eligible success, the
versioned runtime report supplies effective mode/reasons, configured limits and
charged high-water values; the replay manifest records effective resource and
rate settings through its existing accepted configuration owner. CLI output
schemas, help and contract snapshots change together. New code names and defaults
are accepted for implementation; existing `RUNTIME_*` meanings stay intact.

MCP remains serial. Its existing security decision owns trusted startup ceilings,
tool-argument validation, bounded framing/backlog, result construction and report
projection. The selected direction supplies the same runtime defaults as an
integration input, with no agent-requested increase above the server's accepted
ceiling; reject an explicit excess before execution instead of clamping. There
is no new MCP dispatcher semaphore. Its final JSON value, escaped text envelope and wire frame
need their owner's budget as well as the runtime ledger; reserve before handoff
or copying and release source reservations only when ownership truly ends. MCP
integration waits for that decision's exact limits and timeout policy, including
its proposed 30/300-second maxima. This document does not accept those policies.

## Smallest implementation sequence and proof

With resource direction accepted, reconcile tickets through `tkt`, obtain fresh
transfer review and warning-level lint, then dispatch these contained slices.
None is currently implementation-ready. Each has one owning Rust crate;
focused tests are native test targets, not new production abstractions.

| Order / unit | One coherent outcome and dependency |
|---|---|
| 1. `arazzo-runtime`, one `resources.rs` owner | Versioned limits, reservations, terminal cause and report/session surface; charge/move/clone/overflow tests. Freeze public contracts before consumers. |
| 2. `arazzo-runtime`, existing parallel/client owners | Bounded admission/transport permits using slice 1; integrate the scheduler owner's accepted dependency plan after its prerequisite lands. Prove shared-engine concurrency and unchanged eligibility/ordinary-error semantics. |
| 3. `arazzo-runtime`, existing events/decoder/state plus accepted recorder owner | Account retained events, reorder data, decoded responses, outputs and recorder scratch with slice 1. Extend the already-selected recording owner; do not add another evidence collector. Prove tiny-event, parsed-node and late-cap failures. |
| 4. `arazzo-cli`, accepted v2 persistence/verdict owners | Bounded adapters, options, safe reports, schemas and publication using slices 1–3. Enforce exact serialized size and all-or-nothing destination semantics. Preserve v1 artifact compatibility. |
| 5. `arazzo-mcp`, existing security/result owners | Integrate the runtime surface only after its own decision is accepted; prove final result and framing accounting, override rejection and serial dispatch. |

The future hermetic proof matrix must include widths 1/8/40/128, slow-first
siblings, multiple levels, same-level failures and engine-clone concurrent runs;
variable/step/final-output duplication, tiny high-count events, flat/node-heavy
JSON and near-limit body bytes; collect/result-only/trace/record/replay modes;
token/permit/channel wait cancellation, stalled headers/chunks, retries and late
resource failure. Use transport start/finish counters and readiness barriers,
not BeforeStep/RequestSent markers or sleep-based correctness assertions. Assert
source ordering, peak admission/in-flight, charged peaks, reservation release,
terminal cause, no subsequent request, throughput and cancellation-to-join time.
For stalled I/O and admission/channel waits, retain the existing 900 ms completion
control against a 5-second HTTP deadline, timing from the actual cancellation
signal. Report distributions separately; this is a fixture bound, not a CPU or
host-scheduling SLA.

For recorder exhaustion exercise each accounting phase independently, including
classification, parsing, base64/escaping, size counting and report serialization.
Check zero artifact/temp/sidecar creation, unchanged existing destination,
fixed-only stdout/stderr, and no retry or re-execution. Also test every exact
boundary and one byte above it, with adversarial chunk sizes and retained clones.
RSS remains a separately measured observation. Later benchmark sampling and
regression budgets belong to the existing benchmark ticket; no regression
percentage or throughput promise is accepted here.

Use focused targets rather than growing the God tests listed in
[god-files.md](../../god-files.md), and integrate through the existing owners
without widening `engine_impl.rs` into another orchestration module. No dependency
is selected. This assessment needs local-link, diff/scope checks and fresh
architecture review; no production behavior changed and no full Cargo suite is
claimed. Extreme-optimization was used for baseline discipline: no optimization
pass, isomorphic proof, approximate change or performance gain is being shipped.

## Maintainer disposition and separate scope

Steve accepted all five resource-contract decisions on 2026-09-08 as recorded
above. Acceptance settles implementation direction; it does not establish
delivered implementation, measured v2 memory compliance or accepted MCP security
policy. Fresh architecture review of each documentation candidate and the
implementation gates above remain required.

The separate **whole-process hostile-input resource isolation** follow-up is now
tracked by [epic:ac-c2849](https://sonos.scapedeck.com/docs/ac-tickets/ac-c2849),
with [ac-38c60](https://sonos.scapedeck.com/docs/ac-tickets/ac-38c60) for
document-loading/graph-construction admission and
[ac-7a6ba](https://sonos.scapedeck.com/docs/ac-tickets/ac-7a6ba) for synchronous
evaluation resource and cancellation guarantees. Pre-session document graph
construction and general evaluator intermediates still escape retained-evidence
accounting. That work must reuse existing loader, expression-specific limit and
callback ownership before deciding remaining admission/isolation boundaries.
Durable scheduling, distributed workers, crash recovery, disk-backed evidence
and performance micro-optimization remain outside this contract's scope.
