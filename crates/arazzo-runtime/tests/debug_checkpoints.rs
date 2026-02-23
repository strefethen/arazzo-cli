use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, DebugStopReason, Engine, StepBreakpoint, StepCheckpoint};
use arazzo_spec::{
    ArazzoSpec, Info, OnAction, SourceDescription, Step, SuccessCriterion, Workflow,
};
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

#[test]
fn step_over_enters_success_criteria_and_outputs_with_locals() {
    let server = start_server();
    let mut engine = build_engine(server.base_url.clone());
    let controller = Arc::new(DebugController::new());
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("wf", "fetch-rss")]) {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let handle = thread::spawn(move || engine.execute("wf", BTreeMap::new()));

    wait_for_stop(&controller, 1, &handle);
    let events = read_stop_events(&controller);
    assert_eq!(events[0].step_id, "fetch-rss");
    assert_eq!(events[0].checkpoint, StepCheckpoint::Step);
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);

    if let Err(err) = controller.step_over() {
        panic!("step_over to success criterion: {err}");
    }
    wait_for_stop(&controller, 2, &handle);
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
    wait_for_stop(&controller, 3, &handle);
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
    join_success(handle);
}

#[test]
fn on_failure_action_breakpoint_hits_with_failure_locals() {
    let server = start_server_with_status(502);
    let mut engine = build_failure_engine(server.base_url.clone());
    let controller = Arc::new(DebugController::new());
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_action(0)
    ]) {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let handle = thread::spawn(move || engine.execute("wf", BTreeMap::new()));

    wait_for_stop(&controller, 1, &handle);
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
    let joined = match handle.join() {
        Ok(result) => result,
        Err(_) => panic!("execution thread panicked"),
    };
    match joined {
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

#[test]
fn on_failure_retry_selected_and_delay_checkpoints_are_debuggable() {
    let server = start_server_with_status(503);
    let mut engine = build_retry_engine(server.base_url.clone(), 1, 1);
    let controller = Arc::new(DebugController::new());
    if let Err(err) = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_selected(0),
        StepBreakpoint::new("wf", "fetch-rss").at_on_failure_retry_delay(0),
    ]) {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let handle = thread::spawn(move || engine.execute("wf", BTreeMap::new()));

    wait_for_stop(&controller, 1, &handle);
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
        Some(&json!(1))
    );

    if let Err(err) = controller.continue_execution() {
        panic!("continuing after retry selected: {err}");
    }

    wait_for_stop(&controller, 2, &handle);
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
        Some(&json!(1))
    );

    if let Err(err) = controller.set_breakpoints(Vec::new()) {
        panic!("clearing breakpoints after retry delay: {err}");
    }
    if let Err(err) = controller.continue_execution() {
        panic!("continuing after retry delay: {err}");
    }
    let joined = match handle.join() {
        Ok(result) => result,
        Err(_) => panic!("execution thread panicked"),
    };
    match joined {
        Ok(_) => panic!("expected workflow execution to fail after retry limit"),
        Err(err) => {
            assert!(
                err.message.contains("max retries (1) exceeded"),
                "unexpected error message: {}",
                err.message
            );
        }
    }
}

fn build_engine(base_url: String) -> Engine {
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
            type_: "openapi".to_string(),
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                operation_path: "/rss".to_string(),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([("link_1".to_string(), "//item[1]/link".to_string())]),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        components: None,
    };

    match Engine::new(spec) {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

fn build_failure_engine(base_url: String) -> Engine {
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
            type_: "openapi".to_string(),
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                operation_path: "/rss".to_string(),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                on_failure: vec![
                    OnAction {
                        type_: "end".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 502".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: "retry".to_string(),
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
    };

    match Engine::new(spec) {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

fn build_retry_engine(base_url: String, retry_after: i64, retry_limit: i64) -> Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-checkpoints-retry".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url,
            type_: "openapi".to_string(),
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "fetch-rss".to_string(),
                operation_path: "/rss".to_string(),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
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
    };

    match Engine::new(spec) {
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

fn wait_for_stop(
    controller: &Arc<DebugController>,
    count: usize,
    handle: &thread::JoinHandle<Result<BTreeMap<String, Value>, arazzo_runtime::RuntimeError>>,
) {
    let waited = match controller.wait_for_stop_count(count, Duration::from_secs(2)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop count {count}: {err}"),
    };
    if waited {
        return;
    }
    let _ = controller.continue_execution();
    assert!(
        !handle.is_finished(),
        "execution ended unexpectedly while waiting for stop {count}"
    );
    panic!("timed out waiting for stop count {count}");
}

fn read_stop_events(controller: &Arc<DebugController>) -> Vec<arazzo_runtime::DebugStopEvent> {
    match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events: {err}"),
    }
}

fn join_success(
    handle: thread::JoinHandle<Result<BTreeMap<String, Value>, arazzo_runtime::RuntimeError>>,
) {
    let joined = match handle.join() {
        Ok(result) => result,
        Err(_) => panic!("execution thread panicked"),
    };
    if let Err(err) = joined {
        panic!("workflow execution failed: {err}");
    }
}
