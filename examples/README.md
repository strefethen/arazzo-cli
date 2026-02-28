# Examples Catalog

Use these specs to quickly find a scenario and run it with `arazzo-cli`.

## Scenario-First Examples

| File | What it demonstrates | Dry-run command |
| --- | --- | --- |
| `multi-api-orchestration.arazzo.yaml` | Multiple source descriptions, `{sourceName}./path` routing, source URL outputs | `cargo run -p arazzo-cli -- run examples/multi-api-orchestration.arazzo.yaml cross-source-check --dry-run` |
| `auth-flow.arazzo.yaml` | Auth inputs, header interpolation, cookies, auth chaining | `cargo run -p arazzo-cli -- run examples/auth-flow.arazzo.yaml auth-then-fetch --dry-run --input token=my-token --input session=s-123` |
| `error-handling-retry.arazzo.yaml` | Retry + criteria-based goto + workflow fallback | `cargo run -p arazzo-cli -- run examples/error-handling-retry.arazzo.yaml retry-then-recover --dry-run` |
| `sub-workflow.arazzo.yaml` | Parent/child workflow composition with input/output passing | `cargo run -p arazzo-cli -- run examples/sub-workflow.arazzo.yaml parent-flow --dry-run --input item_name=widget` |

## Existing Deep-Dive Examples

| File | Focus area |
| --- | --- |
| `httpbin-components.arazzo.yaml` | Component references (`$components.parameters`, success/failure actions) |
| `httpbin-response-headers.arazzo.yaml` | `$response.header.*` extraction and header chaining |
| `httpbin-chained-posts.arazzo.yaml` | Multi-step POST request body chaining and success `goto` |
| `httpbin-data-flow.arazzo.yaml` | Data flow, interpolation, headers/cookies, embedded sub-workflow examples |
| `httpbin-error-handling.arazzo.yaml` | Expanded error-routing and recovery patterns |
| `httpbin-parallel.arazzo.yaml` | Parallel vs dependency-constrained step execution |
| `httpbin-auth.arazzo.yaml` | Basic/bearer auth success/failure variants |
| `httpbin-get.arazzo.yaml` | Minimal validate/list/run smoke workflow |
| `httpbin-conditions.arazzo.yaml` | Condition expression patterns |
| `httpbin-methods.arazzo.yaml` | Explicit HTTP method coverage (`GET`, `POST`, `PUT`, etc.) |
