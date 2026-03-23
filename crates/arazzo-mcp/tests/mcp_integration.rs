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
    ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, SuccessCriterion, Workflow,
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

/// Parse Content-Length framed responses from output bytes.
fn parse_responses(data: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(data);
    let mut results = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let remaining = &text[pos..];
        if !remaining.starts_with("Content-Length:") {
            break;
        }
        let header_end = match remaining.find("\r\n\r\n") {
            Some(i) => i,
            None => break,
        };
        let length_str = &remaining["Content-Length:".len()..header_end].trim();
        let length: usize = match length_str.parse() {
            Ok(n) => n,
            Err(_) => break,
        };
        let payload_start = header_end + 4;
        let payload_end = payload_start + length;
        if payload_end > remaining.len() {
            break;
        }
        if let Ok(val) = serde_json::from_str::<Value>(&remaining[payload_start..payload_end]) {
            results.push(val);
        }
        pos += payload_end;
    }
    results
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
                    m.insert("value".to_string(), "$response.body.value".to_string());
                    m
                },
                ..Step::default()
            }],
            outputs: {
                let mut m = BTreeMap::new();
                m.insert(
                    "value".to_string(),
                    "$steps.fetch.outputs.value".to_string(),
                );
                m
            },
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
