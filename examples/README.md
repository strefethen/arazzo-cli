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

## Scenario-First Examples

| File | Workflows | What it demonstrates | Dry-run command |
| --- | --- | --- | --- |
| `multi-api-orchestration.arazzo.yaml` | `cross-source-check` | Multiple source descriptions, `{sourceName}./path` routing, source URL outputs | `cargo run -p arazzo-cli -- run examples/multi-api-orchestration.arazzo.yaml cross-source-check --dry-run` |
| `auth-flow.arazzo.yaml` | `auth-then-fetch` | Auth inputs, header interpolation, cookies, auth chaining | `cargo run -p arazzo-cli -- run examples/auth-flow.arazzo.yaml auth-then-fetch --dry-run --input token=my-token --input session=s-123` |
| `error-handling-retry.arazzo.yaml` | `retry-then-recover` | Retry + criteria-based goto + workflow fallback | `cargo run -p arazzo-cli -- run examples/error-handling-retry.arazzo.yaml retry-then-recover --dry-run` |
| `sub-workflow.arazzo.yaml` | `parent-flow`, `child-create` | Parent/child workflow composition with input/output passing | `cargo run -p arazzo-cli -- run examples/sub-workflow.arazzo.yaml parent-flow --dry-run --input item_name=widget` |
| `swagger-petstore-crud.arazzo.yaml` | `crud-pet`, `crud-order`, `crud-user` | Generated CRUD lifecycles against the Swagger Petstore | `cargo run -p arazzo-cli -- run examples/swagger-petstore-crud.arazzo.yaml crud-pet --dry-run` |
| `soap-customer-crud.arazzo.yaml` | `soap-crud` | SOAP envelopes as `text/xml` bodies, XPath success criteria | `cargo run -p arazzo-cli -- run examples/soap-customer-crud.arazzo.yaml soap-crud --dry-run` |

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

`swagger-petstore.openapi.json` is an OpenAPI 3.0 source description, not an
Arazzo document. It is the input that produced
`swagger-petstore-crud.arazzo.yaml`:

```bash
cargo run -p arazzo-cli -- generate --spec examples/swagger-petstore.openapi.json --scenario crud
```

## Notes

- `soap-customer-crud.arazzo.yaml` points at a SOAP mock on
  `http://localhost:4010`, which this repo does not ship. Validate and dry-run
  it as-is; a live run needs your own mock at that address.
- `swagger-petstore-crud.arazzo.yaml` uses the public Swagger Petstore, whose
  data is shared and periodically reset — live runs may fail on state left by
  other callers.
