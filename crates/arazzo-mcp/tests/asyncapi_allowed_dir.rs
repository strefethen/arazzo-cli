//! An `asyncapi` source description outside `--allowed-dir` is refused by
//! `run_workflow` before an engine is built, exactly like an `openapi` one:
//! the local-source inventory the gate consumes now lists AsyncAPI files.
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture setup and assertions.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use arazzo_mcp::protocol;
use arazzo_mcp::state::ServerState;
use serde_json::{json, Value};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "arazzo-mcp-asyncapi-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("allowed")).expect("create allowed dir");
        fs::create_dir_all(root.join("outside")).expect("create outside dir");
        Self { root }
    }

    fn write(&self, rel: &str, data: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::write(&path, data).expect("write fixture");
        path
    }

    fn dir(&self, rel: &str) -> String {
        self.root.join(rel).to_string_lossy().to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove fixture dir");
    }
}

fn frame(msg: &Value) -> Vec<u8> {
    let payload = msg.to_string();
    format!("Content-Length: {}\r\n\r\n{payload}", payload.len()).into_bytes()
}

fn responses(data: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(data)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

const ASYNCAPI: &str = "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: 127.0.0.1:1\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: orders.placed\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n";

fn arazzo(source_url: &str) -> String {
    format!(
        "arazzo: 1.1.0\ninfo:\n  title: Receive fixture\n  version: 1.0.0\nsourceDescriptions:\n  - name: orders\n    url: {source_url}\n    type: asyncapi\nworkflows:\n  - workflowId: await-order\n    steps:\n      - stepId: await\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n"
    )
}

fn run_workflow_text(state: &ServerState) -> (bool, String) {
    let mut input = Vec::new();
    input.extend(frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}})));
    input.extend(frame(
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ));
    input.extend(frame(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_workflow","arguments":{"workflow_id":"await-order","dry_run":true}}})));
    let mut output = Vec::new();
    protocol::serve(Cursor::new(input), &mut output, state).ok();
    let responses = responses(&output);
    assert!(
        responses.len() >= 2,
        "expected 2 responses, got {responses:?}"
    );
    let result = &responses[1]["result"];
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (is_error, text)
}

#[test]
fn asyncapi_source_outside_the_allowed_dir_is_refused_before_the_engine_is_built() {
    let fixture = Fixture::new();
    fixture.write("outside/orders.asyncapi.yaml", ASYNCAPI);
    let spec = fixture.write(
        "allowed/spec.arazzo.yaml",
        &arazzo("../outside/orders.asyncapi.yaml"),
    );
    let state = ServerState::load(
        &[spec.to_string_lossy().to_string()],
        Some(vec![fixture.dir("allowed")]),
    )
    .expect("load server state");

    let (is_error, text) = run_workflow_text(&state);
    assert!(is_error, "run_workflow should be denied, got: {text}");
    assert!(
        text.contains("path not allowed"),
        "expected the allowed-dir denial, got: {text}"
    );
    assert!(
        text.contains("orders"),
        "expected the source name in the denial, got: {text}"
    );
}

#[test]
fn asyncapi_source_inside_the_allowed_dir_passes_the_gate() {
    let fixture = Fixture::new();
    fixture.write("allowed/orders.asyncapi.yaml", ASYNCAPI);
    let spec = fixture.write(
        "allowed/spec.arazzo.yaml",
        &arazzo("./orders.asyncapi.yaml"),
    );
    let state = ServerState::load(
        &[spec.to_string_lossy().to_string()],
        Some(vec![fixture.dir("allowed")]),
    )
    .expect("load server state");

    let (_, text) = run_workflow_text(&state);
    assert!(
        !text.contains("path not allowed"),
        "an in-root AsyncAPI source must not be denied by the gate, got: {text}"
    );
}
