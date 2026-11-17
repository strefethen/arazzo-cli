#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "Fixture setup and assertions"
)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_mcp::protocol;
use arazzo_mcp::state::ServerState;
use serde_json::{json, Value};
use tiny_http::{Response, Server, StatusCode};

#[path = "support/eof_after_responses.rs"]
mod eof_support;
use eof_support::eof_after_responses;

const HEADER_CANARY: &str = "canary-header-7f3a";
const QUERY_CANARY: &str = "canary-query-91bd";
const USERINFO_CANARY: &str = "canary-userinfo-c2e8";
const BODY_CANARY: &str = "canary-body-5d04";
const DOCUMENT_CANARY: &str = "canary-document-a611";
const KAFKA_PASSWORD: &str = "canary-kafka-password-0e77";

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "arazzo-mcp-hygiene-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("create fixture dir");
        Self { root }
    }

    fn write(&self, name: &str, data: &str) -> String {
        let path = self.root.join(name);
        fs::write(&path, data).expect("write fixture");
        path.to_string_lossy().into_owned()
    }

    fn root(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    fn state(&self, specs: &[String]) -> ServerState {
        ServerState::load(specs, Some(vec![self.root()])).expect("load server state")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct EchoServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_echo_server(status: u16) -> EchoServer {
    let server = Server::http("127.0.0.1:0").expect("bind echo server");
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let authorization = request
                        .headers()
                        .iter()
                        .find(|header| header.field.equiv("Authorization"))
                        .map(|header| header.value.as_str().to_string())
                        .unwrap_or_default();
                    let body = json!({
                        "echo": format!(
                            "authorization: {authorization} url: {} password={BODY_CANARY}",
                            request.url()
                        ),
                        "token": BODY_CANARY,
                    });
                    let response = Response::from_string(body.to_string())
                        .with_status_code(StatusCode(status));
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    EchoServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

fn openapi(server_url: &str) -> String {
    format!(
        "openapi: 3.0.3\ninfo:\n  title: Echo\n  version: 1.0.0\nservers:\n  - url: {server_url}\npaths:\n  /echo:\n    get:\n      operationId: echo\n      responses:\n        '200':\n          description: OK\n"
    )
}

fn arazzo_echo() -> String {
    format!(
        "arazzo: 1.1.0\ninfo:\n  title: Echo\n  version: 1.0.0\nsourceDescriptions:\n  - name: echo\n    url: ./echo.openapi.yaml\n    type: openapi\nworkflows:\n  - workflowId: echo\n    steps:\n      - stepId: call\n        operationId: echo\n        parameters:\n          - name: Authorization\n            in: header\n            value: Bearer {HEADER_CANARY}\n          - name: api_key\n            in: query\n            value: {QUERY_CANARY}\n        successCriteria:\n          - condition: $statusCode == 200\n        outputs:\n          value: $response.body#/token\n    outputs:\n      value: $steps.call.outputs.value\n"
    )
}

fn frame(message: &Value) -> Vec<u8> {
    let payload = message.to_string();
    format!("Content-Length: {}\r\n\r\n{payload}", payload.len()).into_bytes()
}

fn session(call: &Value) -> Vec<u8> {
    let mut input = Vec::new();
    input.extend(frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}})));
    input.extend(frame(
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ));
    input.extend(frame(
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":call}),
    ));
    input
}

fn result_of(line: &str) -> Option<(bool, String)> {
    let response: Value = serde_json::from_str(line).ok()?;
    if response.get("id") != Some(&json!(2)) {
        return None;
    }
    let result = &response["result"];
    Some((
        result["isError"].as_bool().unwrap_or(false),
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    ))
}

fn call_tool(state: &ServerState, name: &str, arguments: Value) -> (bool, String) {
    let (reader, mut output) =
        eof_after_responses(session(&json!({"name": name, "arguments": arguments})));
    protocol::serve(reader, &mut output, state).ok();
    String::from_utf8_lossy(&output)
        .lines()
        .find_map(result_of)
        .expect("tool call response")
}

fn assert_absent(text: &str, canaries: &[&str]) {
    for canary in canaries {
        assert!(!text.contains(canary), "{canary} leaked into: {text}");
    }
}

#[test]
fn failing_step_error_text_carries_no_header_query_or_body_secret() {
    let server = start_echo_server(500);
    let fixture = Fixture::new();
    fixture.write("echo.openapi.yaml", &openapi(&server.base_url));
    let spec = fixture.write("echo.arazzo.yaml", &arazzo_echo());
    let state = fixture.state(&[spec]);

    let (is_error, text) = call_tool(&state, "run_workflow", json!({"workflow_id": "echo"}));

    assert!(is_error, "a 500 must fail the step: {text}");
    assert!(text.contains("[REDACTED]"), "{text}");
    assert_absent(&text, &[HEADER_CANARY, QUERY_CANARY, BODY_CANARY]);
}

#[test]
fn transport_error_text_carries_no_url_userinfo_or_query_secret() {
    let fixture = Fixture::new();
    fixture.write(
        "echo.openapi.yaml",
        &openapi(&format!(
            "http://operator:{USERINFO_CANARY}@127.0.0.1:1/base?api_key={QUERY_CANARY}"
        )),
    );
    let spec = fixture.write("echo.arazzo.yaml", &arazzo_echo());
    let state = fixture.state(&[spec]);

    let (is_error, text) = call_tool(
        &state,
        "run_workflow",
        json!({"workflow_id": "echo", "execution_timeout_seconds": 5}),
    );

    assert!(is_error, "a closed port must fail the step: {text}");
    assert_absent(&text, &[USERINFO_CANARY, QUERY_CANARY, HEADER_CANARY]);
}

#[test]
fn successful_run_still_returns_the_intended_output() {
    let server = start_echo_server(200);
    let fixture = Fixture::new();
    fixture.write("echo.openapi.yaml", &openapi(&server.base_url));
    let spec = fixture.write("echo.arazzo.yaml", &arazzo_echo());
    let state = fixture.state(&[spec]);

    let (is_error, text) = call_tool(&state, "run_workflow", json!({"workflow_id": "echo"}));

    assert!(!is_error, "{text}");
    let payload: Value = serde_json::from_str(&text).expect("success payload is JSON");
    assert_eq!(
        payload,
        json!({"kind": "success", "outputs": {"value": BODY_CANARY}})
    );
}

#[test]
fn malformed_arazzo_diagnostics_redact_a_sensitive_scalar() {
    let fixture = Fixture::new();
    let path = fixture.write(
        "broken.arazzo.yaml",
        &format!(
            "arazzo: 1.1.0\ninfo:\n  title: Broken\n  version: 1.0.0\nsourceDescriptions: \"api_key: {DOCUMENT_CANARY}\"\nworkflows: []\n"
        ),
    );
    let state = fixture.state(&[]);

    let (is_error, text) = call_tool(&state, "validate_spec", json!({"file_path": path}));

    assert!(!is_error, "{text}");
    let report: Value = serde_json::from_str(&text).expect("validate report");
    assert_eq!(report["valid"], json!(false), "{text}");
    assert!(text.contains("[REDACTED]"), "{text}");
    assert_absent(&text, &[DOCUMENT_CANARY]);
}

#[test]
fn malformed_openapi_failures_redact_a_sensitive_scalar() {
    let fixture = Fixture::new();
    let path = fixture.write(
        "broken.openapi.yaml",
        &format!(
            "openapi: 3.0.3\ninfo:\n  title: Broken\n  version: 1.0.0\npaths: \"api_key: {DOCUMENT_CANARY}\"\n"
        ),
    );
    let state = fixture.state(&[]);

    for tool in ["generate_workflow", "describe_openapi"] {
        let (is_error, text) = call_tool(&state, tool, json!({"file_path": path}));
        assert!(is_error, "{tool} must fail: {text}");
        assert!(text.contains("[REDACTED]"), "{tool}: {text}");
        assert_absent(&text, &[DOCUMENT_CANARY]);
    }
}

const SASL_ASYNCAPI: &str = "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: 127.0.0.1:1\n    protocol: kafka\n    security:\n      - $ref: '#/components/securitySchemes/scram'\nchannels:\n  orderPlaced:\n    address: orders.placed\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\ncomponents:\n  securitySchemes:\n    scram:\n      type: scramSha256\n";

const RECEIVE_ARAZZO: &str = "arazzo: 1.1.0\ninfo:\n  title: Receive\n  version: 1.0.0\nsourceDescriptions:\n  - name: orders\n    url: ./orders.asyncapi.yaml\n    type: asyncapi\nworkflows:\n  - workflowId: await-order\n    steps:\n      - stepId: await\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n";

struct ServerRun {
    is_error: bool,
    text: String,
    stderr: String,
}

fn run_receive_in_server_process(fixture: &Fixture, spec: &str, credentials: bool) -> ServerRun {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-mcp"));
    command
        .arg(spec)
        .arg("--allowed-dir")
        .arg(fixture.root())
        .current_dir(Path::new(&fixture.root()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("ARAZZO_KAFKA_USERNAME")
        .env_remove("ARAZZO_KAFKA_PASSWORD");
    if credentials {
        command
            .env("ARAZZO_KAFKA_USERNAME", "operator")
            .env("ARAZZO_KAFKA_PASSWORD", KAFKA_PASSWORD);
    }
    let mut child = command.spawn().expect("spawn arazzo-mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(&session(&json!({"name": "run_workflow", "arguments": {
            "workflow_id": "await-order",
            "execution_timeout_seconds": 1,
        }})))
        .expect("write session");
    stdin.flush().expect("flush session");
    let stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let (is_error, text) = stdout
        .lines()
        .map_while(Result::ok)
        .find_map(|line| result_of(&line))
        .expect("run_workflow response");
    drop(stdin);
    let output = child.wait_with_output().expect("server exit");
    ServerRun {
        is_error,
        text,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn kafka_receive_uses_environment_credentials_without_echoing_the_password() {
    let fixture = Fixture::new();
    fixture.write("orders.asyncapi.yaml", SASL_ASYNCAPI);
    let spec = fixture.write("receive.arazzo.yaml", RECEIVE_ARAZZO);

    let without = run_receive_in_server_process(&fixture, &spec, false);
    assert!(without.is_error, "{}", without.text);
    assert!(
        without.text.contains("RUNTIME_MESSAGE_TRANSPORT_CONFIG")
            && without.text.contains("requires SASL"),
        "without credentials the runtime refuses before connecting: {}",
        without.text
    );

    let with = run_receive_in_server_process(&fixture, &spec, true);
    assert!(with.is_error, "{}", with.text);
    let failure: Value = serde_json::from_str(&with.text).expect("run failure is JSON");
    assert!(
        matches!(
            failure["code"].as_str(),
            Some("RUNTIME_EXECUTION_TIMEOUT" | "RUNTIME_MESSAGE_TRANSPORT")
        ) && !with.text.contains("SASL"),
        "with credentials the run fails connecting, past the credential check: {}",
        with.text
    );
    assert_absent(&with.text, &[KAFKA_PASSWORD]);
    assert_absent(&with.stderr, &[KAFKA_PASSWORD]);
}
