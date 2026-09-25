//! How each execution mode records a step attempt: the AfterStep execution
//! event, the observer's callbacks, and the trace record.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    Engine, EngineBuilder, EngineEvent, ExecutionEventKind, ExecutionObserver, ExecutionResult,
    ObserverEvent, RuntimeErrorKind, TraceStepRecord,
};
use arazzo_spec::{ActionType, OnAction, ParamLocation, Parameter, Step, StepTarget, Workflow};
use common::{
    find_event_pos, logged_requests, make_spec_with_base, new_request_log, record_request,
    start_server, start_server_concurrent, success_200, MockHttpResponse, RequestLog, TestObserver,
    TestServer,
};
use serde_json::{json, Value};

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
    let engine = build_engine(rerun_engine(&server));

    let result = engine
        .execute_step(RERUN_WORKFLOW, "b", BTreeMap::new(), false)
        .collect()
        .await;

    assert_rerun_path(&result, &requests);
    // `execute_step` emits no AfterStep or StepCompleted events, so its trace
    // records are the only record of each attempt.
    assert_rerun_trace_records(&result);
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
    // `execute_step` reports no step or workflow lifecycle to the observer.
    assert_eq!(
        callbacks_matching_stream(&observer, &result),
        [
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:false",
            "RetryScheduled:s1:1/1",
            "RequestPrepared:s1:GET",
            "RequestSent:s1:GET",
            "CriterionEvaluated:s1:0:true",
        ]
    );
}
