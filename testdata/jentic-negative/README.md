# Minimized Jentic validation negatives

These four Arazzo 1.0.1 documents isolate defects already recorded in
[`plans/assessments/jentic-corpus-survey-2026-10-18.json`](../../plans/assessments/jentic-corpus-survey-2026-10-18.json).
They are locally authored reductions, not copies of upstream workflows. The
survey pins Jentic commit `290ca550fd1ca326400712aa2493c6e35fd03b63`.
Each source path and SHA-256 below identifies the **original upstream workflow**,
not this minimized fixture.

| Fixture | Pinned upstream path and SHA-256 | Isolated defect; expected diagnostic | Normative vendored rule |
| --- | --- | --- | --- |
| `operation-path.arazzo.json` | `workflows/apacta.com/workflows.arazzo.json` — `482d49bb748cf61502ecf9ebb11a947ea89dda8c984500a78ff4d88197c755ad` | Bare `openapi_source#/paths/~1probe/get`; `unsupportedOperationPath` | [Arazzo 1.0.1 Step Object](../../spec/arazzo/v1.0.1.html#step-object) and [1.1.0 Step Object](../../spec/arazzo/v1.1.0.html#step-object): “A Runtime Expression syntax MUST be used to identify the source description document.” |
| `output-key.arazzo.json` | `workflows/autotask.net/workflows.arazzo.json` — `967067b5a87a7cf8a0711623ee300943074caba984c14fec65456dc5b045cc2c` | Step output key `items[0].id`; `invalidIdentifier` | [Arazzo 1.0.1 Step Object](../../spec/arazzo/v1.0.1.html#step-object) and [1.1.0 Step Object](../../spec/arazzo/v1.1.0.html#step-object): output keys MUST match `^[a-zA-Z0-9\.\-_]+$`. |
| `response-pointer.arazzo.json` | `workflows/devopness.com/workflows.arazzo.json` — `8a90ce469edc54649f1357b3e85b583c40183929fca8d01d5055f0ee2d7e7ceb` | `$response.body#access_token` lacks `/`; `invalidExpression` | [Arazzo 1.0.1 Runtime Expressions](../../spec/arazzo/v1.0.1.html#runtime-expressions) and [1.1.0 Runtime Expressions](../../spec/arazzo/v1.1.0.html#runtime-expressions): `body-reference = "body" ["#" json-pointer ]`; `json-pointer = *( "/" reference-token )`. |
| `duplicate-step.arazzo.json` | `workflows/ticketmaster.com~discovery/workflows.arazzo.json` — `0473a3a0440b8b0450b21bf2104530b0dae2b9709dd679684d8c7a00cc50de98` | Repeated `stepId` in one workflow; `duplicateIdentifier` | [Arazzo 1.0.1 Step Object](../../spec/arazzo/v1.0.1.html#step-object) and [1.1.0 Step Object](../../spec/arazzo/v1.1.0.html#step-object): stepId “MUST be unique amongst all steps described in the workflow.” |

The tests create a matching control by changing only each defect and also
exercise an in-memory version-only 1.1.0 variant. `validate` checks the Arazzo
document's local structure and expressions without resolving the declared
OpenAPI source. A passing control therefore establishes structural validation,
not source availability or executable workflow behavior.
