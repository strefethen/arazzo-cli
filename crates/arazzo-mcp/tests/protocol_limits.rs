#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use std::fs;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use arazzo_mcp::{run_mcp_stdio_configured, McpLimits, McpServerConfig};
use serde_json::{json, Value};

#[path = "support/eof_after_responses.rs"]
mod eof_support;
use eof_support::eof_after_responses;

const FRAME_LIMIT: usize = 256;

fn frame_limits() -> McpLimits {
    McpLimits {
        max_request_bytes: FRAME_LIMIT,
        ..McpLimits::default()
    }
}

fn serve<R>(input: R, specs: &[String], limits: McpLimits) -> Vec<Value>
where
    R: BufRead + Read + Send + 'static,
{
    let mut output = Vec::new();
    run_mcp_stdio_configured(
        input,
        &mut output,
        specs,
        McpServerConfig {
            allowed_dirs: None,
            limits,
        },
    )
    .unwrap();
    responses(&output)
}

fn serve_answered(input: Vec<u8>, specs: &[String], limits: McpLimits) -> Vec<Value> {
    let (reader, mut output) = eof_after_responses(input);
    let config = McpServerConfig {
        allowed_dirs: None,
        limits,
    };
    run_mcp_stdio_configured(reader, &mut output, specs, config).unwrap();
    responses(&output)
}

fn responses(output: &[u8]) -> Vec<Value> {
    std::str::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn ping(id: usize, size: usize) -> String {
    let head = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping""#);
    format!("{head}{}}}", " ".repeat(size - head.len() - 1))
}

fn pong(id: usize) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{}})
}

fn frame_rejection() -> Value {
    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Request exceeds the server frame limit"}})
}

fn fixture() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "arazzo-mcp-limits-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn tool_call(name: &str, arguments: Value) -> Cursor<Vec<u8>> {
    let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    Cursor::new(format!("{request}\n").into_bytes())
}

fn tool_text(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn frames_at_the_limit_are_processed_and_one_byte_over_ends_the_session() {
    for size in [FRAME_LIMIT - 1, FRAME_LIMIT, FRAME_LIMIT + 1] {
        let follow_up = ping(2, 60);
        let newline = format!("{}\n{follow_up}\n", ping(1, size));
        let content_length = format!(
            "Content-Length: {size}\r\n\r\n{}Content-Length: 60\r\n\r\n{follow_up}",
            ping(1, size)
        );
        for input in [newline, content_length] {
            let responses = serve(Cursor::new(input.into_bytes()), &[], frame_limits());
            if size <= FRAME_LIMIT {
                assert_eq!(responses, [pong(1), pong(2)], "size {size}");
            } else {
                assert_eq!(responses, [frame_rejection()], "size {size}");
            }
        }
    }
}

struct BlocksAfterPrefix(Cursor<Vec<u8>>);

impl Read for BlocksAfterPrefix {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.0.read(buffer)?;
        if count == 0 {
            loop {
                thread::park();
            }
        }
        Ok(count)
    }
}

#[test]
fn oversized_content_length_without_a_body_is_rejected_without_reading_it() {
    let header = format!("Content-Length: {}\r\n\r\n", FRAME_LIMIT + 1);
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let input = BufReader::new(BlocksAfterPrefix(Cursor::new(header.into_bytes())));
        let _ = sender.send(serve(input, &[], frame_limits()));
    });
    let responses = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(responses, [frame_rejection()]);
}

#[test]
fn a_burst_beyond_the_pending_limit_is_answered_in_order() {
    let pending = 2;
    let ids = 1..=3 * pending;
    let input: String = ids
        .clone()
        .map(|id| format!("{}\n", ping(id, 60)))
        .collect();
    let responses = serve(
        Cursor::new(input.into_bytes()),
        &[],
        McpLimits {
            max_pending_requests: pending,
            ..McpLimits::default()
        },
    );
    assert_eq!(responses, ids.map(pong).collect::<Vec<_>>());
}

#[test]
fn tool_results_over_the_result_limit_return_the_limit_error() {
    let root = fixture();
    let spec = root.join("probe.arazzo.yaml");
    fs::write(&spec, "arazzo: 1.1.0\ninfo: {title: Limits, version: '1'}\nsourceDescriptions: [{name: api, url: https://example.com, type: openapi}]\nworkflows: [{workflowId: probe, steps: [{stepId: call, operationId: probe}]}]\n").unwrap();
    let specs = [spec.to_str().unwrap().to_string()];
    let describe = |max_result_bytes| {
        let responses = serve(
            tool_call("describe_workflow", json!({"workflow_id":"probe"})),
            &specs,
            McpLimits {
                max_result_bytes,
                ..McpLimits::default()
            },
        );
        responses[0]["result"].clone()
    };
    let full = describe(McpLimits::default().max_result_bytes);
    let length = full["content"][0]["text"].as_str().unwrap().len();
    assert_eq!(full["isError"], false);
    assert_eq!(describe(length), full);
    let over = describe(length - 1);
    assert_eq!(over["isError"], true);
    assert_eq!(
        tool_text(&over),
        json!({"code":"MCP_LIMIT_EXCEEDED","message":"Operation exceeded a server resource limit"})
    );
}

#[test]
fn unknown_tool_errors_over_the_result_limit_return_the_limit_error() {
    let call = |name: &str| {
        let limits = McpLimits {
            max_result_bytes: 1024,
            ..McpLimits::default()
        };
        serve(tool_call(name, json!({})), &[], limits)[0]["result"].clone()
    };
    assert_eq!(
        call("nope"),
        json!({"content":[{"type":"text","text":"unknown tool: nope"}],"isError":true})
    );
    let over = call(&"x".repeat(5000));
    assert_eq!(over["isError"], true);
    assert_eq!(
        tool_text(&over),
        json!({"code":"MCP_LIMIT_EXCEEDED","message":"Operation exceeded a server resource limit"})
    );
}

#[test]
fn response_bodies_over_the_body_limit_fail_the_workflow() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", server.server_addr());
    thread::spawn(move || {
        if let Ok(request) = server.recv() {
            let _ = request.respond(tiny_http::Response::from_string("x".repeat(4096)));
        }
    });
    let root = fixture();
    let openapi = json!({"openapi":"3.0.3","info":{"title":"Body","version":"1"},"servers":[{"url":base_url}],
        "paths":{"/data":{"get":{"operationId":"getData","responses":{"200":{"description":"OK"}}}}}});
    fs::write(root.join("api.json"), openapi.to_string()).unwrap();
    let spec = root.join("body.arazzo.yaml");
    fs::write(&spec, "arazzo: 1.1.0\ninfo: {title: Body, version: '1'}\nsourceDescriptions: [{name: api, url: ./api.json, type: openapi}]\nworkflows: [{workflowId: fetch, steps: [{stepId: fetch, operationId: getData}]}]\n").unwrap();
    let responses = serve_answered(
        tool_call("run_workflow", json!({"workflow_id":"fetch"})).into_inner(),
        &[spec.to_str().unwrap().to_string()],
        McpLimits {
            max_response_body_bytes: 1024,
            ..McpLimits::default()
        },
    );
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(tool_text(result)["code"], "RUNTIME_RESPONSE_TOO_LARGE");
}

fn binary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-mcp"))
        .args(args)
        .current_dir(fixture())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn the_binary_accepts_every_limit_flag_and_refuses_zero_or_unparsable_values() {
    let flags = [
        ("--max-request-bytes", "1000"),
        ("--max-pending-requests", "3"),
        ("--max-result-bytes", "2000"),
        ("--max-response-body-bytes", "3000"),
        ("--max-document-bytes", "4000"),
        ("--max-http-timeout", "5s"),
        ("--max-execution-timeout", "2m 500ms"),
    ];
    let accepted = binary(
        &flags
            .iter()
            .flat_map(|(flag, value)| [*flag, *value])
            .collect::<Vec<_>>(),
    );
    assert!(accepted.status.success());
    assert!(accepted.stdout.is_empty());
    let stderr = String::from_utf8(accepted.stderr).unwrap();
    assert!(
        stderr.lines().any(|line| line == "arazzo-mcp: file policy mode=denied, roots=0; limits max_request_bytes=1000 max_pending_requests=3 max_result_bytes=2000 max_response_body_bytes=3000 max_document_bytes=4000 max_http_timeout=5s max_execution_timeout=120s"),
        "{stderr}"
    );

    for (flag, _) in flags {
        for value in ["0", "many"] {
            let refused = binary(&[flag, value]);
            assert!(!refused.status.success(), "{flag} {value}");
            assert!(refused.stdout.is_empty(), "{flag} {value}");
            assert!(
                String::from_utf8_lossy(&refused.stderr).contains("error"),
                "{flag} {value}"
            );
        }
    }
}
