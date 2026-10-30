#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

struct Fixture {
    spec_path: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let spec_path = std::env::temp_dir().join(format!(
            "arazzo-dap-request-admission-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
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
        let source = dap_test_support::write_openapi_document(
            &spec_path,
            &base,
            &[
                ("/pets", "get", Some("find")),
                ("/pets/{petId}", "delete", Some("remove")),
            ],
        )
        .unwrap_or_else(|error| panic!("write source: {error}"));
        fs::write(
            &spec_path,
            json!({
                "arazzo": "1.1.0",
                "info": {"title": "DAP admission fixture", "version": "1"},
                "sourceDescriptions": [{"name": "api", "url": source, "type": "openapi"}],
                "workflows": [{"workflowId": "guarded", "steps": [
                    {"stepId": "find", "operationId": "find", "outputs": {"id": "$response.body#/id"}},
                    {"stepId": "remove", "operationId": "remove", "parameters": [
                        {"name": "petId", "in": "path", "value": "$steps.find.outputs.id"}
                    ]}
                ]}]
            })
            .to_string(),
        )
        .unwrap_or_else(|error| panic!("write workflow: {error}"));
        Self {
            spec_path,
            requests,
            stop,
            server: Some(server),
        }
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
        dap_test_support::remove_temp_spec(&self.spec_path);
    }
}

#[test]
fn admission_stderr_precedes_failed_exit_and_termination() {
    let fixture = Fixture::new();
    let input = dap_test_support::encode_dap_stream(&[
        json!({"seq": 1, "type": "request", "command": "initialize", "arguments": {}}),
        json!({"seq": 2, "type": "request", "command": "launch", "arguments": {
            "spec": fixture.spec_path.to_string_lossy(), "workflowId": "guarded", "stopOnEntry": false
        }}),
        json!({"seq": 3, "type": "request", "command": "configurationDone", "arguments": {}}),
    ]);
    let mut output = Vec::new();
    dap_test_support::run_until_terminated(input, &mut output)
        .unwrap_or_else(|error| panic!("DAP session completion: {error}"));
    let messages = dap_test_support::decode_dap_stream(&output);
    let launch = messages
        .iter()
        .find(|message| message["type"] == "response" && message["command"] == "launch")
        .unwrap_or_else(|| panic!("missing launch response: {messages:#?}"));
    assert_eq!(launch["success"], true, "{messages:#?}");
    let error_index = messages
        .iter()
        .position(|message| {
            message["event"] == "output"
                && message["body"]["category"] == "stderr"
                && message["body"]["output"].as_str().is_some_and(|text| {
                    text.contains("workflow failed [RUNTIME_UNRESOLVED_REQUEST_VALUE]")
                })
        })
        .unwrap_or_else(|| panic!("missing admission stderr: {messages:#?}"));
    let diagnostic = messages[error_index]["body"]["output"]
        .as_str()
        .unwrap_or_else(|| panic!("stderr output must be present"));
    for expected in [
        "step \"remove\"",
        "parameters[0](path:petId)",
        "$steps.find.outputs.id",
        "request not sent",
    ] {
        assert!(
            diagnostic.contains(expected),
            "missing {expected:?}: {diagnostic}"
        );
    }
    assert!(
        !diagnostic.contains("rex"),
        "resolved data leaked: {diagnostic}"
    );
    let event_index = |event: &str| {
        messages
            .iter()
            .position(|message| message.get("event").and_then(Value::as_str) == Some(event))
            .unwrap_or_else(|| panic!("missing {event}: {messages:#?}"))
    };
    let exited_index = event_index("exited");
    let terminated_index = event_index("terminated");
    assert_eq!(
        messages[exited_index]["body"]["exitCode"], 1,
        "{messages:#?}"
    );
    assert!(
        error_index < exited_index && exited_index < terminated_index,
        "admission output must precede exited and terminated: {messages:#?}"
    );
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap_or_else(|error| panic!("read requests: {error}"))
            .as_slice(),
        ["GET /pets"]
    );
}
