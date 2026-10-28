#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

struct DependencySuite {
    directory: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl DependencySuite {
    fn new(first_path: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "arazzo-test-dependencies-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap_or_else(|error| panic!("creating fixture: {error}"));
        let listener = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("binding fixture server: {error}"));
        let base_url = format!("http://{}", listener.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let server = thread::spawn(move || {
            // Keep stalled requests open while continuing to serve dependents.
            // This makes an incorrectly executed dependent observable even while
            // the prerequisite's request remains unanswered.
            let mut stalled = Vec::new();
            while !stopping.load(Ordering::Relaxed) {
                let request = listener
                    .recv_timeout(Duration::from_millis(10))
                    .unwrap_or_else(|error| panic!("receiving fixture request: {error}"));
                if let Some(request) = request {
                    let path = request.url().to_string();
                    recorded
                        .lock()
                        .unwrap_or_else(|error| panic!("recording request: {error}"))
                        .push(path.clone());
                    if path == "/stall" {
                        stalled.push(request);
                    } else {
                        let status = if path == "/fail" { 500 } else { 200 };
                        request
                            .respond(
                                tiny_http::Response::from_string("{}").with_status_code(status),
                            )
                            .unwrap_or_else(|error| {
                                panic!("responding to fixture request: {error}")
                            });
                    }
                }
            }
        });

        let source = json!({
            "openapi": "3.0.3",
            "info": {"title": "Dependency completion fixture", "version": "1.0.0"},
            "servers": [{"url": base_url}],
            "paths": {
                first_path: {"get": {"operationId": "first", "responses": {"200": {"description": "OK"}}}},
                "/dependent": {"get": {"operationId": "dependent", "responses": {"200": {"description": "OK"}}}}
            }
        });
        fs::write(directory.join("source.json"), source.to_string())
            .unwrap_or_else(|error| panic!("writing source fixture: {error}"));
        let spec = json!({
            "arazzo": "1.1.0",
            "info": {"title": "Dependency completion fixture", "version": "1.0.0"},
            "sourceDescriptions": [{"name": "api", "url": "source.json", "type": "openapi"}],
            "workflows": [
                {"workflowId": "first", "steps": [{"stepId": "request", "operationId": "first",
                    "successCriteria": [{"condition": "$statusCode == 200"}]}]},
                {"workflowId": "dependent", "dependsOn": ["first"], "steps": [
                    {"stepId": "request", "operationId": "dependent",
                        "successCriteria": [{"condition": "$statusCode == 200"}]}]}
            ]
        });
        fs::write(directory.join("suite.arazzo.json"), spec.to_string())
            .unwrap_or_else(|error| panic!("writing suite fixture: {error}"));

        Self {
            directory,
            requests,
            stop,
            server: Some(server),
        }
    }

    fn run(&self, execution_timeout: &str, http_timeout: &str, successful: bool) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(["--json", "test"])
            .arg(self.directory.join("suite.arazzo.json"))
            .args([
                "--execution-timeout",
                execution_timeout,
                "--http-timeout",
                http_timeout,
            ])
            .output()
            .unwrap_or_else(|error| panic!("running test suite: {error}"));
        assert_eq!(
            output.status.success(),
            successful,
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let body: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("parsing test report: {error}; output={output:?}"));
        assert_eq!(body["summary"]["suiteErrors"], 0, "{body}");
        assert_eq!(body["summary"]["totalTests"], 2, "{body}");
        assert_eq!(body["suites"][0]["tests"][0]["workflowId"], "first");
        assert_eq!(body["suites"][0]["tests"][1]["workflowId"], "dependent");
        body
    }

    fn assert_requests(&self, expected: &[&str]) {
        assert_eq!(
            self.requests
                .lock()
                .unwrap_or_else(|error| panic!("reading request log: {error}"))
                .as_slice(),
            expected
        );
    }
}

impl Drop for DependencySuite {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let result = server.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "fixture server panicked");
            }
        }
        fs::remove_dir_all(&self.directory)
            .unwrap_or_else(|error| panic!("removing fixture directory: {error}"));
    }
}

#[test]
fn execution_timeout_does_not_complete_a_dependency() {
    let suite = DependencySuite::new("/stall");
    let body = suite.run("200ms", "5s", false);
    let tests = &body["suites"][0]["tests"];
    assert_eq!(tests[0]["status"], "error", "{body}");
    assert_eq!(tests[0]["errorCode"], "RUNTIME_EXECUTION_TIMEOUT", "{body}");
    assert_eq!(tests[1]["status"], "error", "{body}");
    assert_eq!(
        tests[1]["errorCode"], "RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED",
        "{body}"
    );
    suite.assert_requests(&["/stall"]);
}

#[test]
fn criteria_failure_still_completes_a_dependency() {
    let suite = DependencySuite::new("/fail");
    let body = suite.run("5s", "5s", false);
    let tests = &body["suites"][0]["tests"];
    assert_eq!(tests[0]["status"], "fail", "{body}");
    assert_eq!(tests[0]["errorCode"], "RUNTIME_SUCCESS_CRITERIA_FAILED");
    assert_eq!(tests[1]["status"], "pass", "{body}");
    suite.assert_requests(&["/fail", "/dependent"]);
}

#[test]
fn http_error_still_completes_a_dependency() {
    let suite = DependencySuite::new("/stall");
    let body = suite.run("5s", "200ms", false);
    let tests = &body["suites"][0]["tests"];
    assert_eq!(tests[0]["status"], "error", "{body}");
    assert_eq!(tests[0]["errorCode"], "RUNTIME_HTTP_REQUEST", "{body}");
    assert_eq!(tests[1]["status"], "pass", "{body}");
    suite.assert_requests(&["/stall", "/dependent"]);
}

#[test]
fn passing_case_still_completes_a_dependency() {
    let suite = DependencySuite::new("/pass");
    let body = suite.run("5s", "5s", true);
    let tests = &body["suites"][0]["tests"];
    assert_eq!(tests[0]["status"], "pass", "{body}");
    assert_eq!(tests[1]["status"], "pass", "{body}");
    suite.assert_requests(&["/pass", "/dependent"]);
}
