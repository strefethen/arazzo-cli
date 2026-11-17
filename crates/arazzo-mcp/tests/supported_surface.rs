#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "Fixture setup and assertions"
)]

use std::fs;
use std::io::{self, BufReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use arazzo_mcp::{run_mcp_stdio_configured, McpLimits, McpServerConfig};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(10);
const CANARY: &str = "CANARY_outside_the_root_e560a";
const TOKEN: &str = "tok_e560a_bearer_secret";

const OPENAPI: &str = "openapi: 3.0.3\ninfo: {title: Test, version: '1'}\nservers: [{url: https://example.com}]\npaths:\n  /items:\n    get:\n      operationId: listItems\n      responses: {'200': {description: OK}}\n  /items/{itemId}:\n    get:\n      operationId: getItem\n      parameters: [{name: itemId, in: path, required: true, schema: {type: string}}]\n      responses: {'200': {description: OK}}\n";

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-mcp-surface-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("allowed")).unwrap();
        fs::create_dir_all(path.join("outside")).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }

    fn dir(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ResponseStream {
    pending: Vec<u8>,
    sent: Sender<Value>,
}

impl Write for ResponseStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buffer);
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let response = serde_json::from_slice(&line).map_err(io::Error::other)?;
            let _ = self.sent.send(response);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Session {
    input: Option<PipeWriter>,
    responses: Receiver<Value>,
    served: Receiver<Result<(), String>>,
    last_id: u64,
}

impl Session {
    fn start(specs: &[&Path], allowed_dirs: Option<Vec<String>>, limits: McpLimits) -> Self {
        let (reader, input) = io::pipe().unwrap();
        let (sent, responses) = mpsc::channel();
        let (done, served) = mpsc::channel();
        let specs: Vec<String> = specs
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        thread::spawn(move || {
            let mut output = ResponseStream {
                pending: Vec::new(),
                sent,
            };
            let config = McpServerConfig {
                allowed_dirs,
                limits,
            };
            let _ = done.send(run_mcp_stdio_configured(
                BufReader::new(reader),
                &mut output,
                &specs,
                config,
            ));
        });
        let mut session = Self {
            input: Some(input),
            responses,
            served,
            last_id: 0,
        };
        let initialize = session.request(
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "supported-surface", "version": "0.1"}}),
        );
        let handshake = session.recv();
        assert_eq!(handshake["id"], initialize);
        assert_eq!(handshake["result"]["serverInfo"]["name"], "arazzo-mcp");
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        session
    }

    fn send(&mut self, message: &Value) {
        let input = self.input.as_mut().unwrap();
        input.write_all(format!("{message}\n").as_bytes()).unwrap();
        input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        self.last_id += 1;
        let id = self.last_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn tool(&mut self, name: &str, arguments: Value) -> u64 {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    fn recv(&self) -> Value {
        self.responses.recv_timeout(WAIT).unwrap()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.tool(name, arguments);
        let response = self.recv();
        assert_eq!(response["id"], id);
        response
    }

    fn cancel(&mut self, id: u64) {
        self.send(
            &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": id, "reason": "user abort"}}),
        );
    }

    fn finish(mut self, within: Duration) -> Vec<Value> {
        self.input.take();
        self.served.recv_timeout(within).unwrap().unwrap();
        self.responses.iter().collect()
    }

    fn finish_with_input_open(mut self, within: Duration) -> Vec<Value> {
        self.served.recv_timeout(within).unwrap().unwrap();
        self.input.take();
        self.responses.iter().collect()
    }
}

fn outcome(response: &Value) -> (bool, Value) {
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().unwrap();
    (
        result["isError"].as_bool().unwrap(),
        serde_json::from_str(text).unwrap(),
    )
}

fn tool_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

struct Api {
    base_url: String,
    authorizations: Arc<Mutex<Vec<String>>>,
}

fn start_api() -> Api {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", server.server_addr());
    let authorizations = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&authorizations);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let authorization = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| header.value.as_str().to_string())
                .unwrap_or_default();
            seen.lock().unwrap().push(authorization);
            let _ = request.respond(
                tiny_http::Response::from_string(r#"{"name":"widget"}"#).with_header(
                    "Content-Type: application/json"
                        .parse::<tiny_http::Header>()
                        .unwrap_or_else(|()| panic!("content type header")),
                ),
            );
        }
    });
    Api {
        base_url,
        authorizations,
    }
}

#[test]
fn authenticated_http_workflow_delivers_the_bearer_token_and_dry_run_hides_it() {
    let api = start_api();
    let root = Root::new();
    let openapi = json!({"openapi": "3.0.3", "info": {"title": "Auth", "version": "1"},
        "servers": [{"url": api.base_url}],
        "paths": {"/item": {"get": {"operationId": "getItem",
            "responses": {"200": {"description": "OK"}}}}}});
    root.write("allowed/api.json", &openapi.to_string());
    let spec = root.write(
        "allowed/auth.arazzo.yaml",
        "arazzo: 1.1.0\ninfo: {title: Auth, version: '1'}\nsourceDescriptions: [{name: api, url: ./api.json, type: openapi}]\nworkflows:\n  - workflowId: fetch\n    inputs: {type: object, properties: {token: {type: string}}, required: [token]}\n    steps:\n      - stepId: get\n        operationId: getItem\n        parameters: [{name: Authorization, in: header, value: 'Bearer {$inputs.token}'}]\n        successCriteria: [{condition: $statusCode == 200}]\n        outputs: {name: $response.body#/name}\n    outputs: {name: $steps.get.outputs.name}\n",
    );
    let mut session = Session::start(
        &[&spec],
        Some(vec![root.dir("allowed")]),
        McpLimits::default(),
    );

    let run = session.call(
        "run_workflow",
        json!({"workflow_id": "fetch", "inputs": {"token": TOKEN}}),
    );
    let (is_error, result) = outcome(&run);
    assert!(!is_error, "{run}");
    assert_eq!(result["kind"], "success");
    assert_eq!(result["outputs"]["name"], "widget");
    assert_eq!(
        *api.authorizations.lock().unwrap(),
        [format!("Bearer {TOKEN}")]
    );

    let dry = session.call(
        "run_workflow",
        json!({"workflow_id": "fetch", "dry_run": true, "inputs": {"token": TOKEN}}),
    );
    let (is_error, result) = outcome(&dry);
    assert!(!is_error, "{dry}");
    assert_eq!(result["kind"], "dryRun");
    assert!(!dry.to_string().contains(TOKEN), "{dry}");
    assert_eq!(api.authorizations.lock().unwrap().len(), 1);
    assert!(session.finish(WAIT).is_empty());
}

const ASYNCAPI: &str = "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: HOST\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: ADDRESS\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n";

fn receive_workflow(source_url: &str, workflow_id: &str) -> String {
    format!(
        "arazzo: 1.1.0\ninfo: {{title: Receive, version: '1'}}\nsourceDescriptions: [{{name: orders, url: {source_url}, type: asyncapi}}]\nworkflows:\n  - workflowId: {workflow_id}\n    steps:\n      - stepId: await\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        outputs: {{id: $message.payload#/id}}\n    outputs: {{id: $steps.await.outputs.id}}\n"
    )
}

#[test]
fn kafka_receive_against_a_dead_broker_ends_at_the_execution_timeout() {
    let root = Root::new();
    root.write(
        "allowed/orders.asyncapi.yaml",
        &ASYNCAPI
            .replace("HOST", "127.0.0.1:1")
            .replace("ADDRESS", "orders.placed"),
    );
    let spec = root.write(
        "allowed/receive.arazzo.yaml",
        &receive_workflow("./orders.asyncapi.yaml", "await-order"),
    );
    let mut session = Session::start(
        &[&spec],
        Some(vec![root.dir("allowed")]),
        McpLimits::default(),
    );

    let started = Instant::now();
    let run = session.call(
        "run_workflow",
        json!({"workflow_id": "await-order", "execution_timeout_seconds": 1}),
    );
    assert!(started.elapsed() < Duration::from_secs(5), "{run}");
    let (is_error, result) = outcome(&run);
    assert!(is_error, "{run}");
    assert_eq!(result["code"], "RUNTIME_EXECUTION_TIMEOUT", "{run}");
    assert!(session.finish(WAIT).is_empty());
}

fn broker(args: &[&str], stdin: Option<&str>) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/kafka/broker.sh");
    let mut command = Command::new(script);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "broker.sh {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
fn kafka_receive_through_the_server_returns_the_produced_message() {
    let bootstrap = std::env::var("ARAZZO_KAFKA_BOOTSTRAP")
        .expect("ARAZZO_KAFKA_BOOTSTRAP must be set (scripts/kafka/broker.sh up)");
    let topic = broker(
        &[
            "topic",
            &format!("arazzo-mcp-surface-{}", std::process::id()),
        ],
        None,
    );
    let root = Root::new();
    root.write(
        "allowed/orders.asyncapi.yaml",
        &ASYNCAPI
            .replace("HOST", &bootstrap)
            .replace("ADDRESS", &topic),
    );
    let spec = root.write(
        "allowed/receive.arazzo.yaml",
        &receive_workflow("./orders.asyncapi.yaml", "await-order"),
    );
    let mut session = Session::start(
        &[&spec],
        Some(vec![root.dir("allowed")]),
        McpLimits::default(),
    );

    let id = session.tool(
        "run_workflow",
        json!({"workflow_id": "await-order", "execution_timeout_seconds": 30}),
    );
    let mut response = None;
    for _ in 0..10 {
        broker(&["produce", &topic], Some("source:demo\t{\"id\":42}\n"));
        if let Ok(received) = session.responses.recv_timeout(Duration::from_secs(2)) {
            response = Some(received);
            break;
        }
    }
    let response = response.expect("no response to the receive run");
    assert_eq!(response["id"], id);
    let (is_error, result) = outcome(&response);
    assert!(!is_error, "{response}");
    assert_eq!(result["outputs"]["id"], 42);
    assert!(session.finish(WAIT).is_empty());
}

#[test]
fn inspection_and_generation_tools_work_on_fixtures_inside_the_root() {
    let root = Root::new();
    let openapi = root.write("allowed/openapi.yaml", OPENAPI);
    let spec = root.write(
        "allowed/inspect.arazzo.yaml",
        "arazzo: 1.1.0\ninfo: {title: Inspect, version: '1'}\nsourceDescriptions: [{name: api, url: ./openapi.yaml, type: openapi}]\nworkflows: [{workflowId: list, steps: [{stepId: call, operationId: listItems}]}]\n",
    );
    let mut session = Session::start(&[], Some(vec![root.dir("allowed")]), McpLimits::default());

    let validated = session.call("validate_spec", json!({"file_path": spec}));
    let (is_error, result) = outcome(&validated);
    assert!(!is_error, "{validated}");
    assert_eq!(result["valid"], true);
    assert_eq!(result["workflows"], 1);

    let described = session.call("describe_openapi", json!({"file_path": openapi}));
    let (is_error, result) = outcome(&described);
    assert!(!is_error, "{described}");
    let operations: Vec<&str> = result["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|endpoint| endpoint["operation_id"].as_str().unwrap())
        .collect();
    assert!(operations.contains(&"listItems"), "{result}");
    assert!(operations.contains(&"getItem"), "{result}");

    let generated = session.call("generate_workflow", json!({"file_path": openapi}));
    let (is_error, result) = outcome(&generated);
    assert!(!is_error, "{generated}");
    let yaml = result["yaml"].as_str().unwrap();
    let parsed = arazzo_validate::parse_bytes(yaml.as_bytes()).unwrap();
    assert!(!parsed.workflows.is_empty());
    assert!(session.finish(WAIT).is_empty());
}

#[test]
fn referenced_sources_outside_the_root_are_denied_without_leaking_their_contents() {
    let root = Root::new();
    root.write(
        "outside/openapi.yaml",
        &OPENAPI.replace("title: Test", &format!("title: {CANARY}")),
    );
    root.write(
        "outside/orders.asyncapi.yaml",
        &ASYNCAPI
            .replace("HOST", "127.0.0.1:1")
            .replace("ADDRESS", CANARY),
    );
    let openapi_spec = root.write(
        "allowed/openapi.arazzo.yaml",
        "arazzo: 1.1.0\ninfo: {title: Denied, version: '1'}\nsourceDescriptions: [{name: api, url: ../outside/openapi.yaml, type: openapi}]\nworkflows: [{workflowId: denied-openapi, steps: [{stepId: call, operationId: listItems}]}]\n",
    );
    let asyncapi_spec = root.write(
        "allowed/asyncapi.arazzo.yaml",
        &receive_workflow("../outside/orders.asyncapi.yaml", "denied-asyncapi"),
    );
    let mut session = Session::start(
        &[&openapi_spec, &asyncapi_spec],
        Some(vec![root.dir("allowed")]),
        McpLimits::default(),
    );

    for (workflow_id, source) in [("denied-openapi", "api"), ("denied-asyncapi", "orders")] {
        let run = session.call("run_workflow", json!({"workflow_id": workflow_id}));
        let (is_error, result) = outcome(&run);
        assert!(is_error, "{run}");
        assert_eq!(result["code"], "MCP_FILE_DENIED", "{run}");
        assert_eq!(result["sourceDescription"], source, "{run}");
        assert!(!run.to_string().contains(CANARY), "{run}");
    }
    assert!(session.finish(WAIT).is_empty());
}

#[test]
fn oversized_requests_end_the_session_and_oversized_results_return_the_limit_error() {
    let root = Root::new();
    let openapi = root.write("allowed/openapi.yaml", OPENAPI);
    let allowed = || Some(vec![root.dir("allowed")]);

    let mut unlimited = Session::start(&[], allowed(), McpLimits::default());
    let generated = unlimited.call("generate_workflow", json!({"file_path": openapi}));
    let generated_bytes = tool_text(&generated).len();
    let validated = unlimited.call("validate_spec", json!({"file_path": openapi}));
    assert!(tool_text(&validated).len() < generated_bytes);
    assert!(unlimited.finish(WAIT).is_empty());

    let mut capped = Session::start(
        &[],
        allowed(),
        McpLimits {
            max_result_bytes: generated_bytes - 1,
            ..McpLimits::default()
        },
    );
    let over = capped.call("generate_workflow", json!({"file_path": openapi}));
    let (is_error, result) = outcome(&over);
    assert!(is_error, "{over}");
    assert_eq!(result["code"], "MCP_LIMIT_EXCEEDED");
    assert!(!over.to_string().contains("openapi:"), "{over}");
    let within = capped.call("validate_spec", json!({"file_path": openapi}));
    assert!(!outcome(&within).0, "{within}");
    assert!(capped.finish(WAIT).is_empty());

    let frame_limit = 512;
    let mut bounded = Session::start(
        &[],
        allowed(),
        McpLimits {
            max_request_bytes: frame_limit,
            ..McpLimits::default()
        },
    );
    bounded.send(&json!({"jsonrpc": "2.0", "id": 99, "method": "ping",
        "params": {"padding": "x".repeat(frame_limit)}}));
    let rejection = bounded.recv();
    assert_eq!(rejection["id"], Value::Null);
    assert_eq!(rejection["error"]["code"], -32600);
    assert!(bounded.finish_with_input_open(WAIT).is_empty());
}

struct StalledApi {
    _root: Root,
    spec: PathBuf,
    arrivals: Receiver<()>,
    release: Sender<()>,
    next_step_requests: Arc<AtomicUsize>,
}

impl StalledApi {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", server.server_addr());
        let (arrived, arrivals) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let next_step_requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&next_step_requests);
        thread::spawn(move || {
            for request in server.incoming_requests() {
                if request.url() == "/slow" {
                    let _ = arrived.send(());
                    let _ = released.recv();
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let _ = request.respond(tiny_http::Response::from_string("{}"));
            }
        });
        let root = Root::new();
        let ok = json!({"200": {"description": "OK"}});
        let openapi = json!({"openapi": "3.0.3", "info": {"title": "Stall", "version": "1"},
            "servers": [{"url": base_url}],
            "paths": {"/slow": {"get": {"operationId": "getSlow", "responses": ok}},
                      "/next": {"get": {"operationId": "getNext", "responses": ok}}}});
        root.write("allowed/api.json", &openapi.to_string());
        let spec = root.write(
            "allowed/stall.arazzo.yaml",
            "arazzo: 1.1.0\ninfo: {title: Stall, version: '1'}\nsourceDescriptions: [{name: api, url: ./api.json, type: openapi}]\nworkflows: [{workflowId: stall, steps: [{stepId: slow, operationId: getSlow}, {stepId: next, operationId: getNext}]}]\n",
        );
        Self {
            _root: root,
            spec,
            arrivals,
            release,
            next_step_requests,
        }
    }

    fn session(&self) -> Session {
        Session::start(&[&self.spec], None, McpLimits::default())
    }

    fn await_slow_step(&self) {
        self.arrivals.recv_timeout(WAIT).unwrap();
    }

    fn next_steps(&self) -> usize {
        self.next_step_requests.load(Ordering::SeqCst)
    }
}

#[test]
fn cancellation_stops_a_stalled_run_and_the_server_keeps_answering() {
    let api = StalledApi::start();
    let mut session = api.session();

    let run = session.tool("run_workflow", json!({"workflow_id": "stall"}));
    api.await_slow_step();
    session.cancel(run);
    let ping = session.request("ping", json!({}));
    assert_eq!(session.recv()["id"], ping);
    api.release.send(()).unwrap();
    let second = session.tool("run_workflow", json!({"workflow_id": "stall"}));
    api.await_slow_step();
    assert_eq!(api.next_steps(), 0);
    api.release.send(()).unwrap();
    let finished = session.recv();
    assert_eq!(finished["id"], second);
    assert!(!outcome(&finished).0, "{finished}");
    assert_eq!(api.next_steps(), 1);
    assert!(session.finish(WAIT).is_empty());
}

#[test]
fn client_disconnect_mid_run_returns_the_server_without_the_next_step() {
    let api = StalledApi::start();
    let mut session = api.session();

    session.tool("run_workflow", json!({"workflow_id": "stall"}));
    api.await_slow_step();
    assert!(session.finish(Duration::from_secs(5)).is_empty());
    api.release.send(()).unwrap();
    assert_eq!(api.next_steps(), 0);
}
