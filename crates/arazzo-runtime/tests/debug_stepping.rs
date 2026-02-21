use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, DebugStopReason, Engine, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, Step, Workflow};
use serde_json::{json, Value};

#[test]
fn step_over_skips_subworkflow_internal_steps() {
    let (mut engine, controller) = build_debug_engine();
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("parent", "call-child")])
    {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let inputs = BTreeMap::from([(String::from("code"), json!(429))]);
    let handle = thread::spawn(move || engine.execute("parent", inputs));

    wait_for_stop(&controller, 1, &handle);
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

    wait_for_stop(&controller, 2, &handle);
    let events = read_stop_events(&controller);
    assert_eq!(events[1].step_id, "parent-after");
    assert_eq!(events[1].depth, 0);
    assert_eq!(events[1].reason, DebugStopReason::Step);

    if let Err(err) = controller.continue_execution() {
        panic!("continue_execution: {err}");
    }
    join_success(handle);
}

#[test]
fn step_in_and_step_out_track_stack_frames() {
    let (mut engine, controller) = build_debug_engine();
    if let Err(err) = controller.set_breakpoints(vec![StepBreakpoint::new("parent", "call-child")])
    {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let inputs = BTreeMap::from([(String::from("code"), json!(429))]);
    let handle = thread::spawn(move || engine.execute("parent", inputs));

    wait_for_stop(&controller, 1, &handle);
    if let Err(err) = controller.step_in() {
        panic!("step_in: {err}");
    }

    wait_for_stop(&controller, 2, &handle);
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

    wait_for_stop(&controller, 3, &handle);
    let events = read_stop_events(&controller);
    assert_eq!(events[2].step_id, "parent-after");
    assert_eq!(events[2].depth, 0);
    assert_eq!(events[2].reason, DebugStopReason::Step);

    if let Err(err) = controller.continue_execution() {
        panic!("continue_execution: {err}");
    }
    join_success(handle);
}

fn build_debug_engine() -> (Engine, Arc<DebugController>) {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug-stepping".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: "openapi".to_string(),
        }],
        workflows: vec![
            Workflow {
                workflow_id: "parent".to_string(),
                summary: String::new(),
                description: String::new(),
                inputs: None,
                steps: vec![
                    Step {
                        step_id: "call-child".to_string(),
                        workflow_id: "child".to_string(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "parent-after".to_string(),
                        operation_path: "/status/200".to_string(),
                        outputs: BTreeMap::from([(
                            "observed".to_string(),
                            "$inputs.code".to_string(),
                        )]),
                        ..Step::default()
                    },
                ],
                outputs: BTreeMap::new(),
            },
            Workflow {
                workflow_id: "child".to_string(),
                summary: String::new(),
                description: String::new(),
                inputs: None,
                steps: vec![
                    Step {
                        step_id: "child-one".to_string(),
                        operation_path: "/status/200".to_string(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "child-two".to_string(),
                        operation_path: "/status/200".to_string(),
                        ..Step::default()
                    },
                ],
                outputs: BTreeMap::new(),
            },
        ],
        components: None,
    };

    let mut engine = match Engine::new(spec) {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    };
    engine.set_dry_run_mode(true);
    let controller = Arc::new(DebugController::new());
    (engine, controller)
}

fn wait_for_stop(
    controller: &Arc<DebugController>,
    count: usize,
    handle: &thread::JoinHandle<Result<BTreeMap<String, Value>, arazzo_runtime::RuntimeError>>,
) {
    let waited = match controller.wait_for_stop_count(count, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for stop event count {count}: {err}"),
    };
    if waited {
        return;
    }
    let _ = controller.continue_execution();
    assert!(
        !handle.is_finished(),
        "execution ended unexpectedly while waiting for stop {count}"
    );
    panic!("timed out waiting for stop event count {count}");
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
