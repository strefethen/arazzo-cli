# arazzo-cli

[![CI](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/strefethen/arazzo-cli/actions/workflows/ci.yml)
[![Go](https://img.shields.io/badge/Go-1.23-00ADD8?logo=go&logoColor=white)](https://go.dev)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A standalone CLI and Go library for executing [Arazzo 1.0](https://spec.openapis.org/arazzo/latest.html) workflow specifications without code generation. Designed for both human and agent usage.

Arazzo is a declarative format for describing multi-step API workflows. This tool parses Arazzo YAML specs and executes them at runtime — making HTTP calls, evaluating expressions, extracting outputs, and handling errors — all driven by the spec alone.

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

Environment variables can be passed as inputs with `$` prefix:

```bash
arazzo run spec.yaml my-workflow -i api_key=$MY_API_KEY
```

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
  - workflowId: fetch-data
    summary: Fetch and transform data
    inputs:
      type: object
      properties:
        query:
          type: string
      required: [query]
    steps:
      - stepId: search
        operationPath: /search
        parameters:
          - name: q
            in: query
            value: $inputs.query
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          result: $response.body.data
    outputs:
      result: $steps.search.outputs.result
```

### Expression Language

| Pattern | Resolves To |
|---------|-------------|
| `$inputs.name` | Workflow input value |
| `$steps.<id>.outputs.<name>` | Output from a previous step |
| `$statusCode` | HTTP response status code |
| `$response.body.path.to.field` | JSON response body extraction |
| `//xpath/expression` | XML/RSS response extraction |

### Control Flow

Steps execute sequentially. `onSuccess`/`onFailure` actions override flow:

- **end** — terminate workflow immediately
- **goto** — jump to a named step
- **retry** — re-execute current step (max 3 attempts)

Actions can have `criteria` for conditional routing (e.g., retry on 429, end on 500).

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

## License

MIT
