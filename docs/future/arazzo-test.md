# `arazzo test` Implementation-Ready Plan (5-7 Days)

## 1. Scope Lock (v1)

### In Scope
- Add a first-class `test` command that executes workflows as test cases from one spec file.
- Keep `--json` as the canonical machine-readable stdout contract.
- Add CI report writers: `junit` and `tap`.
- Add workflow filtering, fail-fast, and optional parallel execution.
- Add optional trace capture for failed tests only.

### Out of Scope (v1)
- Fixture server DSL or embedded mock server framework.
- Flaky-test retry policies.
- Multi-spec test suite discovery across directories.

## 2. User-Facing Contract

### Command
```bash
arazzo test <spec>
```

### Flags
- `--workflow <id>` (repeatable exact include)
- `--filter <substring>` (include if workflow id contains substring)
- `--fail-fast`
- `--parallel`
- `--jobs <n>` (default: available CPUs, min 1)
- `--input key=value` (repeatable)
- `--input-json key=<json>` (repeatable)
- `--header Name=value` (repeatable)
- `--openapi <path>` (repeatable)
- `--http-timeout <duration>`
- `--execution-timeout <duration>` (per workflow test)
- `--strict-inputs`
- `--max-response-size <bytes>`
- `--report <junit|tap>`
- `--report-out <path>`
- `--trace-failures-dir <dir>`

### Exit Behavior
- Exit `0` when all selected workflows pass.
- Exit `1` when any selected workflow fails.
- Exit `1` on command/configuration/runtime setup errors.

## 3. JSON Output Contract

Add `TestOutput` in `crates/arazzo-cli/src/output.rs`.

### Success/Failure Suite Envelope
- `kind: "suite"`
- `status: "passed" | "failed"`
- `summary`
  - `total`
  - `passed`
  - `failed`
  - `durationMs`
- `results` (ordered list of case results)
  - `workflowId`
  - `status: "passed" | "failed"`
  - `durationMs`
  - `outputs` (on pass, optional)
  - `error` (on fail)
  - `code` (runtime code on fail, optional)

### Command Error Envelope
- `kind: "error"`
- `error`
- `code` (optional stable code)

## 4. Internal Data Model

In `output.rs`, add:
- `TestSuiteSummary`
- `TestCaseResult`
- `TestOutput`

Determinism rule:
- Always emit `results` in workflow-selection order, even when parallel execution is enabled.

## 5. File-by-File Change Plan

1. `crates/arazzo-cli/src/cli.rs`
- Add `Commands::Test { ...flags... }`.

2. `crates/arazzo-cli/src/main.rs`
- Dispatch `Commands::Test` to `handlers::test_workflows(...)`.

3. `crates/arazzo-cli/src/handlers.rs`
- Implement `test_workflows(...)`.
- Reuse existing `run` input parsing helpers for `--input` and `--input-json`.
- Build `EngineBuilder` using existing runtime knobs (`strict_inputs`, `max_response_bytes`, timeouts, headers, openapi).
- Add workflow selection helper.
- Add sequential and parallel runner paths.
- Add report writing integration.
- Add trace-on-failure artifact generation.

4. `crates/arazzo-cli/src/output.rs`
- Add `TestOutput` schema types.
- Add `emit_test_suite(...)`.
- Add `emit_test_error(...)`.

5. `crates/arazzo-cli/src/handlers.rs` (or `src/test_report.rs`)
- Add JUnit XML writer.
- Add TAP writer.

6. `crates/arazzo-cli/src/handlers.rs` schema function
- Include `"test"` in schema list.
- Map `schema test` to `TestOutput`.

7. `docs/schemas/test.schema.json`
- Generate and check in via `cargo run -p arazzo-cli -- schema test > docs/schemas/test.schema.json`.

8. `README.md`
- Add `test` command docs and CI examples.

## 6. Runner Semantics

### Case Mapping
- Each selected workflow is exactly one test case.

### Pass/Fail Definition
- Pass: `engine.execute_with_timeout(...).collect().outputs` returns `Ok(outputs)`.
- Fail: returns `Err(RuntimeError)` or criteria failure propagated as runtime error.

### Captured Per Case
- `workflowId`
- `durationMs`
- `status`
- `outputs` (pass)
- `error` and `code` (fail)

### Suite Status
- `passed` iff `failed == 0`; otherwise `failed`.

## 7. Workflow Selection Rules

1. Start from all workflows in spec.
2. If any `--workflow` provided, include exact-id matches only.
3. If `--filter` provided, apply substring filter on remaining set.
4. If resulting set is empty, return JSON/text error with code `TEST_NO_WORKFLOWS_SELECTED`.
5. Preserve deterministic order based on spec order.

## 8. Execution Modes

### Sequential Mode
- Execute in selected order.
- If `--fail-fast`, stop after first failed case.

### Parallel Mode
- Use `tokio::task::JoinSet`.
- Respect `--jobs` concurrency.
- Tag each scheduled case with `selection_index`.
- Collect all completed results, then sort by `selection_index` before emit.
- `--fail-fast` in parallel means:
  - stop scheduling new cases once first failure is observed,
  - allow in-flight cases to finish,
  - report only executed cases.

## 9. Reporting Plan

### JUnit
- Write one `<testsuite>` for command invocation.
- One `<testcase>` per executed workflow.
- Failed cases include `<failure message="...">...</failure>`.
- `tests`, `failures`, `time` derived from suite summary.

### TAP
- Header line: `1..N`.
- `ok i - <workflowId>` for pass.
- `not ok i - <workflowId>` for fail.

### Output Paths
- If `--report` set and `--report-out` omitted:
  - junit -> `arazzo-test-results.xml`
  - tap -> `arazzo-test-results.tap`

## 10. Trace-Failure Artifacts

When `--trace-failures-dir` is set:
- For each failed case, write trace file:
  - `<dir>/<workflowId>.trace.json`
- Use existing trace redaction/write pipeline.
- Sanitized file name strategy:
  - replace non `[A-Za-z0-9._-]` with `_`.

## 11. Error Codes (CLI Layer)

Planned stable command-level codes:
- `TEST_SPEC_READ_FILE`
- `TEST_SPEC_PARSE_YAML`
- `TEST_SPEC_VALIDATION`
- `TEST_NO_WORKFLOWS_SELECTED`
- `TEST_OPENAPI_READ_FILE`
- `TEST_REPORT_WRITE_FAILED`

Runtime-level codes are propagated from `RuntimeErrorKind::code()`.

## 12. Test Plan

### Integration Tests (`crates/arazzo-cli/tests/cli_integration.rs`)
1. all-pass suite in `--json` returns `kind=suite`, exit 0.
2. mixed pass/fail returns exit 1, correct summary counts.
3. `--fail-fast` executes fewer cases than full set.
4. `--workflow` exact include behavior.
5. `--filter` substring include behavior.
6. `--parallel --jobs 2` deterministic ordering in JSON results.
7. `--report junit` creates valid file with expected tags.
8. `--report tap` creates expected `1..N` + `ok/not ok` lines.
9. empty selection returns `TEST_NO_WORKFLOWS_SELECTED`.
10. `--trace-failures-dir` writes trace artifacts only for failed cases.

### Snapshot Tests (`crates/arazzo-cli/tests/cli_contract_snapshots.rs`)
- `test-pass.json`
- `test-fail.json`

### Schema Drift (`crates/arazzo-cli/tests/schema_drift.rs`)
- Add `schema_test_matches_checked_in_file`.

### Optional Unit Tests (`handlers.rs`)
- selection logic correctness
- summary aggregation
- report writer escaping

## 13. Day-by-Day Delivery Plan

### Day 1
- CLI command surface (`test`) and schema type scaffolding.
- Add placeholder integration tests.

### Day 2
- Sequential runner and suite JSON output.
- Exit semantics and error handling.

### Day 3
- JUnit and TAP report writers.
- Integration tests for report files.

### Day 4
- Parallel runner with `--jobs`.
- Deterministic ordering guarantees.
- Fail-fast behavior for both sequential and parallel modes.

### Day 5
- `--trace-failures-dir` artifacts.
- Snapshot tests and schema drift test.

### Day 6
- Hardening edge cases.
- README updates and command examples.
- Full verification run.

### Day 7 (Buffer)
- polish, error message cleanup, small UX refinements.

## 14. Risks and Mitigations

### Risk: nondeterministic output under parallel execution
- Mitigation: index-tag each case and sort before emit/report.

### Risk: XML/TAP escaping bugs
- Mitigation: central escaping helpers + targeted tests.

### Risk: fail-fast ambiguity in parallel mode
- Mitigation: explicitly document behavior and test it.

### Risk: scope creep into fixture/mocking features
- Mitigation: keep v1 focused on workflow-as-test execution only.

## 15. Definition of Done

- `arazzo test` implemented with all v1 flags listed above.
- JSON schema available via `schema test` and checked into `docs/schemas/test.schema.json`.
- JUnit and TAP reports implemented and tested.
- Deterministic ordering guaranteed in JSON and report outputs.
- README updated with usage and CI examples.
- All validation gates pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace`
