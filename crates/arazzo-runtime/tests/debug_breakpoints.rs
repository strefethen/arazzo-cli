use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, DebugStopReason, Engine, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, Step, Workflow};
use serde_json::{json, Value};

#[test]
fn breakpoint_hits_follow_step_order() {
    let mut engine = build_engine();
    let controller = Arc::new(DebugController::new());
    let set_res = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "s1"),
        StepBreakpoint::new("wf", "s2"),
    ]);
    if let Err(err) = set_res {
        panic!("setting breakpoints: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let handle = thread::spawn(move || engine.execute("wf", BTreeMap::new()));

    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for first stop event: {err}"),
    };
    if !waited {
        let _ = controller.resume();
        let _ = handle.join();
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
        let _ = handle.join();
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

    let execution = match handle.join() {
        Ok(result) => result,
        Err(_) => panic!("execution thread panicked"),
    };
    if let Err(err) = execution {
        panic!("workflow execution failed: {err}");
    }
}

#[test]
fn conditional_breakpoint_respects_expression() {
    let mut engine = build_engine();
    let controller = Arc::new(DebugController::new());
    let set_res = controller.set_breakpoints(vec![
        StepBreakpoint::new("wf", "s1").with_condition("$inputs.code == 429")
    ]);
    if let Err(err) = set_res {
        panic!("setting conditional breakpoint: {err}");
    }
    engine.set_debug_controller(Arc::clone(&controller));

    let false_result = engine.execute("wf", inputs_with_code(200));
    if let Err(err) = false_result {
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

    let handle = thread::spawn(move || engine.execute("wf", inputs_with_code(429)));
    let waited = match controller.wait_for_stop_count(1, Duration::from_secs(1)) {
        Ok(value) => value,
        Err(err) => panic!("waiting for conditional stop event: {err}"),
    };
    if !waited {
        let _ = controller.resume();
        let _ = handle.join();
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

    let execution = match handle.join() {
        Ok(result) => result,
        Err(_) => panic!("execution thread panicked"),
    };
    if let Err(err) = execution {
        panic!("workflow execution failed: {err}");
    }
}

fn build_engine() -> Engine {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "debug".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: "openapi".to_string(),
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            summary: String::new(),
            description: String::new(),
            inputs: None,
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/status/200".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/status/200".to_string(),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::new(),
        }],
        components: None,
    };

    let mut engine = match Engine::new(spec) {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    };
    engine.set_dry_run_mode(true);
    engine
}

fn inputs_with_code(code: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([(String::from("code"), json!(code))])
}
