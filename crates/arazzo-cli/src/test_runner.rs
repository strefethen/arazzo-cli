use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use arazzo_runtime::{
    ClientConfig, EngineBuilder, EngineEvent, RuntimeErrorKind, TransportWarning,
};
use serde_json::Value;

use crate::cli::ExpressionDiagnosticsMode;
use crate::output::{
    TestCaseResult, TestOutput, TestStatus, TestStepResult, TestSuiteResult, TestSummary,
};
use crate::transport::{self, TransportFlags};

/// Options for running the test suite (mirrors CLI flags).
pub struct TestRunOptions {
    pub inputs: BTreeMap<String, Value>,
    pub http_timeout: Duration,
    pub execution_timeout: Duration,
    pub headers: BTreeMap<String, String>,
    pub openapi_bytes: Vec<Vec<u8>>,
    pub expr_diagnostics: ExpressionDiagnosticsMode,
    pub parallel: bool,
    pub strict_inputs: bool,
    pub max_response_size: Option<usize>,
    pub fail_fast: bool,
    pub filter: Option<regex::Regex>,
    pub transport: TransportFlags,
}

/// Discover Arazzo test specs from a list of file paths and directories.
///
/// Files must end in `.arazzo.yaml` or `.arazzo.yml`. Directories are scanned
/// recursively using the same convention as `arazzo_mcp::state::discover_specs`.
pub fn discover_test_specs(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut specs = Vec::new();

    for path_str in paths {
        let path = Path::new(path_str);
        if !path.exists() {
            return Err(format!("path does not exist: {path_str}"));
        }
        if path.is_dir() {
            let discovered = arazzo_mcp::state::discover_specs(path_str)
                .map_err(|err| format!("scanning {path_str}: {err}"))?;
            for p in discovered {
                specs.push(PathBuf::from(p));
            }
        } else if is_arazzo_spec_file(path_str) {
            specs.push(path.to_path_buf());
        } else {
            return Err(format!(
                "not an Arazzo spec (expected .arazzo.yaml or .arazzo.yml): {path_str}"
            ));
        }
    }

    specs.sort();
    specs.dedup();

    if specs.is_empty() {
        return Err(format!("no test specs found in: {}", paths.join(", ")));
    }

    Ok(specs)
}

/// Run the full test suite: parse specs, execute workflows, collect results.
pub async fn run_test_suite(specs: &[PathBuf], opts: &TestRunOptions) -> TestOutput {
    let run_start = Instant::now();
    let mut suites = Vec::new();
    let mut summary = TestSummary {
        total_suites: specs.len(),
        total_tests: 0,
        passed: 0,
        failed: 0,
        errors: 0,
        suite_errors: 0,
        duration_ms: 0,
    };
    let mut bail = false;
    // Transport-trust bookkeeping across all suites: the engines stay
    // stderr-silent (`apply(.., false)`); this run aggregates their
    // structured warnings, deduplicates cleartext warnings by host so
    // "once per host per run" holds across suites, and tracks which
    // insecure exceptions any suite's request consumed.
    let mut transport_warnings: Vec<TransportWarning> = Vec::new();
    if let Some(startup) = opts.transport.startup_warning() {
        transport_warnings.push(startup);
    }
    let mut warned_cleartext_hosts: BTreeSet<String> = BTreeSet::new();
    let configured_insecure: BTreeSet<String> =
        opts.transport.insecure_hosts.iter().cloned().collect();
    let mut used_insecure: BTreeSet<String> = BTreeSet::new();
    let mut any_live_suite = false;

    for spec_path in specs {
        if bail {
            break;
        }

        let file_str = spec_path.display().to_string();
        let suite_start = Instant::now();

        // Parse the spec.
        let spec = match arazzo_validate::parse(spec_path) {
            Ok(s) => s,
            Err(err) => {
                summary.suite_errors += 1;
                suites.push(TestSuiteResult {
                    file: file_str.clone(),
                    name: spec_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| file_str.clone()),
                    tests: vec![],
                    duration_ms: suite_start.elapsed().as_millis() as u64,
                    error: Some(format!("{err}")),
                });
                if opts.fail_fast {
                    bail = true;
                }
                continue;
            }
        };

        let suite_name = if spec.info.title.is_empty() {
            spec_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file_str.clone())
        } else {
            spec.info.title.clone()
        };

        // Collect workflow IDs before moving spec into the engine builder.
        let workflow_ids: Vec<String> = spec
            .workflows
            .iter()
            .filter(|w| {
                if let Some(re) = &opts.filter {
                    re.is_match(&w.workflow_id)
                } else {
                    true
                }
            })
            .map(|w| w.workflow_id.clone())
            .collect();

        // Build engine.
        let mut cfg = ClientConfig {
            timeout: opts.http_timeout,
            ..ClientConfig::default()
        };
        opts.transport.apply(&mut cfg, false);
        cfg.default_headers = opts.headers.clone();

        let mut builder = EngineBuilder::new(spec)
            .client_config(cfg)
            .parallel(opts.parallel)
            .strict_inputs(opts.strict_inputs)
            .trace(true);

        if let Some(dir) = spec_path.parent() {
            builder = builder.source_base_dir(dir);
        }

        if let Some(max_bytes) = opts.max_response_size {
            builder = builder.max_response_bytes(max_bytes);
        }
        for openapi in &opts.openapi_bytes {
            builder = builder.openapi_spec(openapi.clone());
        }

        let engine = match builder.build() {
            Ok(e) => e,
            Err(err) => {
                summary.suite_errors += 1;
                suites.push(TestSuiteResult {
                    file: file_str.clone(),
                    name: suite_name,
                    tests: vec![],
                    duration_ms: suite_start.elapsed().as_millis() as u64,
                    error: Some(format!("engine build failed: {err}")),
                });
                if opts.fail_fast {
                    bail = true;
                }
                continue;
            }
        };
        any_live_suite = true;

        // Execute each workflow.
        let mut cases = Vec::new();
        for workflow_id in &workflow_ids {
            let wf_start = Instant::now();
            let exec_result = engine
                .execute_with_timeout(workflow_id, opts.inputs.clone(), opts.execution_timeout)
                .collect()
                .await;

            // Collect engine transport warnings, deduplicated by host
            // across suites so "once per host per run" holds for the
            // whole invocation.
            for event in &exec_result.events {
                if let EngineEvent::TransportWarning(warning) = event {
                    if warning
                        .hosts
                        .iter()
                        .any(|host| !warned_cleartext_hosts.contains(host))
                    {
                        warned_cleartext_hosts.extend(warning.hosts.iter().cloned());
                        transport_warnings.push(warning.clone());
                    }
                }
            }

            // Extract per-step results from trace events.
            let steps: Vec<TestStepResult> = exec_result
                .events
                .iter()
                .filter_map(|ev| {
                    if let EngineEvent::TraceStep(record) = ev {
                        Some(record)
                    } else {
                        None
                    }
                })
                .map(|record| {
                    let step_status = if record.error.is_some() {
                        TestStatus::Error
                    } else {
                        TestStatus::Pass
                    };
                    TestStepResult {
                        step_id: record.step_id.clone(),
                        status: step_status,
                        duration_ms: record.duration_ms,
                        status_code: record.response.as_ref().map(|r| r.status_code),
                        error: record.error.clone(),
                    }
                })
                .collect();

            // Check for expression diagnostic warnings.
            let expr_warning_count: usize = exec_result
                .events
                .iter()
                .filter_map(|ev| {
                    if let EngineEvent::TraceStep(record) = ev {
                        Some(record.warnings.len())
                    } else {
                        None
                    }
                })
                .sum();

            // Determine workflow-level status.
            let (status, error, error_code) = match &exec_result.outputs {
                Ok(_) => {
                    // Check expr-diagnostics error mode.
                    if opts.expr_diagnostics == ExpressionDiagnosticsMode::Error
                        && expr_warning_count > 0
                    {
                        (
                            TestStatus::Error,
                            Some(format!(
                                "expression diagnostics reported {expr_warning_count} warning(s)"
                            )),
                            Some("RUNTIME_EXPRESSION_DIAGNOSTICS".to_string()),
                        )
                    } else {
                        (TestStatus::Pass, None, None)
                    }
                }
                Err(err) => {
                    let status = if err.kind == RuntimeErrorKind::SuccessCriteriaFailed {
                        TestStatus::Fail
                    } else {
                        TestStatus::Error
                    };
                    (
                        status,
                        Some(err.message.clone()),
                        Some(err.kind.code().to_string()),
                    )
                }
            };

            let duration_ms = wf_start.elapsed().as_millis() as u64;

            // Update summary counts.
            summary.total_tests += 1;
            match &status {
                TestStatus::Pass => summary.passed += 1,
                TestStatus::Fail => summary.failed += 1,
                TestStatus::Error => summary.errors += 1,
            }

            let failed = !matches!(status, TestStatus::Pass);

            cases.push(TestCaseResult {
                workflow_id: workflow_id.clone(),
                status,
                duration_ms,
                steps,
                error,
                error_code,
            });

            if opts.fail_fast && failed {
                bail = true;
                break;
            }
        }

        // An exception is "used" for the whole run when any suite's
        // request targeted it.
        let engine_unused: BTreeSet<String> = engine.unused_insecure_hosts().into_iter().collect();
        used_insecure.extend(configured_insecure.difference(&engine_unused).cloned());

        suites.push(TestSuiteResult {
            file: file_str,
            name: suite_name,
            tests: cases,
            duration_ms: suite_start.elapsed().as_millis() as u64,
            error: None,
        });
    }

    if any_live_suite && !opts.transport.insecure_all {
        let unused: Vec<String> = configured_insecure
            .difference(&used_insecure)
            .cloned()
            .collect();
        if let Some(warning) = transport::unused_warning(unused) {
            transport_warnings.push(warning);
        }
    }

    summary.duration_ms = run_start.elapsed().as_millis() as u64;

    TestOutput::Results {
        summary,
        suites,
        transport_warnings,
    }
}

fn is_arazzo_spec_file(path: &str) -> bool {
    path.ends_with(".arazzo.yaml") || path.ends_with(".arazzo.yml")
}

// ── TAP formatter ────────────────────────────────────────────────

pub fn format_tap(output: &TestOutput) -> String {
    let TestOutput::Results {
        summary, suites, ..
    } = output
    else {
        if let TestOutput::Error { error, .. } = output {
            return format!("TAP version 13\n1..0\nBail out! {error}\n");
        }
        return String::new();
    };

    let report_count = summary.total_tests + summary.suite_errors;
    let mut out = format!("TAP version 13\n1..{report_count}\n");
    let mut n: usize = 0;

    for suite in suites {
        if let Some(err) = &suite.error {
            n += 1;
            out.push_str(&format!("not ok {n} - {}::(parse)\n", suite.file));
            out.push_str("  ---\n");
            out.push_str(&format!("  error: \"{}\"\n", yaml_escape(err)));
            out.push_str("  ...\n");
            continue;
        }
        for case in &suite.tests {
            n += 1;
            let ok = matches!(case.status, TestStatus::Pass);
            let prefix = if ok { "ok" } else { "not ok" };
            out.push_str(&format!(
                "{prefix} {n} - {}::{}\n",
                suite.file, case.workflow_id
            ));
            out.push_str("  ---\n");
            out.push_str(&format!("  duration_ms: {}\n", case.duration_ms));
            out.push_str(&format!("  steps: {}\n", case.steps.len()));
            if let Some(err) = &case.error {
                out.push_str(&format!("  error: \"{}\"\n", yaml_escape(err)));
            }
            if let Some(code) = &case.error_code {
                out.push_str(&format!("  error_code: {code}\n"));
            }
            out.push_str("  ...\n");
        }
    }

    out
}

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ── JUnit XML formatter ──────────────────────────────────────────

pub fn format_junit(output: &TestOutput) -> String {
    let TestOutput::Results {
        summary, suites, ..
    } = output
    else {
        if let TestOutput::Error { error, .. } = output {
            return format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuites name=\"arazzo\" tests=\"0\" failures=\"0\" errors=\"1\" time=\"0\">\n\
                 <testsuite name=\"arazzo\" tests=\"1\" failures=\"0\" errors=\"1\" time=\"0\">\n\
                 <testcase name=\"(setup)\" classname=\"arazzo\"><error message=\"{}\"/></testcase>\n\
                 </testsuite>\n</testsuites>\n",
                xml_escape(error)
            );
        }
        return String::new();
    };

    let total_time = summary.duration_ms as f64 / 1000.0;
    let total_tests: usize = suites
        .iter()
        .map(|s| if s.error.is_some() { 1 } else { s.tests.len() })
        .sum();
    let total_failures: usize = suites
        .iter()
        .flat_map(|s| &s.tests)
        .filter(|t| matches!(t.status, TestStatus::Fail))
        .count();
    let total_errors: usize = summary.suite_errors
        + suites
            .iter()
            .flat_map(|s| &s.tests)
            .filter(|t| matches!(t.status, TestStatus::Error))
            .count();

    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <testsuites name=\"arazzo\" tests=\"{total_tests}\" failures=\"{total_failures}\" errors=\"{total_errors}\" time=\"{total_time:.3}\">\n"
    );

    for suite in suites {
        let suite_time = suite.duration_ms as f64 / 1000.0;
        if let Some(err) = &suite.error {
            out.push_str(&format!(
                "<testsuite name=\"{}\" package=\"{}\" tests=\"1\" failures=\"0\" errors=\"1\" time=\"{suite_time:.3}\">\n\
                 <testcase name=\"(parse)\" classname=\"{}\" time=\"{suite_time:.3}\"><error message=\"{}\"/></testcase>\n\
                 </testsuite>\n",
                xml_escape(&suite.name),
                xml_escape(&suite.file),
                xml_escape(&suite.file),
                xml_escape(err),
            ));
            continue;
        }

        let suite_tests = suite.tests.len();
        let suite_failures = suite
            .tests
            .iter()
            .filter(|t| matches!(t.status, TestStatus::Fail))
            .count();
        let suite_errors = suite
            .tests
            .iter()
            .filter(|t| matches!(t.status, TestStatus::Error))
            .count();

        out.push_str(&format!(
            "<testsuite name=\"{}\" package=\"{}\" tests=\"{suite_tests}\" failures=\"{suite_failures}\" errors=\"{suite_errors}\" time=\"{suite_time:.3}\">\n",
            xml_escape(&suite.name),
            xml_escape(&suite.file),
        ));

        for case in &suite.tests {
            let case_time = case.duration_ms as f64 / 1000.0;
            out.push_str(&format!(
                "<testcase name=\"{}\" classname=\"{}\" time=\"{case_time:.3}\"",
                xml_escape(&case.workflow_id),
                xml_escape(&suite.file),
            ));
            match case.status {
                TestStatus::Pass => {
                    out.push_str("/>\n");
                }
                TestStatus::Fail => {
                    out.push_str(">\n");
                    let msg = case.error.as_deref().unwrap_or("criteria failure");
                    out.push_str(&format!(
                        "<failure message=\"{}\" type=\"CriteriaFailure\">{}</failure>\n",
                        xml_escape(msg),
                        xml_escape(msg),
                    ));
                    out.push_str("</testcase>\n");
                }
                TestStatus::Error => {
                    out.push_str(">\n");
                    let msg = case.error.as_deref().unwrap_or("runtime error");
                    let err_type = case.error_code.as_deref().unwrap_or("RuntimeError");
                    out.push_str(&format!(
                        "<error message=\"{}\" type=\"{}\">{}</error>\n",
                        xml_escape(msg),
                        xml_escape(err_type),
                        xml_escape(msg),
                    ));
                    out.push_str("</testcase>\n");
                }
            }
        }

        out.push_str("</testsuite>\n");
    }

    out.push_str("</testsuites>\n");
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Human summary (stderr) ───────────────────────────────────────

pub fn print_human_summary(output: &TestOutput) {
    use std::io::IsTerminal;

    let use_color = std::io::stderr().is_terminal();

    let TestOutput::Results {
        summary, suites, ..
    } = output
    else {
        if let TestOutput::Error { error, .. } = output {
            eprintln!("{}", color_str("ERROR", "\x1b[31m", use_color));
            eprintln!("  {error}");
        }
        return;
    };

    for suite in suites {
        if let Some(err) = &suite.error {
            eprintln!(
                "  {}  {}::(parse)",
                color_str("ERROR", "\x1b[33m", use_color),
                suite.file,
            );
            eprintln!("        {err}");
            continue;
        }
        for case in &suite.tests {
            let (label, color) = match case.status {
                TestStatus::Pass => ("PASS", "\x1b[32m"),
                TestStatus::Fail => ("FAIL", "\x1b[31m"),
                TestStatus::Error => ("ERROR", "\x1b[33m"),
            };
            eprintln!(
                "  {}  {}::{} ({}ms)",
                color_str(label, color, use_color),
                suite.file,
                case.workflow_id,
                case.duration_ms,
            );
            if let Some(err) = &case.error {
                eprintln!("        {err}");
            }
        }
    }

    eprintln!();
    let ok_suites = suites.iter().filter(|s| s.error.is_none()).count();
    let errored_suites = summary.suite_errors;
    eprintln!(
        "Suites: {} total ({ok_suites} ok, {errored_suites} errored)",
        summary.total_suites,
    );

    let mut parts = Vec::new();
    if summary.passed > 0 {
        parts.push(color_str(
            &format!("{} passed", summary.passed),
            "\x1b[32m",
            use_color,
        ));
    }
    if summary.failed > 0 {
        parts.push(color_str(
            &format!("{} failed", summary.failed),
            "\x1b[31m",
            use_color,
        ));
    }
    if summary.errors > 0 {
        parts.push(color_str(
            &format!("{} errored", summary.errors),
            "\x1b[33m",
            use_color,
        ));
    }
    parts.push(format!("{} total", summary.total_tests));
    eprintln!("Tests:  {}", parts.join(", "));

    let secs = summary.duration_ms as f64 / 1000.0;
    eprintln!("Time:   {secs:.3}s");
}

fn color_str(text: &str, ansi: &str, use_color: bool) -> String {
    if use_color {
        format!("{ansi}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
