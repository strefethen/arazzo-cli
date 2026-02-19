# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Go](https://img.shields.io/badge/Go-1.23-00ADD8?logo=go&logoColor=white)](https://go.dev)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A standalone CLI and Go library for executing [Arazzo 1.0](https://spec.openapis.org/arazzo/latest.html) workflow specifications without code generation. Designed for both human and agent usage.

Arazzo is a declarative format for describing multi-step API workflows. This tool parses Arazzo YAML specs and executes them at runtime — making HTTP calls, evaluating expressions, extracting outputs, and handling control flow — all driven by the spec alone.

### Features

- **Full HTTP method support** — GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS via `operationPath` (e.g., `PUT /users/{id}`)
- **operationId resolution** — Reference OpenAPI operations by ID instead of path
- **Sub-workflow execution** — Call workflows from other workflows with input/output propagation
- **Components** — Reusable parameters, success actions, and failure actions via `$components.*`
- **Criterion types** — Simple conditions, regex matching, and JSONPath assertions
- **Retry policy** — Spec-driven `retryAfter` (delay) and `retryLimit` (max attempts) per action
- **Parallel execution** — Opt-in `--parallel` flag runs independent steps concurrently via dependency-aware DAG scheduling
- **Execution tracing** — `TraceHook` interface for observing step-by-step execution
- **Expression language** — `$inputs`, `$steps`, `$env`, `$statusCode`, `$response.header`, `$response.body`
- **Agent-friendly** — Structured JSON output with `--json` on every command

## Install

```bash
go install github.com/strefethen/arazzo-cli/cmd/arazzo@latest
```

Or build from source:

```bash
git clone https://github.com/strefethen/arazzo-cli.git
cd arazzo-cli
go build -o arazzo ./cmd/arazzo
```

## Usage

Every command supports `--json` for structured, machine-readable output.

### Execute a workflow

```bash
arazzo run examples/httpbin-get.arazzo.yaml get-origin
```

```json
{
  "origin": "203.0.113.1",
  "url": "https://httpbin.org/get"
}
```

With inputs and custom headers:

```bash
arazzo run spec.yaml status-check -i code=200 -H "Authorization=Bearer $TOKEN"
```

With parallel execution (independent steps run concurrently):

```bash
arazzo run --parallel spec.yaml my-workflow
```

### Validate a spec

```bash
arazzo validate examples/httpbin-get.arazzo.yaml
# Valid Arazzo 1.0.0 spec: HTTPBin Demo

arazzo validate --json examples/httpbin-get.arazzo.yaml
# {"valid": true, "file": "...", "title": "HTTPBin Demo", "workflows": 3, ...}
```

### List workflows in a spec

```bash
arazzo list examples/httpbin-get.arazzo.yaml
arazzo list --json examples/httpbin-get.arazzo.yaml
```

### Catalog all workflows in a directory

```bash
arazzo catalog examples/
arazzo catalog --json examples/
```

### Show workflow details

```bash
arazzo show get-origin --dir examples
arazzo show --json get-origin --dir examples
```

## Agent Usage

All commands emit structured JSON with `--json`. Agents should prefer this mode.

```bash
# Discover available workflows
arazzo catalog --json ./workflows/

# Inspect a workflow's inputs/outputs before executing
arazzo show --json my-workflow --dir ./workflows/

# Execute with inputs, parse JSON output
arazzo run --json spec.yaml my-workflow -i key=value
```

Errors also produce structured JSON when `--json` is set:

```json
{"error": "workflow \"missing\" not found in ./workflows"}
```

Environment variables can be referenced directly in specs with `$env.VAR_NAME`, or passed as inputs from the CLI with `$` prefix:

```bash
# Via CLI flag (shell expands the variable)
arazzo run spec.yaml my-workflow -i api_key=$MY_API_KEY

# Via spec (engine reads env at runtime — no CLI flag needed)
# parameters:
#   - name: Authorization
#     in: header
#     value: $env.API_TOKEN
```

A `.env` file in the working directory is loaded automatically.

## Arazzo Spec Format

An Arazzo spec is a YAML file describing multi-step API workflows:

```yaml
arazzo: 1.0.0
info:
  title: My API Workflow
  version: 1.0.0
sourceDescriptions:
  - name: myapi
    url: https://api.example.com
    type: openapi
workflows:
  - workflowId: create-and-fetch
    summary: Create a resource then retrieve it
    inputs:
      type: object
      properties:
        name: { type: string }
      required: [name]
    steps:
      - stepId: create
        operationPath: POST /resources
        requestBody:
          contentType: application/json
          payload:
            name: $inputs.name
        successCriteria:
          - condition: $statusCode == 201
        outputs:
          id: $response.body.id

      - stepId: fetch
        operationPath: GET /resources/{id}
        parameters:
          - name: id
            in: path
            value: $steps.create.outputs.id
          - name: Authorization
            in: header
            value: $env.API_TOKEN
        successCriteria:
          - condition: $statusCode == 200
          - type: jsonpath
            condition: data.name
        outputs:
          result: $response.body.data
          request_id: $response.header.X-Request-Id
    outputs:
      result: $steps.fetch.outputs.result
```

### Expression Language

| Pattern | Resolves To |
|---------|-------------|
| `$inputs.name` | Workflow input value |
| `$steps.<id>.outputs.<name>` | Output from a previous step |
| `$env.VAR_NAME` | Environment variable (or `.env` file) |
| `$statusCode` | HTTP response status code |
| `$response.header.Name` | HTTP response header value |
| `$response.body.path.to.field` | JSON response body extraction |
| `//xpath/expression` | XML/RSS response extraction |

### Control Flow

Steps execute sequentially. `onSuccess`/`onFailure` actions override flow:

- **end** — terminate workflow immediately
- **goto** — jump to a named step or transfer to another workflow
- **retry** — re-execute current step with configurable delay and limit

Actions can have `criteria` for conditional routing (e.g., retry on 429, end on 500):

```yaml
onFailure:
  - type: retry
    retryAfter: 2      # wait 2 seconds between retries
    retryLimit: 5       # max 5 attempts (default: 3)
    criteria:
      - condition: $statusCode == 429
  - type: goto
    workflowId: fallback-workflow
    criteria:
      - condition: $statusCode == 500
  - type: end           # catch-all
```

### Parallel Execution

The `--parallel` flag enables dependency-aware concurrent execution. The engine analyzes `$steps.*` references to build a dependency graph, groups steps into topological levels, and runs independent steps within each level concurrently.

```
# Given steps A, B, C, D where B and C depend on A, and D depends on B and C:
# Level 0: [A]        — runs first
# Level 1: [B, C]     — run concurrently
# Level 2: [D]        — runs after B and C complete
```

Workflows with control flow (`goto`, `retry`, `end`) automatically fall back to sequential execution — `--parallel` is always safe to pass.

From Go code:

```go
engine.SetParallelMode(true)
```

### Success Criteria Types

```yaml
successCriteria:
  # Simple (default) — expression comparison
  - condition: $statusCode == 200

  # Regex — pattern match against a context expression
  - type: regex
    context: $statusCode
    condition: "^2\\d{2}$"    # any 2xx status

  # JSONPath — check existence/truthiness of a path in the response body
  - type: jsonpath
    condition: data.items.0.id
```

### Sub-Workflows

A step can invoke another workflow by setting `workflowId` instead of `operationPath`:

```yaml
workflows:
  - workflowId: parent
    steps:
      - stepId: authenticate
        workflowId: auth-workflow
        parameters:
          - name: clientId
            value: $inputs.clientId
    outputs:
      token: $steps.authenticate.outputs.token

  - workflowId: auth-workflow
    steps:
      - stepId: get-token
        operationPath: POST /oauth/token
        # ...
    outputs:
      token: $steps.get-token.outputs.token
```

Sub-workflow outputs are propagated to the calling step. Recursion is guarded (max depth: 10).

### Components

Reusable definitions can be defined in `components` and referenced with `$components.*`:

```yaml
components:
  parameters:
    authHeader:
      name: Authorization
      in: header
      value: $env.API_TOKEN
  failureActions:
    retryPolicy:
      - type: retry
        retryAfter: 2
        retryLimit: 5

workflows:
  - workflowId: my-workflow
    steps:
      - stepId: call-api
        operationPath: /data
        parameters:
          - reference: $components.parameters.authHeader
        onFailure:
          - name: $components.failureActions.retryPolicy
```

Step-level values override component defaults.

## Project Structure

```
cmd/arazzo/         CLI entry point
parser/             Arazzo spec types, YAML parsing, validation
runtime/            Execution engine, HTTP client, expressions, variables
examples/           Working example specs
testdata/           Test fixtures
```

## Go Library Usage

The parser and runtime packages can be imported directly:

```go
import (
    "github.com/strefethen/arazzo-cli/parser"
    "github.com/strefethen/arazzo-cli/runtime"
)

spec, _ := parser.Parse("workflow.arazzo.yaml")
engine := runtime.NewEngine(spec, runtime.WithTimeout(10*time.Second))
outputs, _ := engine.Execute(ctx, "my-workflow", map[string]any{"key": "value"})
```

### operationId Resolution

Load an OpenAPI spec to resolve `operationId` references in steps:

```go
openAPIData, _ := os.ReadFile("openapi.yaml")
engine.LoadOpenAPISpec(openAPIData)
// Steps with operationId: "listUsers" now resolve to GET /users
```

### Execution Tracing

Implement the `TraceHook` interface to observe workflow execution:

```go
type myHook struct{}

func (h *myHook) BeforeStep(ctx context.Context, event runtime.StepEvent) {
    log.Printf("Starting step %s in workflow %s", event.StepID, event.WorkflowID)
}

func (h *myHook) AfterStep(ctx context.Context, event runtime.StepEvent) {
    log.Printf("Step %s completed in %v (status=%d)", event.StepID, event.Duration, event.StatusCode)
}

engine.SetTraceHook(&myHook{})
```

`TraceHook` implementations must be safe for concurrent use when parallel mode is enabled.

## License

MIT
