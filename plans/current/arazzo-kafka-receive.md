---
status: accepted
epic: epic:ac-4a01f
created: 2026-10-27
---

# Kafka receive: first production AsyncAPI transport

**Seed:** Steve's 2026-10-27 program mandate, recorded verbatim in the Goal of
[epic:ac-4a01f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4a01f):
"Kafka is the chosen first production transport. Deliver a usable, documented,
release-candidate Kafka receive capability—not another test-only transport
spike. […] A user can run the normal CLI against Kafka using local Arazzo 1.1
and AsyncAPI documents, receive a supported JSON message, evaluate success
criteria, preserve typed outputs, and execute a dependent HTTP operation."
Evidence and the reasoning behind every decision below live in
[`plans/assessments/arazzo-first-asyncapi-transport-decision.md`](../assessments/arazzo-first-asyncapi-transport-decision.md)
(decisions D1–D11 are referenced by number).

## Outcome and non-goals

When this succeeds, `arazzo-cli run` executes an Arazzo 1.1 workflow whose
first step is an AsyncAPI `receive` on a Kafka topic and whose second step is
an ordinary OpenAPI HTTP operation fed by the received message's outputs, with
no code, extension, or flag beyond what the two documents and (when the broker
needs it) credentials require. Every unsupported document form is refused with
a stable `RUNTIME_*` code before any connection. The behaviour is documented
precisely enough that a reader can predict what a run will do before running
it, and the live scenario is reproducible against a pinned broker by one
script.

Non-goals: `action: send` (no payload contract; OAI/Arazzo-Specification#617),
`correlationId` filtering, message schema validation, consumer groups or
committed offsets, transactional-producer support (aborted records are not
filtered and the starting position is the last stable offset), operation-level
security, trait merging, strict operation `messages` subsets, replaying
recorded messages, a transport abstraction for a
second protocol, RabbitMQ or any other broker, remote retrieval of AsyncAPI
documents, further NATS trial work (its tests stay as they are), new Arazzo
syntax, and any change to parameter serialization.

## User-visible workflows

**Happy path.** The author writes `orders.asyncapi.yaml` (AsyncAPI 3.0, one
Kafka server, channel `orderPlaced` with `address: orders.placed`, a JSON
message, operation `receiveOrderPlaced` with `action: receive`) and a
workflow: step `await-order` with
`operationId: $sourceDescriptions.orders.receiveOrderPlaced`, `action: receive`,
`timeout: 30000`, `successCriteria: [{condition: '$message.payload#/status == "placed"'}]`,
`outputs: {orderId: $message.payload#/orderId}`; step `fetch-order` with
`operationId: $sourceDescriptions.shop.getOrder`, `dependsOn: [await-order]`,
path parameter `$steps.await-order.outputs.orderId`. (The equivalent
`channelPath: '{$sourceDescriptions.orders.url}#/channels/orderPlaced'` form
works too.) They run `arazzo-cli run shop.arazzo.yaml order-followup`. The CLI
arms the receiver at invocation start, waits, receives the next message
produced on the topic, evaluates the criterion, publishes the typed output (a
JSON number stays a number), issues the GET, and prints the workflow outputs.
`--verbose` shows one "armed" line per topic before the first step and a
receive line with topic, partition, and offset; `--trace` records the message.

**Trigger then receive.** The same workflow with an HTTP step first (the call
that causes the event) works without authoring tricks: the receiver was armed
before the HTTP step ran, so the event is received even though it was produced
before the receive step began (D5).

**Criterion failure.** A message arrives whose `status` is `rejected`; the
step fails with the usual `SuccessCriteriaFailed` outcome, `onFailure`
routing applies, no dependent HTTP request is sent, and the message is still
visible in the trace.

**Timeout.** Nothing arrives within `timeout`; the step fails with
`RUNTIME_MESSAGE_RECEIVE_TIMEOUT`, routed through `onFailure`; a retry waits
for the next message. Without `timeout` the run ends on `--execution-timeout`
with the existing timeout outcome.

**Cancellation.** The execution timeout (or an embedding host's cancel token)
fires while waiting; the run ends with the existing cancelled/timeout outcome,
the fetch is abandoned, connections close, and nothing is left on the broker.

**Secured broker.** The document's server says `kafka-secure` and lists a
`scramSha256` scheme; the user sets `ARAZZO_KAFKA_USERNAME` and
`ARAZZO_KAFKA_PASSWORD` (the password has no flag) and, for a private CA,
`--kafka-ca-cert ca.pem`. Missing or inapplicable credentials, or a server
whose schemes are all unsupported, are refused before connecting with
`RUNTIME_MESSAGE_TRANSPORT_CONFIG`. PLAIN over a plaintext listener is allowed
with a cleartext-credentials warning. An unreachable or slow broker fails the
run at `--kafka-connect-timeout` with `RUNTIME_MESSAGE_TRANSPORT`.

**Several servers.** The channel is available on `dev` and `prod`; the run is
refused until `--asyncapi-server orders=dev` picks one.

**`--step`, `--parallel`, replay, dry run.** `run --step fetch-order` arms the
receive step in the dependency closure before executing it, so the selected
HTTP step still gets its message. `--parallel` runs receive steps through the
same executor as sequential mode. `replay` of a trace containing a receive
step is refused before any connection. `run --dry-run` resolves the document
and server, connects to nothing, and lists a `RECEIVE kafka://host/topic`
entry evaluated against a simulated empty-object payload.

**Refusals.** `action: send`, `correlationId`, a templated or null address, a
non-Kafka server, a non-JSON content type, a `$ref` channel or operation,
operation `security`/`traits`/`reply`, a schema-registry message binding, or
an AsyncAPI document outside 3.0.x each produce one `--json` error object with
`RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT` or `RUNTIME_MESSAGE_TRANSPORT_CONFIG`
and a message that names the unsupported thing.

**`test`, `serve`, debugger.** `test` accepts the same flags and runs the same
engine path. `serve` and the debug adapter run Kafka receive steps with default
options (plaintext, no credentials); MCP's `--allowed-dir` gate now covers
AsyncAPI source files, `--export-run` refuses to overwrite one, the debugger
can evaluate `$message.*` at receive checkpoints, and SECURITY.md notes the
new destination class.

## Decision-relevant facts

- The vendored Arazzo text defines receive completion by message receipt,
  criteria, optional correlation, and timeout, and is silent on subscription
  timing, starting position, consumer identity, and what a criteria-failing
  message means for waiting (assessment §1).
- AsyncAPI 3.0.0 gives the server `host`/`protocol`/`security`, the channel
  `address`/`servers`, and the operation `action`/`channel`/`security`/
  `traits`/`messages`; operation security must be satisfied in addition to
  server security; TLS has no core-spec signal for Kafka (assessment §1).
- `rskafka` 0.6.0 is pure Rust, rustls/ring TLS, SASL PLAIN/SCRAM, offset-level
  fetch, MSRV 1.85, and was verified against Kafka 4.3.2 (assessment §2, §6).
  Verified client-boundary facts: metadata requests allow topic auto-creation;
  aborted transactional records are not filtered; duplicate header names
  collapse; retries have an optional deadline; a cancelled write finishes on a
  detached task (assessment §2). `rdkafka` would put a C/OpenSSL toolchain
  into a four-target release matrix.
- The engine already carries `StepResult.message` into criteria, outputs, and
  action evaluation; `resolve_local_channel` already projects a local channel
  from the `{$sourceDescriptions.<name>.url}#/channels/<key>` form the shared
  classifier accepts; `prepare_http_request` is the single refusal point for
  async steps; the parallel scheduler calls `execute_http_step` directly;
  `--step` executes the transitive dependency closure; the validator already
  enforces `action` with `channelPath` and rejects parameters on async
  targets; `Step.timeout` is parsed and unused; the debugger's gate context
  carries HTTP context only; the local-source inventory consumed by MCP's
  allowed-dir gate and by export overwrite protection lists only `openapi`
  sources.
- Ordinary `cargo test` runs offline in CI inside a loopback-only namespace;
  broker-backed tests must therefore be opt-in and the broker must be
  startable by one pinned command. GitHub's Ubuntu runners have Docker and
  `openssl`.
- The working tree holds unrelated, uncommitted 0.9.0 release edits (manifest
  version, dev profile, CHANGELOG, README, SECURITY, AsyncAPI checksum). They
  must be preserved and kept out of this initiative's commits; verification
  evidence must come from the exact candidate commit, not the dirty tree.
- `plans/current/` already exceeds SDLC's three-plan limit (eight before this
  one); reconciliation is owned by
  [epic:ac-a0648](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a0648) and
  is not widened here.

## Architecture

Decisions D1–D11 of the assessment are the accepted architecture. Unit
boundaries and published surfaces:

- **`arazzo-runtime` / `runtime_core/kafka/`** (new) owns the broker client
  lifetime: options type, TLS and SASL assembly, connection with the
  preparation deadline as the client's backoff deadline, topic listing before
  any topic-specific request, arming (end offsets per partition), cursor
  state, receive-next with deadline and cancellation, and close. It knows
  nothing of Arazzo steps. The public type is `MessageTransportOptions`
  (username, password in a redacted type, TLS enable, CA PEM bytes,
  connect timeout, server selection map) set through
  `EngineBuilder::message_transport(...)`, defaulting to plaintext and no
  credentials.
- **`runtime_core/asyncapi_target.rs`** stays the general channel projection
  the test-only send/receive trials already use (unchanged behaviour, helpers
  factored out); **`runtime_core/asyncapi_receive.rs`** (new) owns production
  receive admission: step form, both target forms with the operation-level
  refusals of D1, address rules, server eligibility and selection,
  server-variable substitution, security-scheme projection, content-type
  admission with AsyncAPI precedence, and the D11 bindings rule, producing one
  resolved receive target (topic, connection identity, TLS requirement, SASL
  mechanism). Pure functions over the parsed document.
- **`runtime_core/engine_message.rs`** (new) owns receive-step execution the
  way `engine_http.rs` owns HTTP: the invocation-owned receiver registry
  (keyed by connection identity plus topic, shared with parallel attempt
  contexts), arming at workflow entry and for the `--step` closure with
  execution-mode gating first (dry run simulates, replay refuses, neither
  connects) and every receive step admitted before the first connection, the
  armed event delivered live through the existing `ExecutionObserver` and
  collected as an engine event, per-topic locks, waiting with the step deadline measured from
  step entry, `MessageSnapshot` construction, criteria and outputs through the
  existing helpers, and debugger gates that carry the message snapshot.
  `engine_impl.rs` and `engine_parallel.rs` both dispatch to it for any step
  with `channelPath` or `action`; the `cfg(test)` NATS and controlled-exchange
  hooks keep precedence in test builds; `prepare_http_request`'s guard stays
  as defence in depth.
- **Error, trace, and inventory contracts** (`error.rs`, `events.rs`, trace
  schema, `docs/schemas/`, `document_set.rs::local_source_paths`) gain the
  four codes, the optional `message` step record, the armed event, and
  AsyncAPI sources in the local-source inventory. Additive only;
  `RunStepRecord` is unchanged.
- **`arazzo-cli`**: one `MessageTransportFlags` struct (sibling of
  `TransportFlags`) shared by `run` and `test` — username (flag or env),
  password (env only), `--kafka-tls`, `--kafka-ca-cert`,
  `--kafka-connect-timeout`, `--asyncapi-server` — mapped onto the builder;
  armed and receive lines in verbose output; trace redaction of the message
  record; schema regeneration. No new command.
- **Broker harness**: `scripts/kafka/broker.sh up [--secure]|down|topic|produce`
  pinned to the image digest (containers run with `--rm`, so `down` is a
  graceful `docker stop`); `--secure` adds a SASL_SSL listener with
  PEM certificates generated by the script and SCRAM users, so TLS and SCRAM
  have positive live coverage; `produce` supports a compression codec. Used
  by the live tests, the CI job, the docs, and the evidence run. Live tests
  are `#[ignore]`, read `ARAZZO_KAFKA_BOOTSTRAP` (and `_SECURE_BOOTSTRAP`),
  and fail rather than skip when the broker is absent.
- **Docs and examples**: `docs/kafka-receive.md` (contract, limits, flags,
  codes, reproduction), README section and feature-table row, the example
  pair under `examples/`, SECURITY.md MCP note, CHANGELOG Unreleased entry.

Required order: client module and harness → document projection → engine
integration → CLI surface → docs, examples, and the recorded live scenario.
The first three are independently testable without the CLI; nothing in the
HTTP path changes shape.

## Verification and operational risks

Verification runs at the layer where each decision can fail: hermetic unit
tests for projection and option mapping (positive and negative document
shapes, per AGENTS.md's negative-case rule); broker-backed ignored tests for
arming-before-produce, delayed arrival after the armed event, cursor sharing
and re-entry, `--parallel` and `--step` closure, timeout, preparation deadline
against an unreachable address, cancellation, malformed payload, missing
topic, compressed batches for every enabled codec, a successful SCRAM-over-TLS
exchange with the script-generated CA, and wrong-password / untrusted-CA /
unsupported-scheme refusals; a CLI-level live test for the full
receive-then-HTTP scenario including `--json`, `--trace`, criterion failure,
and secret absence in every artefact; the existing drift guards
(`schema_drift`, `cli_contract_snapshots`, golden baselines) for the additive
contract changes; the workspace gate (`fmt`, `clippy -D warnings`, `test`),
the MSRV build, and the platform/cross jobs on the exact candidate commit via
CI. A `kafka-integration` CI job starts the pinned broker (plain and secure)
and runs the ignored suites on Linux so the live path has continuous evidence.

Risks: credential leakage — no password flag, one redacted type, and a test
that greps stdout, stderr, traces, and JSON errors; a hung wait without
`timeout` — bounded by the execution timeout, documented, and tested; a hung
preparation — bounded by the connect timeout as both backoff deadline and
outer bound; a cancelled fetch whose request was mid-write — finished by the
client's own task with no application deadline (documented, not claimed
bounded); `rskafka`'s stream terminating on the first fetch error — the
receiver treats any terminated stream as `RUNTIME_MESSAGE_TRANSPORT`, never as
a silent wait; topic auto-creation — list-first with the residual race
documented; transactional producers — documented limitation; lingering
connections — observed closed with the runtime alive before the broker is
torn down; flaky live tests — the armed event is the readiness barrier before
producing, and the mandate's retry budget forbids claiming a pass from a
rerun; dirty-tree contamination — candidates are verified in a clean
worktree of the exact commit; plan-count breach — recorded above, not
widened. Rollback is `git revert` of the initiative's commits; no persisted
format changes shape, and the trace addition is optional.

## System success condition

From a clean checkout of the final candidate, with the pinned broker started
by the repository script and the release-profile binary: `arazzo-cli run` on
the shipped example prints the armed line, receives a message produced after
that line, exposes a numeric output, and performs the dependent HTTP GET
observed by a local listener; the same command with a criteria-failing
message reports `SuccessCriteriaFailed` with no HTTP request; an unanswered
receive ends by step `timeout` with `RUNTIME_MESSAGE_RECEIVE_TIMEOUT`; a run
cancelled by `--execution-timeout` while waiting ends with the existing
timeout outcome and leaves no consumer group and no open connection on the
broker; `--json` output for every case is a single valid object; and the
workspace gate, MSRV, platform, cross-compile, and `kafka-integration` CI
jobs are green on that commit. The evidence is the recorded transcript of
those commands plus the CI run, linked from the epic's closing note.

---

**Disposition:** Accepted 2026-10-27 under program mandate
[epic:ac-4a01f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4a01f)
(the mandate delegates plan acceptance within this scope; decision fit: the
plan introduces one runtime module and widens the engine's published surface
additively, both named in the mandate's outcomes). Independent review: two
rounds by Astra, recorded in assessment §8; the review budget is spent.
Decomposed into Tier 2 tickets under the same epic — see the epic's members.

**Evidence (2026-10-27):** the system success condition was observed on the
final candidate — engine `38ed5fe`, CLI `288d7c2`, docs/example `49a6578` —
with the release-profile binary built from a clean worktree of `49a6578`
against the pinned broker; transcripts, exit codes, trace files, and the
consumer-group and connection listings are in
[`plans/assessments/arazzo-kafka-receive-evidence-2026-10-27/`](../assessments/arazzo-kafka-receive-evidence-2026-10-27/README.md).
Rollback is `git revert` of the initiative's commits, which are contiguous
on `main` from the plan acceptance `e87fecf` through the evidence commit that
follows `49a6578`: plan and decision record (`e87fecf`, `6fe85aa`, `4e0fc47`),
client, harness and CI job (`83bee8e`, `5c50623`, `36ae159`), admission and
local-source inventory (`2d86cc4`, `5a57318`, `a8d973c`, `baeb9c5`) with the
AsyncAPI spec-pin fix `99e7096`, engine (`e59d7b5`, `38b9a00`, `57678ba`,
`38ed5fe`), CLI (`e53b9ad`, `288d7c2`), and docs (`49a6578` plus the evidence
commit). Revert the range newest-first; no persisted format changes shape, and
the trace `message` field is optional.
