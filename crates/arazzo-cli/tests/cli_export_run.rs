#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture setup and assertions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-export-{}-{stamp}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create fixture directory");
        Self(path)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).expect("write fixture");
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Server {
    url: String,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start<F>(respond: F) -> Self
    where
        F: Fn(usize, &str) -> (u16, String, Vec<(&'static str, String)>) + Send + 'static,
    {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind local server");
        let url = format!("http://{}", server.server_addr());
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_hits = Arc::clone(&hits);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                    let n = thread_hits.fetch_add(1, Ordering::SeqCst);
                    let (status, body, headers) = respond(n, request.url());
                    let mut response = tiny_http::Response::from_string(body)
                        .with_status_code(tiny_http::StatusCode(status));
                    for (name, value) in headers {
                        let header =
                            tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                                .expect("response header");
                        response.add_header(header);
                    }
                    request.respond(response).expect("respond");
                }
            }
        });
        Self {
            url,
            hits,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join server");
        }
    }
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .expect("execute CLI")
}

fn artifact(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read artifact")).expect("parse artifact")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

fn spec(temp: &TempDir, base: &str, workflows: &str) -> PathBuf {
    temp.write(
        "workflow.yaml",
        &format!(
            "arazzo: 1.1.0\ninfo:\n  title: Export fixture\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: {base}\n    type: openapi\nworkflows:\n{workflows}"
        ),
    )
}

fn run_json(spec: &Path, workflow: &str, extras: &[&str]) -> Output {
    let mut args = vec![
        "--json",
        "run",
        spec.to_str().expect("UTF-8 path"),
        workflow,
    ];
    args.extend_from_slice(extras);
    cli(&args)
}

fn assert_utc_times(value: &Value) {
    for key in ["startedAt", "finishedAt", "exportedAt"] {
        let stamp = value["run"][key].as_str().expect("timestamp string");
        assert!(stamp.ends_with('Z'), "{key} must be UTC: {stamp}");
        assert!(stamp.contains('T'), "{key} must be RFC3339: {stamp}");
    }
    assert!(value["run"]["durationMs"].is_u64());
}

#[test]
fn exports_typed_full_outputs_and_body_free_metadata_without_changing_stdout() {
    let temp = TempDir::new();
    let full = json!({"numbers": [1, true, null], "padding": "x".repeat(4096)});
    let body = full.to_string();
    let server = Server::start(move |_, path| {
        if path.starts_with("/full") {
            (
                200,
                body.clone(),
                vec![("Content-Type", "application/json".into())],
            )
        } else {
            (200, "undeclared-body-sentinel".into(), vec![])
        }
    });
    let workflows = "  - workflowId: wf\n    steps:\n      - stepId: full\n        operationPath: /full\n        outputs:\n          full: $response.body\n          item: $response.body#/numbers/0\n      - stepId: hidden\n        operationPath: /hidden\n    outputs:\n      complete: $steps.full.outputs.full\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("run.json");
    let normal = run_json(&spec, "wf", &[]);
    let exported = run_json(&spec, "wf", &["--export-run", path.to_str().unwrap()]);
    assert!(
        normal.status.success(),
        "{}",
        String::from_utf8_lossy(&normal.stderr)
    );
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert_eq!(normal.stdout, exported.stdout);
    let value = artifact(&path);
    assert_eq!(value["schemaVersion"], "run.v1");
    assert_eq!(value["run"]["status"], "success");
    assert_utc_times(&value);
    assert_eq!(value["outputs"]["complete"], full);
    let first = &value["workflows"]["wf"]["steps"]["full"]["executions"][0];
    assert_eq!(first["sequence"], 1);
    assert_eq!(first["attempt"], 1);
    assert_eq!(first["outputs"]["item"], 1);
    assert_eq!(first["outputs"]["full"], full);
    assert_eq!(first["response"]["statusCode"], 200);
    assert!(first["response"]["bodyBytes"].as_u64().unwrap() > 2048);
    assert_eq!(
        value["workflows"]["wf"]["steps"]["hidden"]["executions"][0]["sequence"],
        2
    );
    let serialized = value.to_string();
    assert!(!serialized.contains("undeclared-body-sentinel"));
    assert!(!serialized.contains("bodyPreview"));
    assert!(first["request"].get("body").is_none());
    assert!(first["response"].get("body").is_none());
}

#[test]
fn partial_failures_redact_body_previews_and_keep_command_error() {
    let temp = TempDir::new();
    let server = Server::start(|_, path| {
        if path == "/ok" {
            (200, "{}".into(), vec![])
        } else {
            (503, "failed-body-sentinel".into(), vec![])
        }
    });
    let workflows = "  - workflowId: wf\n    steps:\n      - stepId: ok\n        operationPath: /ok\n      - stepId: failed\n        operationPath: /failed\n        successCriteria:\n          - condition: $statusCode == 200\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("failed.json");
    let output = run_json(&spec, "wf", &["--export-run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    assert_eq!(json_stdout(&output)["kind"], "error");
    let value = artifact(&path);
    assert_eq!(value["run"]["status"], "failure");
    assert_eq!(
        value["run"]["error"]["code"],
        "RUNTIME_SUCCESS_CRITERIA_FAILED"
    );
    assert_eq!(value["outputs"], json!({}));
    assert_eq!(
        value["workflows"]["wf"]["steps"]["ok"]["executions"][0]["sequence"],
        1
    );
    assert_eq!(
        value["workflows"]["wf"]["steps"]["failed"]["executions"][0]["response"]["statusCode"],
        503
    );
    assert!(!value.to_string().contains("failed-body-sentinel"));
}

#[test]
fn nested_failure_keeps_status_without_implicit_body_preview() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| (503, "nested-failed-body-sentinel".into(), vec![]));
    let workflows = "  - workflowId: parent\n    steps:\n      - stepId: call\n        workflowId: child\n  - workflowId: child\n    steps:\n      - stepId: failed\n        operationPath: /failed\n        successCriteria:\n          - condition: $statusCode == 200\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("failed.json");
    let output = run_json(&spec, "parent", &["--export-run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    let value = artifact(&path);
    assert_eq!(value["run"]["error"]["code"], "RUNTIME_SUB_WORKFLOW_FAILED");
    assert!(value["run"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("status=503"));
    assert!(!value.to_string().contains("nested-failed-body-sentinel"));
}

#[test]
fn dry_run_step_parallel_and_trace_are_distinct() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| (200, "{}".into(), vec![]));
    let workflows = "  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n      - stepId: two\n        operationPath: /two\n";
    let spec = spec(&temp, &server.url, workflows);
    let dry_path = temp.path("dry.json");
    let dry = run_json(
        &spec,
        "wf",
        &["--dry-run", "--export-run", dry_path.to_str().unwrap()],
    );
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let value = artifact(&dry_path);
    assert_eq!(value["run"]["status"], "dryRun");
    assert_eq!(
        value["workflows"]["wf"]["steps"]["one"]["executions"][0]["outcome"],
        "dryRun"
    );
    assert!(value["workflows"]["wf"]["steps"]["one"]["executions"][0]
        .get("response")
        .is_none());
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);

    let one_path = temp.path("one.json");
    let one = run_json(
        &spec,
        "wf",
        &[
            "--step",
            "one",
            "--no-deps",
            "--export-run",
            one_path.to_str().unwrap(),
        ],
    );
    assert!(
        one.status.success(),
        "{}",
        String::from_utf8_lossy(&one.stderr)
    );
    let value = artifact(&one_path);
    assert_eq!(value["run"]["stepId"], "one");
    assert!(value["workflows"]["wf"]["steps"].get("one").is_some());
    assert!(value["workflows"]["wf"]["steps"].get("two").is_none());

    let parallel_path = temp.path("parallel.json");
    let trace_path = temp.path("trace.json");
    let parallel = run_json(
        &spec,
        "wf",
        &[
            "--parallel",
            "--trace",
            trace_path.to_str().unwrap(),
            "--export-run",
            parallel_path.to_str().unwrap(),
        ],
    );
    assert!(
        parallel.status.success(),
        "{}",
        String::from_utf8_lossy(&parallel.stderr)
    );
    assert!(trace_path.exists());
    let value = artifact(&parallel_path);
    assert_eq!(value["run"]["parallel"], true);
    assert_eq!(
        value["workflows"]["wf"]["steps"]["one"]["executions"][0]["sequence"],
        1
    );
    assert_eq!(
        value["workflows"]["wf"]["steps"]["two"]["executions"][0]["sequence"],
        2
    );
}

#[test]
fn rejects_aliases_and_preserves_files_on_write_failures() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| (200, "{}".into(), vec![]));
    let workflows =
        "  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n";
    let spec = spec(&temp, &server.url, workflows);
    let trace = temp.path("trace.json");
    let alias = temp.path("trace-alias.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&trace, &alias).expect("symlink");
    let same = run_json(
        &spec,
        "wf",
        &[
            "--trace",
            trace.to_str().unwrap(),
            "--export-run",
            trace.to_str().unwrap(),
        ],
    );
    assert!(!same.status.success());
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    #[cfg(unix)]
    {
        let same = run_json(
            &spec,
            "wf",
            &[
                "--trace",
                trace.to_str().unwrap(),
                "--export-run",
                alias.to_str().unwrap(),
            ],
        );
        assert!(!same.status.success());
        assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    }
    let input_alias = run_json(&spec, "wf", &["--export-run", spec.to_str().unwrap()]);
    assert!(!input_alias.status.success());
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    let openapi = temp.write("api.yaml", "openapi: 3.0.3\ninfo:\n  title: Local source\n  version: 1.0.0\nservers:\n  - url: http://127.0.0.1\npaths: {}\n");
    let local_spec = temp.write("local.yaml", "arazzo: 1.1.0\ninfo:\n  title: Local source\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: ./api.yaml\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n");
    let input_alias = run_json(
        &local_spec,
        "wf",
        &["--export-run", openapi.to_str().unwrap()],
    );
    assert!(!input_alias.status.success());
    assert_eq!(json_stdout(&input_alias)["code"], "RUN_EXPORT_PATH");
    assert!(fs::read_to_string(&openapi)
        .unwrap()
        .starts_with("openapi:"));
    let missing_parent = temp.path("missing/export.json");
    let failed = run_json(
        &spec,
        "wf",
        &["--export-run", missing_parent.to_str().unwrap()],
    );
    assert!(!failed.status.success());
    assert_eq!(json_stdout(&failed)["code"], "RUN_EXPORT_WRITE");
    assert!(String::from_utf8_lossy(&failed.stderr).contains("writing run export"));
    assert!(!missing_parent.exists());
    let directory_target = temp.path("directory-target");
    fs::create_dir(&directory_target).expect("create target directory");
    let failed = run_json(
        &spec,
        "wf",
        &["--export-run", directory_target.to_str().unwrap()],
    );
    assert!(!failed.status.success());
    assert_eq!(json_stdout(&failed)["code"], "RUN_EXPORT_WRITE");
    assert!(directory_target.is_dir());
}

#[test]
fn retries_nested_workflows_and_redaction_keep_order_and_identity() {
    let temp = TempDir::new();
    let server = Server::start(|n, path| {
        if path.starts_with("/retry") {
            (
                if n == 0 { 503 } else { 200 },
                "{\"token\":\"secret\",\"value\":7}".into(),
                vec![("X-Api-Key", "server-secret".into())],
            )
        } else {
            (200, "{}".into(), vec![])
        }
    });
    let workflows = "  - workflowId: parent\n    steps:\n      - stepId: call\n        workflowId: child\n  - workflowId: child\n    steps:\n      - stepId: retry\n        operationPath: /retry?token=query-secret\n        parameters:\n          - name: Authorization\n            in: header\n            value: Bearer header-secret\n        successCriteria:\n          - condition: $statusCode == 200\n        onFailure:\n          - name: again\n            type: retry\n            retryLimit: 1\n        outputs:\n          data: $response.body\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("run.json");
    let output = run_json(
        &spec,
        "parent",
        &[
            "--export-run",
            path.to_str().unwrap(),
            "--input-json",
            "nested={\"password\":\"hidden\"}",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = artifact(&path);
    assert_eq!(value["inputs"]["nested"]["password"], "[REDACTED]");
    let attempts = value["workflows"]["child"]["steps"]["retry"]["executions"]
        .as_array()
        .expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt"], 1);
    assert_eq!(attempts[1]["attempt"], 2);
    assert_eq!(attempts[1]["outputs"]["data"]["token"], "[REDACTED]");
    let headers = attempts[1]["request"]["headers"]
        .as_object()
        .expect("headers");
    assert!(
        headers
            .iter()
            .any(|(key, value)| key.eq_ignore_ascii_case("authorization") && value == "[REDACTED]"),
        "{headers:?}"
    );
    assert!(attempts[1]["request"]["url"]
        .as_str()
        .unwrap()
        .contains("token=[REDACTED]"));
    assert!(value["workflows"]["parent"]["steps"]["call"]["executions"]
        .as_array()
        .is_some());
    assert!(!value.to_string().contains("query-secret"));
    assert!(!value.to_string().contains("server-secret"));
}

#[test]
fn url_userinfo_and_string_leaves_are_sanitized() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| {
        (
            200,
            "{\"note\":\"Bearer body-secret\"}".into(),
            vec![("Content-Type", "application/json".into())],
        )
    });
    let credential_url = server.url.replacen("http://", "http://alice:password@", 1);
    let workflows = "  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n        outputs:\n          note: $response.body#/note\n";
    let spec = spec(&temp, &credential_url, workflows);
    let path = temp.path("run.json");
    let output = run_json(
        &spec,
        "wf",
        &[
            "--export-run",
            path.to_str().unwrap(),
            "--input-json",
            "comment=\"token=input-secret\"",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = artifact(&path);
    let request_url = value["workflows"]["wf"]["steps"]["one"]["executions"][0]["request"]["url"]
        .as_str()
        .expect("URL");
    assert!(request_url.contains("[REDACTED]@"));
    assert!(!request_url.contains("alice"));
    assert!(!request_url.contains("password"));
    assert_eq!(value["inputs"]["comment"], "token=[REDACTED]");
    assert_eq!(
        value["workflows"]["wf"]["steps"]["one"]["executions"][0]["outputs"]["note"],
        "Bearer [REDACTED]"
    );
    assert!(!value.to_string().contains("body-secret"));
}

#[test]
fn timeout_and_combined_export_failure_retain_each_cause() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| {
        thread::sleep(Duration::from_millis(200));
        (200, "{}".into(), vec![])
    });
    let workflows =
        "  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("timeout.json");
    let output = run_json(
        &spec,
        "wf",
        &[
            "--execution-timeout",
            "30ms",
            "--export-run",
            path.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let value = artifact(&path);
    assert_eq!(value["run"]["status"], "failure");
    assert_eq!(value["run"]["error"]["code"], "RUNTIME_EXECUTION_TIMEOUT");
    assert_eq!(
        value["workflows"]["wf"]["steps"]["one"]["executions"][0]["outcome"],
        "failure"
    );

    let missing = temp.path("missing/failed.json");
    let output = run_json(
        &spec,
        "wf",
        &[
            "--execution-timeout",
            "30ms",
            "--export-run",
            missing.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let error = json_stdout(&output);
    assert_eq!(error["code"], "RUN_EXPORT_WRITE");
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("execution timeout"));
    assert!(error["error"]
        .as_str()
        .unwrap()
        .contains("writing run export"));
    assert!(!missing.exists());
}

#[test]
fn revisit_appends_another_attempt_to_the_same_step() {
    let temp = TempDir::new();
    let server = Server::start(|n, path| {
        let status = if path == "/a" && n == 0 { 200 } else { 500 };
        (status, "{}".into(), vec![])
    });
    let workflows = "  - workflowId: wf\n    steps:\n      - stepId: a\n        operationPath: /a\n        successCriteria:\n          - condition: $statusCode == 200\n      - stepId: b\n        operationPath: /b\n        successCriteria:\n          - condition: $statusCode == 200\n        onFailure:\n          - name: again\n            type: goto\n            stepId: a\n";
    let spec = spec(&temp, &server.url, workflows);
    let path = temp.path("revisit.json");
    let output = run_json(&spec, "wf", &["--export-run", path.to_str().unwrap()]);
    assert!(!output.status.success());
    let value = artifact(&path);
    let a = value["workflows"]["wf"]["steps"]["a"]["executions"]
        .as_array()
        .expect("a history");
    assert_eq!(a.len(), 2);
    assert_eq!(
        (a[0]["attempt"].as_u64(), a[0]["sequence"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(
        (a[1]["attempt"].as_u64(), a[1]["sequence"].as_u64()),
        (Some(2), Some(3))
    );
    assert_eq!(
        value["workflows"]["wf"]["steps"]["b"]["executions"][0]["sequence"],
        2
    );
}

#[test]
fn trace_and_export_write_failures_are_both_reported() {
    let temp = TempDir::new();
    let server = Server::start(|_, _| (200, "{}".into(), vec![]));
    let spec = spec(
        &temp,
        &server.url,
        "  - workflowId: wf\n    steps:\n      - stepId: one\n        operationPath: /one\n",
    );
    let trace_target = temp.path("trace-directory");
    fs::create_dir(&trace_target).expect("create trace directory target");
    let export_target = temp.path("missing/run.json");
    let output = run_json(
        &spec,
        "wf",
        &[
            "--trace",
            trace_target.to_str().unwrap(),
            "--export-run",
            export_target.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    let error = json_stdout(&output);
    assert_eq!(error["code"], "RUN_EXPORT_WRITE");
    let message = error["error"].as_str().expect("error message");
    assert!(message.contains("writing trace"));
    assert!(message.contains("writing run export"));
    assert!(trace_target.is_dir());
    assert!(!export_target.exists());
}

#[test]
fn schema_declares_run_envelope_and_execution_shape() {
    let output = cli(&["schema", "export-run"]);
    assert!(output.status.success());
    let schema = json_stdout(&output);
    let required = schema["required"].as_array().expect("root required fields");
    for key in [
        "schemaVersion",
        "tool",
        "run",
        "inputs",
        "outputs",
        "workflows",
    ] {
        assert!(required.contains(&json!(key)), "missing {key} in schema");
    }
    let serialized = schema.to_string();
    for key in [
        "sequence",
        "attempt",
        "outcome",
        "statusCode",
        "bodyBytes",
        "startedAt",
        "exportedAt",
    ] {
        assert!(serialized.contains(key), "missing {key} in schema");
    }
    assert_eq!(schema["$defs"]["Decision"]["required"], json!(["path"]));
}
