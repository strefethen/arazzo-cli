//! How each execution mode records a step attempt: the AfterStep execution
//! event, the observer's callbacks, and the trace record.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    Engine, EngineBuilder, EngineEvent, ExecutionEventKind, ExecutionObserver, ExecutionResult,
    ObserverEvent, RuntimeErrorKind, TraceHook, TraceStepRecord,
};
use arazzo_spec::{
    ActionType, OnAction, ParamLocation, Parameter, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::{
    find_event_pos, logged_requests, make_spec_with_base, new_request_log, record_request,
    start_server, start_server_concurrent, success_200, MockHttpResponse, RequestLog, TestObserver,
    TestServer, TestTraceHook,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

type Outputs = BTreeMap<String, Value>;

/// Keeps every observer callback with its payload. `common::TestObserver`
/// reduces each event to a tag and drops its outputs.
#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<ObserverEvent>>,
}

impl ExecutionObserver for RecordingObserver {
    fn on_event(&self, event: &ObserverEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.clone());
    }
}

impl RecordingObserver {
    /// Outputs of each `StepCompleted` callback for `step_id`, in order.
    fn step_completed_outputs(&self, step_id: &str) -> Vec<Outputs> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(|event| match event {
                ObserverEvent::StepCompleted {
                    step_id: id,
                    outputs,
                    ..
                } if id == step_id => Some(outputs.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Outputs of each AfterStep execution event for `step_id`, in stream order.
fn after_step_outputs(result: &ExecutionResult, step_id: &str) -> Vec<Outputs> {
    result
        .execution_events()
        .into_iter()
        .filter(|event| event.kind == ExecutionEventKind::AfterStep && event.step_id == step_id)
        .map(|event| event.outputs.clone())
        .collect()
}

/// Trace records for `step_id`, in stream order.
fn trace_records<'a>(result: &'a ExecutionResult, step_id: &str) -> Vec<&'a TraceStepRecord> {
    result
        .trace_steps()
        .into_iter()
        .filter(|record| record.step_id == step_id)
        .collect()
}

fn build_engine(builder: EngineBuilder) -> Engine {
    match builder.build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    }
}

// ── A failed re-run of a step that succeeded earlier ───────────────

const RERUN_WORKFLOW: &str = "rerun";

/// `[a, b]`: `a` outputs `tok`, and `b` reads it and goes back to `a` when
/// it fails.
fn rerun_workflow() -> Workflow {
    Workflow {
        workflow_id: RERUN_WORKFLOW.to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([("tok".to_string(), "$response.body#/tok".into())]),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                // Reading `a`'s output puts `a` in `execute_step`'s closure for `b`.
                parameters: vec![Parameter {
                    name: "tok".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: "$steps.a.outputs.tok".into(),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    name: "rerun-a".to_string(),
                    type_: Some(ActionType::Goto),
                    step_id: "a".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }
}

/// `/a` answers 200 `{"tok":"t1"}`, then 500 `{"tok":"t2"}`. `/b` answers 500.
fn start_rerun_server(requests: &RequestLog) -> TestServer {
    let requests = Arc::clone(requests);
    let a_hits = AtomicUsize::new(0);
    start_server(move |method, url, headers, body| {
        record_request(&requests, &method, &url, &headers, &body);
        match url.split('?').next().unwrap_or_default() {
            "/a" => {
                if a_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    MockHttpResponse::json(200, r#"{"tok":"t1"}"#)
                } else {
                    MockHttpResponse::json(500, r#"{"tok":"t2"}"#)
                }
            }
            "/b" => MockHttpResponse::empty(500),
            _ => MockHttpResponse::empty(404),
        }
    })
}

fn rerun_engine(server: &TestServer) -> EngineBuilder {
    EngineBuilder::new(make_spec_with_base(
        &server.base_url,
        vec![rerun_workflow()],
    ))
    .trace(true)
}

fn tok(value: &str) -> Outputs {
    BTreeMap::from([("tok".to_string(), json!(value))])
}

/// The run went `a`, `b`, `a`, with `b` still reading `t1`, and ended on
/// `a`'s failed re-run.
fn assert_rerun_path(result: &ExecutionResult, requests: &RequestLog) {
    let urls: Vec<String> = logged_requests(requests)
        .into_iter()
        .map(|request| request.url)
        .collect();
    assert_eq!(urls, ["/a", "/b?tok=t1", "/a"]);
    match &result.outputs {
        Err(err) => assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed),
        Ok(outputs) => panic!("expected a's re-run to fail the workflow, got {outputs:?}"),
    }
}

/// Attempt 1 of `a` is traced with its outputs, and the failed re-run with
/// none, although `$steps.a.outputs` still holds attempt 1's values.
fn assert_rerun_trace_records(result: &ExecutionResult) {
    let records = trace_records(result, "a");
    let recorded: Vec<(u32, Outputs)> = records
        .iter()
        .map(|record| (record.attempt, record.outputs.clone()))
        .collect();
    assert_eq!(recorded, [(1, tok("t1")), (2, Outputs::new())]);
    let error = records[1].error.as_deref().unwrap_or_default();
    assert!(
        error.starts_with("step a: success criteria not met"),
        "attempt 2 error: {error:?}"
    );
}

#[tokio::test]
async fn execute_records_a_failed_rerun_with_its_own_empty_outputs() {
    let requests = new_request_log();
    let server = start_rerun_server(&requests);
    let observer = Arc::new(RecordingObserver::default());
    let engine = build_engine(
        rerun_engine(&server).observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>),
    );

    let result = engine
        .execute_collect(RERUN_WORKFLOW, BTreeMap::new())
        .await;

    assert_rerun_path(&result, &requests);
    assert_rerun_trace_records(&result);
    assert_eq!(
        after_step_outputs(&result, "a"),
        [tok("t1"), Outputs::new()]
    );
    assert_eq!(
        observer.step_completed_outputs("a"),
        [tok("t1"), Outputs::new()]
    );
}

#[tokio::test]
async fn execute_step_traces_a_failed_rerun_with_its_own_empty_outputs() {
    let requests = new_request_log();
    let server = start_rerun_server(&requests);
    let observer = Arc::new(RecordingObserver::default());
    let engine = build_engine(
        rerun_engine(&server).observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>),
    );

    let result = engine
        .execute_step(RERUN_WORKFLOW, "b", BTreeMap::new(), false)
        .collect()
        .await;

    assert_rerun_path(&result, &requests);
    assert_rerun_trace_records(&result);
    // `execute_step` completes each attempt as `execute` does.
    assert_eq!(
        after_step_outputs(&result, "a"),
        [tok("t1"), Outputs::new()]
    );
    assert_eq!(
        observer.step_completed_outputs("a"),
        [tok("t1"), Outputs::new()]
    );
}

// ── Observer callbacks follow the event stream ─────────────────────

const RETRY_WORKFLOW: &str = "retry";

/// `[s1, s2]`: `s1` retries a failure once. Neither step reads the other, so
/// parallel execution runs them as one level.
fn retry_workflow() -> Workflow {
    Workflow {
        workflow_id: RETRY_WORKFLOW.to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/s1".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    name: "retry-s1".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(1),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/s2".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }
}

/// `/s1` answers 503 once and 200 after, and `/s2` answers 200. Each waits
/// its `delays_ms` entry before answering.
fn start_retry_server(delays_ms: [u64; 2]) -> TestServer {
    let s1_hits = AtomicUsize::new(0);
    start_server_concurrent(move |_method, url, _headers, _body| match url.as_str() {
        "/s1" => {
            let first = s1_hits.fetch_add(1, Ordering::SeqCst) == 0;
            thread::sleep(Duration::from_millis(delays_ms[0]));
            MockHttpResponse::empty(if first { 503 } else { 200 })
        }
        "/s2" => {
            thread::sleep(Duration::from_millis(delays_ms[1]));
            MockHttpResponse::empty(200)
        }
        _ => MockHttpResponse::empty(404),
    })
}

/// Sequential and single-step runs have no finish order to vary.
const NO_DELAYS: [u64; 2] = [0, 0];

fn retry_engine(server: &TestServer, observer: &Arc<TestObserver>) -> EngineBuilder {
    EngineBuilder::new(make_spec_with_base(
        &server.base_url,
        vec![retry_workflow()],
    ))
    .observer(Arc::clone(observer) as Arc<dyn ExecutionObserver>)
}

/// The stream's observer events, tagged the way `TestObserver` tags the
/// callbacks it receives.
fn stream_observer_tags(result: &ExecutionResult) -> Vec<String> {
    let tags = TestObserver::default();
    for event in &result.events {
        if let EngineEvent::Observer(event) = event {
            tags.on_event(event);
        }
    }
    tags.events()
}

/// The observer's callbacks, after checking that they are exactly the
/// stream's observer events: each one delivered once, in stream order.
fn callbacks_matching_stream(observer: &TestObserver, result: &ExecutionResult) -> Vec<String> {
    let observed = observer.events();
    assert_eq!(
        observed,
        stream_observer_tags(result),
        "observer callbacks against the stream's observer events"
    );
    observed
}

/// Observer events of `retry_workflow` in a parallel level: each step's first
/// `StepStarted` when the level starts, then everything else the steps
/// emitted, in step order, once the level finishes.
const PARALLEL_TAGS: [&str; 17] = [
    "StepStarted:s1",
    "StepStarted:s2",
    "RequestPrepared:s1:GET",
    "RequestSent:s1:GET",
    "CriterionEvaluated:s1:0:false",
    "StepCompleted:s1:false",
    "RetryScheduled:s1:1/1",
    "StepStarted:s1",
    "RequestPrepared:s1:GET",
    "RequestSent:s1:GET",
    "CriterionEvaluated:s1:0:true",
    "StepCompleted:s1:true",
    "RequestPrepared:s2:GET",
    "RequestSent:s2:GET",
    "CriterionEvaluated:s2:0:true",
    "StepCompleted:s2:true",
    "WorkflowCompleted:retry:ok",
];

/// Runs `retry_workflow` as one parallel level whose responses wait
/// `delays_ms`, and checks the observer's callbacks against the stream.
async fn assert_parallel_level_callbacks(delays_ms: [u64; 2]) {
    let server = start_retry_server(delays_ms);
    let observer = Arc::new(TestObserver::default());
    let engine = build_engine(retry_engine(&server, &observer).parallel(true));

    let result = engine
        .execute_collect(RETRY_WORKFLOW, BTreeMap::new())
        .await;

    if let Err(err) = &result.outputs {
        panic!("expected s1's retry to succeed, got: {err}");
    }
    let fallbacks = result.sequential_fallbacks();
    assert!(
        fallbacks.is_empty(),
        "the steps must run as a parallel level: {fallbacks:?}"
    );
    let observed = callbacks_matching_stream(&observer, &result);
    // The retried attempt's request follows the RetryScheduled that caused it.
    let retry = find_event_pos(&observed, "RetryScheduled:s1:1/1");
    let retried_request = observed
        .iter()
        .enumerate()
        .filter(|(_, tag)| *tag == "RequestPrepared:s1:GET")
        .nth(1)
        .map(|(pos, _)| pos);
    assert!(
        retried_request.is_some_and(|pos| retry < pos),
        "RetryScheduled:s1 must precede the second RequestPrepared:s1 in {observed:?}"
    );
    assert_eq!(observed, PARALLEL_TAGS);
}

#[tokio::test]
async fn execute_parallel_observer_follows_the_stream_when_the_retried_step_finishes_last() {
    // `s1` answers twice after 60ms each; `s2` answers after 5ms.
    assert_parallel_level_callbacks([60, 5]).await;
}

#[tokio::test]
async fn execute_parallel_observer_follows_the_stream_when_the_retried_step_finishes_first() {
    // `s1` answers twice after 5ms each; `s2` answers after 60ms.
    assert_parallel_level_callbacks([5, 60]).await;
}

#[tokio::test]
async fn execute_observer_follows_the_stream() {
    let server = start_retry_server(NO_DELAYS);
    let observer = Arc::new(TestObserver::default());
    let engine = build_engine(retry_engine(&server, &observer));

    let result = engine
        .execute_collect(RETRY_WORKFLOW, BTreeMap::new())
        .await;

    if let Err(err) = &result.outputs {
        panic!("expected s1's retry to succeed, got: {err}");
    }
    assert_eq!(
        callbacks_matching_stream(&observer, &result),
        [
            "StepStarted:s1",
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:false",
            "StepCompleted:s1:false",
            "RetryScheduled:s1:1/1",
            "StepStarted:s1",
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:true",
            "StepCompleted:s1:true",
            "StepStarted:s2",
            "RequestPrepared:s2:GET",
            "RequestSent:s2:GET",
            "CriterionEvaluated:s2:0:true",
            "StepCompleted:s2:true",
            "WorkflowCompleted:retry:ok",
        ]
    );
}

#[tokio::test]
async fn execute_step_observer_follows_the_stream() {
    let server = start_retry_server(NO_DELAYS);
    let observer = Arc::new(TestObserver::default());
    let engine = build_engine(retry_engine(&server, &observer));

    let result = engine
        .execute_step(RETRY_WORKFLOW, "s1", BTreeMap::new(), false)
        .collect()
        .await;

    if let Err(err) = &result.outputs {
        panic!("expected s1's retry to succeed, got: {err}");
    }
    // `execute_step` reports each attempt's lifecycle to the observer, as
    // `execute` does, but no workflow lifecycle: it does not run the
    // workflow to completion.
    assert_eq!(
        callbacks_matching_stream(&observer, &result),
        [
            "StepStarted:s1",
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:false",
            "StepCompleted:s1:false",
            "RetryScheduled:s1:1/1",
            "StepStarted:s1",
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:true",
            "StepCompleted:s1:true",
        ]
    );
}

// ── Sequential and parallel runs record each attempt alike ──────────

const PARITY_WORKFLOW: &str = "parity";

/// A `GET /<step_id>` step that must answer 200, retries a 503 at most
/// `retry_limit` times, and outputs the response's `ok` field.
fn retried_step(step_id: &str, retry_limit: u64) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath(format!("/{step_id}"))),
        success_criteria: success_200(),
        outputs: BTreeMap::from([("ok".to_string(), "$response.body#/ok".into())]),
        on_failure: vec![OnAction {
            name: "retry-unavailable".to_string(),
            type_: Some(ActionType::Retry),
            retry_limit: Some(retry_limit),
            criteria: vec![SuccessCriterion {
                condition: "$statusCode == 503".to_string(),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }],
        ..Step::default()
    }
}

/// Three independent steps, which parallel execution runs as one level.
fn parity_workflow() -> Workflow {
    Workflow {
        workflow_id: PARITY_WORKFLOW.to_string(),
        steps: vec![
            retried_step("s1", 2),
            retried_step("s2", 2),
            retried_step("s3", 3),
        ],
        ..Workflow::default()
    }
}

/// `/s1` answers 503 once, `/s2` never, and `/s3` twice, then 200
/// `{"ok":true}`. `/s1` answers after 60ms, `/s2` after 5ms, and `/s3` after
/// 20ms, so the attempts of a parallel level finish out of step order.
fn start_parity_server() -> TestServer {
    let hits = Mutex::new(BTreeMap::<String, usize>::new());
    start_server_concurrent(move |_method, url, _headers, _body| {
        let (delay_ms, failures) = match url.as_str() {
            "/s1" => (60, 1),
            "/s2" => (5, 0),
            "/s3" => (20, 2),
            _ => return MockHttpResponse::empty(404),
        };
        let hit = {
            let mut hits = hits.lock().unwrap_or_else(PoisonError::into_inner);
            let count = hits.entry(url).or_insert(0);
            *count += 1;
            *count
        };
        thread::sleep(Duration::from_millis(delay_ms));
        if hit <= failures {
            MockHttpResponse::empty(503)
        } else {
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        }
    })
}

/// One traced run of `parity_workflow` against a server of its own.
struct ParityRun {
    base_url: String,
    result: ExecutionResult,
}

async fn run_parity_workflow(parallel: bool) -> ParityRun {
    let server = start_parity_server();
    let engine = build_engine(
        EngineBuilder::new(make_spec_with_base(
            &server.base_url,
            vec![parity_workflow()],
        ))
        .parallel(parallel)
        .trace(true),
    );
    let result = engine
        .execute_collect(PARITY_WORKFLOW, BTreeMap::new())
        .await;
    if let Err(err) = &result.outputs {
        panic!("expected every retry to succeed (parallel: {parallel}), got: {err}");
    }
    ParityRun {
        base_url: server.base_url.clone(),
        result,
    }
}

/// A trace record's workflow, step, and attempt.
type AttemptKey = (String, String, u32);

/// The run's trace records by attempt, less what differs between two runs
/// of the same responses: `seq`, the duration, the request URL's origin
/// (each run has its own server), and the response headers, where tiny_http
/// stamps a `Date`.
fn comparable_trace_records(run: &ParityRun) -> BTreeMap<AttemptKey, TraceStepRecord> {
    let mut records = BTreeMap::new();
    for record in run.result.trace_steps() {
        let key = (
            record.workflow_id.clone(),
            record.step_id.clone(),
            record.attempt,
        );
        let mut record = record.clone();
        record.seq = 0;
        record.duration_ms = 0;
        if let Some(request) = record.request.as_mut() {
            request.url = match request.url.strip_prefix(&run.base_url) {
                Some(path) => path.to_string(),
                None => panic!(
                    "{key:?} requested {:?}, not its own server {}",
                    request.url, run.base_url
                ),
            };
        }
        if let Some(response) = record.response.as_mut() {
            response.headers.clear();
        }
        if records.insert(key.clone(), record).is_some() {
            panic!("more than one trace record for {key:?}");
        }
    }
    records
}

/// What an AfterStep event reports: status code, outputs, and error.
type Completion = (i64, Outputs, Option<String>);

/// Each step's AfterStep events, in occurrence order.
fn completions_by_step(result: &ExecutionResult) -> BTreeMap<String, Vec<Completion>> {
    let mut completions = BTreeMap::<String, Vec<Completion>>::new();
    for event in result.execution_events() {
        if event.kind == ExecutionEventKind::AfterStep {
            completions.entry(event.step_id.clone()).or_default().push((
                event.status_code,
                event.outputs.clone(),
                event.err.clone(),
            ));
        }
    }
    completions
}

#[tokio::test]
async fn sequential_and_parallel_runs_record_each_attempt_alike() {
    let sequential = run_parity_workflow(false).await;
    let parallel = run_parity_workflow(true).await;
    let fallbacks = parallel.result.sequential_fallbacks();
    assert!(
        fallbacks.is_empty(),
        "the steps must run as one parallel level: {fallbacks:?}"
    );

    // `s1` is retried once, `s2` never, and `s3` twice.
    let attempts = [
        ("s1", 1),
        ("s1", 2),
        ("s2", 1),
        ("s3", 1),
        ("s3", 2),
        ("s3", 3),
    ]
    .map(|(step, attempt)| (PARITY_WORKFLOW.to_string(), step.to_string(), attempt));
    let sequential_records = comparable_trace_records(&sequential);
    let parallel_records = comparable_trace_records(&parallel);
    assert_eq!(
        sequential_records.keys().cloned().collect::<Vec<_>>(),
        attempts
    );
    assert_eq!(
        parallel_records.keys().cloned().collect::<Vec<_>>(),
        attempts
    );
    for (key, record) in &sequential_records {
        assert_eq!(
            parallel_records.get(key),
            Some(record),
            "parallel trace record of {key:?} against the sequential one"
        );
    }

    let failed: Completion = (503, Outputs::new(), None);
    let passed: Completion = (200, BTreeMap::from([("ok".to_string(), json!(true))]), None);
    let sequential_completions = completions_by_step(&sequential.result);
    assert_eq!(
        sequential_completions,
        BTreeMap::from([
            ("s1".to_string(), vec![failed.clone(), passed.clone()]),
            ("s2".to_string(), vec![passed.clone()]),
            ("s3".to_string(), vec![failed.clone(), failed, passed]),
        ])
    );
    assert_eq!(
        completions_by_step(&parallel.result),
        sequential_completions,
        "parallel AfterStep events against the sequential ones"
    );
}

// ── `run --step` records each attempt as `execute` does ─────────────

const SINGLE_STEP_WORKFLOW: &str = "single";

/// One step, `s`, whose first attempt fails and whose retry succeeds.
fn single_step_workflow() -> Workflow {
    Workflow {
        workflow_id: SINGLE_STEP_WORKFLOW.to_string(),
        steps: vec![retried_step("s", 1)],
        ..Workflow::default()
    }
}

/// `/s` answers 503 once, then 200 `{"ok":true}`.
fn start_single_step_server() -> TestServer {
    let hits = AtomicUsize::new(0);
    start_server(move |_method, url, _headers, _body| {
        if url != "/s" {
            return MockHttpResponse::empty(404);
        }
        if hits.fetch_add(1, Ordering::SeqCst) == 0 {
            MockHttpResponse::empty(503)
        } else {
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        }
    })
}

/// One traced run of `single_step_workflow`, with what its observer and
/// trace hook received.
struct LifecycleRun {
    result: ExecutionResult,
    observer: Arc<TestObserver>,
    hook: Arc<TestTraceHook>,
}

/// Runs `single_step_workflow` against a server of its own: the whole
/// workflow through `execute`, or, with `single_step`, only its step through
/// `execute_step` without dependencies.
async fn run_single_step_workflow(single_step: bool) -> LifecycleRun {
    let server = start_single_step_server();
    let observer = Arc::new(TestObserver::default());
    let hook = Arc::new(TestTraceHook::default());
    let engine = build_engine(
        EngineBuilder::new(make_spec_with_base(
            &server.base_url,
            vec![single_step_workflow()],
        ))
        .trace(true)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .trace_hook(Arc::clone(&hook) as Arc<dyn TraceHook>),
    );
    let handle = if single_step {
        engine.execute_step(SINGLE_STEP_WORKFLOW, "s", BTreeMap::new(), true)
    } else {
        engine.execute(SINGLE_STEP_WORKFLOW, BTreeMap::new())
    };
    let result = handle.collect().await;
    if let Err(err) = &result.outputs {
        panic!("expected s's retry to succeed (single step: {single_step}), got: {err}");
    }
    LifecycleRun {
        result,
        observer,
        hook,
    }
}

/// One token per step lifecycle record, in stream order: BeforeStep and
/// AfterStep events, each trace record's attempt and decision, and
/// `RetryScheduled`.
fn lifecycle_tokens(result: &ExecutionResult) -> Vec<String> {
    result
        .events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Execution(event) => {
                let kind = match event.kind {
                    ExecutionEventKind::BeforeStep => "before",
                    ExecutionEventKind::AfterStep => "after",
                    _ => "other",
                };
                Some(format!("{kind}:{}", event.step_id))
            }
            EngineEvent::TraceStep(record) => Some(format!(
                "trace:{}#{}:{:?}",
                record.step_id, record.attempt, record.decision.path
            )),
            EngineEvent::Observer(ObserverEvent::RetryScheduled {
                step_id,
                attempt,
                max_attempts,
                ..
            }) => Some(format!("retry:{step_id}:{attempt}/{max_attempts}")),
            _ => None,
        })
        .collect()
}

/// How many before and after calls a trace hook received.
fn hook_calls(hook: &TestTraceHook) -> (usize, usize) {
    let before = hook
        .before_events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len();
    let after = hook
        .after_events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len();
    (before, after)
}

#[tokio::test]
async fn execute_step_records_each_attempt_as_execute_does() {
    let workflow = run_single_step_workflow(false).await;
    let single_step = run_single_step_workflow(true).await;

    let tokens = lifecycle_tokens(&workflow.result);
    assert_eq!(
        tokens,
        [
            "before:s",
            "after:s",
            "trace:s#1:Retry",
            "retry:s:1/1",
            "before:s",
            "after:s",
            "trace:s#2:Next",
        ]
    );
    assert_eq!(
        lifecycle_tokens(&single_step.result),
        tokens,
        "execute_step's step lifecycle against execute's"
    );

    // Only `execute` runs the workflow to completion.
    let mut tags = workflow.observer.events();
    assert_eq!(tags.pop().as_deref(), Some("WorkflowCompleted:single:ok"));
    assert_eq!(
        single_step.observer.events(),
        tags,
        "execute_step's observer callbacks against execute's"
    );

    assert_eq!(hook_calls(&single_step.hook), (2, 2));
}

/// Tags each observer callback as `TestObserver` does, and cancels the
/// invocation from inside a `StepStarted` callback, which runs while the
/// attempt is being announced, before the event enters the stream.
#[derive(Default)]
struct CancelOnStepStarted {
    token: OnceLock<CancellationToken>,
    tags: TestObserver,
}

impl ExecutionObserver for CancelOnStepStarted {
    fn on_event(&self, event: &ObserverEvent) {
        self.tags.on_event(event);
        if let (ObserverEvent::StepStarted { .. }, Some(token)) = (event, self.token.get()) {
            token.cancel();
        }
    }
}

/// One run of `single_step_workflow` cancelled while its attempt is being
/// announced, with what its observer and server received.
struct CancelledRun {
    result: ExecutionResult,
    observer: Arc<CancelOnStepStarted>,
    requests: RequestLog,
}

/// Runs `single_step_workflow` as `run_single_step_workflow` does, with an
/// observer that cancels the run when its step is announced.
///
/// The caller must use a current-thread runtime. Its spawned invocation
/// cannot run before this function first awaits, so the observer holds the
/// handle's token before the step is announced.
async fn run_cancelled_during_announcement(single_step: bool) -> CancelledRun {
    let requests = new_request_log();
    let log = Arc::clone(&requests);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let observer = Arc::new(CancelOnStepStarted::default());
    let engine = build_engine(
        EngineBuilder::new(make_spec_with_base(
            &server.base_url,
            vec![single_step_workflow()],
        ))
        .trace(true)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>),
    );
    let handle = if single_step {
        engine.execute_step(SINGLE_STEP_WORKFLOW, "s", BTreeMap::new(), true)
    } else {
        engine.execute(SINGLE_STEP_WORKFLOW, BTreeMap::new())
    };
    assert!(
        observer.token.set(handle.cancel_token().clone()).is_ok(),
        "the observer receives one token"
    );
    let result = handle.collect().await;
    CancelledRun {
        result,
        observer,
        requests,
    }
}

// The runtime flavor is load-bearing: see `run_cancelled_during_announcement`.
#[tokio::test(flavor = "current_thread")]
async fn execute_step_ends_an_attempt_cancelled_during_its_announcement_as_execute_does() {
    let workflow = run_cancelled_during_announcement(false).await;
    let single_step = run_cancelled_during_announcement(true).await;

    for (mode, run) in [("execute", &workflow), ("execute_step", &single_step)] {
        match &run.result.outputs {
            Err(err) => assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled, "{mode}"),
            Ok(outputs) => panic!("expected {mode} to end cancelled, got {outputs:?}"),
        }
        let requests = logged_requests(&run.requests);
        assert!(
            requests.is_empty(),
            "{mode} sent the announced attempt's request: {requests:?}"
        );
    }

    // The cancel cuts the attempt's lifecycle after its announcement: no
    // AfterStep, StepCompleted, or trace record follows.
    let tokens = lifecycle_tokens(&workflow.result);
    assert_eq!(tokens, ["before:s"]);
    assert_eq!(
        lifecycle_tokens(&single_step.result),
        tokens,
        "execute_step's step lifecycle against execute's"
    );
    let tags = callbacks_matching_stream(&workflow.observer.tags, &workflow.result);
    assert_eq!(tags, ["StepStarted:s"]);
    assert_eq!(
        callbacks_matching_stream(&single_step.observer.tags, &single_step.result),
        tags,
        "execute_step's observer callbacks against execute's"
    );
}
