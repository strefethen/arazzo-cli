# Plan: `arazzo test` Command (v2 — revised after codex review)

## Goal

Add an `arazzo test` command that discovers Arazzo spec files, executes their workflows as test cases, and reports results in CI-friendly formats (JSON, JUnit XML, TAP).

## Design Principles

- **Zero new spec syntax** — existing Arazzo specs are valid tests as-is. A workflow passes if all steps pass their success criteria. No test-specific annotations needed.
- **Thin orchestration layer** — reuses `Engine`, `ExecutionResult`, `TraceStepRecord`, and all existing runtime infrastructure. The new code is discovery + execution loop + formatting.
- **CLI flag parity with `run`** — every engine-affecting flag on `run` is available on `test`. No surprise "works with run but not test" gaps.
- **Tagged JSON contract** — follows the `RunOutput` pattern: a `#[serde(tag = "kind")]` enum with `Results` and `Error` variants.
- **Single discovery rule** — uses the MCP server's `discover_specs()` convention: recursive scan for `.arazzo.yaml` / `.arazzo.yml` files. No third discovery variant.
- **Two-level accounting** — executed workflow cases and suite-level parse errors are counted separately. Report writers may synthesize parse pseudo-cases, but JSON remains faithful to actual executed workflows.

---

## 1. CLI Interface

### Command Signature

```
arazzo test <PATHS>... [OPTIONS]
```

### Arguments

| Argument | Type | Description |
|----------|------|-------------|
| `paths` | `Vec<String>` (positional, required, 1+) | Spec files or directories. Directories are scanned **recursively** for `.arazzo.yaml`/`.arazzo.yml` files (same convention as `serve`/MCP). |

### Options — Engine Flags (Parity with `run`)

These match `run` exactly in name, type, default, and semantics:

| Flag | Type | Default | Same as `run`? |
|------|------|---------|----------------|
| `-i, --input` | `Vec<String>` | `[]` | Yes |
| `--input-json` | `Vec<String>` | `[]` | Yes |
| `-t, --http-timeout` | `Duration` | `30s` | Yes (uses `parse_duration_value`) |
| `--execution-timeout` | `Duration` | `5m` | Yes (uses `parse_duration_value`) |
| `-H, --header` | `Vec<String>` | `[]` | Yes |
| `--openapi` | `Vec<String>` | `[]` | Yes |
| `--expr-diagnostics` | `ExpressionDiagnosticsMode` | `Off` | Yes |
| `--parallel` | `bool` | `false` | Yes |
| `--strict-inputs` | `bool` | `false` | Yes |
| `--max-response-size` | `Option<usize>` | `None` | Yes |

### Options — Test-Specific

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--format` | `TestFormat {json, junit, tap}` | `tap` (overridden to `json` when `--json` is set) | Output format |
| `--fail-fast` | `bool` | `false` | Stop after first workflow failure |
| `--filter` | `Option<String>` | `None` | Regex filter on workflow IDs |

### Flags NOT Carried Over from `run`

| `run` Flag | Why Excluded |
|------------|-------------|
| `--step` / `--no-deps` | Single-step execution is a debugging tool, not a testing pattern |
| `--dry-run` | See §9 (Non-Goals): dry-run produces synthetic 200 responses with no real criteria evaluation. Reporting these as "passed" is misleading. Users who want to validate spec resolution can use `arazzo run --dry-run`. |
| `--trace` / `--trace-max-body-bytes` | Traces are always enabled internally (for per-step results) but not written to disk. Test output replaces trace files. |

### Clap Definition (in `cli.rs`)

```rust
/// Run Arazzo specs as tests and report results
Test {
    /// Spec files or directories to test (directories scanned recursively
    /// for .arazzo.yaml / .arazzo.yml)
    #[arg(required = true)]
    paths: Vec<String>,

    /// Output format (overridden to json when --json is set)
    #[arg(long, value_enum, default_value_t = TestFormat::Tap)]
    format: TestFormat,

    /// Key=value inputs for all workflows
    #[arg(short = 'i', long = "input")]
    input: Vec<String>,

    /// JSON-typed inputs (key=<json-value>)
    #[arg(long = "input-json")]
    input_json: Vec<String>,

    /// Per-request HTTP timeout
    #[arg(
        short = 't',
        long = "http-timeout",
        default_value = "30s",
        value_parser = parse_duration_value
    )]
    http_timeout: Duration,

    /// Per-workflow execution timeout
    #[arg(
        long = "execution-timeout",
        default_value = "5m",
        value_parser = parse_duration_value
    )]
    execution_timeout: Duration,

    /// Custom HTTP headers
    #[arg(short = 'H', long = "header")]
    header: Vec<String>,

    /// Additional OpenAPI spec files for operationId resolution
    #[arg(long = "openapi")]
    openapi: Vec<String>,

    /// Expression evaluation diagnostics
    #[arg(
        long = "expr-diagnostics",
        value_enum,
        default_value_t = ExpressionDiagnosticsMode::Off
    )]
    expr_diagnostics: ExpressionDiagnosticsMode,

    /// Parallel step execution within each workflow
    #[arg(long)]
    parallel: bool,

    /// Make input validation errors fatal
    #[arg(long = "strict-inputs")]
    strict_inputs: bool,

    /// Maximum response body size in bytes
    #[arg(long = "max-response-size")]
    max_response_size: Option<usize>,

    /// Stop on first failure
    #[arg(long)]
    fail_fast: bool,

    /// Regex filter on workflow IDs
    #[arg(long)]
    filter: Option<String>,
},
```

---

## 2. Spec Discovery

### Rule: Use the MCP/serve convention

Adopt `arazzo-mcp::state::discover_specs()` logic — **recursive** directory walk, only files ending in `.arazzo.yaml` or `.arazzo.yml`.

### Implementation

Use the existing `arazzo_mcp::state::discover_specs()` helper directly from `arazzo-cli`, the same way `serve` already does today. Do **not** copy the recursive walk into `test_runner.rs`; the whole point of this revision is to keep one discovery rule, not two near-identical ones.

### Discovery Function

```rust
fn discover_test_specs(paths: &[String]) -> Result<Vec<PathBuf>, String>
```

1. For each path in `paths`:
   - If it's a file and ends with `.arazzo.yaml` or `.arazzo.yml`: include it.
   - If it's a file with any other extension: return an error ("not an Arazzo spec: {path}"). This prevents silently picking up non-spec YAML.
   - If it's a directory: recursively collect `.arazzo.yaml`/`.arazzo.yml` files.
   - If it doesn't exist: return an error.
2. Sort alphabetically, deduplicate.
3. If empty: return error "no test specs found in {paths}".

---

## 3. JSON Contract

### Tagged Enum (follows `RunOutput` pattern)

```rust
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TestOutput {
    /// All specs parsed and all workflows executed (some may have failed).
    Results {
        summary: TestSummary,
        suites: Vec<TestSuiteResult>,
    },
    /// Could not run tests at all (no specs found, invalid filter regex, etc.)
    Error {
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}
```

### When Each Variant Is Used

| Scenario | Variant | Exit Code |
|----------|---------|-----------|
| Tests executed, all pass | `Results` | 0 |
| Tests executed, some fail | `Results` | 1 |
| Tests executed, some error | `Results` | 1 |
| Tests executed, filter matched zero workflows | `Results` | 0 |
| No specs found | `Error` | 1 |
| Invalid `--filter` regex | `Error` | 1 |
| Spec parse error | `Results` (suite has `error` field, zero test cases) | 1 |

Key rule: if we managed to start running tests, the output is `Results` (even if some suites errored on parse). `Error` is only for pre-execution failures where no test results exist.

---

## 4. Result Types

```rust
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    pub total_suites: usize,
    pub total_tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,                // executed workflow cases with TestStatus::Error
    pub suite_errors: usize,          // parse-error suites
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestSuiteResult {
    pub file: String,
    pub name: String,                  // spec info.title; for parse-error suites, fallback to file name
    pub tests: Vec<TestCaseResult>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,         // spec-level parse error
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestCaseResult {
    pub workflow_id: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub steps: Vec<TestStepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TestStatus {
    Pass,
    Fail,
    Error,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestStepResult {
    pub step_id: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

### No `Skipped` Status

The `Skipped` status is removed. Rationale:
- **Filtered-out workflows** are simply not included in `tests` — they don't exist in the output. The summary counts reflect only workflows that were actually executed.
- **Fail-fast remainder** workflows are also omitted — they were never started, so there's nothing to report. The summary counts reflect what was actually run.
- **Parse-error suites** have `error` set and `tests: []` (empty array). They contribute to `summary.suite_errors`, not `summary.total_tests`.

This keeps accounting simple:
- `total_tests == passed + failed + errors`, always.
- `suite_errors` is separate and does not affect `total_tests`.
- JSON reflects actual executed workflow cases only.

---

## 5. Status Mapping

### Workflow-Level (from `ExecutionResult`)

```
ExecutionResult.outputs = Ok(_)                          → TestStatus::Pass
ExecutionResult.outputs = Err(e)
  e.kind == SuccessCriteriaFailed                        → TestStatus::Fail
  e.kind == anything else                                → TestStatus::Error
```

### Expression Diagnostics (`--expr-diagnostics`)

`test` should mirror `run` exactly:

| Mode | Behavior in `test` |
|------|---------------------|
| `off` | Ignore expression warnings |
| `warn` | Collect warnings from `TraceStepRecord.warnings`; print them to stderr in human mode; do **not** change test status |
| `error` | Treat warnings as a workflow test error for that workflow with `error_code = "RUNTIME_EXPRESSION_DIAGNOSTICS"` and an error message like `expression diagnostics reported N warning(s)` |

Important: `--expr-diagnostics error` is **not** a pre-execution `TestOutput::Error`. It is a per-workflow test outcome, because execution did occur and results exist.

### Step-Level (from `TraceStepRecord`)

A step's `TraceDecisionPath` indicates the control-flow outcome:

| `TraceDecisionPath` | `error` field | Step Status | Rationale |
|---------------------|---------------|-------------|-----------|
| `Next` | `None` | `Pass` | Normal advancement |
| `Done` | `None` | `Pass` | Workflow completed successfully |
| `GotoStep` | `None` | `Pass` | Intentional control flow |
| `GotoWorkflow` | `None` | `Pass` | Intentional control flow |
| `Retry` | `None` | `Pass` | Retry was approved and will re-execute (the retry attempt itself is a separate trace record) |
| `Error` | `Some(msg)` | `Fail` or `Error` | Step failed. If the overall workflow result is `SuccessCriteriaFailed`, mark as `Fail`; otherwise `Error`. |
| Any | `Some(msg)` | `Error` | Step had an error regardless of decision path |

Key insight: `GotoStep`, `GotoWorkflow`, and `Retry` with no error are **valid control-flow decisions**, not failures. Only `Error` path (or presence of an `error` field) indicates a problem.

---

## 6. Output Formats

### JSON (`--format json` or `--json`)

Emit `TestOutput` via `output_json()`. Same pattern as all other commands.

### JUnit XML (`--format junit`)

| Arazzo | JUnit |
|--------|-------|
| Test run | `<testsuites>` root |
| Spec file | `<testsuite name="{suite.name}" package="{file}">` |
| Workflow | `<testcase name="{workflow_id}" classname="{file}">` |
| `Pass` | Empty `<testcase>` |
| `Fail` | `<testcase>` with `<failure message="{error}" type="CriteriaFailure">` |
| `Error` | `<testcase>` with `<error message="{error}" type="{error_code}">` |
| Parse-error suite | `<testsuite>` with `errors="1"` and a single synthetic `<testcase name="(parse)"><error>` |

Parse-error suites need exactly one synthetic `<testcase>` to carry the error — JUnit requires `tests` count ≥ `errors` count. This synthetic `(parse)` case is a **report-format adapter only**; it is not added to `TestSuiteResult.tests` and does not affect JSON `total_tests`.

Hand-write XML via `write!()`. Entity-escape `<`, `>`, `&`, `"`, `'` in all interpolated strings.

### TAP (`--format tap`, default)

```
TAP version 13
1..{report_test_count}
ok 1 - {file}::{workflow_id}
  ---
  duration_ms: 512
  steps: 3
  ...
not ok 2 - {file}::{workflow_id}
  ---
  duration_ms: 418
  error: "step create-pet: success criteria not met (status=500)"
  error_code: RUNTIME_SUCCESS_CRITERIA_FAILED
  ...
```

Where `report_test_count = total_tests + suite_errors`.

Parse-error suites emit `not ok N - {file}::(parse)` with the parse error in the YAML diagnostic block. As with JUnit, this is a report-local synthetic entry and does not change JSON `total_tests`.

### Human Summary (to stderr, all formats)

Always printed to stderr so it doesn't interfere with piped output:

```
  PASS  petstore.arazzo.yaml::list-pets (512ms)
  PASS  petstore.arazzo.yaml::get-pet (304ms)
  FAIL  petstore.arazzo.yaml::create-pet (418ms)
        step create-pet: success criteria not met (status=500)

Suites: 1 total (1 ok, 0 errored)
Tests:  2 passed, 1 failed, 3 total
Time:   1.234s
```

Uses ANSI colors when stderr is a TTY (`PASS` green, `FAIL` red, `ERROR` yellow). No colors when piped.

If `suite_errors > 0`, include that explicitly in the suite summary line, for example:

```text
Suites: 3 total (2 ok, 1 errored)
Tests:  8 passed, 1 failed, 2 errored, 11 total
```

---

## 7. Exit Codes

| Condition | Exit Code |
|-----------|-----------|
| All tests pass | 0 |
| Filter matches zero workflows across all discovered specs | 0 |
| Any test fails or errors | 1 |
| Pre-execution error (no specs, bad filter) | 1 |
| Invalid CLI args | 2 (clap default) |

---

## 8. Files to Create / Modify

### New Files

| File | Purpose |
|------|---------|
| `crates/arazzo-cli/src/test_runner.rs` | Core module: `discover_test_specs()`, `run_test_suite()`, JUnit/TAP formatters |
| `docs/schemas/test.schema.json` | JSON Schema for `TestOutput` (generated, checked in) |

### Modified Files

| File | Change |
|------|--------|
| `crates/arazzo-cli/src/cli.rs` | Add `Test` variant to `Commands`, add `TestFormat` enum |
| `crates/arazzo-cli/src/main.rs` | Add `Commands::Test` match arm, dispatch to handler |
| `crates/arazzo-cli/src/output.rs` | Add `TestOutput`, `TestSuiteResult`, `TestCaseResult`, `TestStepResult`, `TestSummary`, `TestStatus` types. Add `emit_test_results()`. |
| `crates/arazzo-cli/src/handlers.rs` | Add `run_tests()` handler function. Add `Some("test") => output_json(&schema_for!(TestOutput))` to `schema()`. Update the schema command list in both `schema()` and any tests that assert on it. |
| `crates/arazzo-cli/tests/schema_drift.rs` | Add `test_schema_test` case |
| `crates/arazzo-cli/tests/cli_integration.rs` | Add integration tests for `arazzo test` |
| `CLAUDE.md` | Add `test` to the commands list |

### NOT Modified

- No changes to `arazzo-runtime`, `arazzo-expr`, `arazzo-spec`, `arazzo-validate`, `arazzo-mcp`, or `arazzo-generate`.

---

## 9. Non-Goals (Explicit Exclusions)

| Exclusion | Rationale |
|-----------|-----------|
| `--dry-run` | Dry-run uses synthetic 200 `{}` responses (see `engine_http.rs:227`). No real criteria are evaluated, so reporting "pass" is misleading. Users can use `arazzo run --dry-run` directly. |
| Test-specific spec syntax | Arazzo success criteria are the assertions. |
| Watch mode | Use `watchexec` or similar. |
| Snapshot testing | `replay` already handles this. |
| Coverage tracking | Out of scope. |
| Concurrent cross-workflow execution | Future enhancement behind a `--concurrent` flag. |
| HTML reports | CI systems render JUnit; TAP is human-readable. |
| `Skipped` status | Filtered/unstarted workflows are omitted, not reported. |

---

## 10. Execution Model

### Per-Spec Processing

For each discovered spec file:

1. **Parse**: `arazzo_validate::parse(&path)` → `ArazzoSpec` or parse error.
   - Parse errors: create a `TestSuiteResult { file, name, error: Some(msg), tests: [] }`.
   - For parse-error suites, set `name` to the file name (or full path if no file name is available), since `spec.info.title` does not exist.
   - Increment `summary.suite_errors`. Continue to next spec.

2. **Filter workflows**: If `--filter` is set, compile it as a `Regex` (once, before the loop — return `TestOutput::Error` if invalid). Keep only workflows whose `workflow_id` matches.
   - If the regex is valid but matches zero workflows across all successfully parsed specs, return `TestOutput::Results` with zero tests and exit 0.

3. **Build engine**: One `Engine` per spec file. Construct via `EngineBuilder` with `ClientConfig` (from `--header`, `--http-timeout`, `--max-response-size`), `parallel`, `strict_inputs`, `trace(true)`, and `--openapi` files loaded.

4. **Execute workflows sequentially** within each spec:
   - For each workflow: `engine.execute_with_timeout(workflow_id, inputs, execution_timeout).collect().await` → `ExecutionResult`.
   - Extract `TraceStepRecord` events from `EngineEvent::TraceStep` for per-step results.
   - Apply `--expr-diagnostics` semantics exactly as defined in §5 before finalizing the workflow status.
   - Map to `TestCaseResult` using the status mapping in §5.
   - If `--fail-fast` and a workflow fails/errors: stop remaining workflows in this spec AND all remaining specs.

### Why Sequential (Not Concurrent Across Workflows)

- Workflows within a spec may share state (cookies, auth tokens) via source description sessions.
- `--parallel` enables concurrency *within* a workflow where the engine knows it's safe.
- Future: `--concurrent` flag for cross-workflow parallelism.

### Input Merging

Use the same `parse_input_value` (for `--input`) and JSON parsing (for `--input-json`) logic from the `run` handler in `handlers.rs`. When both `--input` and `--input-json` supply the same key, `--input-json` wins (later entries override earlier ones in the merged `BTreeMap<String, Value>`), matching `run` behavior.

### OpenAPI File Loading

`--openapi` files are loaded **once** at startup and passed to every `EngineBuilder`. This matches `run` semantics: one set of supplemental OpenAPI specs applies globally. If different Arazzo specs reference different source descriptions, the same OpenAPI files are offered to all engines — the engine ignores OpenAPI entries that don't match its source descriptions.

---

## 11. Implementation Sequence

### Step 1: Types & CLI skeleton

1. Add `TestFormat` enum and `Test` variant to `cli.rs` (with full flag parity per §1).
2. Add all result types (`TestOutput`, `TestSuiteResult`, etc.) to `output.rs`.
3. Add `Commands::Test` match arm in `main.rs` that calls a stub handler returning `TestOutput::Error { error: "not yet implemented" }`.
4. Add `Some("test")` arm to `handlers::schema()` and update the command list string.
5. **Verify**: `cargo check`, `cargo run -- --json schema` includes `"test"`.

### Step 2: Spec discovery

1. Create `test_runner.rs` with `discover_test_specs()`, delegating directory recursion to `arazzo_mcp::state::discover_specs()`.
2. Wire into handler: discover specs, parse each, count files.
3. **Verify**: `cargo run -- test examples/` discovers and lists files.

### Step 3: Execution loop

1. Implement `run_test_suite()` in `test_runner.rs`:
   - For each spec: build engine, iterate workflows, execute, collect into `TestSuiteResult`.
   - Handle parse errors as suite-level errors (§10 step 1).
   - Handle `--fail-fast`.
   - Handle `--filter` (pre-compiled `Regex`, error variant if invalid).
   - Handle zero-match filter as `Results { total_tests: 0, ... }`, exit 0.
   - Handle `--expr-diagnostics warn/error` exactly like `run`, but translated into per-workflow test outcomes.
   - Compute `TestSummary` from aggregated results.
2. **Verify**: `cargo run -- --json test examples/` produces valid `{"kind":"results",...}`.

### Step 4: TAP formatter

1. Implement `format_tap(output: &TestOutput) -> String`.
2. Include YAML diagnostic blocks for failures/errors.
3. Handle parse-error suites as `not ok N - {file}::(parse)`.
4. Wire as default output.
5. **Verify**: `cargo run -- test examples/` produces valid TAP.

### Step 5: JUnit XML formatter

1. Implement `format_junit(output: &TestOutput) -> String`.
2. XML-escape all interpolated strings.
3. Handle parse-error suites with synthetic `(parse)` test case.
4. **Verify**: output is well-formed XML.

### Step 6: Human summary (stderr)

1. Add colored `PASS`/`FAIL`/`ERROR` lines to stderr.
2. TTY detection for ANSI colors via `std::io::IsTerminal`.
3. Print after all tests, regardless of `--format`.

### Step 7: Schema & tests

1. Run `cargo run -- schema test > docs/schemas/test.schema.json`.
2. Add schema drift test in `crates/arazzo-cli/tests/schema_drift.rs`.
3. Add integration tests in `crates/arazzo-cli/tests/cli_integration.rs`:
   - `test_discovers_arazzo_specs_recursively` (ignores non-.arazzo.yaml files)
   - `test_json_output_results_variant`
   - `test_json_output_error_variant` (no specs found)
   - `test_zero_match_filter_returns_results_with_zero_tests`
   - `test_junit_output_well_formed`
   - `test_tap_output_valid`
   - `test_fail_fast_stops_on_failure`
   - `test_filter_selects_matching_workflows`
   - `test_invalid_filter_returns_error`
   - `test_expr_diagnostics_warn_does_not_fail_tests`
   - `test_expr_diagnostics_error_marks_workflow_as_error`
   - `test_exit_code_zero_on_success`
   - `test_exit_code_one_on_failure`
   - `test_parse_error_suite`

### Step 8: Documentation

1. Update CLAUDE.md commands list.
2. Update README commands table and add "Testing" usage section.

---

## 12. Edge Cases

| Scenario | Behavior |
|----------|----------|
| Spec file fails to parse | `TestSuiteResult { error: Some(msg), tests: [] }`. Counts toward `summary.suite_errors`. |
| Workflow has required inputs not provided | Depends on `--strict-inputs`: error (`TestStatus::Error`) or warning + null defaults |
| Workflow references missing OpenAPI | `TestStatus::Error` on that workflow, others continue |
| Zero specs found | `TestOutput::Error { error: "no test specs found..." }`, exit 1 |
| Zero workflows after filtering | `TestOutput::Results` with empty suites, exit 0 (filter matched nothing is not an error) |
| `--fail-fast` stops mid-suite | Remaining workflows in current and subsequent suites are omitted (not in output) |
| Invalid `--filter` regex | `TestOutput::Error`, exit 1 |
| Non-.arazzo.yaml file passed explicitly | Error: "not an Arazzo spec: {path}" |
| Workflow has no success criteria | Passes if no runtime error occurs |

---

## 13. Example Usage

```bash
# Run all specs in a directory (recursive .arazzo.yaml/.arazzo.yml discovery)
arazzo test tests/api/

# Multiple paths
arazzo test examples/petstore.arazzo.yaml tests/

# JUnit output for CI
arazzo test tests/ --format junit > test-results.xml

# JSON output
arazzo test tests/ --json

# Filter to specific workflows
arazzo test tests/ --filter "create|update"

# With environment-specific inputs and custom headers
arazzo test tests/ -i base_url=https://staging.example.com -H "Authorization: Bearer $TOKEN"

# With additional OpenAPI specs for operationId resolution
arazzo test tests/ --openapi specs/petstore.yaml --openapi specs/payments.yaml

# Fail fast with custom timeouts
arazzo test tests/ --fail-fast --http-timeout 10s --execution-timeout 2m

# JSON-typed inputs
arazzo test tests/ --input-json 'count=42' --input-json 'enabled=true'
```

---

## 14. Estimated Size

| Component | Lines (approx) |
|-----------|---------------|
| `cli.rs` changes | ~60 |
| `main.rs` changes | ~25 |
| `output.rs` types | ~90 |
| `handlers.rs` handler + schema | ~40 |
| `test_runner.rs` (discovery + execution + formatters) | ~500 |
| Integration tests (`cli_integration.rs`) | ~250 |
| Schema drift test | ~10 |
| Schema file (generated) | ~120 |
| **Total** | **~1,100** |
