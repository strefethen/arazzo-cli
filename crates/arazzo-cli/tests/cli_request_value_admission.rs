#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

const CODE: &str = "RUNTIME_UNRESOLVED_REQUEST_VALUE";

struct Fixture {
    directory: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "arazzo-cli-request-admission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory)
            .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
        let listener = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("bind fixture server: {error}"));
        let base = format!("http://{}", listener.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let server = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                if let Some(request) = listener
                    .recv_timeout(Duration::from_millis(10))
                    .unwrap_or_else(|error| panic!("receive fixture request: {error}"))
                {
                    recorded
                        .lock()
                        .unwrap_or_else(|error| panic!("record request: {error}"))
                        .push(format!("{} {}", request.method(), request.url()));
                    request
                        .respond(tiny_http::Response::from_string(r#"{"name":"rex"}"#))
                        .unwrap_or_else(|error| panic!("respond to fixture request: {error}"));
                }
            }
        });
        let source = json!({
            "openapi": "3.0.3",
            "info": {"title": "Admission fixture", "version": "1"},
            "servers": [{"url": base}],
            "paths": {
                "/pets": {"get": {"operationId": "find", "responses": {"200": {"description": "OK"}}}},
                "/pets/{petId}": {"delete": {"operationId": "remove", "responses": {"200": {"description": "OK"}}}}
            }
        });
        fs::write(directory.join("source.json"), source.to_string())
            .unwrap_or_else(|error| panic!("write source: {error}"));
        let spec = json!({
            "arazzo": "1.1.0",
            "info": {"title": "Admission fixture", "version": "1"},
            "sourceDescriptions": [{"name": "api", "url": "source.json", "type": "openapi"}],
            "workflows": [{"workflowId": "guarded", "steps": [
                {"stepId": "find", "operationId": "find", "outputs": {"id": "$response.body#/id"}},
                {"stepId": "remove", "operationId": "remove", "parameters": [
                    {"name": "petId", "in": "path", "value": "$steps.find.outputs.id"}
                ]}
            ]}]
        });
        fs::write(directory.join("workflow.arazzo.json"), spec.to_string())
            .unwrap_or_else(|error| panic!("write workflow: {error}"));
        Self {
            directory,
            requests,
            stop,
            server: Some(server),
        }
    }

    fn run(&self, command: &str, diagnostics: Option<&str>, json_output: bool) -> Output {
        let mut process = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        if json_output {
            process.arg("--json");
        }
        process
            .arg(command)
            .arg(self.directory.join("workflow.arazzo.json"));
        if command == "run" {
            process.arg("guarded");
        }
        if let Some(mode) = diagnostics {
            process.args(["--expr-diagnostics", mode]);
        }
        process
            .output()
            .unwrap_or_else(|error| panic!("run CLI admission fixture: {error}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let joined = server.join();
            if !thread::panicking() {
                assert!(joined.is_ok(), "fixture server panicked");
            }
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn assert_diagnostic(message: &str) {
    for expected in [
        "step \"remove\"",
        "parameters[0](path:petId)",
        "$steps.find.outputs.id",
        "request not sent",
    ] {
        assert!(
            message.contains(expected),
            "missing {expected:?}: {message}"
        );
    }
    assert!(!message.contains("rex"), "resolved data leaked: {message}");
}

#[test]
fn run_and_test_preserve_admission_across_diagnostics_and_presentations() {
    for command in ["run", "test"] {
        for diagnostics in [None, Some("warn"), Some("error")] {
            for json_output in [false, true] {
                let fixture = Fixture::new();
                let output = fixture.run(command, diagnostics, json_output);
                assert!(
                    !output.status.success(),
                    "{command} {diagnostics:?} json={json_output}: {output:?}"
                );
                if json_output {
                    let body: Value =
                        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                            panic!("one JSON result required: {error}; {output:?}")
                        });
                    if command == "run" {
                        assert_eq!(body["kind"], "error", "{body}");
                        assert_eq!(body["code"], CODE, "{body}");
                        assert_diagnostic(
                            body["error"]
                                .as_str()
                                .unwrap_or_else(|| panic!("error message must be present")),
                        );
                        assert!(body.get("requests").is_none(), "{body}");
                    } else {
                        assert_eq!(body["summary"]["totalTests"], 1, "{body}");
                        assert_eq!(body["summary"]["errors"], 1, "{body}");
                        let case = &body["suites"][0]["tests"][0];
                        assert_eq!(case["status"], "error", "{body}");
                        assert_eq!(case["errorCode"], CODE, "{body}");
                        assert_diagnostic(
                            case["error"]
                                .as_str()
                                .unwrap_or_else(|| panic!("test error message must be present")),
                        );
                    }
                } else if command == "run" {
                    assert_diagnostic(&String::from_utf8_lossy(&output.stderr));
                } else {
                    let report = String::from_utf8_lossy(&output.stdout);
                    assert!(report.contains("not ok 1"), "{report}");
                    assert!(report.contains(CODE), "{report}");
                    assert_diagnostic(&report.replace("\\\"", "\""));
                }
                assert_eq!(
                    fixture
                        .requests
                        .lock()
                        .unwrap_or_else(|error| panic!("read requests: {error}"))
                        .as_slice(),
                    ["GET /pets"],
                    "{command} {diagnostics:?} json={json_output}"
                );
            }
        }
    }
}
