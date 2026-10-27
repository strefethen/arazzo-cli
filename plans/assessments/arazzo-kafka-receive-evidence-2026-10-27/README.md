# Kafka receive — system-verify evidence (2026-10-27)

Recorded run of the scenarios in [`docs/kafka-receive.md`](../../../docs/kafka-receive.md)
against the plan's system success condition
([`plans/current/arazzo-kafka-receive.md`](../../current/arazzo-kafka-receive.md)).

**Candidate.** Docs/example commit `49a6578c432c9fcd03be1c9f975ba1e400de74cd`
(`candidate.txt`), which contains the engine (`38ed5fe`) and CLI (`288d7c2`)
candidates. Built with `cargo build -p arazzo-cli --release` in a clean
`git worktree add --detach /tmp/arazzo-kafka-verify 49a6578` (`build.log`);
the binary reports `arazzo 0.8.2`.

**Environment.** `scripts/kafka/broker.sh up --secure` — the pinned
`apache/kafka@sha256:852a0d49…` (4.3.2) single node, plaintext on
`127.0.0.1:19092`, SASL_SSL (SCRAM-SHA-256, harness CA) on `127.0.0.1:19093`;
topic `orders.placed` created with `broker.sh topic`;
`scripts/kafka/order-server.py` answering `GET /orders/{id}` on
`127.0.0.1:18080` (`order-server.log` records every request it served).
Driver: `system-verify.sh` (kept in the session scratch area; `run.log` is its
transcript with timestamps). Each scenario runs
`arazzo-cli --json run examples/kafka-order-followup.arazzo.yaml order-followup --verbose --trace <name>.trace.json`
from the worktree, waits for the `Kafka receiver armed:` line on stderr,
then produces (or not) through `broker.sh produce`, and afterwards records
`broker.sh groups` and `broker.sh connections` (first line = established
client connections to the client listeners; baseline before any run: `0`).

## Scenarios

| Scenario | Produced after the armed line | Exit | `code` | GETs served during the run | Groups after | Connections after |
| --- | --- | --- | --- | --- | --- | --- |
| `arrival` | `{"orderId":42,"status":"placed"}` | 0 | `success`, outputs `{"orderId": 42, "status": "placed"}` (number) | 1 (`/orders/42`) | none | 0 |
| `arrival-text` (no `--json`) | `{"orderId":43,"status":"placed"}` | 0 | — (text mode; stdout shows `[msg] RECEIVE kafka://127.0.0.1:19092/orders.placed partition=0 offset=1`, `[200] GET http://127.0.0.1:18080/orders/43`, outputs) | 1 (`/orders/43`) | none | 0 |
| `criterion-failure` | `{"orderId":0,"status":"cancelled"}` | 1 | `RUNTIME_SUCCESS_CRITERIA_FAILED` | 0 | none | 0 |
| `step-timeout` | nothing (example `timeout: 30000`) | 1 | `RUNTIME_MESSAGE_RECEIVE_TIMEOUT` after 30 s | 0 | none | 0 |
| `execution-timeout` (`--execution-timeout 5s`) | nothing | 1 | `RUNTIME_EXECUTION_TIMEOUT` after 5 s | 0 | none | 0 |
| `secure-scram-tls` | `{"orderId":7,"status":"placed"}` | 0 | `success`, outputs `{"orderId": 7, "status": "placed"}` | 1 (`/orders/7`) | none | 0 |

Per scenario the directory holds `<name>.stdout.json` (or `.stdout.txt`),
`<name>.stderr.txt` (the `Executing workflow`, `Inputs`, and
`Kafka receiver armed: orders.placed on <host> (1 partitions)` lines),
`<name>.exit`, `<name>.trace.json` (the redacted `trace.v1` file; the receive
step record carries the `message` object — transport, host, topic, partition,
offset, timestampMs, headers, payloadBytes, payloadPreview, payload — and no
`request`), `<name>.groups.txt`, and `<name>.connections.txt`.

The secured scenario used the shipped example with its server switched to the
SCRAM listener (`secure-scram-tls.asyncapi.yaml`: `kafka-secure`,
`host: 127.0.0.1:19093`, `security: [{type: scramSha256}]`), run with
`ARAZZO_KAFKA_USERNAME=arazzo ARAZZO_KAFKA_PASSWORD=<harness value>` in the
environment and `--kafka-ca-cert /tmp/arazzo-kafka/ca.pem`. The armed line
names `127.0.0.1:19093`. A recursive grep of this directory for the harness
password found nothing (`run.log`: `secret grep clean`).

Teardown as documented: order server stopped, `broker.sh down` (exit 0),
worktree removed.

## Reading against the success condition

Armed line printed before production, message produced after it received,
numeric output exposed, dependent GET observed by the local listener;
criteria-failing message → `RUNTIME_SUCCESS_CRITERIA_FAILED` with no request;
unanswered receive → step `timeout` → `RUNTIME_MESSAGE_RECEIVE_TIMEOUT`;
`--execution-timeout` cancellation → `RUNTIME_EXECUTION_TIMEOUT` with no
consumer group and no open connection; every `--json` stdout is a single
object. The CI runs for the same code are
[40595878639](https://github.com/strefethen/arazzo-cli/actions/runs/40595878639)
(engine, `38ed5fe`) and
[40598758244](https://github.com/strefethen/arazzo-cli/actions/runs/40598758244)
(CLI, `288d7c2`), both green across the platform builds, cross-compile, MSRV,
security audit, and `Kafka integration (Linux)` jobs; the run for the docs
commits is linked from the epic's closing note once they are pushed.

A first attempt of this run (`23:01`–`23:04` UTC) produced the same outcomes
but its driver lost the `arrival` bookkeeping lines to a shell arithmetic bug;
it was set aside and the whole sequence rerun from a fresh broker. Nothing was
retried individually.
