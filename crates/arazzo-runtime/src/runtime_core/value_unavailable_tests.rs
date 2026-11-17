//! Request-side sites refuse a value that exists but cannot be read, before
//! anything is sent.
//!
//! No workflow can put such a value in front of request preparation yet: the
//! only producer is a response body, a request is prepared before its response
//! exists, and no store keeps the mark. `VarStore::unreadable_response_body`
//! stands in for that stored value, so each site below runs its real code
//! against a live server. `tests/value_unavailable.rs` covers every site a
//! workflow reaches.

use std::sync::atomic::AtomicUsize;

use super::*;

const REASON: &str = "response body is not valid UTF-8 text";

struct CountingServer {
    base_url: String,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl CountingServer {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("binding fixture server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counted, stopped) = (Arc::clone(&hits), Arc::clone(&stop));
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let _ = request.respond(tiny_http::Response::from_string("{}"));
                }
            }
        });
        Self {
            base_url,
            hits,
            stop,
            worker: Some(worker),
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for CountingServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn engine(base: &str) -> Engine {
    let spec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"unavailable", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":base, "type":"openapi"}],
        "workflows":[{"workflowId":"main", "steps":[{"stepId":"send", "operationId":"send"}]}]
    }));
    let openapi = json!({"openapi":"3.2.0", "$self":base,
        "info":{"title":"fixture", "version":"1"}, "servers":[{"url":base}],
        "paths":{"/items/{id}":{"post":{"operationId":"send"}}}});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
}

fn context() -> (ExecutionContext, mpsc::Receiver<EngineEvent>) {
    let (event_tx, event_rx) = mpsc::channel(1024);
    (
        ExecutionContext {
            event_tx,
            role: ContextRole::Invocation,
            trace_seq: AtomicU64::new(0),
            run_seq: AtomicU64::new(0),
            execution_event_seq: AtomicU64::new(0),
            step_attempts: Mutex::default(),
            cancel: CancellationToken::new(),
            is_timeout: Arc::new(AtomicBool::new(false)),
            completed_workflows: Mutex::default(),
            message_receivers: Arc::default(),
        },
        event_rx,
    )
}

fn unreadable() -> VarStore {
    let mut vars = VarStore::default();
    vars.unreadable_response_body = Some(REASON.to_owned());
    vars
}

fn send(fields: Value) -> Step {
    let mut step = json!({"stepId":"send", "operationId":"send",
        "parameters":[{"name":"id", "in":"path", "value":"7"}]});
    for (name, value) in fields.as_object().into_iter().flatten() {
        step[name] = value.clone();
    }
    parse(step)
}

fn selector(pointer: &str) -> Value {
    json!({"context":"$response.body", "selector":pointer, "type":"jsonpointer"})
}

#[tokio::test]
async fn every_request_side_site_refuses_an_unavailable_value_and_sends_nothing() {
    let server = CountingServer::start();
    let engine = engine(&server.base_url);
    let (ctx, _events) = context();
    let path = json!({"name":"id", "in":"path", "value":"7"});
    let body = |payload: Value| json!({"requestBody":{"contentType":"application/json", "payload":payload}});
    let cases = [
        (
            json!({"parameters":[{"name":"id", "in":"path", "value":"$response.body"}]}),
            "parameters[0](path:id)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"q", "in":"query", "value":"$response.body#/id"}]}),
            "parameters[1](query:q)",
            "$response.body#/id",
        ),
        (
            json!({"parameters":[path, {"name":"all", "in":"querystring", "value":"$response.body"}]}),
            "parameters[1](querystring:all)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"X-Photo", "in":"header", "value":"$response.body"}]}),
            "parameters[1](header:X-Photo)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"session", "in":"cookie", "value":"$response.body"}]}),
            "parameters[1](cookie:session)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"X-Photo", "in":"header", "value":"id-{$response.body}"}]}),
            "parameters[1](header:X-Photo)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"q", "in":"query", "value":selector("/id")}]}),
            "parameters[1](query:q)",
            "$response.body",
        ),
        (
            json!({"parameters":[path, {"name":"unplaced", "value":"$response.body"}]}),
            "parameters[1](unspecified:unplaced)",
            "$response.body",
        ),
        (
            body(json!("$response.body")),
            "requestBody.payload",
            "$response.body",
        ),
        (
            body(json!("$response.body#/id")),
            "requestBody.payload",
            "$response.body#/id",
        ),
        (
            body(json!("photo={$response.body}")),
            "requestBody.payload",
            "$response.body",
        ),
        (
            body(json!({"caption":"cat", "photos":["$response.body"]})),
            "requestBody.payload",
            "$response.body",
        ),
        (
            body(selector("/id")),
            "requestBody.payload",
            "$response.body",
        ),
        (
            json!({"requestBody":{"contentType":"application/json", "payload":{"photo":null},
                "replacements":[{"target":"/photo", "value":"$response.body"}]}}),
            "requestBody.replacements[0].value",
            "$response.body",
        ),
        (
            json!({"requestBody":{"contentType":"application/json", "payload":{"photo":null},
                "replacements":[{"target":"/photo", "value":selector("/id")}]}}),
            "requestBody.replacements[0].value",
            "$response.body",
        ),
    ];
    for (fields, location, expression) in cases {
        let step = send(fields);
        let error = match engine
            .execute_http_step(&ctx, "main", &step, &unreadable(), 0)
            .await
        {
            Ok(_) => panic!("{location}: an unavailable value must refuse the request"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE", "{location}");
        assert_eq!(
            error.message,
            format!("step \"send\": {location}: {expression}: {REASON}; request not sent"),
        );
        assert_eq!(server.hits(), 0, "{location}: the request was sent");
    }

    let readable = send(json!({"requestBody":{"contentType":"application/json",
        "payload":{"caption":"cat"}}}));
    let sent = engine
        .execute_http_step(&ctx, "main", &readable, &VarStore::default(), 0)
        .await
        .unwrap_or_else(|error| panic!("control request: {error}"));
    assert!(sent.result.success);
    assert_eq!(server.hits(), 1);
}

#[test]
fn workflow_outputs_refuse_an_unavailable_value() {
    let engine = engine("https://fixture.invalid");
    for (output, expression) in [
        (json!("$response.body"), "$response.body"),
        (json!("$response.body#/id"), "$response.body#/id"),
        (json!("photo={$response.body}"), "$response.body"),
        (selector("/id"), "$response.body"),
    ] {
        let workflow: Workflow = parse(json!({"workflowId":"main",
            "steps":[{"stepId":"send", "operationId":"send"}],
            "outputs":{"before":"literal", "photo":output}}));
        let (outputs, _warnings) = engine.build_outputs(&workflow, &unreadable());
        let error = match outputs {
            Ok(outputs) => panic!("an unavailable output must fail, got {:?}", outputs.values),
            Err(error) => error,
        };
        assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE");
        assert_eq!(
            error.message,
            format!("workflow \"main\": output \"photo\": {expression}: {REASON}"),
        );
        assert!(engine
            .build_outputs(&workflow, &VarStore::default())
            .0
            .is_ok());
    }
}

#[tokio::test]
async fn a_receive_step_refuses_an_unavailable_correlation_id_before_any_fetch() {
    let dir = std::env::temp_dir().join(format!("arazzo-value-unavailable-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("fixture dir: {error}"));
    std::fs::write(
        dir.join("orders.asyncapi.yaml"),
        "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: 127.0.0.1:9\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: orders\n    messages:\n      orderPlaced:\n        contentType: application/json\n        correlationId:\n          location: '$message.header#/id'\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n",
    )
    .unwrap_or_else(|error| panic!("fixture asyncapi: {error}"));
    let spec: ArazzoSpec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"unavailable", "version":"1"},
        "sourceDescriptions":[{"name":"orders", "url":"./orders.asyncapi.yaml", "type":"asyncapi"}],
        "workflows":[{"workflowId":"main", "steps":[{"stepId":"receive",
            "operationId":"$sourceDescriptions.orders.receiveOrderPlaced", "action":"receive",
            "correlationId":"order-{$response.body}"}]}]
    }));
    let step = spec.workflows[0].steps[0].clone();
    let engine = EngineBuilder::new(spec)
        .source_base_dir(&dir)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"));
    let (ctx, _events) = context();

    let refused = engine
        .execute_message_step(&ctx, "main", &step, &unreadable(), 0)
        .await;
    let _ = std::fs::remove_dir_all(&dir);

    let error = match refused {
        Ok(_) => panic!("an unavailable correlationId must fail the step"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE");
    assert_eq!(
        error.message,
        format!("step \"receive\": correlationId: $response.body: {REASON}"),
    );
}
