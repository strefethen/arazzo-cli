//! Focused execution-cancellation coverage for live HTTP waits and the
//! routing boundaries that consume their terminal result.

mod common;

use arazzo_runtime::{
    ClientConfig, Engine, EngineBuilder, EngineEvent, ExecutionEventKind, ExecutionHandle,
    ExecutionObserver, ExecutionResult, ObserverEvent, RuntimeErrorKind, TraceDecisionPath,
};
use arazzo_spec::{ActionType, OnAction, Step, StepTarget, Workflow};
use common::{make_spec_with_base, success_200, TestObserver};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

const HTTP_BUDGET: Duration = Duration::from_secs(5);
const READY_BOUND: Duration = Duration::from_secs(1);
const COMPLETION_BOUND: Duration = Duration::from_millis(900);

#[derive(Clone, Copy)]
enum StallPhase {
    Headers,
    Body,
}

struct StallServer {
    base_url: String,
    stalled_rx: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<String>>>,
    released: Arc<(Mutex<bool>, Condvar)>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StallServer {
    fn start(stall_path: &str, phase: StallPhase) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|err| panic!("binding cancellation fixture: {err}"));
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|err| panic!("configuring cancellation fixture: {err}"));
        let base_url = format!(
            "http://{}",
            listener
                .local_addr()
                .unwrap_or_else(|err| panic!("reading cancellation fixture address: {err}"))
        );
        let stall_path = stall_path.to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_thread = Arc::clone(&requests);
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let released_for_thread = Arc::clone(&released);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let (stalled_tx, stalled_rx) = mpsc::unbounded_channel();

        let handle = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stop_for_thread.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let path = stall_path.clone();
                        let request_log = Arc::clone(&requests_for_thread);
                        let release = Arc::clone(&released_for_thread);
                        let ready = stalled_tx.clone();
                        workers.push(thread::spawn(move || {
                            serve_connection(stream, &path, phase, &request_log, &release, &ready);
                        }));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            release_waiters(&released_for_thread);
            for worker in workers {
                worker
                    .join()
                    .unwrap_or_else(|_| panic!("cancellation fixture worker panicked"));
            }
        });

        Self {
            base_url,
            stalled_rx,
            requests,
            released,
            stop,
            handle: Some(handle),
        }
    }

    async fn wait_until_stalled(&mut self) -> Option<String> {
        tokio::time::timeout(READY_BOUND, self.stalled_rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// Waits until the fixture has received a request for `path`.
    async fn wait_until_requested(&self, path: &str) -> bool {
        tokio::time::timeout(READY_BOUND, async {
            while !self.request_paths().contains(&path.to_string()) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .is_ok()
    }

    fn request_paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn shutdown(&mut self) {
        release_waiters(&self.released);
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .unwrap_or_else(|_| panic!("cancellation fixture accept thread panicked"));
        }
    }
}

impl Drop for StallServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve_connection(
    mut stream: TcpStream,
    stall_path: &str,
    phase: StallPhase,
    requests: &Mutex<Vec<String>>,
    released: &(Mutex<bool>, Condvar),
    stalled_tx: &mpsc::UnboundedSender<String>,
) {
    stream
        .set_nonblocking(false)
        .unwrap_or_else(|err| panic!("configuring accepted fixture socket: {err}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap_or_else(|err| panic!("setting fixture read timeout: {err}"));
    let path = read_request_path(&mut stream);
    requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(path.clone());

    if path != stall_path {
        let status = if path == "/fail" { 500 } else { 200 };
        write_response(&mut stream, status, br#"{"ok":true}"#);
        return;
    }

    if matches!(phase, StallPhase::Body) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n{\"ok\"\r\n",
            )
            .unwrap_or_else(|err| panic!("writing initial response chunk: {err}"));
        stream
            .flush()
            .unwrap_or_else(|err| panic!("flushing initial response chunk: {err}"));
    }

    let _ = stalled_tx.send(path);
    wait_for_release(released);

    match phase {
        StallPhase::Headers => write_response(&mut stream, 200, br#"{"ok":true}"#),
        StallPhase::Body => {
            let _ = stream.write_all(b"6\r\n:true}\r\n0\r\n\r\n");
            let _ = stream.flush();
        }
    }
}

fn read_request_path(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
            Err(err) => panic!("reading fixture request: {err}"),
        }
    }
    let head = String::from_utf8_lossy(&bytes);
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string()
}

fn write_response(stream: &mut TcpStream, status: u16, body: &[u8]) {
    let reason = if status == 200 {
        "OK"
    } else {
        "Internal Server Error"
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn wait_for_release(released: &(Mutex<bool>, Condvar)) {
    let (lock, wake) = released;
    let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    while !*guard {
        guard = wake
            .wait(guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

fn release_waiters(released: &(Mutex<bool>, Condvar)) {
    let (lock, wake) = released;
    *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    wake.notify_all();
}

fn request_step(step_id: &str, path: &str) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath(path.to_string())),
        success_criteria: success_200(),
        ..Step::default()
    }
}

fn terminal_actions() -> Vec<OnAction> {
    vec![
        OnAction {
            name: "retry-with-recovery".to_string(),
            type_: Some(ActionType::Retry),
            workflow_id: "recovery".to_string(),
            retry_limit: Some(1),
            ..OnAction::default()
        },
        OnAction {
            name: "goto-later".to_string(),
            type_: Some(ActionType::Goto),
            step_id: "later".to_string(),
            ..OnAction::default()
        },
    ]
}

fn terminal_routing_spec(base_url: &str) -> arazzo_spec::ArazzoSpec {
    let mut stalled = request_step("stalled", "/stall");
    stalled.on_failure = terminal_actions();
    let mut spec = make_spec_with_base(
        base_url,
        vec![
            Workflow {
                workflow_id: "main".to_string(),
                steps: vec![stalled, request_step("later", "/later")],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "recovery".to_string(),
                steps: vec![request_step("recover", "/recovery")],
                ..Workflow::default()
            },
        ],
    );
    spec.arazzo = "1.1.0".to_string();
    spec
}

/// One level runs `stalled` and `admitted` together; `later` depends on both,
/// so it runs in the next level.
fn parallel_level_spec(base_url: &str) -> arazzo_spec::ArazzoSpec {
    let mut spec = make_spec_with_base(
        base_url,
        vec![Workflow {
            workflow_id: "parallel".to_string(),
            steps: vec![
                request_step("stalled", "/stall"),
                request_step("admitted", "/admitted"),
                Step {
                    depends_on: vec!["stalled".to_string(), "admitted".to_string()],
                    ..request_step("later", "/later")
                },
            ],
            ..Workflow::default()
        }],
    );
    // Step-level `dependsOn` is an Arazzo 1.1 field.
    spec.arazzo = "1.1.0".to_string();
    spec
}

fn engine_builder(
    spec: arazzo_spec::ArazzoSpec,
    http_budget: Duration,
    parallel: bool,
) -> EngineBuilder {
    let config = ClientConfig {
        timeout: http_budget,
        ..ClientConfig::default()
    };
    EngineBuilder::new(spec)
        .client_config(config)
        .parallel(parallel)
        .trace(true)
}

fn engine(spec: arazzo_spec::ArazzoSpec, http_budget: Duration, parallel: bool) -> Engine {
    engine_builder(spec, http_budget, parallel)
        .build()
        .unwrap_or_else(|err| panic!("building cancellation engine: {err}"))
}

/// The cancellation engine with an observer attached, for tests that check
/// what the observer received as well as what the stream carried.
fn observed_engine(spec: arazzo_spec::ArazzoSpec, parallel: bool) -> (Engine, Arc<TestObserver>) {
    let observer = Arc::new(TestObserver::default());
    let engine = engine_builder(spec, HTTP_BUDGET, parallel)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
        .unwrap_or_else(|err| panic!("building observed cancellation engine: {err}"));
    (engine, observer)
}

async fn collect_with_bound(handle: ExecutionHandle) -> Option<ExecutionResult> {
    tokio::time::timeout(COMPLETION_BOUND, handle.collect())
        .await
        .ok()
}

fn assert_terminal(result: &ExecutionResult, kind: RuntimeErrorKind) {
    assert_error_kind(result, kind);

    let failed_traces: Vec<_> = result
        .trace_steps()
        .into_iter()
        .filter(|record| record.error.is_some())
        .collect();
    assert!(!failed_traces.is_empty(), "terminal failure must be traced");
    for record in failed_traces {
        assert_eq!(record.decision.path, TraceDecisionPath::Error);
        assert!(
            record.decision.action_type.is_empty(),
            "terminal cancellation must not select an action: {:?}",
            record.decision
        );
    }
}

fn completed_within_bound(result: Option<ExecutionResult>, context: &str) -> ExecutionResult {
    match result {
        Some(result) => result,
        None => panic!("{context}"),
    }
}

fn execution_error(result: &ExecutionResult) -> &arazzo_runtime::RuntimeError {
    match &result.outputs {
        Ok(outputs) => panic!("cancelled execution unexpectedly succeeded: {outputs:?}"),
        Err(err) => err,
    }
}

fn assert_error_kind(result: &ExecutionResult, kind: RuntimeErrorKind) {
    let err = execution_error(result);
    assert_eq!(err.kind, kind);
    assert_eq!(err.kind.code(), kind.code());
}

fn assert_no_post_cancel_routing(result: &ExecutionResult) {
    assert!(result.events.iter().all(|event| {
        !matches!(
            event,
            EngineEvent::Observer(
                ObserverEvent::RetryScheduled { .. } | ObserverEvent::SubWorkflowStarted { .. }
            )
        )
    }));
}

/// A cancelled invocation did not complete: neither its event stream nor its
/// observer reports `WorkflowCompleted`.
fn assert_not_reported_completed(result: &ExecutionResult, observer: &TestObserver) {
    let streamed: Vec<_> = result
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                EngineEvent::Observer(ObserverEvent::WorkflowCompleted { .. })
            )
        })
        .collect();
    let observed: Vec<_> = observer
        .events()
        .into_iter()
        .filter(|tag| tag.starts_with("WorkflowCompleted:"))
        .collect();
    assert!(
        streamed.is_empty() && observed.is_empty(),
        "cancelled invocation reported WorkflowCompleted; stream: {streamed:?}; observer: {observed:?}"
    );
}

/// `step_id` ran once and its attempt is recorded: an AfterStep execution
/// event, a StepCompleted event in the stream and at the observer, and a
/// trace record.
fn assert_step_recorded(result: &ExecutionResult, observer: &TestObserver, step_id: &str) {
    let after_steps = result
        .execution_events()
        .into_iter()
        .filter(|event| event.kind == ExecutionEventKind::AfterStep && event.step_id == step_id)
        .count();
    let streamed_completions = result
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                EngineEvent::Observer(ObserverEvent::StepCompleted { step_id: id, .. })
                    if id == step_id
            )
        })
        .count();
    let completed_tag = format!("StepCompleted:{step_id}:");
    let observed_completions = observer
        .events()
        .iter()
        .filter(|tag| tag.starts_with(&completed_tag))
        .count();
    let traces = result
        .trace_steps()
        .into_iter()
        .filter(|record| record.step_id == step_id)
        .count();
    assert_eq!(
        (after_steps, streamed_completions, observed_completions, traces),
        (1, 1, 1, 1),
        "{step_id}: AfterStep events, streamed StepCompleted, observed StepCompleted, trace records"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_cancel_interrupts_pending_headers_via_execute() {
    let mut server = StallServer::start("/stall", StallPhase::Headers);
    let engine = engine(terminal_routing_spec(&server.base_url), HTTP_BUDGET, false);
    let handle = engine.execute("main", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("request did not reach the header stall: {stalled:?}");
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "pending header wait must cancel within the independent bound",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionCancelled);
    assert_no_post_cancel_routing(&result);
    assert_eq!(server.request_paths(), vec!["/stall"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_cancel_interrupts_pending_body_via_execute_step() {
    let mut server = StallServer::start("/stall", StallPhase::Body);
    let engine = engine(terminal_routing_spec(&server.base_url), HTTP_BUDGET, false);
    let handle = engine.execute_step("main", "stalled", BTreeMap::new(), true);

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("request did not reach the response-body stall: {stalled:?}");
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "pending body wait must cancel within the independent bound",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionCancelled);
    assert_no_post_cancel_routing(&result);
    assert_eq!(server.request_paths(), vec!["/stall"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watchdog_interrupts_pending_headers_with_timeout_classification() {
    let mut server = StallServer::start("/stall", StallPhase::Headers);
    let engine = engine(terminal_routing_spec(&server.base_url), HTTP_BUDGET, false);
    let handle = engine.execute_with_timeout("main", BTreeMap::new(), Duration::from_millis(350));

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("request did not reach the header stall: {stalled:?}");
    }
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "watchdog must stop the header wait within the independent bound",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionTimeout);
    assert_no_post_cancel_routing(&result);
    assert_eq!(server.request_paths(), vec!["/stall"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watchdog_interrupts_pending_body_with_timeout_classification() {
    let mut server = StallServer::start("/stall", StallPhase::Body);
    let engine = engine(terminal_routing_spec(&server.base_url), HTTP_BUDGET, false);
    let handle = engine.execute_with_timeout("main", BTreeMap::new(), Duration::from_millis(350));

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("request did not reach the response-body stall: {stalled:?}");
    }
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "watchdog must stop the body wait within the independent bound",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionTimeout);
    assert_no_post_cancel_routing(&result);
    assert_eq!(server.request_paths(), vec!["/stall"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_workflow_preserves_terminal_cancel_classification() {
    let mut server = StallServer::start("/nested", StallPhase::Headers);
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "outer".to_string(),
                steps: vec![
                    Step {
                        step_id: "call-inner".to_string(),
                        target: Some(StepTarget::WorkflowId("inner".to_string())),
                        ..Step::default()
                    },
                    request_step("outer-later", "/later"),
                ],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "inner".to_string(),
                steps: vec![request_step("inner-http", "/nested")],
                ..Workflow::default()
            },
        ],
    );
    let engine = engine(spec, HTTP_BUDGET, false);
    let handle = engine.execute("outer", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/nested") {
        server.shutdown();
        panic!("nested request did not reach the stall: {stalled:?}");
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(completed, "nested cancellation must complete promptly");
    assert_terminal(&result, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(server.request_paths(), vec!["/nested"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_retry_reference_preserves_terminal_cancel_classification() {
    let mut server = StallServer::start("/recovery", StallPhase::Headers);
    let mut failed = request_step("failed", "/fail");
    failed.on_failure = vec![OnAction {
        type_: Some(ActionType::Retry),
        workflow_id: "recovery".to_string(),
        retry_limit: Some(1),
        ..OnAction::default()
    }];
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "main".to_string(),
                steps: vec![failed],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "recovery".to_string(),
                steps: vec![request_step("recovery-http", "/recovery")],
                ..Workflow::default()
            },
        ],
    );
    let (engine, observer) = observed_engine(spec, false);
    let handle = engine.execute("main", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/recovery") {
        server.shutdown();
        panic!("workflow recovery request did not reach the stall: {stalled:?}");
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "workflow recovery cancellation must complete promptly",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(server.request_paths(), vec!["/fail", "/recovery"]);
    assert!(result.events.iter().all(|event| !matches!(
        event,
        EngineEvent::Observer(ObserverEvent::RetryScheduled { .. })
    )));
    assert_not_reported_completed(&result, &observer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn step_retry_reference_preserves_terminal_cancel_classification() {
    let mut server = StallServer::start("/recovery", StallPhase::Body);
    let mut failed = request_step("failed", "/fail");
    failed.on_failure = vec![OnAction {
        type_: Some(ActionType::Retry),
        step_id: "recovery-step".to_string(),
        retry_limit: Some(1),
        ..OnAction::default()
    }];
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "main".to_string(),
            steps: vec![failed, request_step("recovery-step", "/recovery")],
            ..Workflow::default()
        }],
    );
    let (engine, observer) = observed_engine(spec, false);
    let handle = engine.execute("main", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/recovery") {
        server.shutdown();
        panic!("step recovery request did not reach the stall: {stalled:?}");
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "step recovery cancellation must complete promptly",
    );
    assert_error_kind(&result, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(server.request_paths(), vec!["/fail", "/recovery"]);
    assert!(result.events.iter().all(|event| !matches!(
        event,
        EngineEvent::Observer(ObserverEvent::RetryScheduled { .. })
    )));
    assert_not_reported_completed(&result, &observer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_boundary_preserves_timeout_and_does_not_dispatch_next_level() {
    let mut server = StallServer::start("/stall", StallPhase::Headers);
    let (engine, observer) = observed_engine(parallel_level_spec(&server.base_url), true);
    let handle =
        engine.execute_with_timeout("parallel", BTreeMap::new(), Duration::from_millis(350));

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("parallel request did not reach the stall: {stalled:?}");
    }
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "parallel watchdog cancellation must complete promptly",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionTimeout);
    let paths = server.request_paths();
    assert!(paths.contains(&"/stall".to_string()));
    assert!(paths.contains(&"/admitted".to_string()));
    assert!(!paths.contains(&"/later".to_string()));
    for step_id in ["stalled", "admitted"] {
        assert_step_recorded(&result, &observer, step_id);
    }
    assert_not_reported_completed(&result, &observer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_external_cancel_records_the_level_without_completing_the_workflow() {
    let mut server = StallServer::start("/stall", StallPhase::Headers);
    let (engine, observer) = observed_engine(parallel_level_spec(&server.base_url), true);
    let handle = engine.execute("parallel", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("parallel request did not reach the stall: {stalled:?}");
    }
    // `admitted` shares the stalled step's level; wait for its request so the
    // cancel lands on a level in which a sibling step already ran.
    if !server.wait_until_requested("/admitted").await {
        server.shutdown();
        panic!(
            "the level's other request did not reach the fixture: {:?}",
            server.request_paths()
        );
    }
    handle.cancel_token().cancel();
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "parallel external cancellation must complete promptly",
    );
    assert_terminal(&result, RuntimeErrorKind::ExecutionCancelled);
    assert!(!server.request_paths().contains(&"/later".to_string()));
    for step_id in ["stalled", "admitted"] {
        assert_step_recorded(&result, &observer, step_id);
    }
    assert_not_reported_completed(&result, &observer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncancelled_chunked_response_still_succeeds() {
    let mut server = StallServer::start("/stall", StallPhase::Body);
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "success".to_string(),
            steps: vec![request_step("success-step", "/stall")],
            ..Workflow::default()
        }],
    );
    let engine = engine(spec, HTTP_BUDGET, false);
    let handle = engine.execute("success", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("success request did not reach the body fixture: {stalled:?}");
    }
    server.shutdown();
    let result = completed_within_bound(
        collect_with_bound(handle).await,
        "released response must complete promptly",
    );

    assert!(result.outputs.is_ok(), "released response should succeed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_http_timeout_still_executes_failure_action() {
    let mut server = StallServer::start("/stall", StallPhase::Headers);
    let mut step = request_step("timeout", "/stall");
    step.on_failure = vec![OnAction {
        type_: Some(ActionType::End),
        ..OnAction::default()
    }];
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "timeout-control".to_string(),
            steps: vec![step],
            ..Workflow::default()
        }],
    );
    let engine = engine(spec, Duration::from_millis(150), false);
    let handle = engine.execute("timeout-control", BTreeMap::new());

    let stalled = server.wait_until_stalled().await;
    if stalled.as_deref() != Some("/stall") {
        server.shutdown();
        panic!("timeout control did not reach the header stall: {stalled:?}");
    }
    let completed = collect_with_bound(handle).await;
    server.shutdown();

    let result = completed_within_bound(
        completed,
        "ordinary HTTP timeout must complete under the fixture bound",
    );
    let err = execution_error(&result);
    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    let trace = result.trace_steps()[0];
    assert_eq!(trace.decision.path, TraceDecisionPath::Done);
    assert_eq!(trace.decision.action_type, "end");
}

#[tokio::test]
async fn ordinary_transport_failure_still_executes_failure_action() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("reserving closed-loopback port: {err}"));
    let base_url = format!(
        "http://{}",
        listener
            .local_addr()
            .unwrap_or_else(|err| panic!("reading reserved port: {err}"))
    );
    drop(listener);

    let mut step = request_step("connection-refused", "/unreachable");
    step.on_failure = vec![OnAction {
        type_: Some(ActionType::End),
        ..OnAction::default()
    }];
    let spec = make_spec_with_base(
        &base_url,
        vec![Workflow {
            workflow_id: "transport-control".to_string(),
            steps: vec![step],
            ..Workflow::default()
        }],
    );
    let result = engine(spec, HTTP_BUDGET, false)
        .execute_collect("transport-control", BTreeMap::new())
        .await;

    let err = execution_error(&result);
    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    let trace = result.trace_steps()[0];
    assert_eq!(trace.decision.path, TraceDecisionPath::Done);
    assert_eq!(trace.decision.action_type, "end");
}
