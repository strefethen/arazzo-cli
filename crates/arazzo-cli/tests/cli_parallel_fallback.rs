#![forbid(unsafe_code)]

//! How `run --parallel` reports a workflow whose steps ran one at a time.
//!
//! The engine keeps a workflow sequential when an action redirects control
//! flow, a retry first runs another step or workflow, or a step calls another
//! workflow. `-v` names the reason on stderr and `--trace` records it under
//! `run.sequentialFallbacks`; a workflow whose only actions are plain retries
//! runs in parallel levels and reports nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SPEC: &str = r#"
arazzo: 1.0.1
info:
  title: Parallel fallback
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: stops-early
    steps:
      - stepId: a
        operationPath: /a
        successCriteria:
          - condition: $statusCode == 200
        onSuccess:
          - name: stop
            type: end
      - stepId: b
        operationPath: /b
        successCriteria:
          - condition: $statusCode == 200
  - workflowId: retries
    steps:
      - stepId: a
        operationPath: /a
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - name: retry-unavailable
            type: retry
            retryLimit: 2
            criteria:
              - condition: $statusCode == 503
      - stepId: b
        operationPath: /b
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - name: retry-unavailable
            type: retry
            retryLimit: 2
            criteria:
              - condition: $statusCode == 503
"#;

const STOPS_EARLY_MESSAGE: &str = "workflow \"stops-early\" runs sequentially because step \"a\" has an onSuccess action of type \"end\", which redirects control flow";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-parallel-fallback-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let target = self.path.join(name);
        fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
        target
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn cli_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        return PathBuf::from(path);
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/arazzo-cli");
    assert!(path.exists(), "CLI binary not found at {}", path.display());
    path
}

fn run(args: &[&str]) -> Output {
    let output = Command::new(cli_bin())
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"));
    assert!(
        output.status.success(),
        "arazzo-cli {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read_trace(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|err| panic!("parsing trace {}: {err}", path.display()))
}

#[test]
fn verbose_parallel_run_names_the_step_that_kept_it_sequential() {
    let temp = TempDir::new();
    let spec = temp.write("fallback.arazzo.yaml", SPEC);
    let spec = spec.to_string_lossy();
    let trace = temp.join("trace.json");
    let trace_arg = trace.to_string_lossy();

    let output = run(&[
        "--json",
        "-v",
        "run",
        &spec,
        "stops-early",
        "--dry-run",
        "--parallel",
        "--trace",
        &trace_arg,
    ]);

    assert!(
        stderr(&output).contains(&format!("Parallel mode: {STOPS_EARLY_MESSAGE}\n")),
        "stderr: {}",
        stderr(&output)
    );
    let trace = read_trace(&trace);
    assert_eq!(trace.pointer("/run/parallel"), Some(&Value::Bool(true)));
    assert_eq!(
        trace.pointer("/run/sequentialFallbacks"),
        Some(&json!([{
            "workflowId": "stops-early",
            "reason": "controlFlowAction",
            "stepId": "a",
            "message": STOPS_EARLY_MESSAGE,
        }]))
    );
}

#[test]
fn quiet_parallel_run_still_records_the_fallback_in_its_trace() {
    let temp = TempDir::new();
    let spec = temp.write("fallback.arazzo.yaml", SPEC);
    let spec = spec.to_string_lossy();
    let trace = temp.join("trace.json");
    let trace_arg = trace.to_string_lossy();

    let output = run(&[
        "--json",
        "run",
        &spec,
        "stops-early",
        "--dry-run",
        "--parallel",
        "--trace",
        &trace_arg,
    ]);

    assert!(
        !stderr(&output).contains("Parallel mode:"),
        "stderr: {}",
        stderr(&output)
    );
    let trace = read_trace(&trace);
    assert_eq!(
        trace.pointer("/run/sequentialFallbacks/0/reason"),
        Some(&json!("controlFlowAction"))
    );
}

#[test]
fn retry_only_parallel_run_reports_no_fallback() {
    let temp = TempDir::new();
    let spec = temp.write("fallback.arazzo.yaml", SPEC);
    let spec = spec.to_string_lossy();
    let trace = temp.join("trace.json");
    let trace_arg = trace.to_string_lossy();

    let output = run(&[
        "--json",
        "-v",
        "run",
        &spec,
        "retries",
        "--dry-run",
        "--parallel",
        "--trace",
        &trace_arg,
    ]);

    assert!(
        !stderr(&output).contains("Parallel mode:"),
        "stderr: {}",
        stderr(&output)
    );
    let trace = read_trace(&trace);
    assert_eq!(trace.pointer("/run/parallel"), Some(&Value::Bool(true)));
    assert_eq!(trace.pointer("/run/sequentialFallbacks"), None);
}

#[test]
fn verbose_parallel_step_run_reports_single_step_execution() {
    let temp = TempDir::new();
    let spec = temp.write("fallback.arazzo.yaml", SPEC);
    let spec = spec.to_string_lossy();

    let output = run(&[
        "--json",
        "-v",
        "run",
        &spec,
        "retries",
        "--step",
        "b",
        "--dry-run",
        "--parallel",
    ]);

    assert!(
        stderr(&output).contains(
            "Parallel mode: workflow \"retries\" runs sequentially because step \"b\" was selected for single-step execution, which runs one step at a time\n"
        ),
        "stderr: {}",
        stderr(&output)
    );
}
