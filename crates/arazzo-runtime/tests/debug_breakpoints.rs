use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use arazzo_runtime::{DebugController, DebugStopReason, EngineBuilder, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breakpoint_hits_follow_step_order() {
    let controller = Arc::new(DebugController::new());
    let engine = build_engine(Arc::clone(&controller));
    let set_res = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "s1"),
        StepBreakpoint::new("wf", "s2"),
    ]);
    if let Err(err) = set_res {
        panic!("setting breakpoints: {err}");
    }

    let handle = engine.execute("wf", BTreeMap::new());

    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for first stop event: {err}"),
    };
    if !waited {
        let _ = controller.resume();
        let _ = handle.collect().await;
        panic!("timed out waiting for first stop event");
    }

    let events = match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events: {err}"),
    };
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].workflow_id, "wf");
    assert_eq!(events[0].step_id, "s1");
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);

    if let Err(err) = controller.resume() {
        panic!("resuming after first stop: {err}");
    }

    let waited = match controller.wait_for_stop_count(2, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for second stop event: {err}"),
    };
    if !waited {
        let _ = controller.resume();
        let _ = handle.collect().await;
        panic!("timed out waiting for second stop event");
    }

    let events = match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events: {err}"),
    };
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[1].workflow_id, "wf");
    assert_eq!(events[1].step_id, "s2");
    assert_eq!(events[1].reason, DebugStopReason::Breakpoint);

    if let Err(err) = controller.resume() {
        panic!("resuming after second stop: {err}");
    }

    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conditional_breakpoint_respects_expression() {
    let controller = Arc::new(DebugController::new());
    let engine = build_engine(Arc::clone(&controller));
    let set_res = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "s1").with_condition("$inputs.code == 429")
    ]);
    if let Err(err) = set_res {
        panic!("setting conditional breakpoint: {err}");
    }

    // First run: condition is false (code=200), no breakpoint fires
    let result = engine.execute_collect("wf", inputs_with_code(200)).await;
    if let Err(err) = result.outputs {
        panic!("expected run with code=200 to succeed: {err}");
    }
    let no_stops = match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events after false condition: {err}"),
    };
    assert!(
        no_stops.is_empty(),
        "expected no stop events for false condition"
    );

    if let Err(err) = controller.clear_stop_events() {
        panic!("clearing stop events: {err}");
    }

    // Second run: condition is true (code=429), breakpoint fires
    let handle = engine.execute("wf", inputs_with_code(429));
    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for conditional stop event: {err}"),
    };
    if !waited {
        let _ = controller.resume();
        let _ = handle.collect().await;
        panic!("timed out waiting for conditional stop event");
    }

    let events = match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading conditional stop events: {err}"),
    };
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].workflow_id, "wf");
    assert_eq!(events[0].step_id, "s1");
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);
    assert_eq!(
        events[0].breakpoint_condition.as_deref(),
        Some("$inputs.code == 429")
    );

    if let Err(err) = controller.resume() {
        panic!("resuming after conditional stop: {err}");
    }

    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
}

fn build_engine(controller: Arc<DebugController>) -> arazzo_runtime::Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: SourceType::OpenApi,
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/status/200".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/status/200".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
        components: None,
    };

    match EngineBuilder::new(spec)
        .dry_run(true)
        .debug_controller(controller)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

fn inputs_with_code(code: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([(String::from("code"), json!(code))])
}
