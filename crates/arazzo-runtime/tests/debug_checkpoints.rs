use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    DebugController, DebugStopReason, EngineBuilder, ExecutionObserver, ObserverEvent,
    StepBreakpoint, StepCheckpoint,
};
use arazzo_spec::{
    ActionType, ArazzoSpec, Info, OnAction, SourceDescription, SourceType, Step, StepTarget,
    SuccessCriterion, Workflow,
};
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn step_over_enters_success_criteria_and_outputs_with_locals() {
    let server = start_server();
    let controller = Arc::new(DebugController::new());
    let engine = build_engine(server.base_url.clone(), Arc::clone(&controller));
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("wf", "fetch-rss")]) {
        panic!("setting breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());

    wait_for_stop(&controller, 1);
    let events = read_stop_events(&controller);
    assert_eq!(events[0].step_id, "fetch-rss");
    assert_eq!(events[0].checkpoint, StepCheckpoint::Step);
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);

    if let Err(err) = controller.step_over() {
        panic!("step_over to success criterion: {err}");
    }
    wait_for_stop(&controller, 2);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[1].checkpoint,
        StepCheckpoint::SuccessCriterion { index: 0 }
    );
    assert_eq!(events[1].reason, DebugStopReason::Step);
    let status = match controller.evaluate_expression("$statusCode") {
        Ok(value) => value,
        Err(err) => panic!("evaluating $statusCode: {err}"),
    };
    assert_eq!(status, json!(200));
    let scopes_at_criterion = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading scopes at criterion: {err}"),
    };
    assert_eq!(
        scopes_at_criterion.locals.get("criterionConditionResult"),
        Some(&json!(true))
    );
    assert!(
        scopes_at_criterion
            .locals
            .get("criterionContextValue")
            .and_then(|value| value.as_str())
            .is_some(),
        "criterion context value should be available at criteria checkpoint"
    );

    if let Err(err) = controller.step_over() {
        panic!("step_over to output checkpoint: {err}");
    }
    wait_for_stop(&controller, 3);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[2].checkpoint,
        StepCheckpoint::Output {
            name: "link_1".to_string()
        }
    );
    assert_eq!(events[2].reason, DebugStopReason::Step);

    let scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading scopes: {err}"),
    };
    assert_eq!(
        scopes.locals.get("link_1"),
        Some(&Value::String("https://example.com/one".to_string()))
    );

    let link_expr = match controller.evaluate_expression("$steps.fetch-rss.outputs.link_1") {
        Ok(value) => value,
        Err(err) => panic!("evaluating output expression: {err}"),
    };
    assert_eq!(link_expr, json!("https://example.com/one"));

    let xpath_watch = match controller.evaluate_watch_expression("//item[1]/link") {
        Ok(value) => value,
        Err(err) => panic!("evaluating xpath watch: {err}"),
    };
    assert_eq!(xpath_watch, json!("https://example.com/one"));

    let named_watch = match controller.evaluate_watch_expression("link_1") {
        Ok(value) => value,
        Err(err) => panic!("evaluating output-name watch: {err}"),
    };
    assert_eq!(named_watch, json!("https://example.com/one"));

    if let Err(err) = controller.continue_execution() {
        panic!("continuing execution: {err}");
    }
    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_failure_action_breakpoint_hits_with_failure_locals() {
    let server = start_server_with_status(502);
    let controller = Arc::new(DebugController::new());
    let engine = build_failure_engine(server.base_url.clone(), Arc::clone(&controller));
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_action(0)
    ]) {
        panic!("setting breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());

    wait_for_stop(&controller, 1);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[0].checkpoint,
        StepCheckpoint::OnFailureAction { index: 0 }
    );
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);

    let scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading scopes at onFailure action: {err}"),
    };
    assert_eq!(scopes.locals.get("actionBranch"), Some(&json!("onFailure")));
    assert_eq!(scopes.locals.get("actionType"), Some(&json!("end")));
    assert_eq!(scopes.locals.get("statusCode"), Some(&json!(502)));

    if let Err(err) = controller.continue_execution() {
        panic!("continue execution: {err}");
    }
    let result = handle.collect().await;
    match result.outputs {
        Ok(_) => panic!("expected workflow execution to fail"),
        Err(err) => {
            assert!(
                err.message.contains("workflow ended by onFailure action"),
                "unexpected error message: {}",
                err.message
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_failure_retry_selected_and_delay_checkpoints_are_debuggable() {
    let server = start_server_with_status_and_retry_after(503, 1);
    let controller = Arc::new(DebugController::new());
    let observer = Arc::new(RetryScheduledObserver::default());
    let engine = build_retry_engine(
        server.base_url.clone(),
        0.25,
        Some(1),
        Arc::clone(&controller),
        Some(Arc::clone(&observer) as Arc<dyn ExecutionObserver>),
    );
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_selected(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_delay(0),
    ]) {
        panic!("setting breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());

    wait_for_stop(&controller, 1);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[0].checkpoint,
        StepCheckpoint::OnFailureRetrySelected { action_index: 0 }
    );
    let scopes_selected = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading scopes at retry selected: {err}"),
    };
    assert_eq!(
        scopes_selected.locals.get("actionBranch"),
        Some(&json!("onFailure"))
    );
    assert_eq!(
        scopes_selected.locals.get("retryStage"),
        Some(&json!("selected"))
    );
    assert_eq!(
        scopes_selected.locals.get("retryWillExecute"),
        Some(&json!(true))
    );
    assert_eq!(
        scopes_selected.locals.get("retryAfterSeconds"),
        Some(&json!(0.25))
    );

    if let Err(err) = controller.continue_execution() {
        panic!("continuing after retry selected: {err}");
    }

    wait_for_stop(&controller, 2);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[1].checkpoint,
        StepCheckpoint::OnFailureRetryDelay { action_index: 0 }
    );
    let scopes_delay = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading scopes at retry delay: {err}"),
    };
    assert_eq!(scopes_delay.locals.get("retryStage"), Some(&json!("delay")));
    assert_eq!(
        scopes_delay.locals.get("retryAfterSeconds"),
        Some(&json!(0.25))
    );
    assert_eq!(
        scopes_delay.locals.get("retryDelaySeconds"),
        Some(&json!(1.0))
    );

    handle.cancel_token().cancel();
    if let Err(err) = controller.set_breakpoints(Vec::new()) {
        panic!("clearing breakpoints after retry delay: {err}");
    }
    if let Err(err) = controller.continue_execution() {
        panic!("continuing after retry delay: {err}");
    }
    let result = handle.collect().await;
    match result.outputs {
        Ok(_) => panic!("expected workflow execution to stop during the retry delay"),
        Err(err) => {
            assert!(
                err.message.contains("execution cancelled"),
                "unexpected error message: {}",
                err.message
            );
        }
    }
    assert_eq!(
        observer.retry_scheduled.load(Ordering::Relaxed),
        0,
        "cancellation at the delay checkpoint must not schedule a retry event"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_retry_after_is_omitted_from_generic_debug_locals_before_rejection() {
    let server = start_server_with_status(503);
    let controller = Arc::new(DebugController::new());
    let engine = build_retry_engine(
        server.base_url.clone(),
        f64::NAN,
        Some(1),
        Arc::clone(&controller),
        None,
    );
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_action(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_selected(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_delay(0),
    ]) {
        panic!("setting invalid retryAfter breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());
    wait_for_stop(&controller, 1);
    let scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading generic invalid retryAfter scopes: {err}"),
    };
    assert_eq!(scopes.locals.get("actionType"), Some(&json!("retry")));
    assert!(
        !scopes.locals.contains_key("actionRetryAfter"),
        "generic action locals must not serialize non-finite retryAfter as JSON null"
    );

    if let Err(err) = controller.continue_execution() {
        panic!("continuing invalid retryAfter action: {err}");
    }
    let result = handle.collect().await;
    match result.outputs {
        Ok(_) => panic!("matched non-finite retryAfter must fail"),
        Err(err) => assert!(
            err.message
                .contains("retryAfter must be a finite non-negative number"),
            "unexpected error message: {}",
            err.message
        ),
    }
    let events = read_stop_events(&controller);
    assert!(
        !events.iter().any(|event| matches!(
            event.checkpoint,
            StepCheckpoint::OnFailureRetrySelected { action_index: 0 }
                | StepCheckpoint::OnFailureRetryDelay { action_index: 0 }
        )),
        "invalid matched retries must fail before retry-specific debug checkpoints"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arazzo_11_exhausted_retry_has_no_delay_checkpoint_and_continues_at_later_index() {
    let server = start_server_with_status(503);
    let controller = Arc::new(DebugController::new());
    let spec = ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "debug-exhausted-retry-fallthrough".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: server.base_url.clone(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                target: Some(StepTarget::OperationPath("/rss".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                on_failure: vec![
                    OnAction {
                        name: "exhaust-first".to_string(),
                        type_: Some(ActionType::Retry),
                        retry_after: 1.0,
                        retry_limit: Some(0),
                        ..OnAction::default()
                    },
                    OnAction {
                        name: "end-second".to_string(),
                        type_: Some(ActionType::End),
                        ..OnAction::default()
                    },
                ],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        components: None,
        ..ArazzoSpec::default()
    };
    let engine = match EngineBuilder::new(spec)
        .debug_controller(Arc::clone(&controller))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating exhausted retry engine: {err}"),
    };
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_selected(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_delay(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_action(1),
    ]) {
        panic!("setting breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());
    wait_for_stop(&controller, 1);
    let selected_scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading exhausted retry scopes: {err}"),
    };
    assert_eq!(
        selected_scopes.locals.get("retryCountCurrent"),
        Some(&json!(0))
    );
    assert_eq!(
        selected_scopes.locals.get("retryLimitResolved"),
        Some(&json!(0))
    );
    assert_eq!(
        selected_scopes.locals.get("retryWillExecute"),
        Some(&json!(false))
    );

    if let Err(err) = controller.continue_execution() {
        panic!("continuing after exhausted retry selection: {err}");
    }
    wait_for_stop(&controller, 2);
    let events = read_stop_events(&controller);
    assert_eq!(
        events[0].checkpoint,
        StepCheckpoint::OnFailureRetrySelected { action_index: 0 }
    );
    assert_eq!(
        events[1].checkpoint,
        StepCheckpoint::OnFailureAction { index: 1 }
    );
    assert!(
        !events.iter().any(|event| matches!(
            event.checkpoint,
            StepCheckpoint::OnFailureRetryDelay { action_index: 0 }
        )),
        "exhaustion must not schedule a retry delay checkpoint"
    );

    if let Err(err) = controller.set_breakpoints(Vec::new()) {
        panic!("clearing exhausted-retry breakpoints: {err}");
    }
    if let Err(err) = controller.continue_execution() {
        panic!("continuing after later action: {err}");
    }
    let result = handle.collect().await;
    assert!(
        result.outputs.is_err(),
        "later end must end the failed step"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_simple_criterion_exposes_criterion_error_local() {
    let server = start_server();
    let controller = Arc::new(DebugController::new());
    let engine = build_engine_with_condition(
        server.base_url.clone(),
        Arc::clone(&controller),
        "$statusCode contains 200",
    );
    if let Err(error) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_success_criterion(0)
    ]) {
        panic!("setting criterion breakpoint: {error}");
    }

    let handle = engine.execute("wf", BTreeMap::new());
    wait_for_stop(&controller, 1);
    let scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(error) => panic!("reading invalid criterion scopes: {error}"),
    };
    assert_eq!(
        scopes.locals.get("criterionConditionResult"),
        Some(&json!(false))
    );
    let error = scopes
        .locals
        .get("criterionError")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(error.contains("invalid simple condition"), "got: {error}");
    assert!(error.contains("byte"), "got: {error}");

    if let Err(error) = controller.continue_execution() {
        panic!("continuing invalid criterion: {error}");
    }
    assert!(
        handle.collect().await.outputs.is_err(),
        "invalid criterion must fail the step"
    );
}

fn build_engine(base_url: String, controller: Arc<DebugController>) -> arazzo_runtime::Engine {
    build_engine_with_condition(base_url, controller, "$statusCode == 200")
}

fn build_engine_with_condition(
    base_url: String,
    controller: Arc<DebugController>,
    condition: &str,
) -> arazzo_runtime::Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-checkpoints".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url,
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                target: Some(StepTarget::OperationPath("/rss".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: condition.to_string(),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([(
                    "link_1".to_string(),
                    "//item[1]/link".to_string().into(),
                )]),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        components: None,
        ..ArazzoSpec::default()
    };

    match EngineBuilder::new(spec)
        .debug_controller(controller)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

fn build_failure_engine(
    base_url: String,
    controller: Arc<DebugController>,
) -> arazzo_runtime::Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-checkpoints-failure".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url,
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                target: Some(StepTarget::OperationPath("/rss".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                on_failure: vec![
                    OnAction {
                        type_: Some(ActionType::End),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 502".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: Some(ActionType::Retry),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 503".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                ],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        components: None,
        ..ArazzoSpec::default()
    };

    match EngineBuilder::new(spec)
        .debug_controller(controller)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

fn build_retry_engine(
    base_url: String,
    retry_after: f64,
    retry_limit: Option<u64>,
    controller: Arc<DebugController>,
    observer: Option<Arc<dyn ExecutionObserver>>,
) -> arazzo_runtime::Engine {
    let spec = ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "debug-checkpoints-retry".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url,
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                target: Some(StepTarget::OperationPath("/rss".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    retry_after,
                    retry_limit,
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 503".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        components: None,
        ..ArazzoSpec::default()
    };

    let mut builder = EngineBuilder::new(spec).debug_controller(controller);
    if let Some(observer) = observer {
        builder = builder.observer(observer);
    }
    match builder.build() {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

#[derive(Debug)]
struct TestServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn start_server() -> TestServer {
    start_server_with_status(200)
}

fn start_server_with_status(status: u16) -> TestServer {
    start_server_with_status_and_optional_retry_after(status, None)
}

fn start_server_with_status_and_retry_after(status: u16, retry_after: u64) -> TestServer {
    start_server_with_status_and_optional_retry_after(status, Some(retry_after))
}

fn start_server_with_status_and_optional_retry_after(
    status: u16,
    retry_after: Option<u64>,
) -> TestServer {
    let server = match Server::http("127.0.0.1:0") {
        Ok(server) => server,
        Err(err) => panic!("binding checkpoint debug server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(request)) => {
                    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss>
  <channel>
    <item>
      <title>one</title>
      <link>https://example.com/one</link>
    </item>
  </channel>
</rss>"#;
                    let mut response =
                        TinyResponse::from_string(body).with_status_code(StatusCode(status));
                    if let Ok(header) =
                        Header::from_bytes(b"Content-Type".as_slice(), b"application/rss+xml")
                    {
                        response = response.with_header(header);
                    }
                    if let Some(retry_after) = retry_after {
                        if let Ok(header) = Header::from_bytes(
                            b"Retry-After".as_slice(),
                            retry_after.to_string().as_bytes(),
                        ) {
                            response = response.with_header(header);
                        }
                    }
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });

    TestServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

#[derive(Default)]
struct RetryScheduledObserver {
    retry_scheduled: std::sync::atomic::AtomicUsize,
}

impl ExecutionObserver for RetryScheduledObserver {
    fn on_event(&self, event: &ObserverEvent) {
        if matches!(event, ObserverEvent::RetryScheduled { .. }) {
            self.retry_scheduled.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn wait_for_stop(controller: &Arc<DebugController>, count: usize) {
    let waited = match controller.wait_for_stop_count(count, Duration::from_secs(2)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop count {count}: {err}"),
    };
    if !waited {
        panic!("timed out waiting for stop count {count}");
    }
}

fn read_stop_events(controller: &Arc<DebugController>) -> Vec<arazzo_runtime::DebugStopEvent> {
    match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events: {err}"),
    }
}
