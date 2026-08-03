#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(delta) => delta.as_nanos(),
            Err(_) => 0,
        };
        let mut path = std::env::temp_dir();
        path.push(format!("{}-{}-{}", prefix, std::process::id(), nanos));
        if let Err(err) = fs::create_dir_all(&path) {
            panic!("creating temp dir {}: {err}", path.display());
        }
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

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => {
            let mut path = repo_root();
            path.push("target/debug/arazzo-cli");
            if path.exists() {
                path
            } else {
                panic!(
                    "CLI binary path not found at {}; CARGO_BIN_EXE_arazzo-cli missing",
                    path.display()
                );
            }
        }
    }
}

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../..");
    match fs::canonicalize(&path) {
        Ok(v) => v,
        Err(err) => panic!("canonicalizing repo root {}: {err}", path.display()),
    }
}

fn fixture_spec() -> PathBuf {
    let mut path = repo_root();
    path.push("examples/httpbin-get.arazzo.yaml");
    path
}

fn run(args: &[&str], current_dir: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(cli_bin());
    cmd.args(args);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    match cmd.output() {
        Ok(output) => output,
        Err(err) => panic!("running command {args:?}: {err}"),
    }
}

fn stdout_json(output: &std::process::Output) -> Value {
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(value) => value,
        Err(err) => panic!(
            "parsing JSON stdout failed: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        ),
    }
}

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn combined_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_run_json_kind(body: &Value, expected: &str) {
    let kind = body.get("kind").and_then(Value::as_str).unwrap_or_default();
    assert_eq!(kind, expected, "unexpected run JSON envelope: {body}");
}

fn run_json_requests(body: &Value) -> &Vec<Value> {
    assert_run_json_kind(body, "dryRun");
    match body.get("requests").and_then(Value::as_array) {
        Some(v) => v,
        None => panic!("expected dryRun.requests array, got: {body}"),
    }
}

fn run_json_warnings(body: &Value) -> Vec<String> {
    let Some(items) = body.get("warnings").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn assert_replay_json_kind(body: &Value, expected: &str) {
    let kind = body.get("kind").and_then(Value::as_str).unwrap_or_default();
    assert_eq!(kind, expected, "unexpected replay JSON envelope: {body}");
}

fn expression_warning_spec() -> &'static str {
    r#"
arazzo: 1.0.0
info:
  title: Expression Diagnostics
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: /items
        outputs:
          bad: $steps.missing.outputs.value
"#
}

fn asyncapi_source_spec() -> &'static str {
    r#"
arazzo: 1.1.0
info:
  title: AsyncAPI Source
  version: 1.0.0
sourceDescriptions:
  - name: events
    url: https://example.com/asyncapi.yaml
    type: asyncapi
workflows:
  - workflowId: inspect-source
    summary: Inspect source metadata
    steps:
      - stepId: inspect
        operationPath: /inspect
"#
}

fn asyncapi_channel_spec() -> &'static str {
    r#"
arazzo: 1.1.0
info:
  title: AsyncAPI Channel
  version: 1.0.0
sourceDescriptions:
  - name: events
    url: https://example.com/asyncapi.yaml
    type: asyncapi
workflows:
  - workflowId: inspect-async
    steps:
      - stepId: prepare
        operationPath: /prepare
      - stepId: receive-event
        channelPath: '{$sourceDescriptions.events.url}#/channels/events'
        action: receive
        timeout: 5000
        correlationId: $inputs.eventId
        dependsOn:
          - prepare
  - workflowId: run-async
    steps:
      - stepId: receive-event
        channelPath: '{$sourceDescriptions.events.url}#/channels/events'
        action: receive
"#
}

fn read_json_file(path: &Path) -> Value {
    let raw = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(err) => panic!("reading JSON file {}: {err}", path.display()),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(err) => panic!("parsing JSON file {}: {err}", path.display()),
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Err(err) = fs::write(path, contents) {
        panic!("writing {}: {err}", path.display());
    }
}

// ── Hermetic httpbin-like server ────────────────────────────────────
//
// The run --step tests execute the CLI subprocess against a local
// tiny_http server instead of the live httpbin.org, so the suite passes
// with no outbound network access.

struct MockHttpbin {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for MockHttpbin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Starts a local server emulating the httpbin endpoints the example specs
/// use: GET /get echoes origin/url, POST /post echoes the JSON body under
/// "json" the way httpbin.org does.
fn start_mock_httpbin() -> MockHttpbin {
    let server = match tiny_http::Server::http("127.0.0.1:0") {
        Ok(server) => server,
        Err(err) => panic!("binding mock httpbin server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let responses_base = base_url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(mut request)) => {
                    let method = request.method().as_str().to_string();
                    let path = request.url().split('?').next().unwrap_or("").to_string();
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);

                    let payload = match (method.as_str(), path.as_str()) {
                        ("GET", "/get") => json!({
                            "origin": "127.0.0.1",
                            "url": format!("{responses_base}/get"),
                        }),
                        ("POST", "/post") | ("PUT", "/put") => {
                            let parsed =
                                serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
                            json!({
                                "origin": "127.0.0.1",
                                "url": format!("{responses_base}{path}"),
                                "json": parsed,
                            })
                        }
                        _ => json!({"error": format!("unhandled {method} {path}")}),
                    };
                    let status = if payload.get("error").is_some() {
                        404
                    } else {
                        200
                    };

                    let header = match tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json"[..],
                    ) {
                        Ok(header) => header,
                        Err(()) => panic!("building content-type header"),
                    };
                    let response = tiny_http::Response::from_string(payload.to_string())
                        .with_status_code(tiny_http::StatusCode(status))
                        .with_header(header);
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    MockHttpbin {
        base_url,
        stop,
        handle: Some(handle),
    }
}

/// Copies an example spec into `dir` with its live httpbin.org source URL
/// rewritten to the local mock server. The example file itself is unchanged.
fn hermetic_example_spec(example: &str, base_url: &str, dir: &Path) -> PathBuf {
    let mut source = repo_root();
    source.push("examples");
    source.push(example);
    let raw = match fs::read_to_string(&source) {
        Ok(value) => value,
        Err(err) => panic!("reading example {}: {err}", source.display()),
    };
    assert!(
        raw.contains("https://httpbin.org"),
        "example {example} should target https://httpbin.org"
    );
    let rewritten = raw.replace("https://httpbin.org", base_url);
    let dest = dir.join(example);
    write_file(&dest, &rewritten);
    dest
}

#[test]
fn validate_json_reports_valid_metadata() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["--json", "validate", &spec_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("title"),
        Some(&Value::String("HTTPBin Demo".to_string()))
    );
    assert_eq!(body.get("workflows"), Some(&Value::Number(3.into())));
}

#[test]
fn validate_json_accepts_arazzo_1_1_asyncapi_source() {
    let temp = TempDir::new("arazzo-asyncapi-validate");
    let spec_path = temp.path().join("asyncapi.arazzo.yaml");
    write_file(&spec_path, asyncapi_source_spec());

    let output = run(
        ["--json", "validate", &spec_path.to_string_lossy()].as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("version"),
        Some(&Value::String("1.1.0".to_string()))
    );
    assert_eq!(body.get("sources"), Some(&Value::Number(1.into())));
}

#[test]
fn validate_json_accepts_supported_async_step_metadata() {
    let temp = TempDir::new("arazzo-async-step-validate");
    let spec_path = temp.path().join("async-step.arazzo.yaml");
    write_file(&spec_path, asyncapi_channel_spec());

    let output = run(
        ["--json", "validate", &spec_path.to_string_lossy()].as_slice(),
        None,
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("version"),
        Some(&Value::String("1.1.0".to_string()))
    );
}

#[test]
fn validate_json_reports_specific_async_and_dependency_diagnostics() {
    let temp = TempDir::new("arazzo-async-step-invalid");
    let spec_path = temp.path().join("invalid-async-step.arazzo.yaml");
    write_file(
        &spec_path,
        r#"
arazzo: 1.1.0
info:
  title: Invalid AsyncAPI Channel
  version: 1.0.0
sourceDescriptions:
  - name: events
    url: https://example.com/asyncapi.yaml
    type: asyncapi
workflows:
  - workflowId: invalid-async
    steps:
      - stepId: publish-event
        channelPath: '{$sourceDescriptions.events.url}#/channels/events'
        action: send
        correlationId: event-1
        dependsOn:
          - $workflows.other.steps.done
"#,
    );

    let output = run(
        ["--json", "validate", &spec_path.to_string_lossy()].as_slice(),
        None,
    );
    assert!(!output.status.success());
    let body = stdout_json(&output);
    let kinds = body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("validation output should contain errors: {body}"))
        .iter()
        .filter_map(|error| error.get("kind").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"invalidAsyncStep"), "diagnostics: {body}");
    assert!(
        kinds.contains(&"unsupportedDependencyScope"),
        "diagnostics: {body}"
    );
}

#[test]
fn catalog_json_reports_asyncapi_source_type() {
    let temp = TempDir::new("arazzo-asyncapi-catalog");
    let spec_path = temp.path().join("asyncapi.arazzo.yaml");
    write_file(&spec_path, asyncapi_source_spec());

    let output = run(
        ["--json", "catalog", &temp.path().to_string_lossy()].as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(
        body.pointer("/0/sources/0/type"),
        Some(&Value::String("asyncapi".to_string()))
    );
}

#[test]
fn show_json_reports_asyncapi_source_type() {
    let temp = TempDir::new("arazzo-asyncapi-show");
    let spec_path = temp.path().join("asyncapi.arazzo.yaml");
    write_file(&spec_path, asyncapi_source_spec());

    let output = run(
        [
            "--json",
            "show",
            "inspect-source",
            "--dir",
            &temp.path().to_string_lossy(),
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(
        body.pointer("/sources/0/type"),
        Some(&Value::String("asyncapi".to_string()))
    );
}

#[test]
fn list_json_does_not_add_source_descriptions() {
    let temp = TempDir::new("arazzo-asyncapi-list");
    let spec_path = temp.path().join("asyncapi.arazzo.yaml");
    write_file(&spec_path, asyncapi_source_spec());

    let output = run(
        ["--json", "list", &spec_path.to_string_lossy()].as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let workflow = body
        .get(0)
        .unwrap_or_else(|| panic!("list output should contain one workflow: {body}"));
    assert_eq!(
        workflow.get("id"),
        Some(&Value::String("inspect-source".to_string()))
    );
    assert!(workflow.get("sources").is_none());
    assert!(workflow.get("sourceDescriptions").is_none());
}

#[test]
fn list_json_contains_expected_workflows() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["--json", "list", &spec_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected list output array, got: {body}"),
    };
    assert_eq!(rows.len(), 3);

    let mut ids = Vec::<String>::new();
    for row in rows {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        ids.push(id.to_string());
    }
    ids.sort();

    assert_eq!(
        ids,
        vec![
            "echo-headers".to_string(),
            "get-origin".to_string(),
            "status-check".to_string()
        ]
    );
}

#[test]
fn catalog_and_show_json_work_with_temp_directory() {
    let temp = TempDir::new("arazzo-catalog-test");
    let source = fixture_spec();
    let mut one = temp.path().to_path_buf();
    one.push("one.yaml");
    let mut two = temp.path().to_path_buf();
    two.push("two.yaml");

    if let Err(err) = fs::copy(&source, &one) {
        panic!("copying fixture to {}: {err}", one.display());
    }
    let second_content = match fs::read_to_string(&source) {
        Ok(v) => v,
        Err(err) => panic!("reading fixture {}: {err}", source.display()),
    };
    let second_content =
        second_content.replace("workflowId: status-check", "workflowId: status-check-two");
    if let Err(err) = fs::write(&two, second_content) {
        panic!("writing adjusted fixture {}: {err}", two.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let catalog = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(catalog.status.success());
    let catalog_body = stdout_json(&catalog);
    let rows = match catalog_body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {catalog_body}"),
    };
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("file"),
        Some(&Value::String("one.yaml".to_string()))
    );
    assert_eq!(
        rows[1].get("file"),
        Some(&Value::String("two.yaml".to_string()))
    );

    let show = run(
        ["--json", "show", "status-check", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(show.status.success());
    let show_body = stdout_json(&show);
    assert_eq!(
        show_body.get("id"),
        Some(&Value::String("status-check".to_string()))
    );
    assert_eq!(show_body.get("step_count"), Some(&Value::Number(2.into())));
    let steps_arr = show_body.get("steps").and_then(|v| v.as_array());
    assert_eq!(steps_arr.map(|a| a.len()), Some(2));
}

#[test]
fn catalog_and_show_support_yml_extension() {
    let temp = TempDir::new("arazzo-catalog-yml");
    let source = fixture_spec();
    let mut yml_path = temp.path().to_path_buf();
    yml_path.push("only.yml");

    if let Err(err) = fs::copy(&source, &yml_path) {
        panic!("copying fixture to {}: {err}", yml_path.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let catalog = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(catalog.status.success());
    let catalog_body = stdout_json(&catalog);
    let rows = match catalog_body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {catalog_body}"),
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("file"),
        Some(&Value::String("only.yml".to_string()))
    );

    let show = run(
        ["--json", "show", "status-check", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(show.status.success());
}

#[test]
fn show_errors_on_duplicate_workflow_id() {
    let temp = TempDir::new("arazzo-show-dup-test");
    let source = fixture_spec();
    let mut one = temp.path().to_path_buf();
    one.push("a.yaml");
    let mut two = temp.path().to_path_buf();
    two.push("b.yaml");

    if let Err(err) = fs::copy(&source, &one) {
        panic!("copying fixture to {}: {err}", one.display());
    }
    if let Err(err) = fs::copy(&source, &two) {
        panic!("copying fixture to {}: {err}", two.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let show = run(
        ["show", "status-check", "--dir", &dir_str].as_slice(),
        Some(temp.path()),
    );
    assert!(!show.status.success());
    let stderr = String::from_utf8_lossy(&show.stderr);
    assert!(stderr.contains("found in multiple files"));
}

#[test]
fn run_dry_run_json_returns_request_plan() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert_eq!(rows.len(), 2);

    let first = &rows[0];
    assert_eq!(
        first.get("stepId"),
        Some(&Value::String("check-status".to_string()))
    );
    assert_eq!(first.get("method"), Some(&Value::String("GET".to_string())));
    let first_url = first.get("url").and_then(Value::as_str).unwrap_or_default();
    assert!(first_url.contains("/status/429"));

    let second = &rows[1];
    assert_eq!(
        second.get("stepId"),
        Some(&Value::String("handle-error".to_string()))
    );
    assert_eq!(
        second.get("method"),
        Some(&Value::String("GET".to_string()))
    );
    let second_url = second
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(second_url.contains("/get"));
}

#[test]
fn run_expr_diagnostics_warn_includes_warnings_in_json() {
    let temp = TempDir::new("arazzo-expr-diag-warn");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("diag.yaml");
    write_file(&spec_path, expression_warning_spec());

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--expr-diagnostics",
            "warn",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let requests = run_json_requests(&body);
    assert_eq!(requests.len(), 1);

    let warnings = run_json_warnings(&body);
    assert!(!warnings.is_empty(), "expected warnings in warn mode");
    assert!(warnings[0].contains("step \"step-one\""));
    assert!(warnings[0].contains("output \"bad\""));
}

#[test]
fn run_expr_diagnostics_error_fails_with_code() {
    let temp = TempDir::new("arazzo-expr-diag-error");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("diag.yaml");
    write_file(&spec_path, expression_warning_spec());

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--expr-diagnostics",
            "error",
        ]
        .as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "error diagnostics mode should fail on expression warnings"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUNTIME_EXPRESSION_DIAGNOSTICS")
    );
    let warnings = run_json_warnings(&body);
    assert!(!warnings.is_empty(), "error mode should include warnings");
}

#[test]
fn run_rejects_invalid_input_format() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "bad",
        ]
        .as_slice(),
        None,
    );
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid input format"));
}

#[test]
fn validate_json_reports_invalid_spec_errors() {
    let temp = TempDir::new("arazzo-validate-invalid");
    let mut invalid = temp.path().to_path_buf();
    invalid.push("bad.yaml");
    let content = r#"
arazzo: 1.0.0
info:
  version: 1.0.0
sourceDescriptions:
  - name: s1
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps: []
"#;
    if let Err(err) = fs::write(&invalid, content) {
        panic!("writing invalid spec {}: {err}", invalid.display());
    }

    let invalid_str = invalid.to_string_lossy().to_string();
    let output = run(["--json", "validate", &invalid_str].as_slice(), None);
    assert!(!output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(false)));
    let errors = body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    assert!(!errors.is_empty());
    let first = errors[0].as_object().unwrap_or_else(|| {
        panic!(
            "expected structured validate error object, got: {}",
            errors[0]
        )
    });
    assert_eq!(
        first.get("source"),
        Some(&Value::String("validation".to_string()))
    );
    assert!(first.get("kind").and_then(Value::as_str).is_some());
    assert!(first.get("message").and_then(Value::as_str).is_some());
}

#[test]
fn validate_json_reports_unknown_operation_path_source_reference() {
    let temp = TempDir::new("arazzo-validate-unknown-source");
    let mut invalid = temp.path().to_path_buf();
    invalid.push("unknown-source.yaml");
    let content = r#"
arazzo: 1.0.0
info:
  title: Unknown Source Validate
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: "{missing}./items"
"#;
    write_file(&invalid, content);

    let invalid_str = invalid.to_string_lossy().to_string();
    let output = run(["--json", "validate", &invalid_str].as_slice(), None);
    assert!(!output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(false)));
    let errors = body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    assert!(!errors.is_empty());
    let issue = errors
        .iter()
        .find(|item| {
            item.get("kind").and_then(Value::as_str) == Some("invalidReference")
                && item
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("operationPath")
        })
        .unwrap_or_else(|| panic!("expected invalidReference on operationPath, got: {errors:?}"));
    assert!(issue
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .contains("unknown sourceDescription"));
}

#[test]
fn show_not_found_returns_non_zero_exit() {
    let temp = TempDir::new("arazzo-show-not-found");
    let source = fixture_spec();
    let mut file = temp.path().to_path_buf();
    file.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &file) {
        panic!("copying fixture to {}: {err}", file.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(
        ["show", "missing-workflow", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
}

#[test]
fn run_json_reports_error_for_missing_workflow() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        ["--json", "run", &spec_str, "missing-workflow", "--dry-run"].as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert!(body.get("error").is_some());
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUNTIME_WORKFLOW_NOT_FOUND")
    );
}

#[test]
fn run_json_reports_error_code_for_missing_spec_file() {
    let output = run(
        [
            "--json",
            "run",
            "/definitely/missing/spec.yaml",
            "any-workflow",
            "--dry-run",
        ]
        .as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUN_SPEC_READ_FILE")
    );
}

#[test]
fn run_json_reports_error_code_for_missing_openapi_file() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--openapi",
            "/definitely/missing/openapi.yaml",
        ]
        .as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUN_OPENAPI_READ_FILE")
    );
}

#[test]
fn run_json_reports_validation_error_for_unknown_source_description_reference() {
    let temp = TempDir::new("arazzo-run-unknown-source");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("unknown-source.yaml");
    let spec = r#"
arazzo: 1.0.0
info:
  title: Unknown Source Test
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: "{missing}./items"
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        ["--json", "run", &spec_str, "wf", "--dry-run"].as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUN_SPEC_VALIDATION")
    );
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(error.contains("unknown sourceDescription"));
}

#[test]
fn catalog_json_skips_invalid_yaml_files() {
    let temp = TempDir::new("arazzo-catalog-skip-invalid");
    let source = fixture_spec();
    let mut valid = temp.path().to_path_buf();
    valid.push("valid.yaml");
    if let Err(err) = fs::copy(&source, &valid) {
        panic!("copying fixture to {}: {err}", valid.display());
    }

    let mut invalid = temp.path().to_path_buf();
    invalid.push("invalid.yaml");
    if let Err(err) = fs::write(&invalid, "not: [valid") {
        panic!("writing invalid yaml {}: {err}", invalid.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {body}"),
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("file"),
        Some(&Value::String("valid.yaml".to_string()))
    );
}

#[test]
fn run_dry_run_expands_env_input_values() {
    std::env::set_var("ARAZZO_TEST_CODE", "204");
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=$ARAZZO_TEST_CODE",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert!(!rows.is_empty());
    let first = &rows[0];
    let url = first.get("url").and_then(Value::as_str).unwrap_or_default();
    assert!(url.contains("/status/204"));
}

#[test]
fn run_dry_run_input_json_preserves_integer_type() {
    let temp = TempDir::new("arazzo-input-json");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input-json",
            "code=429",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert!(!rows.is_empty());
    let url = rows[0]
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        url.ends_with("/status/429"),
        "unexpected request url: {url}"
    );

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/inputs/code"),
        Some(&Value::Number(429.into()))
    );
}

#[test]
fn run_trace_writes_file_and_preserves_json_stdout() {
    let temp = TempDir::new("arazzo-trace-success");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert_eq!(rows.len(), 2);

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.get("schemaVersion"),
        Some(&Value::String("trace.v1".to_string()))
    );
    assert_eq!(
        trace.pointer("/run/status"),
        Some(&Value::String("success".to_string()))
    );
    assert_eq!(
        trace.pointer("/run/workflowId"),
        Some(&Value::String("status-check".to_string()))
    );
    let trace_steps = trace
        .get("steps")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    assert!(!trace_steps.is_empty());
}

#[test]
fn run_trace_honors_execution_timeout_flag() {
    let temp = TempDir::new("arazzo-trace-timeout");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
            "--execution-timeout",
            "2s",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/run/timeoutMs"),
        Some(&Value::Number(2000.into()))
    );
}

#[test]
fn run_trace_writes_on_failure() {
    let temp = TempDir::new("arazzo-trace-failure");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "missing-workflow",
            "--dry-run",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    assert!(body.get("error").is_some());

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/run/status"),
        Some(&Value::String("failure".to_string()))
    );
    let run_error = trace
        .pointer("/run/error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(run_error.contains("missing-workflow"));
}

#[test]
fn run_trace_redacts_sensitive_headers_and_query_values() {
    let temp = TempDir::new("arazzo-trace-redact-headers");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("trace.yaml");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Trace Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: /items
        parameters:
          - name: token
            in: query
            value: $inputs.token
          - name: page
            in: query
            value: "1"
          - name: Authorization
            in: header
            value: $inputs.auth
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--input",
            "token=super-secret",
            "--input",
            "auth=Bearer top-secret",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);
    let auth = trace
        .pointer("/steps/0/request/headers/Authorization")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(auth, "[REDACTED]");

    let url = trace
        .pointer("/steps/0/request/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(url.contains("token=%5BREDACTED%5D") || url.contains("token=[REDACTED]"));
    assert!(url.contains("page=1"));
}

#[test]
fn run_trace_redacts_sensitive_json_fields() {
    let temp = TempDir::new("arazzo-trace-redact-json");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("trace.yaml");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Trace JSON Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: /submit
        requestBody:
          contentType: application/json
          payload:
            username: alice
            password: $inputs.password
            nested:
              client_secret: abc123
              token: $inputs.token
              safe: ok
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--input",
            "password=p4ss",
            "--input",
            "token=tok123",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/inputs/password"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        trace.pointer("/inputs/token"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/password"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/nested/client_secret"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/nested/token"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/nested/safe"),
        Some(&Value::String("ok".to_string()))
    );
}

#[test]
fn run_trace_write_failure_returns_error() {
    let temp = TempDir::new("arazzo-trace-write-fail");
    let trace_dir = temp.path().to_string_lossy().to_string();

    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        [
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=200",
            "--trace",
            &trace_dir,
        ]
        .as_slice(),
        None,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("writing trace"));
}

#[test]
fn replay_json_reexecutes_trace_successfully() {
    let temp = TempDir::new("arazzo-replay-success");
    let source = fixture_spec();
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &spec_path) {
        panic!("copying fixture to {}: {err}", spec_path.display());
    }
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let trace_output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(trace_output.status.success());

    let replay_output = run(["--json", "replay", &trace_str].as_slice(), None);
    assert!(replay_output.status.success());
    let body = stdout_json(&replay_output);
    assert_replay_json_kind(&body, "success");
    assert_eq!(
        body.get("requestsChecked"),
        Some(&Value::Number(2.into())),
        "expected replay to validate two HTTP requests"
    );
}

#[test]
fn replay_json_reports_request_drift_error() {
    let temp = TempDir::new("arazzo-replay-drift");
    let source = fixture_spec();
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &spec_path) {
        panic!("copying fixture to {}: {err}", spec_path.display());
    }
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let trace_output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(trace_output.status.success());

    let spec_raw = match fs::read_to_string(&spec_path) {
        Ok(raw) => raw,
        Err(err) => panic!("reading {}: {err}", spec_path.display()),
    };
    let drifted = spec_raw.replace("/status/{code}", "/status/200");
    if let Err(err) = fs::write(&spec_path, drifted) {
        panic!("writing drifted spec {}: {err}", spec_path.display());
    }

    let replay_output = run(["--json", "replay", &trace_str].as_slice(), None);
    assert!(!replay_output.status.success());
    let body = stdout_json(&replay_output);
    assert_replay_json_kind(&body, "error");
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some("RUNTIME_REPLAY_REQUEST_MISMATCH")
    );
}

// ---------------------------------------------------------------------------
// --parallel flag tests
// ---------------------------------------------------------------------------

fn parallel_spec_content() -> &'static str {
    r#"
arazzo: 1.0.0
info:
  title: Parallel Test
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: independent
    steps:
      - stepId: a
        operationPath: /get
        parameters:
          - name: label
            in: query
            value: "alpha"
        successCriteria:
          - condition: $statusCode == 200
      - stepId: b
        operationPath: /get
        parameters:
          - name: label
            in: query
            value: "bravo"
        successCriteria:
          - condition: $statusCode == 200
  - workflowId: dependent
    steps:
      - stepId: first
        operationPath: /get
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          val: $response.body.origin
      - stepId: second
        operationPath: /get
        parameters:
          - name: echo
            in: query
            value: $steps.first.outputs.val
        successCriteria:
          - condition: $statusCode == 200
"#
}

#[test]
fn run_parallel_dry_run_json_returns_all_steps() {
    let temp = TempDir::new("arazzo-parallel-dry-run");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("parallel.yaml");
    write_file(&spec_path, parallel_spec_content());

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "independent",
            "--dry-run",
            "--parallel",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert_eq!(rows.len(), 2);

    let ids: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("stepId").and_then(Value::as_str))
        .collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"b"));
}

#[test]
fn run_dry_run_operation_id_resolves_with_openapi_flag() {
    let temp = TempDir::new("arazzo-opid-openapi");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("opid.arazzo.yaml");
    let mut openapi_path = temp.path().to_path_buf();
    openapi_path.push("api.yaml");

    write_file(
        &spec_path,
        r#"
arazzo: 1.0.0
info:
  title: OperationId CLI Test
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: opid
    steps:
      - stepId: list-widgets
        operationId: listWidgets
"#,
    );

    write_file(
        &openapi_path,
        r#"
openapi: 3.0.3
info:
  title: Test API
  version: 1.0.0
paths:
  /widgets:
    get:
      operationId: listWidgets
      responses:
        "200":
          description: ok
"#,
    );

    let spec_str = spec_path.to_string_lossy().to_string();
    let openapi_str = openapi_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "opid",
            "--dry-run",
            "--openapi",
            &openapi_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = run_json_requests(&body);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("stepId"),
        Some(&Value::String("list-widgets".to_string()))
    );
    assert_eq!(
        rows[0].get("method"),
        Some(&Value::String("GET".to_string()))
    );
    assert_eq!(
        rows[0].get("url"),
        Some(&Value::String("https://example.com/widgets".to_string()))
    );
}

#[test]
fn run_parallel_trace_records_parallel_flag() {
    let temp = TempDir::new("arazzo-parallel-trace");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("parallel.yaml");
    write_file(&spec_path, parallel_spec_content());
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "independent",
            "--dry-run",
            "--parallel",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/run/parallel"),
        Some(&Value::Bool(true)),
        "trace should record parallel: true"
    );
    assert_eq!(
        trace.pointer("/run/status"),
        Some(&Value::String("success".to_string()))
    );
}

#[test]
fn run_sequential_trace_records_parallel_false() {
    let temp = TempDir::new("arazzo-sequential-trace");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("parallel.yaml");
    write_file(&spec_path, parallel_spec_content());
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "dependent",
            "--dry-run",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);
    assert_eq!(
        trace.pointer("/run/parallel"),
        Some(&Value::Bool(false)),
        "trace should record parallel: false when flag is not set"
    );
}

// ---------------------------------------------------------------------------
// schema command tests
// ---------------------------------------------------------------------------

#[test]
fn version_flag_prints_crate_version() {
    let output = run(["--version"].as_slice(), None);
    assert!(output.status.success());
    let text = stdout_text(&output);
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "--version output should contain the crate version: {text}"
    );
}

#[test]
fn root_help_surfaces_agent_workflow() {
    let output = run(["--help"].as_slice(), None);
    assert!(output.status.success());
    let help = stdout_text(&output);

    assert!(help.contains("For agents: use --json"));
    assert!(help.contains("For agents:"));
    assert!(help.contains("arazzo-cli schema <command>"));
    assert!(help.contains("arazzo-cli run <spec> <workflow-id> --dry-run --json"));
    assert!(help.contains("Diagnostics go to stderr"));
}

#[test]
fn bare_cli_surfaces_agent_guidance() {
    let output = run([].as_slice(), None);
    let help = combined_text(&output);

    assert!(help.contains("For agents:"));
    assert!(help.contains("arazzo-cli schema <command>"));
    assert!(help.contains("arazzo-cli run <spec> <workflow-id> --dry-run --json"));
}

#[test]
fn run_help_surfaces_agent_safe_execution_notes() {
    let output = run(["run", "--help"].as_slice(), None);
    assert!(output.status.success());
    let help = stdout_text(&output);

    assert!(help.contains("Execute a workflow from an Arazzo spec"));
    assert!(help.contains("Path to an Arazzo YAML spec file"));
    assert!(help.contains("String input as key=value"));
    assert!(help.contains("Build and print the request plan without sending HTTP requests"));
    assert!(help.contains("For agents:"));
    assert!(help.contains("Use --dry-run --json to preview requests without sending HTTP traffic"));
    assert!(help.contains("Use schema run to inspect the JSON output contract"));
}

#[test]
fn schema_lists_available_commands() {
    let output = run(["schema"].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    let names = match body.as_array() {
        Some(v) => v,
        None => panic!("expected schema array, got: {body}"),
    };
    let strs: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
    assert!(strs.contains(&"validate"));
    assert!(strs.contains(&"list"));
    assert!(strs.contains(&"catalog"));
    assert!(strs.contains(&"show"));
    assert!(strs.contains(&"run"));
}

#[test]
fn schema_validate_returns_json_schema() {
    let output = run(["schema", "validate"].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert!(
        body.get("type").is_some() || body.get("$schema").is_some(),
        "expected JSON Schema document, got: {body}"
    );
}

#[test]
fn schema_run_returns_json_schema() {
    let output = run(["schema", "run"].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert!(
        body.get("type").is_some() || body.get("$schema").is_some() || body.get("anyOf").is_some(),
        "expected JSON Schema document, got: {body}"
    );
}

#[test]
fn schema_unknown_command_fails() {
    let output = run(["schema", "nonexistent"].as_slice(), None);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"));
}

// ---------------------------------------------------------------------------
// validate --json edge cases
// ---------------------------------------------------------------------------

#[test]
fn validate_json_reports_error_for_unparseable_yaml() {
    let temp = TempDir::new("arazzo-validate-bad-yaml");
    let mut bad = temp.path().to_path_buf();
    bad.push("broken.yaml");
    write_file(&bad, "not: [valid yaml {{{{");

    let bad_str = bad.to_string_lossy().to_string();
    let output = run(["--json", "validate", &bad_str].as_slice(), None);
    // validate exits non-zero and reports valid: false in JSON mode
    assert!(!output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(false)));
    let errors = body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    assert!(!errors.is_empty());
    let first = errors[0].as_object().unwrap_or_else(|| {
        panic!(
            "expected structured validate error object, got: {}",
            errors[0]
        )
    });
    assert_eq!(
        first.get("source"),
        Some(&Value::String("parseYaml".to_string()))
    );
    assert!(first.get("kind").is_none() || first.get("kind") == Some(&Value::Null));
    assert!(first.get("message").and_then(Value::as_str).is_some());
}

// ---------------------------------------------------------------------------
// show --json input detail tests
// ---------------------------------------------------------------------------

#[test]
fn show_json_includes_input_details() {
    let temp = TempDir::new("arazzo-show-inputs");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("with-inputs.yaml");
    let spec = r#"
arazzo: 1.0.0
info:
  title: Input Detail Test
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: with-inputs
    inputs:
      type: object
      properties:
        name:
          type: string
        count:
          type: integer
      required:
        - name
    steps:
      - stepId: step-one
        operationPath: /items
        successCriteria:
          - condition: $statusCode == 200
    outputs:
      result: $steps.step-one.outputs.val
"#;
    write_file(&spec_path, spec);

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(
        ["--json", "show", "with-inputs", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);

    assert_eq!(
        body.get("id"),
        Some(&Value::String("with-inputs".to_string()))
    );

    let inputs = match body.get("inputs") {
        Some(v) => v,
        None => panic!("expected inputs key in show output: {body}"),
    };
    let name_input = match inputs.get("name") {
        Some(v) => v,
        None => panic!("expected 'name' in inputs: {inputs}"),
    };
    assert_eq!(name_input.get("required"), Some(&Value::Bool(true)));
    assert_eq!(
        name_input.get("type"),
        Some(&Value::String("string".to_string()))
    );

    let count_input = match inputs.get("count") {
        Some(v) => v,
        None => panic!("expected 'count' in inputs: {inputs}"),
    };
    assert_eq!(count_input.get("required"), Some(&Value::Bool(false)));

    let outputs = match body.get("outputs").and_then(Value::as_array) {
        Some(v) => v,
        None => panic!("expected outputs array in show output: {body}"),
    };
    assert!(outputs.contains(&Value::String("result".to_string())));
}

// ---------------------------------------------------------------------------
// list --json output shape tests
// ---------------------------------------------------------------------------

#[test]
fn list_json_workflow_entry_has_expected_fields() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["--json", "list", &spec_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected list output array, got: {body}"),
    };
    assert!(!rows.is_empty());

    let first = &rows[0];
    assert!(first.get("id").is_some(), "workflow entry should have 'id'");
    assert!(
        first.get("summary").is_some() || first.get("summary").is_none(),
        "summary may be present or absent"
    );
    assert!(
        first.get("inputs").is_some(),
        "workflow entry should have 'inputs' array"
    );
    assert!(
        first.get("outputs").is_some(),
        "workflow entry should have 'outputs' array"
    );
}

// ---------------------------------------------------------------------------
// catalog --json output shape tests
// ---------------------------------------------------------------------------

#[test]
fn catalog_json_entries_have_expected_fields() {
    let temp = TempDir::new("arazzo-catalog-fields");
    let source = fixture_spec();
    let mut dest = temp.path().to_path_buf();
    dest.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &dest) {
        panic!("copying fixture: {err}");
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {body}"),
    };
    assert_eq!(rows.len(), 1);

    let entry = &rows[0];
    assert!(
        entry.get("file").is_some(),
        "catalog entry should have 'file'"
    );
    assert!(
        entry.get("title").is_some(),
        "catalog entry should have 'title'"
    );
    assert!(
        entry.get("version").is_some(),
        "catalog entry should have 'version'"
    );
    assert!(
        entry.get("sources").and_then(Value::as_array).is_some(),
        "catalog entry should have 'sources' array"
    );
    assert!(
        entry.get("workflows").and_then(Value::as_array).is_some(),
        "catalog entry should have 'workflows' array"
    );
}

// ---------------------------------------------------------------------------
// steps --json tests
// ---------------------------------------------------------------------------

#[test]
fn steps_json_returns_step_array_with_expected_fields() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        ["--json", "steps", &spec_str, "get-origin"].as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    let steps = match body.as_array() {
        Some(v) => v,
        None => panic!("expected steps array, got: {body}"),
    };
    assert_eq!(steps.len(), 1);

    let step = &steps[0];
    assert_eq!(
        step.get("stepId"),
        Some(&Value::String("fetch-ip".to_string()))
    );
    assert_eq!(step.get("method"), Some(&Value::String("GET".to_string())));
    assert_eq!(step.get("url"), Some(&Value::String("/get".to_string())));
    assert_eq!(step.get("position"), Some(&Value::Number(0.into())));
    for field in [
        "channelPath",
        "action",
        "timeout",
        "correlationId",
        "dependsOn",
    ] {
        assert!(
            step.get(field).is_none(),
            "existing HTTP step output should omit optional async field {field}"
        );
    }
}

#[test]
fn steps_and_show_json_expose_async_metadata_without_http_defaults() {
    let temp = TempDir::new("arazzo-async-step-output");
    let spec_path = temp.path().join("async-step.arazzo.yaml");
    write_file(&spec_path, asyncapi_channel_spec());
    let spec_path_str = spec_path.to_string_lossy().to_string();

    let steps_output = run(
        ["--json", "steps", &spec_path_str, "inspect-async"].as_slice(),
        None,
    );
    assert!(steps_output.status.success());
    let steps = stdout_json(&steps_output);
    let channel = &steps
        .as_array()
        .unwrap_or_else(|| panic!("steps output should be an array: {steps}"))[1];
    assert_eq!(
        channel.get("channelPath").and_then(Value::as_str),
        Some("{$sourceDescriptions.events.url}#/channels/events")
    );
    assert_eq!(
        channel.get("action").and_then(Value::as_str),
        Some("receive")
    );
    assert_eq!(channel.get("timeout").and_then(Value::as_u64), Some(5000));
    assert_eq!(
        channel.get("correlationId").and_then(Value::as_str),
        Some("$inputs.eventId")
    );
    assert_eq!(
        channel.get("dependsOn"),
        Some(&serde_json::json!(["prepare"]))
    );
    assert!(channel.get("method").is_none());
    assert!(channel.get("url").is_none());

    let show_output = run(
        [
            "--json",
            "show",
            "inspect-async",
            "--dir",
            &temp.path().to_string_lossy(),
        ]
        .as_slice(),
        None,
    );
    assert!(show_output.status.success());
    let show = stdout_json(&show_output);
    let show_channel = show
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| steps.get(1))
        .unwrap_or_else(|| panic!("show output should include channel summary: {show}"));
    assert_eq!(show_channel.get("channelPath"), channel.get("channelPath"));
    assert_eq!(show_channel.get("action"), channel.get("action"));
    assert_eq!(show_channel.get("timeout"), channel.get("timeout"));
    assert_eq!(
        show_channel.get("correlationId"),
        channel.get("correlationId")
    );
    assert_eq!(show_channel.get("dependsOn"), channel.get("dependsOn"));
}

#[test]
fn run_json_channel_normal_and_dry_run_share_fail_closed_error() {
    let temp = TempDir::new("arazzo-async-step-run");
    let spec_path = temp.path().join("async-step.arazzo.yaml");
    write_file(&spec_path, asyncapi_channel_spec());
    let spec_path_str = spec_path.to_string_lossy().to_string();

    let normal = run(
        ["--json", "run", &spec_path_str, "run-async"].as_slice(),
        None,
    );
    let dry_run = run(
        ["--json", "run", &spec_path_str, "run-async", "--dry-run"].as_slice(),
        None,
    );
    assert!(!normal.status.success());
    assert!(!dry_run.status.success());

    let normal_body = stdout_json(&normal);
    let dry_run_body = stdout_json(&dry_run);
    for body in [&normal_body, &dry_run_body] {
        assert_run_json_kind(body, "error");
        assert_eq!(
            body.get("code").and_then(Value::as_str),
            Some("RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT")
        );
    }
    assert_eq!(normal_body.get("error"), dry_run_body.get("error"));
}

#[test]
fn steps_json_chained_workflow_returns_multiple_steps() {
    let mut spec = repo_root();
    spec.push("examples/httpbin-chained-posts.arazzo.yaml");
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        ["--json", "steps", &spec_str, "post-chain"].as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    let steps = match body.as_array() {
        Some(v) => v,
        None => panic!("expected steps array, got: {body}"),
    };
    assert_eq!(steps.len(), 3);

    let ids: Vec<&str> = steps
        .iter()
        .filter_map(|s| s.get("stepId").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, vec!["post-initial", "post-enriched", "post-final"]);

    // All steps should have POST method (operationPath starts with "POST ")
    for step in steps {
        assert_eq!(
            step.get("method"),
            Some(&Value::String("POST".to_string())),
            "expected POST for step {:?}",
            step.get("stepId")
        );
    }

    // Positions should be sequential
    let positions: Vec<u64> = steps
        .iter()
        .filter_map(|s| s.get("position").and_then(Value::as_u64))
        .collect();
    assert_eq!(positions, vec![0, 1, 2]);
}

#[test]
fn steps_json_missing_workflow_fails() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        ["--json", "steps", &spec_str, "nonexistent"].as_slice(),
        None,
    );
    assert!(!output.status.success());
}

#[test]
fn steps_human_output_includes_header() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["steps", &spec_str, "get-origin"].as_slice(), None);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("get-origin"), "should mention workflow ID");
    assert!(stdout.contains("fetch-ip"), "should mention step ID");
}

#[test]
fn steps_schema_is_available() {
    let output = run(["schema", "steps"].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_eq!(
        body.get("title"),
        Some(&Value::String("Array_of_StepInfo".to_string()))
    );

    let schema = body.to_string();
    for field in [
        "channelPath",
        "action",
        "timeout",
        "correlationId",
        "dependsOn",
    ] {
        assert!(
            schema.contains(field),
            "steps schema should expose additive field {field}: {body}"
        );
    }

    let show_output = run(["schema", "show"].as_slice(), None);
    assert!(show_output.status.success());
    let show_schema = stdout_json(&show_output).to_string();
    for field in [
        "channelPath",
        "action",
        "timeout",
        "correlationId",
        "dependsOn",
    ] {
        assert!(
            show_schema.contains(field),
            "show schema should expose additive step field {field}: {show_schema}"
        );
    }
}

// ---------------------------------------------------------------------------
// show --json step detail tests
// ---------------------------------------------------------------------------

#[test]
fn show_json_includes_step_details() {
    let temp = TempDir::new("arazzo-show-steps");
    let source = fixture_spec();
    let mut dest = temp.path().to_path_buf();
    dest.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &dest) {
        panic!("copying fixture: {err}");
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(
        ["--json", "show", "get-origin", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);

    // step_count should match steps array length
    let step_count = body.get("step_count").and_then(Value::as_u64).unwrap_or(0);
    let steps = match body.get("steps").and_then(Value::as_array) {
        Some(s) => s,
        None => panic!("show should include steps array"),
    };
    assert_eq!(step_count as usize, steps.len());

    // Verify step content
    let step = &steps[0];
    assert_eq!(
        step.get("stepId"),
        Some(&Value::String("fetch-ip".to_string()))
    );
    assert_eq!(step.get("method"), Some(&Value::String("GET".to_string())));
    assert_eq!(step.get("url"), Some(&Value::String("/get".to_string())));
}

// ---------------------------------------------------------------------------
// run --step tests
// ---------------------------------------------------------------------------

#[test]
fn run_step_json_single_step_no_deps() {
    let server = start_mock_httpbin();
    let temp = TempDir::new("arazzo-run-step-single");
    let spec = hermetic_example_spec("httpbin-get.arazzo.yaml", &server.base_url, temp.path());
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            "--step",
            "fetch-ip",
            &spec_str,
            "get-origin",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "success");
    let outputs = match body.get("outputs") {
        Some(o) => o,
        None => panic!("should have outputs field"),
    };
    assert_eq!(
        outputs.get("origin"),
        Some(&Value::String("127.0.0.1".to_string())),
        "origin output should come from the mock server"
    );
    assert_eq!(
        outputs.get("url"),
        Some(&Value::String(format!("{}/get", server.base_url))),
        "url output should come from the mock server"
    );
}

#[test]
fn run_step_json_with_dependency_resolution() {
    let server = start_mock_httpbin();
    let temp = TempDir::new("arazzo-run-step-deps");
    let spec = hermetic_example_spec(
        "httpbin-chained-posts.arazzo.yaml",
        &server.base_url,
        temp.path(),
    );
    let spec_str = spec.to_string_lossy().to_string();

    // post-enriched depends on post-initial — should auto-resolve
    let output = run(
        [
            "--json",
            "run",
            "--step",
            "post-enriched",
            &spec_str,
            "post-chain",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "success");
    let outputs = match body.get("outputs") {
        Some(o) => o,
        None => panic!("should have outputs field"),
    };
    assert_eq!(
        outputs.get("enriched_action"),
        Some(&Value::String("enrich".to_string())),
        "enriched_action should echo the payload posted by post-enriched"
    );
}

#[test]
fn run_step_no_deps_succeeds_for_standalone_step() {
    let server = start_mock_httpbin();
    let temp = TempDir::new("arazzo-run-step-standalone");
    let spec = hermetic_example_spec("httpbin-get.arazzo.yaml", &server.base_url, temp.path());
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            "--step",
            "fetch-ip",
            "--no-deps",
            &spec_str,
            "get-origin",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "success");
}

#[test]
fn run_step_no_deps_fails_when_deps_exist() {
    let mut spec = repo_root();
    spec.push("examples/httpbin-chained-posts.arazzo.yaml");
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            "--step",
            "post-enriched",
            "--no-deps",
            &spec_str,
            "post-chain",
        ]
        .as_slice(),
        None,
    );
    assert!(!output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
    let code = body.get("code").and_then(Value::as_str).unwrap_or_default();
    assert_eq!(code, "STEP_MISSING_DEPENDENCY");
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        error.contains("post-initial"),
        "error should mention the missing dep: {error}"
    );
}

#[test]
fn run_step_dry_run_returns_request() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            "--step",
            "fetch-ip",
            "--dry-run",
            &spec_str,
            "get-origin",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "dryRun");
    let requests = run_json_requests(&body);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].get("stepId"),
        Some(&Value::String("fetch-ip".to_string()))
    );
}

#[test]
fn run_step_missing_step_id_fails() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            "--step",
            "nonexistent",
            &spec_str,
            "get-origin",
        ]
        .as_slice(),
        None,
    );
    assert!(!output.status.success());
    let body = stdout_json(&output);
    assert_run_json_kind(&body, "error");
}

#[test]
fn run_no_deps_without_step_fails() {
    // --no-deps requires --step
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        ["--json", "run", "--no-deps", &spec_str, "get-origin"].as_slice(),
        None,
    );
    assert!(
        !output.status.success(),
        "--no-deps without --step should be rejected by clap"
    );
}

// ---------------------------------------------------------------------------
// Security redaction tests
// ---------------------------------------------------------------------------

#[test]
fn run_dry_run_json_redacts_sensitive_request_parts() {
    let temp = TempDir::new("arazzo-dry-run-json-redact");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Dry-Run JSON Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: /items
        parameters:
          - name: Authorization
            in: header
            value: "Bearer top-secret-jwt"
          - name: X-API-Key
            in: header
            value: "sk-1234567890"
          - name: Accept
            in: header
            value: "application/json"
          - name: token
            in: query
            value: "query-secret-123"
          - name: page
            in: query
            value: "1"
        requestBody:
          contentType: application/json
          payload:
            clientSecret: body-secret-123
            safeName: alice
            nested:
              dbPassword: hunter2
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        ["--json", "run", &spec_str, "wf", "--dry-run"].as_slice(),
        None,
    );
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        !stdout.contains("top-secret-jwt")
            && !stdout.contains("sk-1234567890")
            && !stdout.contains("query-secret-123")
            && !stdout.contains("body-secret-123")
            && !stdout.contains("hunter2"),
        "dry-run JSON stdout should not contain raw secrets, got: {stdout}"
    );

    let body = stdout_json(&output);
    let reqs = run_json_requests(&body);
    assert!(!reqs.is_empty());

    let headers = reqs[0]
        .get("headers")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected headers object in dry-run JSON"));

    // Authorization and X-API-Key should be redacted
    let auth = headers
        .get("Authorization")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        auth, "[REDACTED]",
        "Authorization header should be redacted in --json --dry-run output"
    );

    let api_key = headers
        .get("X-API-Key")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        api_key, "[REDACTED]",
        "X-API-Key header should be redacted in --json --dry-run output"
    );

    // Non-sensitive header should survive
    let accept = headers
        .get("Accept")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        accept, "application/json",
        "Accept header should not be redacted"
    );

    let url = reqs[0]
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        url.contains("token=[REDACTED]") || url.contains("token=%5BREDACTED%5D"),
        "sensitive query value should be redacted: {url}"
    );
    assert!(
        url.contains("page=1"),
        "safe query value should survive: {url}"
    );

    let req_body = reqs[0]
        .get("body")
        .unwrap_or_else(|| panic!("expected body in dry-run JSON"));
    assert_eq!(
        req_body.pointer("/clientSecret"),
        Some(&Value::String("[REDACTED]".to_string())),
        "clientSecret body field should be redacted"
    );
    assert_eq!(
        req_body.pointer("/nested/dbPassword"),
        Some(&Value::String("[REDACTED]".to_string())),
        "dbPassword body field should be redacted"
    );
    assert_eq!(
        req_body.pointer("/safeName"),
        Some(&Value::String("alice".to_string())),
        "safeName body field should survive"
    );
}

#[test]
fn run_dry_run_human_redacts_sensitive_request_parts() {
    let temp = TempDir::new("arazzo-dry-run-human-redact");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Dry-Run Human Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: step-one
        operationPath: /items
        parameters:
          - name: Authorization
            in: header
            value: "Bearer top-secret-jwt"
          - name: Accept
            in: header
            value: "application/json"
          - name: token
            in: query
            value: "query-secret-123"
          - name: page
            in: query
            value: "1"
        requestBody:
          contentType: application/json
          payload:
            clientSecret: body-secret-123
            safeName: alice
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(["run", &spec_str, "wf", "--dry-run"].as_slice(), None);
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        !stdout.contains("top-secret-jwt")
            && !stdout.contains("query-secret-123")
            && !stdout.contains("body-secret-123"),
        "dry-run human stdout should not contain raw secrets, got: {stdout}"
    );
    assert!(
        stdout.contains("[REDACTED]"),
        "dry-run human stdout should show redaction marker, got: {stdout}"
    );
    assert!(
        stdout.contains("application/json")
            && stdout.contains("page=1")
            && stdout.contains("alice"),
        "dry-run human stdout should preserve non-sensitive values, got: {stdout}"
    );
}

#[test]
fn run_trace_redacts_compound_key_names() {
    let temp = TempDir::new("arazzo-trace-redact-compound");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");
    let mut trace_path = temp.path().to_path_buf();
    trace_path.push("trace.json");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Compound Key Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        bearerToken:
          type: string
        dbPassword:
          type: string
        safeName:
          type: string
    steps:
      - stepId: step-one
        operationPath: /items
        requestBody:
          contentType: application/json
          payload:
            bearerToken: $inputs.bearerToken
            dbPassword: $inputs.dbPassword
            safeName: $inputs.safeName
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let trace_str = trace_path.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--input",
            "bearerToken=jwt-secret-abc",
            "--input",
            "dbPassword=hunter2",
            "--input",
            "safeName=alice",
            "--trace",
            &trace_str,
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let trace = read_json_file(&trace_path);

    // Compound key names should be redacted via stem matching
    assert_eq!(
        trace.pointer("/inputs/bearerToken"),
        Some(&Value::String("[REDACTED]".to_string())),
        "bearerToken input should be redacted (contains 'token' stem)"
    );
    assert_eq!(
        trace.pointer("/inputs/dbPassword"),
        Some(&Value::String("[REDACTED]".to_string())),
        "dbPassword input should be redacted (contains 'password' stem)"
    );
    // Non-sensitive input should survive
    assert_eq!(
        trace.pointer("/inputs/safeName"),
        Some(&Value::String("alice".to_string())),
        "safeName should not be redacted"
    );

    // Request body should also have compound keys redacted
    assert_eq!(
        trace.pointer("/steps/0/request/body/bearerToken"),
        Some(&Value::String("[REDACTED]".to_string())),
        "bearerToken in request body should be redacted"
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/dbPassword"),
        Some(&Value::String("[REDACTED]".to_string())),
        "dbPassword in request body should be redacted"
    );
    assert_eq!(
        trace.pointer("/steps/0/request/body/safeName"),
        Some(&Value::String("alice".to_string())),
        "safeName in request body should survive"
    );
}

#[test]
fn run_verbose_redacts_sensitive_inputs() {
    let temp = TempDir::new("arazzo-verbose-redact");
    let mut spec_path = temp.path().to_path_buf();
    spec_path.push("spec.yaml");

    let spec = r#"
arazzo: 1.0.0
info:
  title: Verbose Redaction
  version: 1.0.0
sourceDescriptions:
  - name: sample
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        apiSecret:
          type: string
        userName:
          type: string
    steps:
      - stepId: step-one
        operationPath: /items
        successCriteria:
          - condition: $statusCode == 200
"#;
    write_file(&spec_path, spec);

    let spec_str = spec_path.to_string_lossy().to_string();
    let output = run(
        [
            "--verbose",
            "run",
            &spec_str,
            "wf",
            "--dry-run",
            "--input",
            "apiSecret=super-secret-key",
            "--input",
            "userName=alice",
        ]
        .as_slice(),
        None,
    );
    // Dry-run may succeed or fail — we only care about stderr content
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Verbose output should NOT contain the secret value
    assert!(
        !stderr.contains("super-secret-key"),
        "verbose stderr should not contain secret value, got: {stderr}"
    );
    // Verbose output SHOULD contain the redaction marker
    assert!(
        stderr.contains("[REDACTED]"),
        "verbose stderr should show [REDACTED] for sensitive inputs, got: {stderr}"
    );
    // Non-sensitive value should appear
    assert!(
        stderr.contains("alice"),
        "verbose stderr should show non-sensitive input value, got: {stderr}"
    );
}
