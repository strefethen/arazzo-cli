# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-08-02

### Added

#### CLI
- `test` command — CI-native API contract testing. Recursively discovers
  `.arazzo.yaml`/`.arazzo.yml` specs, executes every workflow, and reports in
  TAP (default), JUnit XML (`--format junit`), or JSON (`--json`). Full flag
  parity with `run` (`--input`, `--input-json`, `--header`, `--openapi`,
  `--http-timeout`, `--execution-timeout`, `--expr-diagnostics`, `--parallel`,
  `--strict-inputs`, `--max-response-size`), plus `--fail-fast` and
  `--filter <regex>`. Exits non-zero on any failure or error; parse-error
  suites are tracked separately.
- Agent-facing help text: root and subcommand `--help` now carry "For agents"
  sections covering `--json` envelopes, `schema <command>` discovery, dry-run
  previews, and trace/replay evidence.
- `warnings` array in `run --json` output; new `steps --json` schema; `show
  --json` now includes async step metadata (`action`, `channelPath`,
  `correlationId`, `dependsOn`, `timeout`).
- `--version` flag on the CLI, reporting the crate version.

#### Expression Language
- `$self` expression (resolves the current workflow document; no sub-path).
- `$sourceDescriptions.<name>.<reference>` extended beyond `.url` to `.type`
  and named operation references.
- Arazzo 1.1 `$message` expressions: `$message.header.<name>`,
  `$message.payload`, and `$message.payload#/json/pointer`, evaluated without
  assuming a transport.

#### Workflow Engine
- `requestBody.replacements` — JSON Pointer overlays applied to a resolved
  payload before serialization.
- Arazzo 1.1 source/step model: typed AsyncAPI source descriptions, selector
  objects, and async step metadata are parsed, validated, and preserved.
- Preserve Arazzo vendor extensions (`x-*`) through parse and serialize.

### Changed

#### Workflow Engine
- Arazzo 1.1 async steps fail closed: channel/send/receive execution returns
  `RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT` before any HTTP request preparation.
  1.1 async source and step metadata is typed and preserved, but async
  transport execution is not yet supported.

#### CLI
- `--json` mode no longer writes the human-readable summary to stderr (only
  TAP/JUnit do), keeping structured output clean for programmatic consumers.
- Pre-execution errors (no specs discovered, invalid `--filter`) now exit
  non-zero instead of exiting 0.

#### Quality
- Split the DAP adapter (`dap.rs`, 2,593 → 119-line root coordinator) and the
  runtime core into focused modules; public surfaces and behavior unchanged.

#### Generator
- `generate` now detects OpenAPI 3.1/3.2 input specs and reports an actionable
  message ("generate supports OpenAPI 3.0.x, but this spec declares …") instead
  of a cryptic `invalid type: sequence, expected a string` deserialization
  error. Full 3.1/3.2 ingestion remains future work.

### Fixed

#### Expression Language
- Unsupported JSONPath constructs (recursive descent `..`, wildcards `*`/`[*]`,
  array slices `[a:b]`) now raise an "unsupported JSONPath" diagnostic on the
  criterion instead of silently evaluating to false. Quoted literals are masked
  so filter predicates are not misflagged.
- Fixed JSONPath count-predicate tokenization.

#### Workflow Engine
- Filtered single-step (`--step`) execution: a `goto` whose target is outside
  the filtered set now seeks the first in-scope step at or after the target
  instead of running an earlier step; a `goto` past the filtered tail ends the
  run.
- Preserve an explicit action `type` override (e.g. `type: end`) when merging
  component action references.

#### Generator
- Added reference-cycle guards to the request-body and response resolvers; a
  cyclic `requestBodies`/`responses` `$ref` now returns cleanly instead of
  overflowing the stack.

### Security
- Upgraded `rustls-webpki` 0.103.10 → 0.103.13 (RUSTSEC-2026-0104,
  RUSTSEC-2026-0098, RUSTSEC-2026-0099). This dependency is in the shipped
  binary via `reqwest`/`rustls`.
- Centralized dry-run redaction so all dry-run and trace output share one
  redaction path.
- Cleared dev/optional-dependency advisories not present in the shipped
  binary: `quinn-proto` 0.11.14 → 0.11.16 (RUSTSEC-2026-0185),
  `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204), `anyhow`
  1.0.102 → 1.0.104 (RUSTSEC-2026-0190).

## [0.2.2] - 2026-04-06

### Fixed

#### Workflow Engine
- Preserve multi-valued HTTP response headers instead of last-write-wins
- Special-case `Set-Cookie` handling to avoid comma-join corruption of multi-value headers
- Encode quotes and backslashes in cookie values per RFC 6265
- Return raw body text for non-JSON responses instead of null
- Preserve retry counts across goto cycles and skip retry-count increments on goto self-loops
- Account for per-step retry limits in the workflow iteration cap
- Reject goto actions that specify both `stepId` and `workflowId` at validation time
- Warn on retry fields set on non-retry actions (stderr warning instead of a fatal validation error)

#### Expression Language
- Use approximate (epsilon) equality for f64 comparisons, applied consistently to ordered comparisons
- Handle escaped quotes when splitting list elements in `in` conditions

#### CLI
- Load `.env` before starting the tokio runtime so `$env.*` sees dotenv values in every execution path

#### Security
- Avoid URL normalization during query-parameter redaction so traces record the URL as sent
- Truncate trace body previews at a character boundary to keep valid UTF-8

### Changed

#### Quality
- Reduced debug build size via line-tables-only debug info

## [0.2.1] - 2026-03-29

### Added

#### MCP Server
- `generate_workflow`, `describe_openapi`, and `generate_example` MCP tools

#### CLI
- Improved OpenAPI example value generation in `generate`

### Changed

#### Quality
- Extracted the `arazzo-generate` crate from `arazzo-cli`
- Reduced allocations in the expression evaluator, runtime engine, and validator

### Security
- Upgraded `rustls-webpki` to 0.103.10 (RUSTSEC-2026-0049)

## [0.2.0] - 2026-03-21

### Added

#### MCP Server
- New `arazzo-mcp` crate: Model Context Protocol server exposing workflow tools for AI agent integration
- `serve` CLI subcommand to start the MCP server over stdio

## [0.1.3] - 2026-03-17

### Fixed

#### Workflow Engine
- Honor control-flow decisions (goto, retry, end) in single-step and parallel execution

## [0.1.2] - 2026-03-15

### Fixed

#### Workflow Engine
- Route runtime errors through `onFailure` handlers and preserve original error kinds in step results

## [0.1.1] - 2026-03-15

### Added

#### Expression Language
- `!` (NOT) and parenthesized grouping operators in conditions
- `$workflows.<id>` expressions
- `Retry-After` response header support for retry actions

### Fixed

#### Workflow Engine
- Resolved 8 correctness bugs across expression evaluation, runtime, and the spec model found in cross-agent review

### Changed

#### Quality
- Migrated YAML serialization from `serde_yml` to `serde_yaml_ng` (unmaintained upstream)

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
- `arazzo-debug-adapter` — DAP server with JSON-line debug protocol

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
