mod common;

use std::collections::BTreeMap;
use std::process::{Child, Command};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arazzo_runtime::{DebugController, RuntimeErrorKind, StepBreakpoint, StepCheckpoint};
use arazzo_spec::{
    ActionType, ArazzoSpec, Info, OnAction, SourceDescription, SourceType, Step, StepTarget,
    SuccessCriterion, Workflow,
};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_case(case: &str) {
    let mut child = ChildGuard(
        Command::new(
            std::env::current_exe().unwrap_or_else(|err| panic!("test executable: {err}")),
        )
        .args(["--exact", "cancellation_case_child", "--nocapture"])
        .env("ARAZZO_DEBUG_CANCEL_CASE", case)
        .spawn()
        .unwrap_or_else(|err| panic!("spawning cancellation case: {err}")),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child
            .0
            .try_wait()
            .unwrap_or_else(|err| panic!("polling cancellation case: {err}"))
        {
            assert!(status.success(), "cancellation case {case}: {status}");
            return;
        }
        assert!(Instant::now() < deadline, "cancellation case {case} hung");
        std::thread::sleep(Duration::from_millis(10));
    }
}

macro_rules! cancellation_cases {
    ($($name:ident),+ $(,)?) => {$(
        #[test]
        fn $name() {
            run_case(stringify!($name));
        }
    )+};
}

cancellation_cases!(
    step,
    criterion,
    output,
    success_action,
    success_action_criterion,
    failure_action,
    failure_action_criterion,
    success_retry_selected,
    success_retry_delay,
    failure_retry_selected,
    failure_retry_delay,
    timeout_identity,
    dry_run_output,
    cancellation_prevents_later_output,
);

#[test]
fn cancellation_case_child() {
    let Ok(case) = std::env::var("ARAZZO_DEBUG_CANCEL_CASE") else {
        return;
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap_or_else(|err| panic!("runtime: {err}"))
        .block_on(check_cancellation(&case));
}

async fn check_cancellation(case: &str) {
    let failure = case.starts_with("failure_");
    let status = if failure { 503 } else { 200 };
    let server = common::start_server(move |_, _, _, _| {
        common::MockHttpResponse::json(status, r#"{"value":42}"#)
    });
    let controller = Arc::new(DebugController::new());
    let checkpoint = match case {
        "step" => StepCheckpoint::Step,
        "criterion" | "timeout_identity" | "cancellation_prevents_later_output" => {
            StepCheckpoint::SuccessCriterion { index: 0 }
        }
        "output" | "dry_run_output" => StepCheckpoint::Output { name: "a".into() },
        "success_action" => StepCheckpoint::OnSuccessAction { index: 0 },
        "success_action_criterion" => StepCheckpoint::OnSuccessCriterion {
            action_index: 0,
            criterion_index: 0,
        },
        "failure_action" => StepCheckpoint::OnFailureAction { index: 0 },
        "failure_action_criterion" => StepCheckpoint::OnFailureCriterion {
            action_index: 0,
            criterion_index: 0,
        },
        "success_retry_selected" => StepCheckpoint::OnSuccessRetrySelected { action_index: 0 },
        "success_retry_delay" => StepCheckpoint::OnSuccessRetryDelay { action_index: 0 },
        "failure_retry_selected" => StepCheckpoint::OnFailureRetrySelected { action_index: 0 },
        "failure_retry_delay" => StepCheckpoint::OnFailureRetryDelay { action_index: 0 },
        _ => panic!("unknown case {case}"),
    };
    let mut breakpoint = StepBreakpoint::new("wf", "fetch");
    breakpoint.checkpoint = checkpoint.clone();
    let mut breakpoints = vec![breakpoint];
    if case == "cancellation_prevents_later_output" {
        breakpoints.push(StepBreakpoint::new("wf", "fetch").at_output("a"));
        breakpoints.push(StepBreakpoint::new("wf", "fetch").at_output("b"));
    }
    controller
        .set_breakpoints(breakpoints)
        .unwrap_or_else(|err| panic!("breakpoints: {err}"));
    let action = OnAction {
        name: "retry".into(),
        type_: Some(ActionType::Retry),
        retry_after: 1.0,
        retry_limit: Some(1),
        criteria: vec![SuccessCriterion {
            condition: format!("$statusCode == {status}"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let spec = ArazzoSpec {
        arazzo: "1.1.0".into(),
        info: Info {
            title: "debug cancellation".into(),
            version: "1.0.0".into(),
            ..Default::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".into(),
            url: server.base_url.clone(),
            type_: SourceType::OpenApi,
            ..Default::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".into(),
            steps: vec![Step {
                step_id: "fetch".into(),
                target: Some(StepTarget::OperationPath(
                    "{$sourceDescriptions.test.url}#/paths/~1test/get".into(),
                )),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".into(),
                    ..Default::default()
                }],
                outputs: BTreeMap::from([
                    ("a".into(), "$response.body#/value".into()),
                    ("b".into(), "$response.body#/value".into()),
                ]),
                on_success: if failure {
                    vec![]
                } else {
                    vec![action.clone()]
                },
                on_failure: if failure { vec![action] } else { vec![] },
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let engine = common::fixture_engine_builder(spec)
        .dry_run(case == "dry_run_output")
        .debug_controller(Arc::clone(&controller))
        .build()
        .unwrap_or_else(|err| panic!("engine: {err}"));
    let handle = engine.execute("wf", BTreeMap::new());
    assert!(controller
        .wait_for_stop_count(1, Duration::from_secs(2))
        .unwrap_or_else(|err| panic!("stop handshake: {err}")));
    assert_eq!(
        controller
            .stop_events()
            .unwrap_or_else(|err| panic!("stops: {err}"))[0]
            .checkpoint,
        checkpoint
    );
    if case == "timeout_identity" {
        handle.timeout_flag().store(true, Ordering::Release);
    }
    handle.cancel_token().cancel();
    if case == "cancellation_prevents_later_output" {
        controller
            .force_resume()
            .unwrap_or_else(|err| panic!("single teardown resume: {err}"));
    }
    let mut collected = Box::pin(handle.collect());
    let result = tokio::time::timeout(Duration::from_secs(1), &mut collected).await;
    let returned_without_rescue = result.is_ok();
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            controller
                .set_breakpoints(Vec::new())
                .unwrap_or_else(|err| panic!("rescue clear: {err}"));
            controller
                .force_resume()
                .unwrap_or_else(|err| panic!("rescue resume: {err}"));
            tokio::time::timeout(Duration::from_secs(2), &mut collected)
                .await
                .unwrap_or_else(|err| panic!("rescue must reap the engine: {err}"))
        }
    };
    assert!(
        returned_without_rescue,
        "cancellation did not release {checkpoint:?} without rescue"
    );
    let error = result
        .outputs
        .err()
        .unwrap_or_else(|| panic!("canceled execution must fail"));
    assert_eq!(
        error.kind,
        if case == "timeout_identity" {
            RuntimeErrorKind::ExecutionTimeout
        } else {
            RuntimeErrorKind::ExecutionCancelled
        }
    );
    assert_eq!(
        controller
            .stop_events()
            .unwrap_or_else(|err| panic!("final stops: {err}"))
            .len(),
        1,
        "canceled invocation published another stop"
    );
}
