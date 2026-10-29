//! Workflow-target steps publish only their own successful run's outputs.

mod common;

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arazzo_runtime::{ExecutionEventKind, ExecutionObserver, ExecutionResult, ObserverEvent};
use arazzo_spec::{
    ActionType, OnAction, ParamLocation, Parameter, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::{
    logged_requests, make_spec_with_base, new_request_log, record_request, start_server,
    MockHttpResponse, RequestLog,
};
use serde_json::{json, Value};

type Outputs = BTreeMap<String, Value>;

#[derive(Default)]
struct Completions(Mutex<Vec<(String, String, Outputs)>>);

impl ExecutionObserver for Completions {
    fn on_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::StepCompleted {
            workflow_id,
            step_id,
            outputs,
            ..
        } = event
        {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((
                workflow_id.clone(),
                step_id.clone(),
                outputs.clone(),
            ));
        }
    }
}

/// Reach and report every regression comparison even when an earlier one fails.
#[derive(Default)]
struct Comparisons(Vec<String>);

impl Comparisons {
    fn equal<T: Debug + PartialEq>(&mut self, label: &str, actual: T, expected: T) {
        if actual == expected {
            eprintln!("PASS {label}: {actual:?}");
        } else {
            let failure = format!("{label}: actual {actual:?}, expected {expected:?}");
            eprintln!("FAIL {failure}");
            self.0.push(failure);
        }
    }

    fn finish(self) {
        assert!(self.0.is_empty(), "{}", self.0.join("\n"));
    }
}

fn query(name: &str, expression: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Query),
        value: expression.into(),
        ..Parameter::default()
    }
}

fn http_step(id: &str, path: &str) -> Step {
    Step {
        step_id: id.to_string(),
        target: Some(StepTarget::OperationPath(format!(
            "{{$sourceDescriptions.test.url}}#/paths/~1{path}/get"
        ))),
        ..Step::default()
    }
}

fn goto_step(id: &str) -> OnAction {
    OnAction {
        name: format!("go-{id}"),
        type_: Some(ActionType::Goto),
        step_id: id.to_string(),
        ..OnAction::default()
    }
}

fn child_workflow() -> Workflow {
    let mut child = http_step("child", "child");
    child.outputs = BTreeMap::from([("v".to_string(), "$response.body#/v".into())]);
    Workflow {
        workflow_id: "c".to_string(),
        steps: vec![child],
        outputs: BTreeMap::from([("v".to_string(), "$steps.child.outputs.v".into())]),
        ..Workflow::default()
    }
}

fn criteria_parent(rerun: bool) -> Workflow {
    let call = Step {
        step_id: "call".to_string(),
        target: Some(StepTarget::WorkflowId("c".to_string())),
        success_criteria: vec![SuccessCriterion {
            condition: "$steps.call.outputs.v == 'pass'".to_string(),
            ..SuccessCriterion::default()
        }],
        on_failure: vec![goto_step("probe")],
        ..Step::default()
    };
    let mut probe = http_step("probe", "check");
    probe.parameters = vec![
        query("step", "$steps.call.outputs.v"),
        query("child", "$workflows.c.outputs.v"),
    ];
    let mut steps = vec![call];
    if rerun {
        let mut router = http_step("router", "b");
        // Both references matter: execute_step(false) must select the router,
        // and the router must select call before it routes back there.
        router.parameters = vec![query("v", "$steps.call.outputs.v")];
        router.outputs = BTreeMap::from([("route".to_string(), "$response.body#/route".into())]);
        router.on_success = vec![goto_step("call")];
        steps.push(router);
        probe
            .parameters
            .push(query("route", "$steps.router.outputs.route"));
    }
    steps.push(probe);
    Workflow {
        workflow_id: "parent".to_string(),
        steps,
        ..Workflow::default()
    }
}

struct Run {
    result: ExecutionResult,
    observer: Arc<Completions>,
    requests: RequestLog,
}

async fn run_criteria(rerun: bool, single_step: bool) -> Run {
    let requests = new_request_log();
    let log = Arc::clone(&requests);
    let hits = AtomicUsize::new(0);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log, &method, &url, &headers, &body);
        match url.split('?').next().unwrap_or_default() {
            "/child" => {
                if rerun && hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    MockHttpResponse::json(200, r#"{"v":"pass"}"#)
                } else {
                    MockHttpResponse::json(200, r#"{"v":"fail"}"#)
                }
            }
            "/b" => MockHttpResponse::json(200, r#"{"route":"ready"}"#),
            "/check" => MockHttpResponse::empty(200),
            _ => MockHttpResponse::empty(404),
        }
    });
    let observer = Arc::new(Completions::default());
    let engine = match common::fixture_engine_builder(make_spec_with_base(
        &server.base_url,
        vec![criteria_parent(rerun), child_workflow()],
    ))
    .trace(true)
    .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
    .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building workflow-output fixture: {err}"),
    };
    let handle = if single_step {
        engine.execute_step("parent", "probe", BTreeMap::new(), false)
    } else {
        engine.execute("parent", BTreeMap::new())
    };
    let result = handle.collect().await;
    if let Err(err) = &result.outputs {
        panic!("probe must complete workflow-output fixture: {err}");
    }
    Run {
        result,
        observer,
        requests,
    }
}

/// Compare only the changed attempt. Existing targets own compatibility checks
/// for the earlier success and the child-output success criterion itself.
fn compare_call_recordings(checks: &mut Comparisons, run: &Run, index: usize, expected: Outputs) {
    let traces: Vec<_> = run
        .result
        .trace_steps()
        .into_iter()
        .filter(|record| record.workflow_id == "parent" && record.step_id == "call")
        .map(|record| record.outputs.clone())
        .collect();
    checks.equal("call trace outputs", traces.get(index), Some(&expected));
    let after: Vec<_> = run
        .result
        .execution_events()
        .into_iter()
        .filter(|event| {
            event.workflow_id == "parent"
                && event.step_id == "call"
                && event.kind == ExecutionEventKind::AfterStep
        })
        .map(|event| event.outputs.clone())
        .collect();
    checks.equal("call AfterStep outputs", after.get(index), Some(&expected));
    let completed: Vec<_> = run
        .observer
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(workflow, step, _)| workflow == "parent" && step == "call")
        .map(|(_, _, outputs)| outputs.clone())
        .collect();
    checks.equal(
        "call StepCompleted outputs",
        completed.get(index),
        Some(&expected),
    );
}

fn probe_urls(run: &Run) -> Vec<String> {
    logged_requests(&run.requests)
        .into_iter()
        .filter(|request| request.url.starts_with("/check"))
        .map(|request| request.url)
        .collect()
}

async fn compare_failed_rerun(single_step: bool) {
    let run = run_criteria(true, single_step).await;
    let mut checks = Comparisons::default();
    compare_call_recordings(&mut checks, &run, 1, Outputs::new());
    // This coherent query checks both the prior step value and fresh child
    // workflow registration after the parent's criterion rejects that child.
    checks.equal(
        "probe preserves step output and fresh child registration",
        probe_urls(&run),
        vec!["/check?step=pass&child=fail&route=ready".to_string()],
    );
    checks.finish();
}

#[tokio::test]
async fn execute_rejected_rerun_preserves_prior_outputs() {
    compare_failed_rerun(false).await;
}

#[tokio::test]
async fn execute_step_rejected_rerun_preserves_prior_outputs() {
    compare_failed_rerun(true).await;
}

#[tokio::test]
async fn first_rejected_run_restores_absence() {
    let run = run_criteria(false, false).await;
    let mut checks = Comparisons::default();
    compare_call_recordings(&mut checks, &run, 0, Outputs::new());
    checks.equal(
        "probe omits unpublished step value",
        probe_urls(&run),
        vec!["/check?child=fail".to_string()],
    );
    let missing_step_warning = run
        .result
        .trace_steps()
        .into_iter()
        .filter(|record| record.workflow_id == "parent" && record.step_id == "probe")
        .flat_map(|record| &record.warnings)
        .any(|warning| warning.contains("step \"call\" not found"));
    checks.equal(
        "unpublished step remains absent",
        missing_step_warning,
        true,
    );
    checks.finish();
}

#[tokio::test]
async fn successful_goto_rerun_replaces_output_key_set() {
    let requests = new_request_log();
    let log = Arc::clone(&requests);
    let child_hits = AtomicUsize::new(0);
    let router_hits = AtomicUsize::new(0);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log, &method, &url, &headers, &body);
        match url.split('?').next().unwrap_or_default() {
            "/child" => MockHttpResponse::json(
                if child_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    200
                } else {
                    201
                },
                r#"{"v":"first"}"#,
            ),
            "/b" => MockHttpResponse::empty(if router_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                200
            } else {
                201
            }),
            "/c" => MockHttpResponse::json(200, r#"{"v":"second"}"#),
            "/check" => MockHttpResponse::empty(200),
            _ => MockHttpResponse::empty(404),
        }
    });
    let call = Step {
        step_id: "call".to_string(),
        target: Some(StepTarget::WorkflowId("c".to_string())),
        ..Step::default()
    };
    let mut router = http_step("router", "b");
    router.on_success = vec![OnAction {
        criteria: vec![SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        }],
        ..goto_step("call")
    }];
    let mut probe = http_step("probe", "check");
    probe.parameters = vec![
        query("old", "$steps.call.outputs.old"),
        query("fresh", "$steps.call.outputs.fresh"),
    ];
    let parent = Workflow {
        workflow_id: "parent".to_string(),
        steps: vec![call, router, probe],
        ..Workflow::default()
    };
    let mut child = child_workflow();
    child.outputs = BTreeMap::from([("old".to_string(), "$steps.child.outputs.v".into())]);
    child.steps[0].on_success = vec![OnAction {
        name: "change-output-shape".to_string(),
        type_: Some(ActionType::Goto),
        workflow_id: "alternate".to_string(),
        criteria: vec![SuccessCriterion {
            condition: "$statusCode == 201".to_string(),
            ..SuccessCriterion::default()
        }],
        ..OnAction::default()
    }];
    let mut alternate_step = http_step("alternate", "c");
    alternate_step.outputs = BTreeMap::from([("v".to_string(), "$response.body#/v".into())]);
    let alternate = Workflow {
        workflow_id: "alternate".to_string(),
        steps: vec![alternate_step],
        outputs: BTreeMap::from([("fresh".to_string(), "$steps.alternate.outputs.v".into())]),
        ..Workflow::default()
    };
    let observer = Arc::new(Completions::default());
    let engine = match common::fixture_engine_builder(make_spec_with_base(
        &server.base_url,
        vec![parent, child, alternate],
    ))
    .trace(true)
    .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
    .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building goto-output fixture: {err}"),
    };
    let result = engine.execute_collect("parent", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        panic!("probe must complete goto-output fixture: {err}");
    }
    let run = Run {
        result,
        observer,
        requests,
    };
    let mut checks = Comparisons::default();
    compare_call_recordings(
        &mut checks,
        &run,
        1,
        BTreeMap::from([("fresh".to_string(), json!("second"))]),
    );
    checks.equal(
        "probe sees only the rerun's output names",
        probe_urls(&run),
        vec!["/check?fresh=second".to_string()],
    );
    checks.finish();
}
