use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use arazzo_runtime::{DebugController, DebugStopReason, EngineBuilder, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn step_over_skips_subworkflow_internal_steps() {
    let (engine, controller) = build_debug_engine();
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("parent", "call-child")])
    {
        panic!("setting breakpoints: {err}");
    }

    let inputs = BTreeMap::from([(String::from("code"), json!(429))]);
    let handle = engine.execute("parent", inputs);

    wait_for_stop(&controller, 1);
    let events = read_stop_events(&controller);
    assert_eq!(events[0].step_id, "call-child");
    assert_eq!(events[0].depth, 0);
    assert_eq!(events[0].reason, DebugStopReason::Breakpoint);

    let scopes = match controller.current_scopes() {
        Ok(scopes) => scopes,
        Err(err) => panic!("reading current scopes: {err}"),
    };
    assert_eq!(scopes.inputs.get("code"), Some(&json!(429)));

    if let Err(err) = controller.step_over() {
        panic!("step_over: {err}");
    }

    wait_for_stop(&controller, 2);
    let events = read_stop_events(&controller);
    assert_eq!(events[1].step_id, "parent-after");
    assert_eq!(events[1].depth, 0);
    assert_eq!(events[1].reason, DebugStopReason::Step);

    if let Err(err) = controller.continue_execution() {
        panic!("continue_execution: {err}");
    }
    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn step_in_and_step_out_track_stack_frames() {
    let (engine, controller) = build_debug_engine();
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("parent", "call-child")])
    {
        panic!("setting breakpoints: {err}");
    }

    let inputs = BTreeMap::from([(String::from("code"), json!(429))]);
    let handle = engine.execute("parent", inputs);

    wait_for_stop(&controller, 1);
    if let Err(err) = controller.step_in() {
        panic!("step_in: {err}");
    }

    wait_for_stop(&controller, 2);
    let events = read_stop_events(&controller);
    assert_eq!(events[1].step_id, "child-one");
    assert_eq!(events[1].depth, 1);
    assert_eq!(events[1].reason, DebugStopReason::Step);

    let stack = match controller.current_stack() {
        Ok(stack) => stack,
        Err(err) => panic!("reading current stack: {err}"),
    };
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0].workflow_id, "parent");
    assert_eq!(stack[0].step_id, "call-child");
    assert_eq!(stack[1].workflow_id, "child");
    assert_eq!(stack[1].step_id, "child-one");

    if let Err(err) = controller.step_out() {
        panic!("step_out: {err}");
    }

    wait_for_stop(&controller, 3);
    let events = read_stop_events(&controller);
    assert_eq!(events[2].step_id, "parent-after");
    assert_eq!(events[2].depth, 0);
    assert_eq!(events[2].reason, DebugStopReason::Step);

    if let Err(err) = controller.continue_execution() {
        panic!("continue_execution: {err}");
    }
    let result = handle.collect().await;
    if let Err(err) = result.outputs {
        panic!("workflow execution failed: {err}");
    }
}

fn build_debug_engine() -> (arazzo_runtime::Engine, Arc<DebugController>) {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-stepping".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: SourceType::OpenApi,
        }],
        workflows: vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![
                    Step {
                        step_id: "call-child".to_string(),
                        target: Some(StepTarget::WorkflowId("child".to_string())),
                        ..Step::default()
                    },
                    Step {
                        step_id: "parent-after".to_string(),
                        target: Some(StepTarget::OperationPath("/status/200".to_string())),
                        outputs: BTreeMap::from([(
                            "observed".to_string(),
                            "$inputs.code".to_string(),
                        )]),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![
                    Step {
                        step_id: "child-one".to_string(),
                        target: Some(StepTarget::OperationPath("/status/200".to_string())),
                        ..Step::default()
                    },
                    Step {
                        step_id: "child-two".to_string(),
                        target: Some(StepTarget::OperationPath("/status/200".to_string())),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
        ],
        components: None,
    };

    let controller = Arc::new(DebugController::new());
    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .debug_controller(Arc::clone(&controller))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    };
    (engine, controller)
}

fn wait_for_stop(controller: &Arc<DebugController>, count: usize) {
    let waited = match controller.wait_for_stop_count(count, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop event count {count}: {err}"),
    };
    if !waited {
        panic!("timed out waiting for stop event count {count}");
    }
}

fn read_stop_events(controller: &Arc<DebugController>) -> Vec<arazzo_runtime::DebugStopEvent> {
    match controller.stop_events() {
        Ok(events) => events,
        Err(err) => panic!("reading stop events: {err}"),
    }
}
