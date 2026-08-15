#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

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

fn fixture_path() -> PathBuf {
    repo_root().join("testdata/arazzo-1.1-conformance.arazzo.yaml")
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

fn combined_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct RouteServer {
    base_url: String,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for RouteServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_route_server() -> RouteServer {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("bind hermetic tiny_http server: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let paths = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&paths);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(mut request)) => {
                    let path = format!(
                        "{} {}",
                        request.method().as_str(),
                        request.url().split('?').next().unwrap_or_default()
                    );
                    observed
                        .lock()
                        .unwrap_or_else(|_| panic!("request path lock"))
                        .push(path);
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);
                    let response = tiny_http::Response::from_string(r#"{"ok":true}"#)
                        .with_status_code(tiny_http::StatusCode(200));
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    RouteServer {
        base_url,
        paths,
        stop,
        handle: Some(handle),
    }
}

fn hermetic_fixture(server: &RouteServer, dir: &Path) -> PathBuf {
    let source = fixture_path();
    let raw = fs::read_to_string(&source)
        .unwrap_or_else(|err| panic!("reading {}: {err}", source.display()));
    let destination = dir.join("arazzo-1.1-conformance.arazzo.yaml");
    fs::write(&destination, raw)
        .unwrap_or_else(|err| panic!("writing {}: {err}", destination.display()));
    let openapi_source = repo_root().join("testdata/petstore.openapi.yaml");
    let openapi = fs::read_to_string(&openapi_source)
        .unwrap_or_else(|err| panic!("reading {}: {err}", openapi_source.display()));
    let openapi = openapi.replace("https://petstore.example.com/v1", &server.base_url);
    fs::write(dir.join("petstore.openapi.yaml"), openapi)
        .unwrap_or_else(|err| panic!("writing temporary OpenAPI source: {err}"));
    destination
}

fn request_paths(server: &RouteServer) -> Vec<String> {
    server
        .paths
        .lock()
        .unwrap_or_else(|_| panic!("request path lock"))
        .clone()
}

#[test]
fn committed_fixture_validates_without_diagnostics() {
    let path = fixture_path();
    let path = path.to_string_lossy().to_string();
    let output = run(["--json", "validate", &path].as_slice());
    assert!(output.status.success(), "{}", combined_text(&output));
    let body = stdout_json(&output);
    assert_eq!(body["valid"], true, "validation body: {body}");
    assert!(
        body.get("errors").is_none_or(|value| value == &json!([])),
        "validation errors: {body}"
    );
    assert!(
        body.get("warnings").is_none_or(|value| value == &json!([])),
        "validation warnings: {body}"
    );
}

#[test]
fn typed_jsonpath_replacement_dry_run_has_exact_body_without_warning() {
    let path = fixture_path();
    let path = path.to_string_lossy().to_string();
    let output = run(["--json", "run", &path, "typed-replacement", "--dry-run"].as_slice());
    assert!(output.status.success(), "{}", combined_text(&output));
    let body = stdout_json(&output);
    assert_eq!(body["kind"], "dryRun", "dry-run body: {body}");
    let requests = body["requests"]
        .as_array()
        .unwrap_or_else(|| panic!("dry-run requests array: {body}"));
    assert_eq!(requests.len(), 1, "dry-run body: {body}");
    assert_eq!(requests[0]["stepId"], "replace");
    assert_eq!(
        requests[0]["body"],
        json!({
            "items": [
                {"sku": "ABC123", "quantity": 99},
                {"sku": "XYZ999", "quantity": 2}
            ]
        })
    );
    assert!(
        requests[0]
            .get("warnings")
            .is_none_or(|value| value == &json!([])),
        "replacement warnings: {body}"
    );
}

#[test]
fn action_reference_and_workflow_dependency_guard_use_hermetic_server() {
    let server = start_route_server();
    let temp = TempDir::new("arazzo-1-1-conformance");
    let path = hermetic_fixture(&server, temp.path());
    let path = path.to_string_lossy().to_string();

    let action = run(["run", &path, "action-reference"].as_slice());
    assert!(action.status.success(), "{}", combined_text(&action));
    assert_eq!(
        request_paths(&server),
        ["GET /pets", "GET /pets/42"],
        "action reference must skip s2"
    );

    server
        .paths
        .lock()
        .unwrap_or_else(|_| panic!("request path lock"))
        .clear();
    let direct = run(["--json", "run", &path, "dependent"].as_slice());
    assert!(!direct.status.success(), "unsatisfied dependency must fail");
    let direct_body = stdout_json(&direct);
    assert_eq!(
        direct_body["kind"], "error",
        "direct dependency body: {direct_body}"
    );
    assert_eq!(
        direct_body["code"],
        "RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED"
    );
    assert!(
        request_paths(&server).is_empty(),
        "dependency rejection must happen before HTTP"
    );

    let test = run(["--json", "test", &path].as_slice());
    assert!(test.status.success(), "{}", combined_text(&test));
    let test_body = stdout_json(&test);
    let tests = test_body["suites"][0]["tests"]
        .as_array()
        .unwrap_or_else(|| panic!("test suite workflow entries: {test_body}"));
    let workflow_ids: Vec<_> = tests
        .iter()
        .filter_map(|test| test["workflowId"].as_str())
        .collect();
    assert_eq!(
        workflow_ids,
        [
            "typed-replacement",
            "action-reference",
            "prerequisite",
            "dependent"
        ]
    );
    assert_eq!(
        request_paths(&server),
        [
            "POST /pets",
            "GET /pets",
            "GET /pets/42",
            "DELETE /pets/42",
            "POST /pets"
        ]
    );
}
