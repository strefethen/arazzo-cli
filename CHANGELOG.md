# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-03-13

### Added

#### CLI
- `run` command — execute workflows with inputs, headers, timeout, dry-run, parallel, and trace options
- `validate` command — parse and structurally validate Arazzo YAML specs
- `list` command — list workflows in a spec
- `catalog` command — discover specs across a directory tree
- `show` command — display workflow details with step listing
- `schema` command — print JSON Schema for any command's `--json` output
- `steps` command — list steps within a workflow
- `replay` command — deterministic trace replay with drift detection
- `generate` command — OpenAPI-to-Arazzo CRUD workflow generation
- `--json` flag on all commands for structured output
- `--trace <path>` execution trace output with automatic sensitive value redaction
- `--step` flag for single-step execution with automatic dependency resolution
- `--no-deps` flag for isolated single-step execution (skip dependencies)
- `--strict-inputs` flag for fatal input validation errors
- `--input-json` flag for JSON-typed input values
- `--http-timeout` flag for per-request timeout (default 30s)
- `--execution-timeout` flag for overall workflow timeout (default 300s)
- `--max-response-size` flag for response body size limit (default 10 MiB)
- `--expr-diagnostics` flag for expression evaluation warning surfacing
- Human-readable output for `run` command (structured JSON still available via `--json`)
- Structured JSON error codes with non-zero exit on failure

#### Expression Language
- `$inputs.name` — workflow input parameters
- `$steps.<id>.outputs.<name>` — previous step outputs
- `$env.VAR_NAME` — environment variables (`.env` auto-loaded)
- `$statusCode` — HTTP response status code
- `$method` — HTTP method (GET, POST, etc.)
- `$url` — fully constructed request URL
- `$response.header.Name` — response header (case-insensitive)
- `$response.body.path` — JSON dot-path body access
- `$response.body#/json/pointer` — RFC 6901 JSON Pointer body access
- `$request.header.Name` — request header introspection
- `$request.query.Name` — request query parameter introspection
- `$request.path.Name` — request path parameter introspection
- `$request.body` / `$request.body.path` / `$request.body#/pointer` — request body introspection
- `$outputs.name` — workflow outputs map (within `workflow.outputs`)
- `$sourceDescriptions.<name>.url` — source description URL lookup
- `{$expr}` interpolation in string values
- `//xpath/expression` — XML/HTML body extraction
- Condition operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `contains`, `matches`, `in`
- Expression evaluation diagnostics with warning surfacing

#### Workflow Engine
- HTTP execution with parameter types: header, query, path, cookie, body
- Control flow via `onSuccess` / `onFailure` actions (goto, retry, end)
- Workflow-level default `successActions` and `failureActions`
- Workflow-level `parameters` with step-level override
- Sub-workflow calls via `workflowId` with input/output passing
- Multiple source descriptions with `{sourceName}./path` operationPath routing
- Retry actions with configurable delay and limit
- Parallel step execution via `--parallel`
- Dry-run mode (`--dry-run`) — resolves requests without sending
- Async engine API for non-blocking execution
- `ExecutionObserver` trait for rich event streaming
- Rate limiting via token-bucket algorithm (10 req/sec default)
- Response body size limit (10 MiB default, configurable)
- Goto cross-reference validation at parse time (stepId and workflowId)
- Runtime expression support in goto targets

#### Security
- Trace redaction with stem/substring matching for 14 sensitive key patterns
- Non-JSON body pattern redaction (bearer tokens, key-value secrets)
- Dry-run header redaction
- Consistent `TRACE_REDACTED` constant across all redaction paths
- Output redaction in `--json` structured output

#### VS Code Debugger
- Full Debug Adapter Protocol (DAP) implementation
- Breakpoints on steps, success criteria, actions, and outputs
- Conditional breakpoints using runtime expressions
- Step Over, Step In, Step Out, Continue, Pause controls
- Variable inspection: Locals, Request, Response, Inputs, Steps scopes
- Watch expressions and hover evaluation
- Call stack with sub-workflow depth tracking
- Three-thread coordinator architecture (no deadlocks during slow HTTP)
- YAML parser migrated to yaml-rust2
- Marketplace-ready extension packaging

#### Crate Workspace
- `arazzo-spec` — typed Arazzo 1.0.1 domain model with enum-based types
- `arazzo-validate` — YAML parser with structured validation errors (kind, path, message)
- `arazzo-expr` — expression parser/evaluator with diagnostics and proptest fuzzing
- `arazzo-runtime` — async execution engine with debug controller and rate limiter
- `arazzo-cli` — CLI binary
- `arazzo-debug-protocol` — internal JSON-line debug protocol types
- `arazzo-debug-adapter` — DAP server

#### Performance
- Compiled regex caching via `LazyLock`
- Arc-shared HTTP responses to reduce cloning
- Lazy-init OpenAPI index for faster startup
- Release profile optimization (LTO, `codegen-units=1`, strip, `panic=abort`)
- Benchmark infrastructure with criterion

#### Quality
- `unsafe_code = "forbid"` across all crates
- `unwrap_used = "deny"`, `expect_used = "deny"` via workspace clippy lints
- 337 tests, all hermetic (tiny_http test servers, no external API calls)
- Proptest fuzzing on expression evaluator
- CI: cross-platform build (including aarch64-linux cross-compile), Linux test, MSRV, cargo audit, perf baseline, VS Code extension typecheck + build
- Private-release safeguards: `publish = false` across workspace with CI enforcement script
- Internal release workflow for tagged binaries + `SHA256SUMS.txt`
- Release helper scripts for local preflight, tag cutting, and downloaded-asset verification
- Structured error types with error chain support across runtime and validation crates
- Replaced unmaintained `sxd-document`/`sxd-xpath` with `uppsala`
