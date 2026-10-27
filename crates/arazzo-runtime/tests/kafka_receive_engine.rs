//! Engine-level Kafka receive behaviour.
//!
//! Hermetic cases point the AsyncAPI document at `127.0.0.1:1` (nothing
//! listens) with a short connect timeout and prove the refusals, the arming
//! order, dry run, replay and debugger gates without a broker. Live cases are
//! `#[ignore]`, read `ARAZZO_KAFKA_BOOTSTRAP`, and need the harness broker:
//!
//! ```sh
//! scripts/kafka/broker.sh up
//! ARAZZO_KAFKA_BOOTSTRAP=127.0.0.1:19092 \
//!   cargo test -p arazzo-runtime --test kafka_receive_engine -- --ignored --test-threads=1
//! ```
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture steps use labelled failures.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arazzo_runtime::{
    DebugController, EngineBuilder, EngineEvent, ExecutionObserver, ExecutionResult,
    MessageTransportOptions, ObserverEvent, RuntimeErrorKind, StepBreakpoint, StepCheckpoint,
    TraceStepRecord,
};
use arazzo_spec::{parse_unvalidated_bytes, ArazzoSpec};
use common::{start_server, MockHttpResponse, TestServer};
use serde_json::{json, Value};
use tokio::sync::Notify;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arazzo-kafka-engine-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create fixture dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn asyncapi(&self, host: &str, topic: &str) {
        fs::write(
            self.dir.join("orders.asyncapi.yaml"),
            format!(
                "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: {host}\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: {topic}\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n"
            ),
        )
        .expect("write asyncapi");
    }

    fn openapi(&self, base_url: &str) {
        fs::write(
            self.dir.join("shop.openapi.yaml"),
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Shop\n  version: 1.0.0\nservers:\n  - url: {base_url}\npaths:\n  /events/{{id}}:\n    get:\n      operationId: getEvent\n      parameters:\n        - name: id\n          in: path\n          required: true\n          schema:\n            type: integer\n      responses:\n        '200':\n          description: ok\n"
            ),
        )
        .expect("write openapi");
    }

    fn arazzo(&self, workflows_yaml: &str) -> ArazzoSpec {
        let text = format!(
            "arazzo: 1.1.0\ninfo:\n  title: Kafka receive engine fixture\n  version: 1.0.0\nsourceDescriptions:\n  - name: orders\n    url: ./orders.asyncapi.yaml\n    type: asyncapi\n  - name: shop\n    url: ./shop.openapi.yaml\n    type: openapi\nworkflows:\n{workflows_yaml}"
        );
        fs::write(self.dir.join("spec.arazzo.yaml"), &text).expect("write arazzo");
        parse_unvalidated_bytes(text.as_bytes()).expect("parse arazzo")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const RECEIVE_THEN_HTTP: &str = "  - workflowId: followup\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        successCriteria:\n          - condition: $message.payload#/id > 0\n        outputs:\n          id: $message.payload#/id\n      - stepId: fetch-event\n        operationId: $sourceDescriptions.shop.getEvent\n        dependsOn: [await-order]\n        parameters:\n          - name: id\n            in: path\n            value: $steps.await-order.outputs.id\n        successCriteria:\n          - condition: $statusCode == 200\n    outputs:\n      id: $steps.await-order.outputs.id\n";

const HTTP_THEN_RECEIVE: &str = "  - workflowId: trigger\n    steps:\n      - stepId: trigger\n        operationId: $sourceDescriptions.shop.getEvent\n        parameters:\n          - name: id\n            in: path\n            value: 1\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        dependsOn: [trigger]\n        outputs:\n          id: $message.payload#/id\n    outputs:\n      id: $steps.await-order.outputs.id\n";

fn options(connect_timeout: Duration) -> MessageTransportOptions {
    MessageTransportOptions {
        connect_timeout,
        ..MessageTransportOptions::default()
    }
}

fn ok_server() -> (TestServer, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let paths = Arc::new(Mutex::new(Vec::new()));
    let (hits_handler, paths_handler) = (Arc::clone(&hits), Arc::clone(&paths));
    let server = start_server(move |_method, url, _headers, _body| {
        hits_handler.fetch_add(1, Ordering::SeqCst);
        paths_handler.lock().unwrap().push(url);
        MockHttpResponse::json(200, "{\"ok\":true}")
    });
    (server, hits, paths)
}

fn trace_records(result: &ExecutionResult) -> Vec<TraceStepRecord> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::TraceStep(record) => Some(record.clone()),
            _ => None,
        })
        .collect()
}

fn dry_run_requests(result: &ExecutionResult) -> Vec<arazzo_runtime::DryRunRequest> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::DryRunRequest(request) => Some(request.clone()),
            _ => None,
        })
        .collect()
}

// ───────────────────────── hermetic ─────────────────────────

#[tokio::test]
async fn send_correlation_and_replay_are_refused_before_any_connection() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");

    let send = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: s\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: send\n");
    let started = Instant::now();
    let err = EngineBuilder::new(send)
        .source_base_dir(fixture.path())
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine")
        .execute_collect("w", BTreeMap::new())
        .await
        .outputs
        .expect_err("send is refused");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    assert!(err.message.contains("617"), "{}", err.message);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no connection attempt"
    );

    let correlation = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: s\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        correlationId: $inputs.id\n");
    let err = EngineBuilder::new(correlation)
        .source_base_dir(fixture.path())
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine")
        .execute_collect("w", BTreeMap::new())
        .await
        .outputs
        .expect_err("correlationId is refused");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    assert!(err.message.contains("correlationId"), "{}", err.message);

    let replay = fixture.arazzo(RECEIVE_THEN_HTTP);
    let started = Instant::now();
    let err = EngineBuilder::new(replay)
        .source_base_dir(fixture.path())
        .replay_trace_steps(Vec::new())
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine")
        .execute_collect("followup", BTreeMap::new())
        .await
        .outputs
        .expect_err("replay refuses receive steps");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    assert!(err.message.contains("replay"), "{}", err.message);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no connection attempt"
    );
}

#[tokio::test]
async fn dry_run_simulates_a_message_without_connecting() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let started = Instant::now();
    let result = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .dry_run(true)
        .trace(true)
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine")
        .execute_collect("followup", BTreeMap::new())
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no connection attempt"
    );
    let requests = dry_run_requests(&result);
    let receive = requests
        .iter()
        .find(|request| request.step_id == "await-order")
        .expect("receive dry-run entry");
    assert_eq!(receive.method, "RECEIVE");
    assert_eq!(receive.url, "kafka://127.0.0.1:1/orders.placed");
    let record = trace_records(&result)
        .into_iter()
        .find(|record| record.step_id == "await-order")
        .expect("receive trace record");
    let message = record.message.expect("simulated message record");
    assert!(message.simulated);
    assert_eq!(message.topic, "orders.placed");
    assert_eq!(record.kind, "asyncapi");
    // `$message.payload#/id` on the simulated `{}` is null; the outputs are
    // still evaluated and the HTTP step still gets a simulated response.
    let outputs = result.outputs.expect("dry run completes");
    assert_eq!(outputs.get("id"), Some(&Value::Null));
}

#[tokio::test]
async fn parallel_mode_routes_receive_steps_through_the_message_executor() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    // Two independent steps so parallel execution is not a sequential fallback.
    let spec = fixture.arazzo("  - workflowId: both\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n      - stepId: ping\n        operationId: $sourceDescriptions.shop.getEvent\n        parameters:\n          - name: id\n            in: path\n            value: 1\n");
    let result = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .parallel(true)
        .dry_run(true)
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine")
        .execute_collect("both", BTreeMap::new())
        .await;
    assert!(
        result
            .events
            .iter()
            .all(|event| !matches!(event, EngineEvent::SequentialFallback(_))),
        "parallel execution expected"
    );
    let methods: Vec<String> = dry_run_requests(&result)
        .into_iter()
        .map(|request| request.method)
        .collect();
    assert!(methods.contains(&"RECEIVE".to_string()), "{methods:?}");
    assert!(methods.contains(&"GET".to_string()), "{methods:?}");
}

#[tokio::test]
async fn arming_fails_the_run_before_any_http_request_and_covers_the_step_closure() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    let (server, hits, _) = ok_server();
    fixture.openapi(&server.base_url);
    let spec = fixture.arazzo(HTTP_THEN_RECEIVE);
    let engine = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .message_transport(options(Duration::from_millis(300)))
        .build()
        .expect("engine");

    let started = Instant::now();
    let err = engine
        .execute_collect("trigger", BTreeMap::new())
        .await
        .outputs
        .expect_err("unreachable broker fails the run");
    assert_eq!(err.kind, RuntimeErrorKind::MessageTransport);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "no HTTP request before arming"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // --step on the HTTP step whose dependency is the receive step arms the
    // dependency first and therefore fails the same way.
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let engine = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .message_transport(options(Duration::from_millis(300)))
        .build()
        .expect("engine");
    let err = engine
        .execute_step("followup", "fetch-event", BTreeMap::new(), false)
        .collect()
        .await
        .outputs
        .expect_err("closure arming fails");
    assert_eq!(err.kind, RuntimeErrorKind::MessageTransport);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unsupported_later_target_is_refused_before_the_first_one_connects() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: first\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n      - stepId: second\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: send\n");
    let started = Instant::now();
    let err = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .message_transport(options(Duration::from_secs(5)))
        .build()
        .expect("engine")
        .execute_collect("w", BTreeMap::new())
        .await
        .outputs
        .expect_err("send refused");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "refusal must precede the first connection attempt: {:?}",
        started.elapsed()
    );
}

// The controller's waits block the calling thread, so the engine needs a
// worker of its own. Every check is collected before the controller is
// released: a panic while the engine is parked at a gate would otherwise keep
// the runtime from shutting down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debugger_evaluates_message_expressions_at_a_receive_checkpoint() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let controller = Arc::new(DebugController::new());
    controller
        .set_breakpoints(vec![StepBreakpoint::new("followup", "await-order")])
        .expect("breakpoint");
    let engine = EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .dry_run(true)
        .debug_controller(Arc::clone(&controller))
        .message_transport(options(Duration::from_secs(10)))
        .build()
        .expect("engine");
    let handle = engine.execute("followup", BTreeMap::new());

    let checks: Result<(), String> = (|| {
        if !controller
            .wait_for_stop_count(1, Duration::from_secs(5))
            .map_err(|e| e.to_string())?
        {
            return Err("not paused at the receive step".into());
        }
        // Before the message exists `$message.payload` is unresolved.
        let before = controller.evaluate_expression("$message.payload");
        if !(matches!(&before, Ok(Value::Null)) || before.is_err()) {
            return Err(format!("before receipt: {before:?}"));
        }
        controller.step_over().map_err(|e| e.to_string())?;
        if !controller
            .wait_for_stop_count(2, Duration::from_secs(5))
            .map_err(|e| e.to_string())?
        {
            return Err("not paused at the next checkpoint".into());
        }
        // Dry run evaluates outputs, not criteria, so the next checkpoint is
        // the first output; the simulated message is visible there.
        let checkpoint = controller
            .stop_events()
            .map_err(|e| e.to_string())?
            .last()
            .map(|event| event.checkpoint.clone());
        if checkpoint
            != Some(StepCheckpoint::Output {
                name: "id".to_string(),
            })
        {
            return Err(format!("unexpected checkpoint {checkpoint:?}"));
        }
        let payload = controller
            .evaluate_expression("$message.payload")
            .map_err(|e| e.to_string())?;
        if payload != json!({}) {
            return Err(format!("simulated payload not visible: {payload}"));
        }
        Ok(())
    })();

    let _ = controller.continue_execution();
    let _ = handle.collect().await;
    checks.expect("debugger sees the message snapshot");
}

// ───────────────────────── live ─────────────────────────

fn bootstrap() -> String {
    std::env::var("ARAZZO_KAFKA_BOOTSTRAP")
        .expect("ARAZZO_KAFKA_BOOTSTRAP must be set (scripts/kafka/broker.sh up)")
}

fn broker_script(args: &[&str]) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/kafka/broker.sh");
    let output = Command::new(script)
        .args(args)
        .output()
        .expect("run broker.sh");
    assert!(
        output.status.success(),
        "broker.sh {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn unique_topic(label: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "arazzo-engine-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

struct Producer {
    client: rskafka::client::Client,
}

impl Producer {
    async fn connect(bootstrap: &str) -> Self {
        let client = rskafka::client::ClientBuilder::new(vec![bootstrap.to_string()])
            .build()
            .await
            .expect("producer connect");
        Self { client }
    }

    /// Creates the topic and waits until every partition has a leader that
    /// answers an offset query, so arming does not race leader election.
    async fn create_topic(&self, topic: &str, partitions: i32) {
        self.client
            .controller_client()
            .unwrap()
            .create_topic(topic, partitions, 1, 10_000)
            .await
            .expect("create topic");
        let deadline = Instant::now() + Duration::from_secs(20);
        for partition in 0..partitions {
            loop {
                let ready = match self
                    .client
                    .partition_client(
                        topic,
                        partition,
                        rskafka::client::partition::UnknownTopicHandling::Retry,
                    )
                    .await
                {
                    Ok(pc) => pc
                        .get_offset(rskafka::client::partition::OffsetAt::Latest)
                        .await
                        .is_ok(),
                    Err(_) => false,
                };
                if ready {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "topic {topic} partition {partition} never became ready"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    async fn delete_topic(&self, topic: &str) {
        let _ = self
            .client
            .controller_client()
            .unwrap()
            .delete_topic(topic, 10_000)
            .await;
    }

    async fn produce(&self, topic: &str, partition: i32, value: &str) {
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        let pc = self
            .client
            .partition_client(
                topic,
                partition,
                rskafka::client::partition::UnknownTopicHandling::Error,
            )
            .await
            .expect("partition client");
        pc.produce(
            vec![rskafka::record::Record {
                key: None,
                value: Some(value.as_bytes().to_vec()),
                headers: BTreeMap::from([("source".to_string(), b"engine-test".to_vec())]),
                timestamp: rskafka::chrono::DateTime::from_timestamp_millis(now).unwrap(),
            }],
            rskafka::client::partition::Compression::NoCompression,
        )
        .await
        .expect("produce");
    }
}

/// Observer that wakes a waiter once the first receiver is armed.
struct ArmedObserver {
    armed: Arc<Notify>,
    count: AtomicUsize,
    seen: Mutex<Vec<(String, String, u32)>>,
}

impl ArmedObserver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            armed: Arc::new(Notify::new()),
            count: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl ExecutionObserver for ArmedObserver {
    fn on_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::MessageReceiverArmed {
            topic,
            host,
            partitions,
            ..
        } = event
        {
            self.seen
                .lock()
                .unwrap()
                .push((topic.clone(), host.clone(), *partitions));
            self.count.fetch_add(1, Ordering::SeqCst);
            self.armed.notify_one();
        }
    }
}

async fn wait_armed(observer: &ArmedObserver) {
    if observer.count.load(Ordering::SeqCst) == 0 {
        tokio::time::timeout(Duration::from_secs(15), observer.armed.notified())
            .await
            .expect("armed event within 15 s");
    }
}

fn live_engine(
    fixture: &Fixture,
    spec: ArazzoSpec,
    observer: Arc<ArmedObserver>,
    parallel: bool,
) -> arazzo_runtime::Engine {
    EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .trace(true)
        .parallel(parallel)
        .observer(observer)
        .message_transport(options(Duration::from_secs(15)))
        .build()
        .expect("engine")
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_receive_then_http_delivers_a_message_produced_after_arming() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("followup");
    producer.create_topic(&topic, 2).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    let (server, hits, paths) = ok_server();
    fixture.openapi(&server.base_url);
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(RECEIVE_THEN_HTTP),
        Arc::clone(&observer),
        false,
    );

    let handle = engine.execute("followup", BTreeMap::new());
    wait_armed(&observer).await;
    assert_eq!(
        observer.seen.lock().unwrap()[0],
        (topic.clone(), bootstrap.clone(), 2)
    );
    producer
        .produce(&topic, 1, r#"{"id":42,"status":"placed"}"#)
        .await;
    let result = handle.collect().await;
    let outputs = result.outputs.as_ref().expect("workflow succeeds").clone();
    assert_eq!(
        outputs.get("id"),
        Some(&json!(42)),
        "typed output preserved"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(paths.lock().unwrap()[0], "/events/42");
    let record = trace_records(&result)
        .into_iter()
        .find(|record| record.step_id == "await-order")
        .expect("receive trace record");
    let message = record.message.expect("message record");
    assert_eq!(message.topic, topic);
    assert_eq!(message.partition, 1);
    assert_eq!(
        message.headers.get("source").map(String::as_str),
        Some("engine-test")
    );
    assert!(!message.simulated);
    assert!(
        result.events.iter().any(
            |event| matches!(event, EngineEvent::ReceiverArmed(armed) if armed.topic == topic)
        ),
        "collected events carry the armed record"
    );
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_trigger_then_receive_gets_the_event_the_http_step_caused() {
    let bootstrap = bootstrap();
    let producer = Arc::new(Producer::connect(&bootstrap).await);
    let topic = unique_topic("trigger");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    // The HTTP handler signals the test, which produces the event the call
    // "caused" — before the receive step has begun.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let server = start_server(move |_m, _u, _h, _b| {
        let _ = tx.send(());
        MockHttpResponse::json(200, "{}")
    });
    fixture.openapi(&server.base_url);
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(HTTP_THEN_RECEIVE),
        Arc::clone(&observer),
        false,
    );
    let handle = engine.execute_with_timeout("trigger", BTreeMap::new(), Duration::from_secs(20));
    let produced = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let producer_task = {
        let producer = Arc::clone(&producer);
        let topic = topic.clone();
        let produced = Arc::clone(&produced);
        tokio::spawn(async move {
            let signalled = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await;
            assert!(signalled.is_ok(), "the HTTP handler never signalled");
            producer.produce(&topic, 0, r#"{"id":7}"#).await;
            produced.store(true, Ordering::SeqCst);
        })
    };
    let result = handle.collect().await;
    producer_task.await.unwrap();
    assert!(
        produced.load(Ordering::SeqCst),
        "event was produced after the HTTP call"
    );
    let records = trace_records(&result);
    let outputs = result
        .outputs
        .unwrap_or_else(|err| panic!("workflow failed: {err}; trace: {records:?}"));
    assert_eq!(outputs.get("id"), Some(&json!(7)));
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_criterion_failure_stops_before_the_dependent_http_step() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("criterion");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    let (server, hits, _) = ok_server();
    fixture.openapi(&server.base_url);
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(RECEIVE_THEN_HTTP),
        Arc::clone(&observer),
        false,
    );
    let handle = engine.execute("followup", BTreeMap::new());
    wait_armed(&observer).await;
    producer.produce(&topic, 0, r#"{"id":0}"#).await;
    let result = handle.collect().await;
    let err = result.outputs.as_ref().expect_err("criterion fails");
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed, "{err}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no dependent HTTP request");
    let record = trace_records(&result)
        .into_iter()
        .find(|record| record.step_id == "await-order")
        .expect("receive trace record");
    assert!(
        record.message.is_some(),
        "the failing message is still traced"
    );
    assert!(!record.criteria[0].result);
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_step_timeout_and_execution_timeout_are_distinct_and_leave_no_connection() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("timeouts");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    fixture.openapi("http://127.0.0.1:1");
    let baseline: usize = broker_script(&["connections"])
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();

    // Step timeout: fails the attempt with the message code.
    let with_timeout = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        timeout: 1500\n");
    let observer = ArmedObserver::new();
    let engine = live_engine(&fixture, with_timeout, observer, false);
    let started = Instant::now();
    let err = engine
        .execute_collect("w", BTreeMap::new())
        .await
        .outputs
        .expect_err("times out");
    assert_eq!(err.kind, RuntimeErrorKind::MessageReceiveTimeout, "{err}");
    assert_eq!(err.code(), "RUNTIME_MESSAGE_RECEIVE_TIMEOUT");
    assert!(
        started.elapsed() >= Duration::from_millis(1_400)
            && started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    drop(engine);

    // Execution timeout while waiting: the existing timeout outcome, and the
    // broker sees no connection from this process afterwards.
    let no_timeout = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n");
    let observer = ArmedObserver::new();
    let engine = live_engine(&fixture, no_timeout, observer, false);
    let started = Instant::now();
    let err = engine
        .execute_with_timeout("w", BTreeMap::new(), Duration::from_secs(2))
        .collect()
        .await
        .outputs
        .expect_err("execution timeout");
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionTimeout, "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now: usize = broker_script(&["connections"])
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        if now <= baseline {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "connections did not return to baseline {baseline} (now {now})"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_consecutive_parallel_and_subworkflow_receives_share_one_cursor() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("cursor");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    fixture.openapi("http://127.0.0.1:1");
    let two_receives = "  - workflowId: two\n    steps:\n      - stepId: first\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        outputs:\n          n: $message.payload#/n\n      - stepId: second\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        outputs:\n          n: $message.payload#/n\n    outputs:\n      first: $steps.first.outputs.n\n      second: $steps.second.outputs.n\n  - workflowId: parent\n    steps:\n      - stepId: first\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        outputs:\n          n: $message.payload#/n\n      - stepId: child\n        workflowId: child\n        outputs:\n          n: $outputs.n\n    outputs:\n      first: $steps.first.outputs.n\n      child: $steps.child.outputs.n\n  - workflowId: child\n    steps:\n      - stepId: receive\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        outputs:\n          n: $message.payload#/n\n    outputs:\n      n: $steps.receive.outputs.n\n";

    // Sequential: consecutive records.
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(two_receives),
        Arc::clone(&observer),
        false,
    );
    let handle = engine.execute("two", BTreeMap::new());
    wait_armed(&observer).await;
    producer.produce(&topic, 0, r#"{"n":1}"#).await;
    producer.produce(&topic, 0, r#"{"n":2}"#).await;
    let outputs = handle.collect().await.outputs.expect("two receives");
    assert_eq!(outputs.get("first"), Some(&json!(1)));
    assert_eq!(outputs.get("second"), Some(&json!(2)));

    // Parallel: two independent receive steps get two distinct records.
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(two_receives),
        Arc::clone(&observer),
        true,
    );
    let handle = engine.execute("two", BTreeMap::new());
    wait_armed(&observer).await;
    producer.produce(&topic, 0, r#"{"n":3}"#).await;
    producer.produce(&topic, 0, r#"{"n":4}"#).await;
    let outputs = handle.collect().await.outputs.expect("parallel receives");
    let mut got = vec![outputs["first"].clone(), outputs["second"].clone()];
    got.sort_by_key(|v| v.as_i64());
    assert_eq!(
        got,
        vec![json!(3), json!(4)],
        "distinct records, order unspecified"
    );

    // Sub-workflow: the child reuses the parent's cursor and gets the next record.
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(two_receives),
        Arc::clone(&observer),
        false,
    );
    let handle = engine.execute("parent", BTreeMap::new());
    wait_armed(&observer).await;
    producer.produce(&topic, 0, r#"{"n":5}"#).await;
    producer.produce(&topic, 0, r#"{"n":6}"#).await;
    let outputs = handle.collect().await.outputs.expect("parent and child");
    assert_eq!(outputs.get("first"), Some(&json!(5)));
    assert_eq!(outputs.get("child"), Some(&json!(6)));
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_malformed_payload_and_retry_after_timeout() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("malformed");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    fixture.openapi("http://127.0.0.1:1");

    let plain = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n");
    let observer = ArmedObserver::new();
    let engine = live_engine(&fixture, plain, Arc::clone(&observer), false);
    let handle = engine.execute("w", BTreeMap::new());
    wait_armed(&observer).await;
    producer.produce(&topic, 0, "not json").await;
    let err = handle.collect().await.outputs.expect_err("malformed");
    assert_eq!(err.kind, RuntimeErrorKind::MessageMalformed, "{err}");
    assert_eq!(err.code(), "RUNTIME_MESSAGE_MALFORMED");

    // A timed-out attempt retried by onFailure consumes the message produced
    // in between.
    let retry = fixture.arazzo("  - workflowId: w\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        timeout: 1500\n        onFailure:\n          - name: again\n            type: retry\n            retryAfter: 0\n            retryLimit: 1\n        outputs:\n          id: $message.payload#/id\n    outputs:\n      id: $steps.await-order.outputs.id\n");
    let observer = ArmedObserver::new();
    let engine = live_engine(&fixture, retry, Arc::clone(&observer), false);
    let handle = engine.execute("w", BTreeMap::new());
    wait_armed(&observer).await;
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    producer.produce(&topic, 0, r#"{"id":9}"#).await;
    let result = handle.collect().await;
    let attempts: Vec<u32> = trace_records(&result)
        .into_iter()
        .filter(|record| record.step_id == "await-order")
        .map(|record| record.attempt)
        .collect();
    let outputs = result.outputs.expect("retry succeeds");
    assert_eq!(outputs.get("id"), Some(&json!(9)));
    assert_eq!(
        attempts,
        vec![1, 2],
        "first attempt timed out, second received"
    );
    producer.delete_topic(&topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_single_step_execution_arms_the_receive_dependency() {
    let bootstrap = bootstrap();
    let producer = Producer::connect(&bootstrap).await;
    let topic = unique_topic("closure");
    producer.create_topic(&topic, 1).await;
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    let (server, hits, paths) = ok_server();
    fixture.openapi(&server.base_url);
    let observer = ArmedObserver::new();
    let engine = live_engine(
        &fixture,
        fixture.arazzo(RECEIVE_THEN_HTTP),
        Arc::clone(&observer),
        false,
    );
    let handle = engine.execute_step("followup", "fetch-event", BTreeMap::new(), false);
    wait_armed(&observer).await;
    producer.produce(&topic, 0, r#"{"id":11}"#).await;
    let result = handle.collect().await;
    result
        .outputs
        .expect("selected step and its receive dependency succeed");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(paths.lock().unwrap()[0], "/events/11");
    producer.delete_topic(&topic).await;
}
