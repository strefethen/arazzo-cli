# Examples Catalog

Runnable Arazzo specs, one row per file. Every command below works from the
repo root; substitute `arazzo-cli` for `cargo run -p arazzo-cli --` if you have
the binary installed.

Every spec validates:

```bash
cargo run -p arazzo-cli -- validate examples/httpbin-get.arazzo.yaml
```

`--dry-run` resolves parameters, bodies, and expressions without sending a
request, so the commands below need no network access. Drop the flag to
actually call the endpoint — the `httpbin-*` specs target `https://httpbin.org`.
Source descriptions point to local OpenAPI documents; operation targets select
the method and path from those documents.

## Scenario-First Examples

| File | Workflows | What it demonstrates | Dry-run command |
| --- | --- | --- | --- |
| `multi-api-orchestration.arazzo.yaml` | `cross-source-check` | Named source descriptions, canonical operation pointers, source URL outputs | `cargo run -p arazzo-cli -- run examples/multi-api-orchestration.arazzo.yaml cross-source-check --dry-run` |
| `auth-flow.arazzo.yaml` | `auth-then-fetch` | Auth inputs, header interpolation, cookies, auth chaining | `cargo run -p arazzo-cli -- run examples/auth-flow.arazzo.yaml auth-then-fetch --dry-run --input token=my-token --input session=s-123` |
| `error-handling-retry.arazzo.yaml` | `retry-then-recover` | Retry + criteria-based goto + workflow fallback | `cargo run -p arazzo-cli -- run examples/error-handling-retry.arazzo.yaml retry-then-recover --dry-run` |
| `sub-workflow.arazzo.yaml` | `parent-flow`, `child-create` | Parent/child workflow composition with input/output passing | `cargo run -p arazzo-cli -- run examples/sub-workflow.arazzo.yaml parent-flow --dry-run --input item_name=widget` |
| `swagger-petstore-crud.arazzo.yaml` | `crud-pet`, `crud-order`, `crud-user` | Generated CRUD lifecycles against the Swagger Petstore | `cargo run -p arazzo-cli -- run examples/swagger-petstore-crud.arazzo.yaml crud-pet --dry-run` |
| `soap-customer-crud.arazzo.yaml` | `soap-crud` | SOAP envelopes as `text/xml` bodies, XPath success criteria | `cargo run -p arazzo-cli -- run examples/soap-customer-crud.arazzo.yaml soap-crud --dry-run` |
| `kafka-order-followup.arazzo.yaml` | `order-followup`, `await-only` | AsyncAPI `receive` step on a Kafka topic feeding an HTTP step; both target spellings (`operationId` and `channelPath`); needs the pinned broker — see [docs/kafka-receive.md](../docs/kafka-receive.md) | `cargo run -p arazzo-cli -- run examples/kafka-order-followup.arazzo.yaml order-followup --dry-run` |
| `self-base-referencing.arazzo.yaml` | `list-then-read` | `$self` as the base URI for a relative `sourceDescriptions` url; requests resolve against the bound document's own `servers` | `cargo run -p arazzo-cli -- run examples/self-base-referencing.arazzo.yaml list-then-read --dry-run` |

## Deep-Dive Examples

| File | Workflows | Focus area |
| --- | --- | --- |
| `httpbin-get.arazzo.yaml` | `get-origin`, `echo-headers`, `status-check` | Minimal validate/list/run smoke workflows |
| `httpbin-methods.arazzo.yaml` | `post-json`, `put-json`, `patch-and-delete` | Explicit HTTP method coverage |
| `httpbin-auth.arazzo.yaml` | `basic-auth-success`, `basic-auth-failure`, `bearer-auth`, `bearer-no-token` | Basic/bearer auth success and failure variants |
| `httpbin-conditions.arazzo.yaml` | `status-code-comparisons`, `body-string-equality`, `contains-operator`, `compound-conditions` | `simple` condition grammar patterns |
| `httpbin-data-flow.arazzo.yaml` | `chained-outputs`, `interpolation`, `custom-headers-and-cookies`, `parent-workflow`, `child-workflow` | Output chaining, `{$expr}` interpolation, headers/cookies, embedded sub-workflows |
| `httpbin-chained-posts.arazzo.yaml` | `post-chain`, `success-goto`, `create-and-update` | Multi-step POST body chaining and success `goto` |
| `httpbin-error-handling.arazzo.yaml` | `retry-exhaustion`, `route-server-error`, `route-client-error`, `workflow-default-failure`, `goto-error-recovery` | Error routing, retries, and recovery |
| `httpbin-parallel.arazzo.yaml` | `independent-steps`, `dependent-chain`, `diamond-dependency` | Parallel vs dependency-constrained step execution |
| `httpbin-components.arazzo.yaml` | `reuse-params`, `reuse-actions`, `workflow-level-reuse` | `$components.parameters`, shared success/failure actions |
| `httpbin-response-headers.arazzo.yaml` | `read-custom-headers`, `header-to-header-chain`, `inspect-standard-headers` | `$response.header.*` extraction and header chaining |
| `httpbin-reusable-inputs.arazzo.yaml` | `echo-name`, `echo-name-post` | `$ref` into `components.inputs`; pairs with `--strict-inputs` |
| `httpbin-replacements.arazzo.yaml` | `replacements-demo` | `requestBody.replacements` JSON Pointer overlays on a templated payload |

## Supporting Files

`descriptions/httpbin.openapi.yaml` defines the operations used by the HTTPBin
examples. `descriptions/soap-customer.openapi.yaml` defines the SOAP mock
operation. Their `servers` specify the request destinations.

`swagger-petstore.openapi.json` is an OpenAPI 3.0 source description, not an
Arazzo document. It is the input that produced
`swagger-petstore-crud.arazzo.yaml`:

```bash
cargo run -p arazzo-cli -- generate --spec examples/swagger-petstore.openapi.json --scenario crud
```

`descriptions/orders.asyncapi.yaml` (AsyncAPI 3.0, the `orders.placed` Kafka
topic on the harness broker) and `descriptions/orders.openapi.yaml` (the
`GET /orders/{orderId}` the demo responder `scripts/kafka/order-server.py`
answers) are the two sources of `kafka-order-followup.arazzo.yaml`.

`descriptions/petstore.openapi.yaml` is the document
`self-base-referencing.arazzo.yaml` binds to. It is deliberately *not* beside
that file: the Arazzo document is read from `examples/`, but its `$self`
identifies it as living in `descriptions/`, and Arazzo 1.1 §5.6.1 resolves
relative references against the identity rather than the retrieval location.
Delete the `$self` line and the run fails, naming the file it looked for in
`examples/` instead — which is the whole point of the example.

## Notes

- `soap-customer-crud.arazzo.yaml` points at a SOAP mock on
  `http://localhost:4010`, which this repo does not ship. Validate and dry-run
  it as-is; a live run needs your own mock at that address.
- `kafka-order-followup.arazzo.yaml` validates and dry-runs offline; a live run
  needs `scripts/kafka/broker.sh up` and `scripts/kafka/order-server.py`, and
  a record produced after the armed line — the full sequence is in
  [docs/kafka-receive.md](../docs/kafka-receive.md).
- `swagger-petstore-crud.arazzo.yaml` uses the public Swagger Petstore, whose
  data is shared and periodically reset — live runs may fail on state left by
  other callers.
