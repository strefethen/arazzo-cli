#![forbid(unsafe_code)]

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_mcp::{protocol, state::ServerState};
use serde_json::{json, Value};

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
            "arazzo-mcp-request-admission-{}-{}",
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
        fs::write(
            directory.join("source.json"),
            json!({
                "openapi": "3.0.3",
                "info": {"title": "MCP admission fixture", "version": "1"},
                "servers": [{"url": base}],
                "paths": {
                    "/pets": {"get": {"operationId": "find", "responses": {"200": {"description": "OK"}}}},
                    "/pets/{petId}": {"delete": {"operationId": "remove", "responses": {"200": {"description": "OK"}}}}
                }
            })
            .to_string(),
        )
        .unwrap_or_else(|error| panic!("write source: {error}"));
        Self {
            directory,
            requests,
            stop,
            server: Some(server),
        }
    }

    fn call_with_native_platform_control(&self, workflow: &str, inputs: Value) -> (bool, Value) {
        let spec = serde_json::from_value(json!({
            "arazzo": "1.1.0",
            "info": {"title": "MCP admission fixture", "version": "1"},
            "sourceDescriptions": [{"name": "api", "url": "source.json", "type": "openapi"}],
            "workflows": [
                {"workflowId": "parent", "steps": [
                    {"stepId": "find", "operationId": "find", "outputs": {"id": "$response.body#/id"}},
                    {"stepId": "call", "workflowId": "child", "parameters": [
                        {"name": "id", "value": "$steps.find.outputs.id"}
                    ]}
                ]},
                {"workflowId": "child", "inputs": {
                    "type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}
                }, "steps": [
                    {"stepId": "remove", "operationId": "remove", "parameters": [
                        {"name": "petId", "in": "path", "value": "$inputs.id"}
                    ]}
                ]}
            ]
        }))
        .unwrap_or_else(|error| panic!("parse fixture model: {error}"));
        let mut state = ServerState::load(
            &[],
            Some(vec![self.directory.to_string_lossy().into_owned()]),
        )
        .unwrap_or_else(|error| panic!("configure explicit fixture root: {error}"));
        state.specs.push(arazzo_mcp::state::LoadedSpec {
            file_path: self
                .directory
                .join("workflow.arazzo.json")
                .to_string_lossy()
                .into_owned(),
            spec,
        });
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"admission-test","version":"1"}
            }}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"run_workflow","arguments":{"workflow_id":workflow,"inputs":inputs.clone()}
            }}),
        ];
        let mut input = Vec::new();
        for message in messages {
            let payload = message.to_string();
            input.extend_from_slice(
                format!("Content-Length: {}\r\n\r\n{payload}", payload.len()).as_bytes(),
            );
        }
        let mut output = Vec::new();
        protocol::serve(Cursor::new(input), &mut output, &state)
            .unwrap_or_else(|error| panic!("serve MCP messages: {error}"));
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap_or_else(|error| panic!("UTF-8 MCP output: {error}"))
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|error| panic!("JSON MCP response: {error}"))
            })
            .collect();
        assert_eq!(responses.len(), 2, "{responses:?}");
        let response = responses
            .into_iter()
            .find(|response| response["id"] == 2)
            .unwrap_or_else(|| panic!("tools/call response must be present"));
        (response["result"]["isError"] == true, tool_body(&response))
    }

    fn assert_requests(&self, expected: &[&str]) {
        assert_eq!(
            self.requests
                .lock()
                .unwrap_or_else(|error| panic!("read requests: {error}"))
                .as_slice(),
            expected
        );
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

fn tool_body(response: &Value) -> Value {
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool text must be present")),
    )
    .unwrap_or_else(|error| panic!("one JSON tool result: {error}"))
}

#[test]
fn strict_run_workflow_preserves_nested_admission_code_and_source() {
    let fixture = Fixture::new();
    let response = fixture.call_with_native_platform_control("parent", json!({}));
    assert!(response.0, "{response:?}");
    let body = response.1;
    assert_eq!(body["code"], "RUNTIME_UNRESOLVED_REQUEST_VALUE", "{body}");
    let message = body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("admission message must be present"));
    for expected in [
        "step \"remove\"",
        "parameters[0](path:petId)",
        "$inputs.id",
        "request not sent",
    ] {
        assert!(message.contains(expected), "missing {expected:?}: {body}");
    }
    assert!(!message.contains("rex"), "resolved data leaked: {body}");
    fixture.assert_requests(&["GET /pets"]);
}

#[test]
fn strict_run_workflow_keeps_concrete_input_validation_and_valid_input() {
    for inputs in [json!({}), json!({"id": null}), json!({"id": 7})] {
        let fixture = Fixture::new();
        let response = fixture.call_with_native_platform_control("child", inputs.clone());
        assert!(response.0, "{inputs}: {response:?}");
        let body = response.1;
        assert_eq!(body["code"], "RUNTIME_INPUT_VALIDATION", "{inputs}: {body}");
        fixture.assert_requests(&[]);
    }
    let fixture = Fixture::new();
    let response = fixture.call_with_native_platform_control("child", json!({"id": "known"}));
    assert!(!response.0, "{response:?}");
    assert_eq!(response.1["kind"], "success", "{response:?}");
    fixture.assert_requests(&["DELETE /pets/known"]);
}
