# Explicit recovery examples

These complete Arazzo 1.1.0 documents use the adjacent OpenAPI description.
They require an API implementing the assumptions below, served at the OpenAPI
`servers` URL (default `http://127.0.0.1:8080`). Change that URL for your API.
They are executable demonstrations, not a bundled production service.
`crates/arazzo-runtime/tests/recovery_scenarios.rs` runs the same documents
against a hermetic local server with an independent effect ledger.

| Document / entry workflow | Inputs | API requirement |
| --- | --- | --- |
| `idempotent-retry.arazzo.yaml` / `createOnce` | `key`, `businessKey` strings | Atomically bind a caller key to the request payload, effect, and result; repeated requests return that result without a second effect. Reject key reuse with different payloads. Retain keys for the entire retry period. |
| `reconcile.arazzo.yaml` / `submit` | `businessKey` string | Create records with a unique business key and provide a lookup that can find a committed operation after its response is lost. |
| `compensate.arazzo.yaml` / `purchase` | none | Reserve returns an ID; a rejected charge has no payment effect; explicit release by reservation ID removes the reservation. |

The idempotent example retries with the same input key and payload after the
server commits but disconnects before replying. Arazzo supplies the retry;
the API supplies deduplication. A key generated anew on each attempt defeats
this guarantee. This is not general exactly-once execution.

The reconciliation example transfers control through `onFailure` to another
workflow, passing the business key in action `parameters`. Lookup returns the
existing ID and `lookupStatus: 200`, without another create. Missing records,
lookup errors, and a response without an ID fail safely with no resubmission.
A missing result does not establish that the original request had no effect;
an operator may still need to reconcile it, especially with an eventually
consistent API.

The compensation example transfers the successful reserve step's ID through
`$steps.reserve.outputs.id` to the release workflow when charge fails.
Successful release returns `cleanupStatus: 204`; failed release leaves a
reservation needing intervention. Neither result is a successful purchase.
The engine's successful return from a recovery workflow describes that target
workflow's outcome, not the original business operation's success. There is no
automatic rollback or atomic transaction across these calls.

The tests prove effects from server ledgers, including two create attempts
and one effect for idempotency, one create followed by lookup for reconciliation,
and reservation plus explicit cleanup for compensation. They do not infer
effects from traces of failed transports. They also do not depend on workflow
outputs from a failed `end` action, durable resume, or replay.
