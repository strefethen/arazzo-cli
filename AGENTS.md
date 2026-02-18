# AGENTS.md — arazzo-cli

Instructions for AI coding agents working on this repository.

## What This Is

A standalone CLI and Go library for executing Arazzo 1.0 workflow specifications. It parses YAML specs describing multi-step API workflows and executes them at runtime — no code generation.

## Build & Verify

```bash
go build ./...          # Must pass before any commit
go test ./...           # 17 tests, all must pass
go vet ./...            # Must be clean
```

Always run all three before committing changes.

## Project Layout

| Path | Purpose |
|------|---------|
| `cmd/arazzo/main.go` | CLI entry point (5 commands: run, validate, list, catalog, show) |
| `parser/` | Arazzo spec types, YAML parsing, structural validation |
| `runtime/engine.go` | Execution loop — step routing, retry, onSuccess/onFailure handling |
| `runtime/client.go` | Rate-limited HTTP client with JSON/XML response extraction |
| `runtime/expressions.go` | Expression evaluator ($inputs, $steps, $response, $statusCode) |
| `runtime/vars.go` | Scoped variable store |
| `runtime/engine_test.go` | Tests — all use httptest (no external network calls) |
| `examples/` | Working specs for smoke testing |
| `testdata/` | Test fixtures |

## Rules

- **No domain-specific code.** This is a generic Arazzo executor. Do not add application-specific logic, hardcoded API paths, or vendor-specific payload transformations.
- **Every CLI command must support `--json`.** Agents are first-class users. All output must be parseable.
- **Tests use httptest only.** No external API calls in tests. Use `net/http/httptest` servers.
- **Minimal dependencies.** Do not add dependencies without justification. Current: cobra, gjson, xmlquery, yaml.v3, x/time.
- **Go conventions.** Standard library style. `go vet` clean. No `any` type where a concrete type works.

## How the Engine Works

1. `parser.Parse()` loads YAML into `ArazzoSpec` struct
2. `runtime.NewEngine(spec)` builds O(1) lookup indexes for workflows and steps
3. `engine.Execute(ctx, workflowID, inputs)` runs the workflow:
   - Steps execute sequentially by default
   - Each step: build URL → make HTTP request → evaluate success criteria → extract outputs
   - `onSuccess`/`onFailure` actions can override flow: `end`, `goto`, `retry`
   - Actions support `criteria` for conditional routing (e.g., retry on 429, end on 500)
   - Max 3 retries per step, max `steps * 10` total iterations
4. Returns `map[string]any` of workflow outputs

## Expression Language

- `$inputs.name` → workflow input
- `$steps.<id>.outputs.<name>` → previous step output
- `$statusCode` → HTTP status code
- `$response.body.path.to.field` → JSON extraction (gjson syntax)
- `//xpath/expression` → XML extraction (auto-detected from Content-Type)

## Adding a New CLI Command

1. Define `*cobra.Command` in `cmd/arazzo/main.go`
2. Add to `rootCmd.AddCommand()` in `init()`
3. Support `--json` flag for structured output
4. Human-readable text as default, JSON as opt-in
5. Return structured error JSON when `--json` is set

## Adding New Engine Features

1. Add types to `parser/types.go` if new Arazzo spec fields are needed
2. Update `parser/validate.go` for structural validation
3. Implement in `runtime/engine.go`
4. Add tests in `runtime/engine_test.go` using httptest
5. Run `go test ./... && go vet ./...`
