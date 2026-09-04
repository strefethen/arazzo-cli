mod common;

use arazzo_runtime::{EngineBuilder, EngineEvent, ObserverEvent, RuntimeErrorKind};
use arazzo_spec::{Step, StepTarget, SuccessCriterion, Workflow};
use common::{make_spec_with_base, start_server, start_server_concurrent, MockHttpResponse};
use std::collections::BTreeMap;
use std::time::Duration;

const COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);

fn criteria(count: usize) -> Vec<SuccessCriterion> {
    (0..count)
        .map(|_| SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        })
        .collect()
}

fn step(step_id: &str, path: &str, success_criteria: Vec<SuccessCriterion>) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath(path.to_string())),
        success_criteria,
        ..Step::default()
    }
}

fn spec(base_url: &str, workflow_id: &str, steps: Vec<Step>) -> arazzo_spec::ArazzoSpec {
    let mut spec = make_spec_with_base(
        base_url,
        vec![Workflow {
            workflow_id: workflow_id.to_string(),
            steps,
            ..Workflow::default()
        }],
    );
    spec.arazzo = "1.1.0".to_string();
    spec
}

async fn execute_with_timeout(
    engine: &arazzo_runtime::Engine,
    workflow_id: &str,
) -> arazzo_runtime::ExecutionResult {
    match tokio::time::timeout(
        COMPLETION_TIMEOUT,
        engine.execute_collect(workflow_id, BTreeMap::new()),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => panic!("workflow {workflow_id:?} did not finish within {COMPLETION_TIMEOUT:?}"),
    }
}

#[allow(unreachable_patterns)]
fn observer_tags(events: &[EngineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Observer(ObserverEvent::StepStarted { step_id, .. }) => {
                Some(format!("started:{step_id}"))
            }
            EngineEvent::Observer(ObserverEvent::RequestPrepared { step_id, .. }) => {
                Some(format!("prepared:{step_id}"))
            }
            EngineEvent::Observer(ObserverEvent::RequestSent { step_id, .. }) => {
                Some(format!("sent:{step_id}"))
            }
            EngineEvent::Observer(ObserverEvent::CriterionEvaluated {
                step_id,
                index,
                passed,
                ..
            }) => Some(format!("criterion:{step_id}:{index}:{passed}")),
            EngineEvent::Observer(ObserverEvent::StepCompleted {
                step_id,
                criteria_passed,
                ..
            }) => Some(format!("completed:{step_id}:{criteria_passed}")),
            EngineEvent::Observer(ObserverEvent::WorkflowCompleted { error, .. }) => {
                Some(format!("workflow:{}", error.is_none()))
            }
            _ => None,
        })
        .collect()
}

fn criterion_events(events: &[EngineEvent]) -> Vec<(String, usize, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Observer(ObserverEvent::CriterionEvaluated {
                step_id,
                index,
                passed,
                ..
            }) => Some((step_id.clone(), *index, *passed)),
            _ => None,
        })
        .collect()
}

async fn assert_parallel_criteria_complete(criteria_count: usize) {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let workflow_id = format!("parallel-{criteria_count}");
    let spec = spec(
        &server.base_url,
        &workflow_id,
        vec![step("many", "/many", criteria(criteria_count))],
    );
    let engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building parallel engine: {err}"),
    };

    let result = execute_with_timeout(&engine, &workflow_id).await;
    assert!(
        result.outputs.is_ok(),
        "execution failed: {:?}",
        result.outputs
    );

    let observed = criterion_events(&result.events);
    let expected: Vec<_> = (0..criteria_count)
        .map(|index| ("many".to_string(), index, true))
        .collect();
    assert_eq!(observed, expected);
}

#[tokio::test]
async fn parallel_step_drains_seventy_criterion_events_while_executing() {
    assert_parallel_criteria_complete(70).await;
}

#[tokio::test]
async fn parallel_step_drains_high_volume_criterion_events_while_executing() {
    assert_parallel_criteria_complete(512).await;
}

#[tokio::test]
async fn parallel_siblings_replay_complete_event_sequences_in_source_order() {
    let server = start_server_concurrent(|_method, url, _headers, _body| {
        if url == "/first" {
            std::thread::sleep(Duration::from_millis(75));
        }
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let mut second_criteria = criteria(1);
    second_criteria.push(SuccessCriterion {
        condition: "$statusCode == 201".to_string(),
        ..SuccessCriterion::default()
    });
    let spec = spec(
        &server.base_url,
        "siblings",
        vec![
            step("first", "/first", criteria(3)),
            step("second", "/second", second_criteria),
        ],
    );
    let engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building parallel engine: {err}"),
    };

    let result = execute_with_timeout(&engine, "siblings").await;
    let error = match &result.outputs {
        Ok(outputs) => panic!("expected failing criterion, got outputs: {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(error.kind, RuntimeErrorKind::SuccessCriteriaFailed);
    assert_eq!(
        observer_tags(&result.events),
        vec![
            "started:first",
            "started:second",
            "prepared:first",
            "sent:first",
            "criterion:first:0:true",
            "criterion:first:1:true",
            "criterion:first:2:true",
            "completed:first:true",
            "prepared:second",
            "sent:second",
            "criterion:second:0:true",
            "criterion:second:1:false",
            "completed:second:false",
            "workflow:false",
        ]
    );
}

#[tokio::test]
async fn sequential_control_keeps_the_same_single_step_event_sequence() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = spec(
        &server.base_url,
        "sequential",
        vec![step("control", "/control", criteria(3))],
    );
    let engine = match EngineBuilder::new(spec).parallel(false).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building sequential engine: {err}"),
    };

    let result = execute_with_timeout(&engine, "sequential").await;
    assert!(
        result.outputs.is_ok(),
        "execution failed: {:?}",
        result.outputs
    );
    assert_eq!(
        observer_tags(&result.events),
        vec![
            "started:control",
            "prepared:control",
            "sent:control",
            "criterion:control:0:true",
            "criterion:control:1:true",
            "criterion:control:2:true",
            "completed:control:true",
            "workflow:true",
        ]
    );
}
