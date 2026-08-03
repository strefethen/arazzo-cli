//! Integration tests for the MCP server.
//!
//! Uses `tiny_http` mock servers so tests run without external API calls.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_mcp::protocol;
use arazzo_mcp::state::ServerState;
use arazzo_spec::{
    ArazzoSpec, Info, ParamLocation, Parameter, RequestBody, SourceDescription, SourceType, Step,
    StepTarget, SuccessCriterion, Workflow,
};
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

// ---------------------------------------------------------------------------
// Test HTTP server (adapted from arazzo-runtime/tests/common/mod.rs)
// ---------------------------------------------------------------------------

struct TestServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_server<F>(handler: F) -> TestServer
where
    F: Fn(String, String) -> (u16, String) + Send + Sync + 'static,
{
    let server = Server::http("127.0.0.1:0").unwrap_or_else(|err| panic!("bind: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handler = Arc::new(handler);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let method = request.method().as_str().to_string();
                    let url = request.url().to_string();
                    let (status, body) = handler(method, url);
                    let response = TinyResponse::from_string(&body)
                        .with_status_code(StatusCode(status))
                        .with_header(
                            Header::from_bytes(b"Content-Type", b"application/json")
                                .unwrap_or_else(|_| panic!("header")),
                        );
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    TestServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

// ---------------------------------------------------------------------------
// MCP message helpers
// ---------------------------------------------------------------------------

fn frame(payload: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload).into_bytes()
}

fn frame_msg(msg: &Value) -> Vec<u8> {
    let s = serde_json::to_string(msg).unwrap_or_default();
    frame(&s)
}

fn build_messages(msgs: &[Value]) -> Vec<u8> {
    msgs.iter().flat_map(frame_msg).collect()
}

/// Parse newline-delimited JSON responses from output bytes.
fn parse_responses(data: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(data);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

/// Extract the text content from an MCP tool result.
fn extract_tool_text(response: &Value) -> Option<Value> {
    let text = response
        .get("result")?
        .get("content")?
        .get(0)?
        .get("text")?
        .as_str()?;
    serde_json::from_str(text).ok()
}

fn is_tool_error(response: &Value) -> bool {
    response
        .get("result")
        .and_then(|r| r.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Spec builders
// ---------------------------------------------------------------------------

fn make_spec(base_url: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "Test Spec".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "get-data".to_string(),
            summary: "Fetch test data".to_string(),
            steps: vec![Step {
                step_id: "fetch".to_string(),
                target: Some(StepTarget::OperationPath("/data".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                outputs: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "value".to_string(),
                        "$response.body.value".to_string().into(),
                    );
                    m
                },
                ..Step::default()
            }],
            outputs: {
                let mut m = BTreeMap::new();
                m.insert(
                    "value".to_string(),
                    "$steps.fetch.outputs.value".to_string().into(),
                );
                m
            },
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

fn make_sensitive_spec(base_url: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "Sensitive Test Spec".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "send-secret".to_string(),
            steps: vec![Step {
                step_id: "submit".to_string(),
                target: Some(StepTarget::OperationPath("/submit".to_string())),
                parameters: vec![
                    Parameter {
                        name: "Authorization".to_string(),
                        in_: Some(ParamLocation::Header),
                        value: serde_yaml_ng::Value::String("Bearer top-secret-jwt".to_string())
                            .into(),
                        ..Parameter::default()
                    },
                    Parameter {
                        name: "Accept".to_string(),
                        in_: Some(ParamLocation::Header),
                        value: serde_yaml_ng::Value::String("application/json".to_string()).into(),
                        ..Parameter::default()
                    },
                    Parameter {
                        name: "token".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String("query-secret-123".to_string()).into(),
                        ..Parameter::default()
                    },
                    Parameter {
                        name: "page".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String("1".to_string()).into(),
                        ..Parameter::default()
                    },
                ],
                request_body: Some(RequestBody {
                    content_type: "application/json".to_string(),
                    payload: Some(
                        serde_yaml_ng::to_value(json!({
                            "clientSecret": "body-secret-123",
                            "safeName": "alice",
                            "nested": { "dbPassword": "hunter2" }
                        }))
                        .unwrap_or_else(|err| panic!("building YAML payload: {err}"))
                        .into(),
                    ),
                    ..RequestBody::default()
                }),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_full_flow() {
    let server = start_server(|_method, url| match url.as_str() {
        "/data" => (200, r#"{"value":"hello-world"}"#.to_string()),
        _ => (404, "{}".to_string()),
    });

    let spec = make_spec(&server.base_url);
    let state = ServerState::from_spec("test.arazzo.yaml", spec);

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_workflows","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"describe_workflow","arguments":{"workflow_id":"get-data"}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"get-data"}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);

    // 5 responses (notification gets no response)
    assert_eq!(
        responses.len(),
        5,
        "expected 5 responses, got {}",
        responses.len()
    );

    // 1: initialize
    assert!(responses[0]["result"]["serverInfo"]["name"]
        .as_str()
        .is_some_and(|s| s == "arazzo-mcp"));

    // 2: tools/list
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(tools, 7);

    // 3: list_workflows
    let workflows = extract_tool_text(&responses[2]);
    assert!(workflows.is_some());
    let wf_list = workflows.as_ref().and_then(Value::as_array);
    assert!(wf_list.is_some_and(|a| a.len() == 1));
    assert_eq!(wf_list.unwrap_or(&vec![])[0]["id"], "get-data");

    // 4: describe_workflow
    let detail = extract_tool_text(&responses[3]);
    assert!(detail.is_some());
    assert_eq!(detail.as_ref().unwrap_or(&Value::Null)["stepCount"], 1);

    // 5: run_workflow
    assert!(!is_tool_error(&responses[4]));
    let run_result = extract_tool_text(&responses[4]);
    assert!(run_result.is_some());
    let run = run_result.unwrap_or(Value::Null);
    assert_eq!(run["kind"], "success");
    assert_eq!(run["outputs"]["value"], "hello-world");
}

#[test]
fn test_validate_spec() {
    let temp_dir = std::env::temp_dir();
    let spec_path = temp_dir.join("arazzo_mcp_test_validate.arazzo.yaml");
    let spec_yaml = r#"arazzo: "1.0.0"
info:
  title: Temp Test
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: https://example.com
    type: openapi
workflows:
  - workflowId: noop
    steps:
      - stepId: placeholder
        operationPath: /noop
        successCriteria:
          - condition: "$statusCode == 200"
"#;
    std::fs::write(&spec_path, spec_yaml).unwrap_or_else(|e| panic!("write temp: {e}"));

    let path_str = spec_path.to_string_lossy().to_string();
    let state = ServerState::empty();

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"validate_spec","arguments":{"file_path": path_str}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);

    let validate_result = extract_tool_text(&responses[1]);
    assert!(validate_result.is_some());
    let result = validate_result.unwrap_or(Value::Null);
    assert_eq!(result["valid"], true);
    assert_eq!(result["workflows"], 1);

    // Cleanup
    let _ = std::fs::remove_file(&spec_path);
}

#[test]
fn test_run_workflow_error() {
    let server = start_server(|_method, _url| (500, r#"{"error":"internal"}"#.to_string()));

    let spec = make_spec(&server.base_url);
    let state = ServerState::from_spec("test.arazzo.yaml", spec);

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"get-data"}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);
    assert!(is_tool_error(&responses[1]));
}

#[test]
fn test_unknown_tool() {
    let state = ServerState::empty();

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nonexistent_tool","arguments":{}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);
    assert!(is_tool_error(&responses[1]));
}

#[test]
fn test_unknown_method() {
    let state = ServerState::empty();

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"nonexistent/method","params":{}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);
    // JSON-RPC method not found error
    assert!(responses[1].get("error").is_some());
    assert_eq!(responses[1]["error"]["code"], -32601);
}

#[test]
fn test_dry_run() {
    let server = start_server(|_method, _url| {
        panic!("dry-run should not make HTTP requests");
    });

    let spec = make_spec(&server.base_url);
    let state = ServerState::from_spec("test.arazzo.yaml", spec);

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"get-data","dry_run":true}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);
    assert!(!is_tool_error(&responses[1]));

    let result = extract_tool_text(&responses[1]);
    assert!(result.is_some());
    let dry_run = result.unwrap_or(Value::Null);
    assert_eq!(dry_run["kind"], "dryRun");
    let requests = dry_run["requests"].as_array();
    assert!(requests.is_some_and(|r| !r.is_empty()));
}

#[test]
fn test_dry_run_redacts_sensitive_request_parts() {
    let server = start_server(|_method, _url| {
        panic!("dry-run should not make HTTP requests");
    });

    let spec = make_sensitive_spec(&server.base_url);
    let state = ServerState::from_spec("sensitive.arazzo.yaml", spec);

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"send-secret","dry_run":true}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(responses.len() >= 2);
    assert!(!is_tool_error(&responses[1]));

    let result_text = responses[1]
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected tool text response: {:?}", responses[1]));
    assert!(
        !result_text.contains("top-secret-jwt")
            && !result_text.contains("query-secret-123")
            && !result_text.contains("body-secret-123")
            && !result_text.contains("hunter2"),
        "MCP dry-run response should not contain raw secrets, got: {result_text}"
    );

    let result = extract_tool_text(&responses[1]);
    assert!(result.is_some());
    let dry_run = result.unwrap_or(Value::Null);
    assert_eq!(dry_run["kind"], "dryRun");
    let requests = dry_run["requests"]
        .as_array()
        .unwrap_or_else(|| panic!("expected dryRun requests array: {dry_run}"));
    assert!(!requests.is_empty());

    let request = &requests[0];
    let headers = request
        .get("headers")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("expected headers object: {request}"));
    assert_eq!(
        headers.get("Authorization").and_then(Value::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        headers.get("Accept").and_then(Value::as_str),
        Some("application/json")
    );

    let url = request
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

    let body = request
        .get("body")
        .unwrap_or_else(|| panic!("expected body object: {request}"));
    assert_eq!(
        body.pointer("/clientSecret"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        body.pointer("/nested/dbPassword"),
        Some(&Value::String("[REDACTED]".to_string()))
    );
    assert_eq!(
        body.pointer("/safeName"),
        Some(&Value::String("alice".to_string()))
    );
}

// ---------------------------------------------------------------------------
// sourceDescriptions document loading (relative urls)
// ---------------------------------------------------------------------------

fn testdata_path(rel: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let joined = if rel.is_empty() { dir } else { dir.join(rel) };
    joined.to_string_lossy().to_string()
}

#[test]
fn test_run_workflow_relative_source_dry_run() {
    // Allowed-dirs restriction is active, so the resolved source path passes
    // through check_path_allowed (and is allowed: it is inside testdata/).
    let state = match ServerState::load(
        &[testdata_path("petstore-relative.arazzo.yaml")],
        Some(vec![testdata_path("")]),
    ) {
        Ok(state) => state,
        Err(err) => panic!("loading server state: {err}"),
    };

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"list-pets","dry_run":true}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(
        responses.len() >= 2,
        "expected 2 responses, got {responses:?}"
    );
    assert!(
        !is_tool_error(&responses[1]),
        "run_workflow should succeed, got: {}",
        responses[1]
    );

    let run = extract_tool_text(&responses[1]).unwrap_or(Value::Null);
    assert_eq!(run["kind"], "dryRun", "unexpected result: {run}");
    assert_eq!(run["requests"][0]["method"], "GET");
    // The operationId resolves from the loaded source document and the base
    // derives from its servers[0].url — no openapi input exists over MCP.
    assert_eq!(
        run["requests"][0]["url"],
        "https://petstore.example.com/v1/pets"
    );
}

#[test]
fn test_run_workflow_relative_source_outside_allowed_dirs_denied() {
    // The spec sits inside the allowed dir, but its relative source url
    // resolves to a document outside it — check_path_allowed must refuse.
    let state = match ServerState::load(
        &[testdata_path("mcp-denied/spec.arazzo.yaml")],
        Some(vec![testdata_path("mcp-denied")]),
    ) {
        Ok(state) => state,
        Err(err) => panic!("loading server state: {err}"),
    };

    let messages = build_messages(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"list-pets-denied","dry_run":true}}}),
    ]);

    let reader = Cursor::new(messages);
    let mut output = Vec::new();
    protocol::serve(reader, &mut output, &state).ok();

    let responses = parse_responses(&output);
    assert!(
        responses.len() >= 2,
        "expected 2 responses, got {responses:?}"
    );
    assert!(
        is_tool_error(&responses[1]),
        "run_workflow should be denied, got: {}",
        responses[1]
    );

    let text = responses[1]["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("path not allowed"),
        "expected path denial, got: {text}"
    );
    assert!(
        text.contains("outside"),
        "expected the source name in the denial, got: {text}"
    );
}
