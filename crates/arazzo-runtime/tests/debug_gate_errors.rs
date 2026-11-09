mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_runtime::{DebugController, RuntimeError, RuntimeErrorKind, StepBreakpoint};
use arazzo_spec::{ActionType, OnAction, Step, StepTarget, SuccessCriterion, Workflow};
use common::{MockHttpResponse, RequestLog};

const ERRING_CONDITION: &str = "1";

#[derive(Clone, Copy, Debug)]
enum Checkpoint {
    StepEntry,
    SuccessCriterion,
    Output,
}

impl Checkpoint {
    fn breakpoint(self, workflow_id: &str, step_id: &str) -> StepBreakpoint {
        let breakpoint = StepBreakpoint::new(workflow_id, step_id);
        match self {
            Self::StepEntry => breakpoint,
            Self::SuccessCriterion => breakpoint.at_success_criterion(0),
            Self::Output => breakpoint.at_output("id"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Recovery {
    Retry,
    Goto,
}

impl Recovery {
    fn on_failure(self) -> Vec<OnAction> {
        match self {
            Self::Retry => vec![OnAction {
                name: "retry-create".to_string(),
                type_: Some(ActionType::Retry),
                retry_after: 0.0,
                retry_limit: Some(2),
                ..OnAction::default()
            }],
            Self::Goto => vec![OnAction {
                name: "goto-fallback".to_string(),
                type_: Some(ActionType::Goto),
                step_id: "fallback".to_string(),
                ..OnAction::default()
            }],
        }
    }
}

fn create_step(on_failure: Vec<OnAction>) -> Step {
    Step {
        step_id: "create".to_string(),
        target: Some(StepTarget::OperationPath(
            "{$sourceDescriptions.test.url}#/paths/~1create/post".to_string(),
        )),
        success_criteria: vec![SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        }],
        outputs: BTreeMap::from([("id".to_string(), "$response.body#/id".to_string().into())]),
        on_failure,
        ..Step::default()
    }
}

fn fallback_step() -> Step {
    Step {
        step_id: "fallback".to_string(),
        target: Some(StepTarget::OperationPath(
            "{$sourceDescriptions.test.url}#/paths/~1fallback/get".to_string(),
        )),
        ..Step::default()
    }
}

fn workflow(workflow_id: &str, steps: Vec<Step>) -> Workflow {
    Workflow {
        workflow_id: workflow_id.to_string(),
        steps,
        ..Workflow::default()
    }
}

fn recording_server(status: u16) -> (common::TestServer, RequestLog) {
    let log = common::new_request_log();
    let recorder = Arc::clone(&log);
    let server = common::start_server(move |method, url, headers, body| {
        common::record_request(&recorder, &method, &url, &headers, &body);
        MockHttpResponse::json(status, r#"{"id":"created-1"}"#)
    });
    (server, log)
}

async fn run(
    server: &common::TestServer,
    workflows: Vec<Workflow>,
    breakpoint: StepBreakpoint,
) -> Result<BTreeMap<String, serde_json::Value>, RuntimeError> {
    let controller = Arc::new(DebugController::new());
    let engine =
        common::fixture_engine_builder(common::make_spec_with_base(&server.base_url, workflows))
            .debug_controller(Arc::clone(&controller))
            .build()
            .unwrap_or_else(|err| panic!("building debug engine: {err}"));
    controller
        .set_breakpoints(vec![breakpoint])
        .unwrap_or_else(|err| panic!("setting breakpoint: {err}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(
        controller
            .stop_events()
            .unwrap_or_else(|err| panic!("reading stop events: {err}"))
            .is_empty(),
        "an erring or false breakpoint condition must not stop"
    );
    result.outputs
}

fn request_urls(log: &RequestLog) -> Vec<String> {
    common::logged_requests(log)
        .into_iter()
        .map(|request| format!("{} {}", request.method, request.url))
        .collect()
}

fn expect_gate_error(
    outputs: Result<BTreeMap<String, serde_json::Value>, RuntimeError>,
    context: &str,
) -> RuntimeError {
    let err = match outputs {
        Ok(outputs) => panic!("{context}: the gate error must end the run, got {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::DebugController, "{context}");
    assert_eq!(
        err.message,
        "debug controller: invalid breakpoint condition: \"1\" in workflow: \"wf\", step: \"create\": boolean or null required (byte 0)",
        "{context}"
    );
    err
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gate_error_in_the_attempt_ends_the_run_without_retry_or_goto() {
    for checkpoint in [Checkpoint::SuccessCriterion, Checkpoint::Output] {
        for recovery in [Recovery::Retry, Recovery::Goto] {
            let context = format!("{checkpoint:?} with {recovery:?}");
            let (server, log) = recording_server(200);
            let outputs = run(
                &server,
                vec![workflow(
                    "wf",
                    vec![create_step(recovery.on_failure()), fallback_step()],
                )],
                checkpoint
                    .breakpoint("wf", "create")
                    .with_condition(ERRING_CONDITION),
            )
            .await;

            expect_gate_error(outputs, &context);
            assert_eq!(request_urls(&log), ["POST /create"], "{context}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_checkpoint_ends_the_run_with_the_same_gate_error() {
    let mut messages = Vec::new();
    for checkpoint in [
        Checkpoint::StepEntry,
        Checkpoint::SuccessCriterion,
        Checkpoint::Output,
    ] {
        let context = format!("{checkpoint:?}");
        let (server, log) = recording_server(200);
        let outputs = run(
            &server,
            vec![workflow(
                "wf",
                vec![create_step(Recovery::Retry.on_failure())],
            )],
            checkpoint
                .breakpoint("wf", "create")
                .with_condition(ERRING_CONDITION),
        )
        .await;

        messages.push(expect_gate_error(outputs, &context).message);
        let expected_requests: &[&str] = match checkpoint {
            Checkpoint::StepEntry => &[],
            Checkpoint::SuccessCriterion | Checkpoint::Output => &["POST /create"],
        };
        assert_eq!(request_urls(&log), expected_requests, "{context}");
    }
    messages.dedup();
    assert_eq!(messages.len(), 1, "{messages:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sub_workflow_gate_error_ends_the_parent_run_without_its_retry() {
    let (server, log) = recording_server(200);
    let parent = workflow(
        "parent",
        vec![Step {
            step_id: "call".to_string(),
            target: Some(StepTarget::WorkflowId("wf".to_string())),
            on_failure: Recovery::Retry.on_failure(),
            ..Step::default()
        }],
    );
    let child = workflow("wf", vec![create_step(Vec::new())]);
    let controller = Arc::new(DebugController::new());
    let engine = common::fixture_engine_builder(common::make_spec_with_base(
        &server.base_url,
        vec![parent, child],
    ))
    .debug_controller(Arc::clone(&controller))
    .build()
    .unwrap_or_else(|err| panic!("building debug engine: {err}"));
    controller
        .set_breakpoints(vec![Checkpoint::SuccessCriterion
            .breakpoint("wf", "create")
            .with_condition(ERRING_CONDITION)])
        .unwrap_or_else(|err| panic!("setting breakpoint: {err}"));

    let result = engine.execute_collect("parent", BTreeMap::new()).await;

    expect_gate_error(result.outputs, "sub-workflow");
    assert_eq!(request_urls(&log), ["POST /create"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_criterion_failure_under_a_false_breakpoint_still_routes_on_failure() {
    let (server, log) = recording_server(500);
    let outputs = run(
        &server,
        vec![workflow(
            "wf",
            vec![create_step(Recovery::Retry.on_failure())],
        )],
        Checkpoint::SuccessCriterion
            .breakpoint("wf", "create")
            .with_condition("false"),
    )
    .await;

    let err = match outputs {
        Ok(outputs) => panic!("a failing criterion must exhaust its retries, got {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RetryLimitExceeded);
    assert_eq!(
        request_urls(&log),
        ["POST /create", "POST /create", "POST /create"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_criterion_failure_under_a_false_breakpoint_still_takes_its_goto() {
    let (server, log) = recording_server(500);
    let outputs = run(
        &server,
        vec![workflow(
            "wf",
            vec![create_step(Recovery::Goto.on_failure()), fallback_step()],
        )],
        Checkpoint::SuccessCriterion
            .breakpoint("wf", "create")
            .with_condition("false"),
    )
    .await;

    if let Err(err) = outputs {
        panic!("the goto must reach the fallback step: {err}");
    }
    assert_eq!(request_urls(&log), ["POST /create", "GET /fallback"]);
}
