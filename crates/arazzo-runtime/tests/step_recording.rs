//! How each execution mode records a step attempt: the AfterStep execution
//! event, the observer's StepCompleted callback, and the trace record.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arazzo_runtime::{
    Engine, EngineBuilder, ExecutionEventKind, ExecutionObserver, ExecutionResult, ObserverEvent,
    RuntimeErrorKind, TraceStepRecord,
};
use arazzo_spec::{ActionType, OnAction, ParamLocation, Parameter, Step, StepTarget, Workflow};
use common::{
    logged_requests, make_spec_with_base, new_request_log, record_request, start_server,
    success_200, MockHttpResponse, RequestLog, TestServer,
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
