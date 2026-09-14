#![forbid(unsafe_code)]

//! CLI-surface proof that GJSON-only `#` syntax is rejected on typed JSONPath
//! surfaces and that the rejection is *visible* to an operator (audit finding
//! F20).
//!
//! Arazzo v1.1.0 §5.8.11.4.3 requires a `type: jsonpath` condition to be *"a
//! valid JSONPath expression conforming to [RFC9535]"*; §5.8.12 requires an
//! implementation to apply the named version's semantics; and §5.8.12.1 allows
//! only `rfc9535` and `draft-goessner-dispatch-jsonpath-00` as JSONPath
//! version tokens. GJSON's `#` forms are syntax in neither.
//!
//! Two surfaces, because they see different halves of the run:
//!
//! * a dry run resolves parameters, payloads and replacement targets but never
//!   evaluates success criteria, so it is where selector and replacement
//!   rejection becomes observable — as `--json` warnings plus an unchanged
//!   resolved body;
//! * a live run against a hermetic `tiny_http` server is where criterion
//!   rejection becomes observable — as the ordinary criteria failure, the
//!   `--json` error warning list, and the `--trace` file's criterion record.
//!
//! Neither asserts that no request is issued: the settled policy is warn and
//! continue, not a hard runtime error with zero HTTP.
//!
//! Engine-level routing for the same forms is owned by
//! `crates/arazzo-runtime/tests/jsonpath_gjson_rejection.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SYNTAX_ERROR: &str = "invalid JSONPath syntax";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repo_root() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&path).unwrap_or_else(|err| panic!("canonicalizing {}: {err}", path.display()))
}

fn cli_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        return PathBuf::from(path);
    }
    let path = repo_root().join("target/debug/arazzo-cli");
    assert!(path.exists(), "CLI binary not found at {}", path.display());
    path
}

fn write_file(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(cli_bin())
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "parsing JSON stdout failed: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

struct EchoServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Hermetic `tiny_http` server that always answers with the fixture document.
/// No network: it binds an ephemeral loopback port.
///
/// `body` is owned rather than borrowed so this file carries no explicit
/// lifetime: the conformance manifest's evidence scanner
/// (`conformance_manifest.rs`) treats a `'` as the start of a character
/// literal, so a `&'static str` parameter would hide every test item after it
/// from the evidence gate.
fn start_echo_server(body: String) -> EchoServer {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("bind hermetic tiny_http server: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(mut request)) => {
                    let mut drained = String::new();
                    let _ = request.as_reader().read_to_string(&mut drained);
                    let response = tiny_http::Response::from_string(body.clone())
                        .with_status_code(tiny_http::StatusCode(200))
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"application/json"[..],
                            )
                            .unwrap_or_else(|()| panic!("building the content-type header")),
                        );
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    EchoServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

/// A dry run resolves the request without sending it, so a GJSON replacement
/// target and a GJSON Selector Object used as a replacement value both surface
/// as `--json` warnings with the resolved body left untouched. The run still
/// succeeds: the settled policy is warn and continue, so this deliberately
/// does not assert a hard error or a zero-request outcome.
#[test]
fn run_dry_run_reports_gjson_selector_and_replacement_rejection() {
    let temp = TempDir::new("arazzo-cli-jsonpath-gjson-dry-run");
    let spec_path = temp.path().join("gjson-dry-run.arazzo.yaml");
    write_file(
        &spec_path,
        r#"arazzo: 1.1.0
info:
  title: GJSON typed-surface rejection
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        document:
          type: object
    steps:
      - stepId: update
        operationPath: POST /items
        requestBody:
          contentType: application/json
          payload:
            keep: old
            items:
              - sku: A
                q: old
              - sku: B
                q: old
            picked: old
          replacements:
            - target: $.keep
              targetSelectorType: jsonpath
              value: new
            - target: '$.items.#(sku=="A").q'
              targetSelectorType: jsonpath
              value: new
            - target: $.picked
              targetSelectorType: jsonpath
              value:
                context: $inputs.document
                selector: '$.items.#(sku=="A")#.q'
                type: jsonpath
"#,
    );

    let spec = spec_path.to_string_lossy().to_string();
    let document = r#"document={"items":[{"sku":"A","q":1},{"sku":"A","q":2}]}"#;
    let output = run(&["--json", "run", &spec, "wf", "--dry-run", "-i", document]);
    assert!(
        output.status.success(),
        "a rejected selector warns and continues; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = stdout_json(&output);
    let requests = match body.get("requests").and_then(Value::as_array) {
        Some(requests) => requests,
        None => panic!("expected dryRun.requests: {body}"),
    };
    assert_eq!(requests.len(), 1, "{body}");

    // Only the sound replacement moved. Every `q` a GJSON write would have
    // reached, and the value target whose selector failed, keep their input.
    assert_eq!(
        requests[0].get("body"),
        Some(&json!({
            "keep": "new",
            "items": [
                {"sku": "A", "q": "old"},
                {"sku": "B", "q": "old"}
            ],
            "picked": "old"
        })),
        "{body}"
    );

    let warnings = string_array(&requests[0], "warnings");
    let target_warnings: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.starts_with("requestBody.replacements[1]:"))
        .collect();
    assert_eq!(target_warnings.len(), 1, "{warnings:?}");
    assert!(target_warnings[0].contains(SYNTAX_ERROR), "{warnings:?}");

    // A failed replacement *value* reports twice: the selector diagnostic and
    // the notice that the replacement was skipped and the body left alone.
    let value_warnings: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.starts_with("requestBody.replacements[2]:"))
        .collect();
    assert_eq!(value_warnings.len(), 2, "{warnings:?}");
    assert!(value_warnings[0].contains(SYNTAX_ERROR), "{warnings:?}");
    assert!(
        value_warnings[1].contains("value did not resolve; replacement skipped and body unchanged"),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.starts_with("requestBody.replacements[0]:")),
        "the sound replacement must not warn: {warnings:?}"
    );
}

/// A dry run never evaluates success criteria, so criterion rejection is
/// proven against a live hermetic server. The GJSON condition fails the step
/// with the ordinary criteria failure, the `--json` error carries the
/// diagnostic, and the `--trace` file records it on the criterion itself —
/// the permanent operator-visible surface.
#[test]
fn run_trace_reports_gjson_criterion_rejection_against_a_live_server() {
    let server =
        start_echo_server(r#"{"items":[{"sku":"A","q":1},{"sku":"B","q":2}]}"#.to_string());
    let temp = TempDir::new("arazzo-cli-jsonpath-gjson-criterion");
    let spec_path = temp.path().join("gjson-criterion.arazzo.yaml");
    let trace_path = temp.path().join("trace.json");
    write_file(
        &spec_path,
        &format!(
            r#"arazzo: 1.1.0
info:
  title: GJSON criterion rejection
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {base}
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: check
        operationPath: GET /items
        successCriteria:
          - context: $response.body
            condition: '$.items.#(sku=="A").q'
            type: jsonpath
"#,
            base = server.base_url
        ),
    );

    let spec = spec_path.to_string_lossy().to_string();
    let trace = trace_path.to_string_lossy().to_string();
    // `--expr-diagnostics warn` is what promotes step warnings into the
    // structured `--json` list; the `--trace` file records them either way.
    let output = run(&[
        "--json",
        "run",
        &spec,
        "wf",
        "--trace",
        &trace,
        "--expr-diagnostics",
        "warn",
    ]);
    assert!(
        !output.status.success(),
        "an unparseable condition must fail the step; stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );

    let body = stdout_json(&output);
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUNTIME_SUCCESS_CRITERIA_FAILED"),
        "{body}"
    );
    let warnings = string_array(&body, "warnings");
    let reported: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.contains(SYNTAX_ERROR))
        .collect();
    assert_eq!(reported.len(), 1, "{warnings:?}");
    assert!(
        reported[0].contains("workflow \"wf\" step \"check\": successCriteria[0]:"),
        "the diagnostic names the failing criterion: {}",
        reported[0]
    );

    let trace_file = read_json(&trace_path);
    let steps = match trace_file.get("steps").and_then(Value::as_array) {
        Some(steps) => steps,
        None => panic!("expected trace steps: {trace_file}"),
    };
    assert_eq!(steps.len(), 1, "{trace_file}");
    // The request was issued and answered: the rejection is a criterion
    // decision, not a refusal to send.
    assert!(steps[0].get("request").is_some(), "{trace_file}");
    assert_eq!(
        steps[0]
            .get("response")
            .and_then(|response| response.get("statusCode")),
        Some(&json!(200)),
        "{trace_file}"
    );
    let criteria = match steps[0].get("criteria").and_then(Value::as_array) {
        Some(criteria) => criteria,
        None => panic!("expected trace criteria: {trace_file}"),
    };
    assert_eq!(criteria.len(), 1, "{trace_file}");
    assert_eq!(
        criteria[0].get("result"),
        Some(&json!(false)),
        "{trace_file}"
    );
    assert_eq!(
        criteria[0].get("type"),
        Some(&json!("jsonpath")),
        "{trace_file}"
    );
    let criterion_warnings = string_array(&criteria[0], "warnings");
    assert_eq!(criterion_warnings.len(), 1, "{criterion_warnings:?}");
    assert!(
        criterion_warnings[0].contains(SYNTAX_ERROR),
        "{criterion_warnings:?}"
    );
    assert!(
        !criterion_warnings[0].contains("draft-goessner"),
        "an unsupported dialect is not an excuse to execute it: {criterion_warnings:?}"
    );
}
