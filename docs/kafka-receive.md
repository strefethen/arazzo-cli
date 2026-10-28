# Kafka receive steps

arazzo-cli executes Arazzo 1.1 `receive` steps whose source description is an
AsyncAPI 3.0 document and whose server is a Kafka broker. A workflow can wait
for the next JSON record on a topic, evaluate success criteria against it,
expose typed outputs from it, and feed those outputs to ordinary HTTP steps.
Sending is not supported.

This page is the contract: what a document may contain, what the runtime does
with it, how to configure a connection, what the diagnostics mean, and what is
deliberately out of scope. Where the Arazzo or AsyncAPI specification leaves a
behaviour open, the paragraph says so and names the choice this tool made.
The reasoning behind each choice is recorded in
[`plans/assessments/arazzo-first-asyncapi-transport-decision.md`](../plans/assessments/arazzo-first-asyncapi-transport-decision.md).

## Supported documents

**Source description.** `sourceDescriptions[].type: asyncapi` with a local
`url`, resolved exactly like an OpenAPI source (relative to the Arazzo file;
no network retrieval). The document must be AsyncAPI 3.0.x.

**Step target.** Either spelling the shared classifier accepts:

```yaml
# operationId form (the specification says this SHOULD be preferred)
- stepId: await-order
  operationId: $sourceDescriptions.orders.receiveOrderPlaced
  action: receive

# channelPath form
- stepId: await-order
  channelPath: '{$sourceDescriptions.orders.url}#/channels/orderPlaced'
  action: receive
```

`action` is required and must be `receive`; with the operationId form the
named Operation Object must itself have `action: receive` and
`channel: {$ref: '#/channels/<key>'}`. A step without `action` is refused.

**Topic.** The channel's `address` is the Kafka topic. It must be a non-empty
literal string. *Interpretation:* AsyncAPI says the address "is typically the
topic name" and defines no other topic field, so the address is the topic.

**Servers.** The channel's `servers` references, or every root server when the
channel lists none. A server is eligible when its `protocol` is `kafka` or
`kafka-secure` (the latter is the bindings repository's conventional name and
means TLS is required — a documented choice). Exactly one eligible server is
used as is; several require `--asyncapi-server <source>=<server-key>`; none
is an error. `host` may use Server Variables that have `default` values; an
unresolved variable is an error. `pathname` is refused.

**Security.** SASL PLAIN, SCRAM-SHA-256, and SCRAM-SHA-512 are taken from the
selected server's `security` list: the first supported scheme wins, and a
`$ref` to `components.securitySchemes` is followed once. A server whose list
is non-empty but names no supported scheme is refused, whether or not
credentials were supplied. Operation-level `security` is refused (AsyncAPI
requires it in addition to server security; one SASL identity cannot satisfy
two lists).

**References.** `$ref` is followed exactly one hop, and only into the
document's own `components`: a channel message into `components.messages`, a
trait into `components.messageTraits`, a server variable into
`components.serverVariables`, and a security requirement into
`components.securitySchemes`; a Correlation ID Object may reference
`components.correlationIds`. The channel itself must be inline under
`channels` and the operation's `channel` must be a local `#/channels/<key>`
reference. Refused: a server that is a Reference Object; a `bindings` object
(or its `kafka` entry) on a channel, message, or trait that is a Reference
Object; a reference that is not local, or that resolves to another reference
(nested). A local reference whose target does not exist is a configuration
error naming the missing component.

**Messages.** JSON only. A channel message's *effective* content type follows
AsyncAPI precedence — the message's own `contentType`, otherwise the last
`contentType` among its `traits` in order, otherwise the document
`defaultContentType` — and must be absent, `application/json`, or a `+json`
suffix; anything else is refused naming the message key. An operation
`messages` list, when present, must name every message of the channel: a
strict subset is refused, because without schema validation the runtime
cannot tell which message type a record is. Message schemas are not
validated at runtime (a documented choice, matching the HTTP side).

**Compression.** Batches compressed with gzip, snappy, or lz4 are decoded.
**zstd is not built in** (its only compatible crate line carries open security
advisories): a zstd-compressed batch fails the attempt with
`RUNTIME_MESSAGE_TRANSPORT` naming the codec.

**Refused before any connection.** Unsupported forms report
`RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT` naming the field: `action: send`; a
step without `action`; `requestBody` on a receive step; a
`$ref` channel, operation, or server; a null or templated `address` (Channel
Address Expressions and channel `parameters`); operation `traits`, `security`,
or `reply`; a strict `messages` subset; server `pathname`; a security list
with no supported scheme; a non-JSON effective content type. Structural and
selection problems — a malformed target, an undeclared source, a `$ref` to a
missing component, no eligible server, several without a selection, an
unresolved server variable — report `RUNTIME_MESSAGE_TRANSPORT_CONFIG`. The
Codes table below has the full split.

### Bindings

One rule: bindings are descriptive unless they change what is on the wire.
The bindings specification is not vendored, so no field is given meaning
here — the table only decides what is safe to skip and what must stop the run.

| Where | Field | Effect |
| --- | --- | --- |
| server `bindings.kafka` | all | ignored |
| channel `bindings.kafka` | `topic` | must be absent or equal to `address`; otherwise refused |
| channel `bindings.kafka` | `partitions`, `replicas`, `topicConfiguration` | ignored (they describe the topic, not consumption) |
| operation `bindings.kafka` | `groupId`, `clientId` | ignored — there are no consumer groups; the client id is `arazzo-cli/<version>` |
| message `bindings.kafka` | `schemaIdLocation`, `schemaIdPayloadEncoding`, `schemaLookupStrategy` | refused (a schema-registry framing prefix makes the value non-JSON) |
| message `bindings.kafka` | `key` | ignored |
| anywhere | bindings for other protocols | ignored |

## Execution semantics

**Armed at invocation start.** When a workflow run starts — a `run`, a `test`
case, a sub-workflow entry, or the dependency closure selected by `--step` —
the engine resolves every Kafka receive step that invocation can execute,
connects, and records the end offset of every partition of each topic
*before the first step runs*. A receive step consumes from those offsets. So
an event produced by an earlier step of the same workflow (the HTTP call that
triggers it) is received even though it arrived before the receive step
began, while, on topics without transactional producers, records produced
before the run started are never received (see Limitations for the
transactional case). This is the tool's documented starting position, not an
option. Arming
completion is observable: `--verbose` prints one line per armed topic, and
that line is the readiness signal to produce against.

Arming failures — unreachable broker, missing topic, document or credential
problems, the preparation deadline — fail the run before any step executes,
including for a receive step that control flow would never have reached, and
bypass step `onFailure` (preparation precedes steps). The runtime never asks
a broker to create a topic: it lists topics first and refuses one that does
not exist.

**No consumer group, no committed offsets.** Each run is independent, runs
do not interfere, and nothing needs cleanup beyond closing connections.
Cursors belong to the invocation, keyed by connection identity (source name,
server key, resolved host) plus topic: a sub-workflow entering, a `goto`
re-entering, or a retry reuses the same cursor, so a second receive on the
topic gets the *next* unconsumed record. Partitions are polled round-robin;
no ordering across partitions is promised. Under `--parallel`, concurrent
receives on one topic are serialized and get distinct records in an
unspecified order. A cursor the broker has since trimmed fails the attempt
with `RUNTIME_MESSAGE_TRANSPORT`; the runtime never re-seeks.

**The first matching record is the message.** Without `correlationId`, the
first record at or after the cursor
is exposed as `$message.payload` (the parsed JSON value; a record with no
value is `null`) and `$message.header.<name>` (UTF-8 header values; duplicate
names collapse to the last value). Criteria evaluate it, outputs are taken
from it, and the cursor advances past it. A record that fails
`successCriteria` fails the step; the runtime does not wait for a later one.
*Interpretation:* Arazzo makes criteria the definition of success for the
received message and reserves filtering for `correlationId`. A non-JSON
value or a non-UTF-8 header fails the attempt with `RUNTIME_MESSAGE_MALFORMED`.
Record key, partition, offset, and timestamp are diagnostics only (trace),
because Arazzo defines no expression for them.

**Correlation.** Set the receive step's `correlationId` to the identifier
you want, for example `correlationId: $inputs.orderId`. *Interpretation:*
the field is a literal-or-expression value: a plain string is used as-is,
a Runtime Expression resolves at step entry, and a braced template renders
to a string. The expected value is rendered to text: strings as-is, numbers
and booleans as JSON text; null, arrays, and objects are configuration errors
before any fetch (arming has already occurred).

The location comes from the AsyncAPI message's Correlation ID Object: its
own `correlationId` wins, otherwise the last trait declaring one wins; one
local reference hop into `components.correlationIds` is supported. The
`location` is an AsyncAPI expression over `$message.header` or
`$message.payload`, optionally followed by a JSON Pointer, for example
`$message.header#/orderId` or `$message.payload#/orderId`. Every channel
message must define the same location text. A missing object or malformed
location is `RUNTIME_MESSAGE_TRANSPORT_CONFIG`; disagreeing locations are
`RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT`.

*Interpretation (comparison and channel-wide location are repository
choices):* compare exact text after rendering the located scalar the same
way. Expected `"42"` matches a header string `42` or payload number `42`,
but number `42.0` renders differently from `42`. Missing, null, array, or
object values at the location never match. Decoded non-matching records
advance the cursor and are skipped without evaluating criteria or outputs;
the first match wins. Malformed JSON or non-UTF-8 headers still fail before
matching. The step timeout bounds the entire wait, including skipped
records; when the skipped count is nonzero, its error text adds
`after skipping <n> non-matching record(s)`.

**Timeouts.** Step `timeout` (milliseconds) bounds the attempt from step
entry, including any wait for the cursor; expiry is
`RUNTIME_MESSAGE_RECEIVE_TIMEOUT`, routed through `onFailure` like any step
failure (a retry continues from the shared cursor). Author a `timeout`:
without one, the wait is bounded only by `--execution-timeout` and
cancellation. Preparation has its own deadline, `--kafka-connect-timeout`
(default 30 s), shared by every connection of one arming pass; expiry is
`RUNTIME_MESSAGE_TRANSPORT` at arming. HTTP steps still ignore `timeout`.

**Cancellation.** Execution timeout and cancellation win over a ready fetch
and end with the existing `RUNTIME_EXECUTION_TIMEOUT` /
`RUNTIME_EXECUTION_CANCELLED` outcomes; the client is dropped and its
connections close. Exact bound: a fetch request whose bytes were still being
written when cancellation struck is finished by the client's own detached
task, which holds that one connection until the write completes or the
socket fails. The live tests observe, with the runtime alive, that no
connection remains after a cancelled wait in the normal case.

**Modes.** `--dry-run` resolves the document and server, connects to nothing,
lists the step as a `RECEIVE kafka://<host>/<topic>` request, and evaluates
the step's outputs against a simulated empty-object payload (criteria are not
evaluated, as for the simulated HTTP `200 {}`). `replay` refuses, before
connecting, any invocation whose executable steps include a receive step.
`serve` (MCP) and the debugger run with default connection options:
plaintext, no credentials, bundled roots, 30 s.

## Configuration

| Flag or variable | Commands | Meaning |
| --- | --- | --- |
| `--kafka-username <USER>` / `ARAZZO_KAFKA_USERNAME` | `run`, `test` | SASL username, when the selected server declares a supported scheme |
| `ARAZZO_KAFKA_PASSWORD` | `run`, `test` | SASL password. **Environment only** — there is no flag, so the secret is never a process argument visible to other users. An inline `VAR=value command` assignment can still land in your shell's history; load it from a protected source (a secret manager, or `export` from a file your shell does not record) instead. |
| `--kafka-tls` | `run`, `test` | Use TLS for a plain `kafka` server (`kafka-secure` servers always use TLS) |
| `--kafka-ca-cert <PEM>` | `run`, `test` | Additional CA certificate(s) trusted for broker verification, on top of the bundled Mozilla roots |
| `--kafka-connect-timeout <DURATION>` | `run`, `test` | Preparation deadline for one arming pass (default `30s`) |
| `--asyncapi-server <SOURCE=KEY>` | `run`, `test` | Pick the server for a source whose channel lists several eligible Kafka servers; repeatable, one entry per source |

A username without `ARAZZO_KAFKA_PASSWORD`, or the password without a
username, is refused before an engine is built (`RUN_KAFKA_CREDENTIALS` /
`TEST_KAFKA_CREDENTIALS`). Credentials supplied for a server that declares no
security scheme are `RUNTIME_MESSAGE_TRANSPORT_CONFIG`. PLAIN over a plaintext
listener emits the `cleartextCredentials` transport warning. `--insecure` and
`--insecure-host` do not apply to Kafka; there is no broker verification
bypass. The password is held in one redacted type and never written to
traces, verbose output, or error text.

## Diagnostics

**Verbose lines** (stderr for the armed line, stdout for the step listing):

```
Kafka receiver armed: orders.placed on 127.0.0.1:19092 (1 partitions)
  [msg] RECEIVE kafka://127.0.0.1:19092/orders.placed partition=0 offset=17  (812ms)
  [200] GET http://127.0.0.1:18080/orders/42  (4ms)
```

When correlation filtering skipped records, the receive line adds
` skipped=<n>` before the duration; it is unchanged when none were skipped.

**Trace.** A receive step's `trace.v1` record has no `request`/`response`;
it carries an optional `message` object: `transport` (`kafka`), `host`,
`topic`, `partition`, `offset`, `timestampMs`, `key`, `headers`,
`payloadBytes`, `payloadPreview`, `payload`, `payloadLossy`, and `simulated`
(dry run). With correlation enabled, `correlationId` holds the resolved
expected text (also in dry runs); `skippedRecords` counts decoded records
skipped before the match and is omitted when zero. Without correlation,
`correlationId` is omitted. Header values and the key are redacted by the same patterns as
HTTP headers; the payload and its preview are redacted like HTTP response
bodies and truncated to `--trace-max-body-bytes`. See
[trace-schema-v1.md](trace-schema-v1.md).

**Codes.**

| Code | Cause |
| --- | --- |
| `RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT` | A document form this release does not execute: `send`, a step without `action`, `requestBody` on a receive, a `$ref` channel, operation, server, or bindings object, a null or templated address, `bindings.kafka.topic` differing from the address, an operation whose `action` is not `receive`, operation `traits`/`security`/`reply`, a strict `messages` subset, server `pathname`, a security list with no supported scheme, a non-JSON effective content type, the schema-registry message binding fields, disagreeing correlation locations among channel messages, and replay of a receive step |
| `RUNTIME_MESSAGE_TRANSPORT_CONFIG` | Structural or selection problems found before connecting: a target that is not `channelPath` or a source-qualified `operationId`, an undeclared source, a `$ref` that names a missing component, no eligible Kafka server, several servers without `--asyncapi-server` (or a selection that names none of them), an undeclared or default-less server variable, malformed `messages`/`servers` lists, credentials supplied for a server that declares no security, incomplete credentials, an unreadable CA bundle; with step `correlationId`, any channel message missing a Correlation ID Object, an invalid object or malformed `location`. At step entry before fetching: an expected correlation value resolving to null, an array, or an object |
| `RUNTIME_MESSAGE_TRANSPORT` | Connecting, TLS, SASL, metadata, a missing topic, the preparation deadline, a trimmed cursor, or a fetch failure |
| `RUNTIME_MESSAGE_RECEIVE_TIMEOUT` | The step `timeout` elapsed before a matching record arrived (any record when no correlation is set); the error names the skipped count when nonzero |
| `RUNTIME_MESSAGE_MALFORMED` | The record's value is not JSON or a header value is not UTF-8 |
| `RUN_KAFKA_CREDENTIALS` / `TEST_KAFKA_CREDENTIALS` | Username without `ARAZZO_KAFKA_PASSWORD`, or the reverse |
| `RUN_KAFKA_CA_CERT_READ` / `TEST_KAFKA_CA_CERT_READ` | `--kafka-ca-cert` could not be read |
| `RUN_ASYNCAPI_SERVER_FLAG` / `TEST_ASYNCAPI_SERVER_FLAG` | `--asyncapi-server` is not `<source>=<key>`, has an empty half, or repeats a source |

With `--json`, every failure is the usual `{"kind":"error","error":…,"code":…}`
object. In `test`, a run-time failure is a case error with `errorCode`.

## Limitations

- **Sending** (`action: send`) is refused; there is no
  send-payload contract yet.
- **One correlation location per channel**; correlation never applies to `send`.
- **No schema validation** of received payloads against the channel's
  message schema.
- **No zstd.** gzip, snappy, and lz4 batches decode; a zstd batch fails with
  `RUNTIME_MESSAGE_TRANSPORT`.
- **Transactional producers.** Records from aborted transactions are
  delivered (client limitation), and the arming position is the last
  *stable* offset, so non-transactional records queued behind a transaction
  open at arming are delivered once it resolves. Use topics written only by
  non-transactional producers.
- **Topic re-creation.** The runtime never creates topics, but the client's
  own metadata refreshes carry the protocol's default auto-creation flag, so a
  topic deleted during a run may be re-created by a permissive broker.
- **Cancellation bound** as stated above: one connection may outlive
  cancellation while a request frame still being written completes.
- **Operation `security`, `traits`, `reply`, and strict `messages` subsets**
  are refused rather than merged or filtered.
- **No consumer groups**, no committed offsets, no `--kafka-start`: the start
  position is always the end of each partition at arming.
- **`replay`** refuses receive steps; **`serve`** and the debugger use default
  connection options.
- **`--insecure*`** flags are HTTP-only; there is no broker verification bypass.
- **HTTP steps still ignore `timeout`**; receive steps are the first to honour it.

## Reproduce the shipped example

Requirements: Docker, `openssl` (for `--secure`), and the repository checkout.
The broker image is pinned by digest in `scripts/kafka/broker.sh`
(`apache/kafka` 4.3.2).

```bash
# 1. Start the pinned single-node broker (plaintext on 127.0.0.1:19092).
scripts/kafka/broker.sh up
scripts/kafka/broker.sh topic orders.placed

# 2. Start the demo HTTP responder for GET /orders/{orderId}.
python3 scripts/kafka/order-server.py &

# 3. Run the example. It arms the receiver and waits.
cargo run -p arazzo-cli -- run examples/kafka-order-followup.arazzo.yaml order-followup --verbose
```

When stderr shows

```
Kafka receiver armed: orders.placed on 127.0.0.1:19092 (1 partitions)
```

produce the event from another shell (the console producer wants
`headers<TAB>payload`; headers are `name:value,…` and must not be empty):

```bash
printf 'source:demo\t{"orderId":42,"status":"placed"}\n' | scripts/kafka/broker.sh produce orders.placed
```

The run prints the step lines and the typed outputs, and exits 0:

```
  [msg] RECEIVE kafka://127.0.0.1:19092/orders.placed partition=0 offset=0  (…ms)
  [200] GET http://127.0.0.1:18080/orders/42  (…ms)

Outputs:

  orderId: 42
  status: placed
```

Variations: produce `{"orderId":0,"status":"cancelled"}` to see the criterion
fail with `RUNTIME_SUCCESS_CRITERIA_FAILED` and no GET; produce nothing to see
the step `timeout` end the run with `RUNTIME_MESSAGE_RECEIVE_TIMEOUT` after
30 s; add `--execution-timeout 5s` to see the run cancelled with
`RUNTIME_EXECUTION_TIMEOUT`, after which `scripts/kafka/broker.sh groups`
prints nothing and `scripts/kafka/broker.sh connections` shows no connection
from this process. `--json` turns every outcome into one JSON object;
`--trace trace.json` writes the redacted record. For the secured listener,
start the broker with `scripts/kafka/broker.sh up --secure`, point a
`kafka-secure` server at `127.0.0.1:19093` with a `scramSha256` security
scheme, export `ARAZZO_KAFKA_USERNAME=arazzo` and `ARAZZO_KAFKA_PASSWORD` (the
harness's fixed test value, `arazzo-secret`, is fine to type here; a real
password should come from a secret store rather than an inline assignment),
and run with `--kafka-ca-cert /tmp/arazzo-kafka/ca.pem`.

### Correlated receive

With the broker, `orders.placed` topic, and HTTP responder above running,
build a release binary and select the third workflow. The original
`order-followup` and `await-only` workflows still accept any next record.

```bash
cargo build --release
target/release/arazzo-cli run examples/kafka-order-followup.arazzo.yaml order-followup-correlated --input orderId=42 --verbose --json --trace /tmp/kafka-correlated.trace.json
```

The CLI parses `orderId=42` as a number and warns that the example declares
a string input; execution succeeds because correlation renders either scalar
as the text `42`.

After the armed line, run these two lines in another shell. Both headers
occupy one tab-delimited field, separated by a comma:

```bash
printf 'source:demo,orderId:41\t{"orderId":41,"status":"placed"}\n' | scripts/kafka/broker.sh produce orders.placed
printf 'source:demo,orderId:42\t{"orderId":42,"status":"placed"}\n' | scripts/kafka/broker.sh produce orders.placed
```

The run exits 0 with `"outputs":{"orderId":42,"status":"placed"}` and
performs just `GET /orders/42`. The `await-my-order` trace message contains
`"correlationId":"42","skippedRecords":1`. Repeat the same run without
`--json`, producing the two lines again after arming, to see
`[msg] RECEIVE … skipped=1  (…ms)`. `scripts/kafka/broker.sh groups`
prints nothing and `scripts/kafka/broker.sh connections` returns to its
pre-run count (normally `0`).

Finish with:

```bash
kill %1                       # the order server
scripts/kafka/broker.sh down
```
