# First AsyncAPI transport decision: Kafka receive (2026-10-27)

Mandate epic: [epic:ac-4a01f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4a01f).
Plan: [`plans/completed/arazzo-kafka-receive.md`](../completed/arazzo-kafka-receive.md).
Predecessor evidence: the test-only Core NATS trial
([`arazzo-local-nats-transport-trial.md`](arazzo-local-nats-transport-trial.md)),
the send-payload clarification gate
([`arazzo-asyncapi-send-payload-contract.md`](arazzo-asyncapi-send-payload-contract.md)),
and design ticket [ac-eb31c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-eb31c),
whose deliverable this document is.

This assessment records the evidence and the decisions behind the first
production transport. The plan carries the desired state; tickets carry
execution detail. Where the specification is silent the choice is labelled
**implementation choice**; where it is ambiguous the reading is labelled
**interpretation** and was put through independent review before the plan was
accepted.

## 1. What the specifications require

Quoted from the vendored documents only.

Arazzo 1.1.0, §5.8.5.3 *Defining Success for Asynchronous Steps*
(`spec/arazzo/v1.1.0.html`):

> For steps that reference AsyncAPI operations (via operationId or
> channelPath), tools will send or receive messages on the specified channel
> as defined by the referenced AsyncAPI description. For AsyncAPI send
> operations, the step completes immediately after the message is sent. Arazzo
> does not model broker acknowledgment or delivery confirmation. For AsyncAPI
> receive operations, step completion depends on message receipt as described
> below. […] authors SHOULD define successCriteria for AsyncAPI receive steps
> to explicitly evaluate the received message payload (for example, via
> $message.payload) […] When successCriteria is omitted, receiving any message
> matching the correlationId (if specified) within the timeout period is
> considered a successful step completion. If correlationId is specified, only
> messages matching the correlation identifier are considered. If no matching
> message is received within the timeout period, the step fails and triggers
> any defined onFailure actions.

Step Object fixed fields (same document): `channelPath` is "A reference to a
Source Description Object combined with a JSON Pointer to reference an event
channel […] If the referenced operation has an operationId defined then the
operationId SHOULD be preferred over the channelPath." `timeout` is "The
maximum number of milli-seconds to wait for the step to complete before
aborting and failing the step. Consequently this will fail the workflow unless
onFailure actions are defined." `correlationId` is "Only applicable to
asyncapi steps with action receive and has to be in-sync with correlationId
defined in the AsyncAPI document." `action` "Indicates whether the step will
send (publish) or receive (subscribe) to a channel in an AsyncAPI document."

AsyncAPI 3.0.0 (`spec/asyncapi/v3.0.0.html`): Server Object — `host`
"REQUIRED. The server host name. It MAY include the port. This field supports
Server Variables"; `protocol` "REQUIRED. The protocol this server supports for
connection"; `security` "A declaration of which security schemes can be used
with this server […] Only one of the security scheme objects need to be
satisfied to authorize a connection or operation." Channel Object — `address`
"The address is typically the 'topic name', 'routing key', 'event type', or
'path'. When null or absent, it MUST be interpreted as unknown"; `servers` "If
servers is absent or empty, this channel MUST be available on all the servers
defined in the Servers Object." Operation Object — `action` "Required. Use
send when it's expected that the application will send a message to the given
channel, and receive when the application should expect receiving messages
from the given channel"; `channel` "Required. A $ref pointer to the definition
of the channel". Security Scheme Object `type` values include `"plain"`,
`"scramSha256"`, `"scramSha512"`. The specification's own Server Object example
is a Kafka broker: `host: kafka.in.mycompany.com:9092`, `protocol: kafka`.

What the specifications do **not** say, and therefore what this project has
to decide: when a receiver is subscribed relative to the rest of the workflow;
where consumption starts in a retained log; whether the receiving party is a
consumer-group member; what happens when a message arrives that fails
successCriteria; how TLS is signalled for Kafka in AsyncAPI 3.0 (there is no
`kafka-secure` protocol in the 3.0.0 text, only in the separate bindings
repository, which is not vendored); how credentials reach the tool; and what a
step without `timeout` waits for.

## 2. Client library

| Candidate | Version / date | Verdict |
|---|---|---|
| `rskafka` | 0.6.0, 2025-03-20, MSRV 1.85; main has 72 later commits through 2026-10-26 | **Selected.** Pure Rust; TLS via rustls (`ring`, same provider as our `reqwest`); SASL PLAIN, SCRAM-SHA-256/512, OAUTHBEARER via `rsasl`; per-partition fetch with explicit offsets; no consumer groups, which this design does not need. Verified against Kafka 4.3.2 in a scratch crate (§6). |
| `rdkafka` | 0.39.0, 2026-01-25 | Rejected. Binds librdkafka (C, cmake, OpenSSL). The release matrix builds Linux, macOS, Windows and cross-compiles aarch64 Linux; a C/OpenSSL toolchain on all four is a cost this project has deliberately avoided (`reqwest` runs on rustls for the same reason). |
| `kafka` (kafka-rust) | 0.10.0, 2023-09-24 | Rejected. Unmaintained, no SASL, no rustls TLS. |
| `samsa` | 0.1.8, 2025-09-24 | Rejected. Pre-1.0, small user base, no production track record. |
| `kafka-protocol` | 0.18.0, 2026-08-20 | Rejected for this milestone. Protocol codec only; a client on top is a project in itself. |

Pin: `rskafka = { version = "=0.6.0", default-features = false, features =
["transport-tls", "compression-gzip", "compression-lz4",
"compression-snappy"] }`. The exact pin matches the repository's existing
practice for `async-nats`. gzip, snappy and lz4 are enabled (**implementation
choice**): a producer's compression setting is invisible to the Arazzo author,
and a receive capability that fails on compressed batches is a sharp edge.
`lz4-sys` compiles C through `cc`; `ring` already does, so this adds build
time, not a new toolchain requirement. **zstd is deliberately excluded**
(decided by recommendation during ac-a16ea, 2026-10-27): rskafka 0.6.0 only
accepts `zstd 0.13` / `zstd-safe 7`, and `cargo audit` on the candidate
lockfile reported RUSTSEC-2026-0391, -0398 and -0403 (patched only in zstd
0.14.3 / zstd-safe 8.2.0, which the client's unreleased main adopts) plus
eleven `unsound` warnings on that line. A release binary does not ship those; a
zstd-compressed batch fails with an explicit `RUNTIME_MESSAGE_TRANSPORT`
naming the codec, the live suite proves it, and the docs list it. The same
audit surfaced two pre-existing advisories on the TLS path this feature now
depends on directly (`webpki-roots` 1.0.6 → RUSTSEC-2026-0381, patched 1.0.10;
`rustls-webpki` 0.103.15 → RUSTSEC-2026-0382, patched 0.103.16); the lockfile
is moved to the patched versions in the same candidate. The unreleased
main branch raises MSRV to 1.95, so the next `rskafka` release will not be
adoptable until this workspace's MSRV moves; 0.6.0 is sufficient for this
milestone.

Four properties of this client were verified in its source (independent
review found them; `~/.cargo/registry/src/*/rskafka-0.6.0`) and shape the
decisions below rather than being hidden by them:

- Topic-specific metadata requests are sent with `allow_auto_topic_creation`
  unset, which the encoder writes as `true` (`protocol/messages/metadata.rs`).
  A broker with `auto.create.topics.enable=true` (Kafka's default) would create
  a missing topic the first time the runtime asks about it. Mitigation in D6:
  the runtime lists all topics first (an all-topics request names no topic and
  cannot create one) and refuses a missing topic before any topic-specific
  request; the residual window is a topic deleted between the listing and
  arming, which is documented.
- Fetches use `ReadCommitted` isolation and skip control batches, but the
  aborted-transaction list in the response is discarded
  (`client/partition.rs`), so records from *aborted* transactions can be
  delivered. Transactional producers are therefore outside the supported
  subset (D2); the runtime cannot detect them at its boundary, so this is a
  documented limitation, not a refusal.
- Record headers reach the client as `BTreeMap<String, Vec<u8>>`: header
  names are decoded as UTF-8 at the wire boundary (a malformed name fails the
  whole fetch) and duplicate names collapse to the last value before the
  runtime sees them (D2).
- Retries use `BackoffConfig { deadline: Option<Duration> }` with no deadline
  by default; the deadline only stops further retries once accumulated backoff
  exceeds it and does not bound an in-flight request or throttle wait, so the
  runtime's own outer timeout around preparation is load-bearing. A cancelled
  in-flight write is finished by a detached task (`messenger.rs`,
  `CancellationSafeFuture`). D8 therefore sets an explicit preparation
  deadline and states the cleanup bound honestly.

## 3. Broker pin

`apache/kafka:4.3.2`,
`sha256:852a0d49fc4e656b022e7f23785411802c9a088c78a7c5e39f33969de9c227a7`
(multi-arch amd64/arm64, published 2026-10-20). Single-node KRaft, plaintext
listener, used by the live tests and the documented reproduction script.
Kafka 4.x removed pre-2.1 protocol versions (KIP-896); `rskafka` 0.6.0
negotiated successfully against it (§6).

## 4. Decisions

Each item names the question the mandate asked to settle, the decision, and
its basis.

**D1 Supported document subset.** Source: `sourceDescriptions[].type: asyncapi`
with a local-file `url` (resolved like OpenAPI sources; no network retrieval).
Document: AsyncAPI 3.0.x. Step target, in the forms the shared classifier
already accepts: `channelPath: '{$sourceDescriptions.<name>.url}#/channels/<key>'`
(the operationPath spelling the Arazzo examples use, with a `/channels/<key>`
pointer) or `operationId: $sourceDescriptions.<name>.<operationId>` where the
named Operation Object has `action: receive` and
`channel: {$ref: '#/channels/<key>'}`; the operationId form is included because
the specification says it SHOULD be preferred. Arazzo `action` must be
present and `receive` (an AsyncAPI step without `action` is refused), and for
the operationId form it must agree with the operation's own `action`. The Operation Object's other executable fields are admitted only in
the forms this runtime can honour: `security` on the operation is refused when
present (AsyncAPI requires it to be satisfied *in addition to* server security,
and one SASL identity cannot satisfy two lists; connection-level security is
the Kafka norm), `traits` are refused (trait merging is not implemented),
`messages`, when present, must list every message of the channel — a strict
subset is refused, because without schema validation the runtime cannot tell
which message type a received record is and an excluded type could drive the
step — and `reply` is refused. Channel `address` must be a non-empty literal string (no
Channel Address Expressions) and is the Kafka topic. Rejected explicitly,
before any connection: `action: send` (send-payload contract unresolved,
OAI/Arazzo-Specification#617), `correlationId` (semantics owned by
[ac-dc3f2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-dc3f2), refusing
is compatible with every disposition there), `requestBody` on a receive step,
a `$ref` channel or operation, channel `parameters`, and an address that is
null or templated. **Interpretation:** "the address is typically the topic
name" plus the absence of any other topic field in the core specification
makes `address` the topic. Bindings follow the single rule in D11.

**D2 Message formats.** JSON only. A record's value bytes must parse as JSON
and become `$message.payload`; a record with no value yields an explicit
`null` payload. Content-type admission computes each channel message's *effective*
content type with AsyncAPI's precedence — the message's own `contentType`;
otherwise the last `contentType` among its `traits` in order (refs to
`components.messages`/`components.messageTraits` followed once; traits never
override a property the message sets itself); otherwise the document
`defaultContentType` — and admits only an effective value that is absent,
`application/json`, or a `+json` structured suffix; anything else is refused
at resolution naming the message key. Every message of the channel must be
admitted (see D1 on `messages`).
Record headers are exposed as `$message.header.<name>` when their value bytes
are valid UTF-8; the client collapses duplicate header names to the last value
before the runtime sees them, and that is the documented behaviour. A non-UTF-8
header value or a non-JSON payload fails the attempt with
`RUNTIME_MESSAGE_MALFORMED`, which `onFailure` may route like any other step
failure. Record key, partition, offset, and timestamp are diagnostics only
(trace), because Arazzo defines no expression for them. No runtime validation
against the channel's message schema (**implementation choice**, consistent
with the HTTP side, which does not validate responses against OpenAPI).
Supported topics are those written only by non-transactional producers.
If any producer writes transactionally, two things change and are documented
rather than detected: the client delivers records from aborted transactions
(assessment §2), and the starting position taken at arming is the last
*stable* offset, so non-transactional records queued behind a transaction
that was open at arming are delivered once it resolves even though they were
produced earlier. Both are recorded as follow-up work against the client, not
hidden.

**D3 Connection configuration.** The broker comes from the AsyncAPI document:
the channel's `servers` references, or every root server when the channel
lists none. Eligible servers have `protocol: kafka` or `kafka-secure`. Exactly
one eligible server is used as is; several require
`--asyncapi-server <sourceName>=<serverKey>` (repeatable), and none is an
error. `host` supports Server Variables with `default` values only; an
unresolved variable is an error. `pathname` is rejected (meaningless for
Kafka). `kafka-secure` is accepted (**implementation choice**, the bindings
repository's conventional name) and means TLS is required. Connection
identity for everything below is the resolved `(source name, server key,
host)` triple, so two documents or servers that happen to share a topic name
never share state.

**D4 Credentials and transport security.** TLS is enabled by `kafka-secure`
or `--kafka-tls`; roots are the bundled Mozilla set (`webpki-roots`, already in
the dependency graph through `reqwest`) plus `--kafka-ca-cert <pem>` for a
private CA. The HTTP `--insecure`/`--insecure-host` flags do not apply to
Kafka; there is no verification bypass for brokers in this milestone. SASL
mechanisms PLAIN, SCRAM-SHA-256, and SCRAM-SHA-512 are selected from the
selected server's `security` list (first supported scheme wins; `$ref` to
`components.securitySchemes` is followed once). A server whose `security` list
is non-empty but contains no supported scheme is refused unconditionally,
whether or not credentials were supplied. The username comes from
`--kafka-username` or `ARAZZO_KAFKA_USERNAME`; the password comes **only** from
`ARAZZO_KAFKA_PASSWORD` — there is no password flag, so the secret never enters
process arguments or shell history. A SASL scheme with missing credentials,
or credentials supplied for a server that declares no security, are
configuration errors raised before connecting. PLAIN over a plaintext listener
emits the existing `CleartextCredentials` transport warning. The password is
held in one redacted-`Debug` type, is never written to traces, verbose output,
or error text, and the live tests grep every artefact for it.

**D5 Subscription readiness and starting position.** **Implementation choice,
the central one.** Receive steps are *armed when their invocation starts*: on
entry to a workflow run (and to the dependency closure selected by `--step`),
the engine resolves every Kafka receive step that invocation can execute,
connects, and records the end offset of every partition of each topic before
the first step runs. A receive step then consumes from those offsets. Hence a
message produced by an earlier step of the same workflow — the HTTP request
that triggers an event — is received even though it arrived before the receive
step began, while, on topics without transactional writers, messages produced before
the invocation started are never received (the D2 caveat applies when a
transactional producer is present).
This uses Kafka's retained log to close the registration race that the NATS
trial could only paper over with fixture barriers, and it makes the starting
position a documented invariant rather than an option. Arming is bounded by a
preparation deadline (D8) and its failures — unreachable broker, missing
topic, document or credential problems — fail the run before any step
executes, including for a receive step that control flow would never have
reached; that fail-fast is intended. Arming completion is observable: the
engine emits an event and `--verbose` prints one line per armed topic, which
is also the readiness barrier the live tests and docs use before producing.

**D6 Consumer isolation, cursors, and offset handling.** No consumer group, no
committed offsets: each run is independent, concurrent runs do not interfere,
and nothing needs cleanup beyond closing connections. The runtime never asks a broker to create a topic: it lists topics before
any topic-specific request and refuses a topic that does not exist at arming.
What it cannot prevent is the client's own topic-specific metadata refreshes
(at arming, and on reconnect or leader change during the run) carrying the
protocol's default "allow auto-creation" flag, so a topic deleted *during* the
run may be re-created by a broker configured to allow it (client limitation,
documented; a one-line upstream fix is filed as follow-up work).
Cursors are owned by the invocation (the execution context), never by the
`Engine`, keyed by connection identity (D3) plus topic. Within one invocation
the first arming of a key sets its start and every later arming — a
sub-workflow entering, a `goto` re-entering the workflow, a retry — reuses the
same cursor, so a second receive step on the topic gets the *next* unconsumed
message in offset order per partition; partitions are polled round-robin and
no ordering across partitions is promised. A cursor whose offset the broker
has since trimmed (retention) fails the attempt with
`RUNTIME_MESSAGE_TRANSPORT`; the runtime never re-seeks on its own. Under
`--parallel`, concurrent receive steps on one key are serialized by the
cursor's lock and receive distinct messages in lock-acquisition order, which
the docs state is unspecified between them. Operation bindings
`groupId`/`clientId` are ignored (D11); the client id is
`arazzo-cli/<version>`.

**D7 Criteria and the first message.** The first message at or after the
cursor is *the* message for the step: it is exposed as `$message.*`, criteria
evaluate it, outputs are taken from it, and the cursor advances past it. A
message that fails `successCriteria` fails the step; the runtime does not keep
waiting for a later message that would pass. **Interpretation:** the quoted
text makes criteria the definition of success for the received message and
reserves filtering for `correlationId`; reading criteria as a filter would
turn an authored failure condition into an indefinite wait and would make
"the step fails and triggers any defined onFailure actions" unreachable for
payload-level failures.

**D8 Timeout, preparation deadline, and cancellation.** Step `timeout`
(milliseconds) bounds the attempt from step entry — including waiting for the
cursor's lock — and expiry fails the attempt with
`RUNTIME_MESSAGE_RECEIVE_TIMEOUT`, routed through `onFailure` (a retry
continues from the shared cursor). Without `timeout`, the wait is bounded only
by the run's `--execution-timeout` and cancellation, exactly as an HTTP step
without one; the docs recommend authoring `timeout`. Preparation (connect,
TLS, SASL, metadata, offset snapshot) has its own finite deadline,
`--kafka-connect-timeout` (default 30 s), applied both as the client's retry
backoff deadline and as an outer bound around arming; expiry is
`RUNTIME_MESSAGE_TRANSPORT` at arming, which is a run error (preparation
precedes steps, so it bypasses step `onFailure` by construction — stated in
the docs). This is the first consumer of `Step.timeout` in the runtime; HTTP
steps still ignore it (recorded, not changed here). Cancellation and execution
timeout win over a ready fetch (`biased` select on the token), produce the
existing `RUNTIME_EXECUTION_CANCELLED`/`RUNTIME_EXECUTION_TIMEOUT` outcomes,
and drop the client. Cleanup bound, stated exactly: the runtime spawns no task
of its own; the client's connections close when it is dropped, except that a
fetch request whose bytes were still being written when cancellation struck
is completed by the client's own detached task, which holds that connection
until the write finishes or the socket fails — there is no application-level
deadline on it (client limitation; the only writes this module issues are
small request frames, so the stall case requires a broker that has stopped
reading). The live tests observe, with the runtime still alive, that no
connection to the broker remains after a cancelled wait in the normal case;
the stall case is documented, not claimed.

**D9 Failures and diagnostics.** New runtime error kinds, all additive to the
`RUNTIME_*` contract: `RUNTIME_MESSAGE_TRANSPORT_CONFIG` (document or flag
problems found before connecting), `RUNTIME_MESSAGE_TRANSPORT` (connect,
metadata, missing topic, preparation deadline, fetch failures),
`RUNTIME_MESSAGE_RECEIVE_TIMEOUT`, `RUNTIME_MESSAGE_MALFORMED`. Unsupported
document forms keep the existing `RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT`.
The trace step record gains an optional `message` object (transport, topic,
partition, offset, timestamp, headers, payload preview with the same
truncation and redaction as response bodies, and `simulated` for dry runs);
`--verbose` prints an armed line per topic and a receive line per message;
`--json` errors are the ordinary `{kind, error, code}` object. Debugger gates
carry the message snapshot so `$message.*` evaluates at receive checkpoints.
Execution modes are decided before any broker work: dry run resolves the
document and server, connects to nothing, and evaluates the step against a
simulated empty-object payload listed as a `RECEIVE kafka://host/topic`
dry-run request; replay refuses, at arming and before connecting, any
invocation whose executable steps include a receive step (no recorded-message
injection in this milestone).

**D10 Surfaces.** `run` and `test` gain the flags above through one shared
flag struct; `serve` (MCP) and the debug adapter build engines with default
message-transport options, so plaintext, credential-free documents work there
unchanged. The engine's local-source inventory
(`relative_openapi_source_paths`, which MCP's `--allowed-dir` gate and
`--export-run`'s overwrite protection both consume) is extended to AsyncAPI
sources, so an AsyncAPI file outside the allowed roots is refused by MCP
exactly like an OpenAPI file and cannot be overwritten by a run export.
SECURITY.md records that AsyncAPI documents can now name broker destinations.
No new Arazzo syntax, no `x-` extension, no change to parameter serialization.

**D11 Bindings — one rule.** Bindings are descriptive unless they change what
is on the wire. Server bindings and non-`kafka` bindings anywhere are ignored.
Channel `bindings.kafka`: `topic` must be absent or equal to the address
(otherwise refused); `partitions`, `replicas`, `topicConfiguration` are
ignored (they describe the topic, not consumption). Operation
`bindings.kafka`: `groupId` and `clientId` are ignored and documented (no
consumer groups). Message `bindings.kafka`: `schemaIdLocation`,
`schemaIdPayloadEncoding`, and `schemaLookupStrategy` are refused when present
(a schema-registry framing prefix makes the value non-JSON); `key` is
ignored. Every ignored field is listed in the docs; every refused field names
itself in the error. The bindings specification is not vendored, so none of
these fields is given *meaning* here — the rule only decides what is safe to
skip and what must stop the run.

## 5. Alternatives considered and rejected

- *Subscribe at step start* (the NATS trial's model): loses the
  trigger-then-receive pattern to a race the author cannot see. Rejected for
  Kafka because the log makes arming at workflow start cheap.
- *Consumer groups with committed offsets*: would make runs stateful on the
  broker, require group ids, rebalance handling, and cleanup, and would change
  semantics between the first and second run of the same workflow. Rejected;
  revisit only with a concrete user need.
- *`--kafka-bootstrap` override of the document's host*: drifts the run from
  the document. Rejected in favour of server selection by key; environment
  differences belong in Server Variables or separate documents.
- *Criteria as a filter* (keep waiting until a message passes): see D7.
- *Vendoring the Kafka bindings document*: not needed for this subset, which
  gives no binding field meaning (D11 only decides which fields are safe to
  skip and which must stop the run); revisit when a binding field is given
  meaning.
- *A `--kafka-password` flag*: rejected after review; process arguments are
  visible to other users and shell history, and no redaction pass covers
  them. Environment only.
- *Refusing every non-empty `bindings` map*: the strictest consistent rule,
  but it would refuse most real AsyncAPI Kafka documents, which routinely
  declare `partitions`/`replicas` and `groupId`; D11 keeps refusal for the
  fields that change wire semantics and documents the rest as ignored.

## 6. Evidence gathered before the decision

- Scratch crate (`rskafka =0.6.0`, `transport-tls` + all codecs) against
  `apache/kafka:4.3.2` at the digest above, single-node KRaft on loopback: topic
  creation, two partitions, a pre-existing record on partition 1, two
  `StartOffset::Latest` consumers, a record produced 1.5 s after waiting began.
  Result: the pre-existing record was not delivered, the delayed record arrived
  at offset 1 with its `content-type` header after 1.53 s, delete-topic
  succeeded. `cargo +1.88.0 check` of the same crate passed.
- Dependency delta of that crate: 107 packages total; C sources only in
  `lz4-sys`, `zstd-sys`, and `ring` (already present).
- Current repository boundaries confirmed by reading: `prepare_http_request`
  refuses any step with `channelPath` or `action`; `resolve_local_channel`
  already projects `/channels/<key>` from a local AsyncAPI 3.0.x file;
  `StepResult.message` and `with_message` already carry a `MessageSnapshot`
  into criteria, outputs, and action evaluation; the validator already
  requires `action` with `channelPath` and rejects parameters on async
  targets; `Step.timeout` is parsed and unused.
- Release and CI matrix: Linux, macOS, Windows builds; aarch64 Linux
  cross-compile; MSRV 1.88 job; hermetic test rerun in a loopback-only
  network namespace. These rule out a C/OpenSSL client and require that
  ordinary `cargo test` never needs a broker.

## 7. Limits of this decision

This settles the *receive* subset for Kafka. It does not decide send
semantics, correlation filtering, message schema validation, consumer-group
semantics, replaying recorded messages, transactional-producer support
(aborted-record filtering needs a client change), operation-level security or
trait merging, or a second protocol. Each of those is a separate decision with
its own evidence; none is implied by this one. Known client-boundary limits
carried into the docs rather than solved here: duplicate header names collapse
to the last value; a topic deleted between the pre-arming listing and arming
may be auto-created by a permissive broker; a cancelled in-flight write
completes on the client's own task before its connection closes.

## 8. Independent review

Round 1 (2026-10-27, gpt-6-astra, fresh context, read-only) returned fifteen
findings against the first draft. All were verified against the client source
or the repository and are reflected above: the `channelPath` spelling (D1),
topic auto-creation (§2, D6), transactional records (§2, D2), the cleanup
bound (§2, D8), operation security/traits/messages (D1, D2), the MCP and
export-run source inventories (D10), cursor identity and re-entry (D3, D6),
the `--parallel` and `--step` paths (D5 and the plan), preparation deadlines
(D8), one bindings rule (D11), dry-run/replay gating (D9), header
preservation (D2), the password flag (D4), debugger gates (D9), and the
verification gate (plan). The reviewer's four "unresolved decisions" were
settled by recommendation under the mandate: keep `rskafka` with the bounded
mitigations and a documented transactional limitation; invocation-owned
cursors keyed by connection identity plus topic with reuse on re-entry; the
subset in D1/D2/D4/D11 with preparation failures as run errors; dry-run
simulation and environment-only passwords.

Round 2 (same day, gpt-6-astra, fresh context) found eleven of the fifteen
resolved, four partially resolved, and one new should-fix. The remaining
items were settled by recommendation and are now stated exactly rather than
optimistically: auto-creation is prevented at arming but not during the
client's later metadata refreshes (D6); the pre-invocation exclusion in D5 is
scoped to topics without transactional writers (D2, D5); the teardown bound
names the unbounded mid-write stall case instead of claiming a deadline (D8);
strict `operation.messages` subsets are refused (D1); content-type admission
follows AsyncAPI's precedence (message, then traits, then default) instead of
refusing on any declared non-JSON value (D2). A client fork to close the
first three at the source was considered and rejected for this milestone
(maintenance ownership and supply chain); the three are filed as follow-up
work. The review budget is spent; further review needs a specific unresolved
risk.
