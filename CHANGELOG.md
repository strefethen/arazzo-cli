# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-02-23

### Added

#### CLI
- `run` command — execute workflows with inputs, headers, timeout, dry-run, parallel, and trace options
- `validate` command — parse and structurally validate Arazzo YAML specs
- `list` command — list workflows in a spec
- `catalog` command — discover specs across a directory tree
- `show` command — display workflow details
- `schema` command — print JSON Schema for any command's `--json` output
- `--json` flag on all commands for structured output
- `--trace <path>` execution trace output with automatic sensitive value redaction

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

#### VS Code Debugger
- Full Debug Adapter Protocol (DAP) implementation
- Breakpoints on steps, success criteria, actions, and outputs
- Conditional breakpoints using runtime expressions
- Step Over, Step In, Step Out, Continue, Pause controls
- Variable inspection: Locals, Request, Response, Inputs, Steps scopes
- Watch expressions and hover evaluation
- Call stack with sub-workflow depth tracking
- Three-thread coordinator architecture (no deadlocks during slow HTTP)

#### Crate Workspace
- `arazzo-spec` — typed Arazzo 1.0.1 domain model
- `arazzo-validate` — YAML parser with structural validation
- `arazzo-expr` — expression parser/evaluator with proptest fuzzing
- `arazzo-runtime` — execution engine with debug controller
- `arazzo-cli` — CLI binary
- `arazzo-debug-adapter` — DAP server

#### Quality
- `unsafe_code = "forbid"` across all crates
- `unwrap_used = "deny"`, `expect_used = "deny"` via workspace clippy lints
- Execution trace redaction for Authorization, Cookie, API keys, passwords
- 228 tests, all hermetic (tiny_http test servers, no external API calls)
- Proptest fuzzing on expression evaluator
- CI: cross-platform build, Linux test, MSRV, cargo audit, perf baseline, VS Code extension typecheck + build
- Private-release safeguards: `publish = false` across workspace with CI enforcement script
- Internal release workflow for tagged binaries + `SHA256SUMS.txt`
- Release helper scripts for local preflight, tag cutting, and downloaded-asset verification
