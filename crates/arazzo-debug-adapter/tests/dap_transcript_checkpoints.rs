#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

#[test]
fn dap_step_over_reaches_success_criteria_and_outputs_locals() {
    let server = start_server();
    let spec_path = write_temp_spec(&server.base_url);
    let input = dap_test_support::encode_dap_stream(&[
        json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {}
        }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "spec": spec_path.to_string_lossy(),
                "workflowId": "get-hackernews",
                "stopOnEntry": true
            }
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "configurationDone",
            "arguments": {}
        }),
        json!({
            "seq": 4,
            "type": "request",
            "command": "next",
            "arguments": {}
        }),
        json!({
            "seq": 5,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "$response" }
        }),
        json!({
            "seq": 6,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "$response.body" }
        }),
        json!({
            "seq": 7,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "$statusCode == 200" }
        }),
        json!({
            "seq": 8,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "$response.body != null" }
        }),
        json!({
            "seq": 9,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "title_1" }
        }),
        json!({
            "seq": 10,
            "type": "request",
            "command": "next",
            "arguments": {}
        }),
        json!({
            "seq": 11,
            "type": "request",
            "command": "scopes",
            "arguments": { "frameId": 100 }
        }),
        json!({
            "seq": 12,
            "type": "request",
            "command": "variables",
            "arguments": { "variablesReference": 1 }
        }),
        json!({
            "seq": 13,
            "type": "request",
            "command": "variables",
            "arguments": { "variablesReference": 2 }
        }),
        json!({
            "seq": 14,
            "type": "request",
            "command": "variables",
            "arguments": { "variablesReference": 3 }
        }),
        json!({
            "seq": 15,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "//item[1]/link" }
        }),
        json!({
            "seq": 16,
            "type": "request",
            "command": "continue",
            "arguments": {}
        }),
        json!({
            "seq": 17,
            "type": "request",
            "command": "disconnect",
            "arguments": {}
        }),
    ]);

    let reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();
    let run = run_dap_stdio(reader, &mut output);
    assert!(run.is_ok(), "running DAP loop");

    let messages = dap_test_support::decode_dap_stream(&output);
    let stopped_reasons = messages
        .iter()
        .filter(|message| message.get("event").and_then(|v| v.as_str()) == Some("stopped"))
        .filter_map(|message| message.pointer("/body/reason").and_then(|v| v.as_str()))
        .collect::<Vec<_>>();
    assert!(
        stopped_reasons.len() >= 3,
        "expected at least three stopped events (entry + criterion + output)"
    );
    assert_eq!(stopped_reasons[0], "pause");
    assert_eq!(stopped_reasons[1], "step");
    assert_eq!(stopped_reasons[2], "step");

    let variables_response = messages
        .iter()
        .find(|message| message.get("command").and_then(|v| v.as_str()) == Some("variables"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let variables = variables_response
        .pointer("/body/variables")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let link_1 = variables
        .iter()
        .find(|entry| entry.get("name").and_then(|v| v.as_str()) == Some("link_1"));
    assert!(
        link_1.is_some(),
        "locals should include link_1 at output checkpoint"
    );
    assert_eq!(
        link_1
            .and_then(|entry| entry.get("value"))
            .and_then(|v| v.as_str()),
        Some("https://example.com/one")
    );

    let scopes_response = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("scopes")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(11)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let scopes = scopes_response
        .pointer("/body/scopes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(scopes
        .iter()
        .any(|scope| { scope.get("name").and_then(|v| v.as_str()) == Some("Request") }));
    assert!(scopes
        .iter()
        .any(|scope| { scope.get("name").and_then(|v| v.as_str()) == Some("Response") }));

    let request_variables = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("variables")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(13)
        })
        .and_then(|message| message.pointer("/body/variables"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(request_variables.iter().any(|entry| {
        entry.get("name").and_then(|v| v.as_str()) == Some("method")
            && entry.get("value").and_then(|v| v.as_str()) == Some("GET")
    }));
    assert!(request_variables.iter().any(|entry| {
        entry.get("name").and_then(|v| v.as_str()) == Some("url")
            && entry
                .get("value")
                .and_then(|v| v.as_str())
                .is_some_and(|value| value.contains("/rss"))
    }));

    let response_variables = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("variables")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(14)
        })
        .and_then(|message| message.pointer("/body/variables"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(response_variables.iter().any(|entry| {
        entry.get("name").and_then(|v| v.as_str()) == Some("statusCode")
            && entry.get("value").and_then(|v| v.as_str()) == Some("200")
    }));
    assert!(response_variables.iter().any(|entry| {
        entry.get("name").and_then(|v| v.as_str()) == Some("bodyPreview")
            && entry
                .get("value")
                .and_then(|v| v.as_str())
                .is_some_and(|value| value.contains("<rss"))
    }));

    let evaluate_title = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(9)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_title
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("one")
    );

    let evaluate_response_alias = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(5)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let response_alias_result = evaluate_response_alias
        .pointer("/body/result")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        response_alias_result.contains("\"statusCode\":200"),
        "$response hover should expose the response status, got {response_alias_result:?}"
    );
    assert!(
        response_alias_result.contains("<rss>"),
        "$response hover should include the same body available through $response.body"
    );
    assert!(
        evaluate_response_alias
            .pointer("/body/variablesReference")
            .and_then(|v| v.as_u64())
            .is_some_and(|reference| reference > 0),
        "$response hover should be expandable"
    );

    let evaluate_response_body = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(6)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert!(
        evaluate_response_body
            .pointer("/body/result")
            .and_then(|v| v.as_str())
            .is_some_and(|value| value.contains("<rss>")),
        "$response.body should continue to expose the response body"
    );

    let evaluate_status_condition = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(7)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_status_condition
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("true")
    );

    let evaluate_body_condition = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(8)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_body_condition
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("true")
    );

    let evaluate_response = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(15)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_response
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("https://example.com/one")
    );

    let _ = fs::remove_file(spec_path);
}

#[test]
fn dap_evaluate_condition_with_response_json_pointer() {
    let server = start_server();
    let spec_path = write_temp_geo_spec(&server.base_url);
    let input = dap_test_support::encode_dap_stream(&[
        json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {}
        }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "spec": spec_path.to_string_lossy(),
                "workflowId": "geocode",
                "stopOnEntry": true
            }
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "configurationDone",
            "arguments": {}
        }),
        json!({
            "seq": 4,
            "type": "request",
            "command": "next",
            "arguments": {}
        }),
        json!({
            "seq": 5,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "$response.body#/features/0/properties/display_name != null"
            }
        }),
        json!({
            "seq": 6,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "$response.body#/features/0/properties/display_name"
            }
        }),
        json!({
            "seq": 7,
            "type": "request",
            "command": "continue",
            "arguments": {}
        }),
        json!({
            "seq": 8,
            "type": "request",
            "command": "disconnect",
            "arguments": {}
        }),
    ]);

    let reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();
    let run = run_dap_stdio(reader, &mut output);
    assert!(run.is_ok(), "running DAP loop");

    let messages = dap_test_support::decode_dap_stream(&output);
    let evaluate_condition = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(5)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_condition
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("true")
    );

    let evaluate_value = messages
        .iter()
        .find(|message| {
            message.get("command").and_then(|v| v.as_str()) == Some("evaluate")
                && message.get("request_seq").and_then(|v| v.as_u64()) == Some(6)
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        evaluate_value
            .pointer("/body/result")
            .and_then(|v| v.as_str()),
        Some("Apple Park Way, Cupertino")
    );

    let _ = fs::remove_file(spec_path);
}

fn write_temp_spec(base_url: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-checkpoint-{nanos}.yaml"));
    let spec = format!(
        r#"
arazzo: "1.0.0"
info:
  title: Demo
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: {base_url}
    type: openapi
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: /rss
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          title_1: {{context: $response.body, selector: '//item[1]/title', type: {{type: xpath, version: xpath-10}}}}
          link_1: {{context: $response.body, selector: '//item[1]/link', type: {{type: xpath, version: xpath-10}}}}
"#
    );
    if let Err(err) = fs::write(&path, spec) {
        panic!("writing temp spec: {err}");
    }
    path
}

fn write_temp_geo_spec(base_url: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-geo-{nanos}.yaml"));
    let spec = format!(
        r#"
arazzo: "1.0.0"
info:
  title: Geo
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: {base_url}
    type: openapi
workflows:
  - workflowId: geocode
    steps:
      - stepId: lookup
        operationPath: /geo
        successCriteria:
          - condition: $statusCode == 200
"#
    );
    if let Err(err) = fs::write(&path, spec) {
        panic!("writing temp spec: {err}");
    }
    path
}

#[derive(Debug)]
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

fn start_server() -> TestServer {
    let server = match Server::http("127.0.0.1:0") {
        Ok(server) => server,
        Err(err) => panic!("binding checkpoint server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let path = request.url().to_string();
                    if path == "/geo" {
                        let body = r#"{"features":[{"properties":{"display_name":"Apple Park Way, Cupertino"}}]}"#;
                        let mut response =
                            TinyResponse::from_string(body).with_status_code(StatusCode(200));
                        if let Ok(header) =
                            Header::from_bytes(b"Content-Type".as_slice(), b"application/json")
                        {
                            response = response.with_header(header);
                        }
                        let _ = request.respond(response);
                        continue;
                    }

                    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <item><title>one</title><link>https://example.com/one</link></item>
  </channel>
</rss>"#;
                    let mut response =
                        TinyResponse::from_string(body).with_status_code(StatusCode(200));
                    if let Ok(header) =
                        Header::from_bytes(b"Content-Type".as_slice(), b"application/rss+xml")
                    {
                        response = response.with_header(header);
                    }
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
