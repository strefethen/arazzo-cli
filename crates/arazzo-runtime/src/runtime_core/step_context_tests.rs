#![allow(clippy::expect_used)] // Fixture construction failures are test failures.

use super::step_attempt::{RouteScope, SettledAttempt};
use super::*;
use serde_json::json;

fn spec(on_success: Value, on_failure: Value) -> ArazzoSpec {
    serde_json::from_value(json!({
        "arazzo": "1.1.0",
        "info": {"title": "step context tests", "version": "1"},
        "workflows": [
            {"workflowId": "main", "steps": [{
                "stepId": "receive",
                "operationId": "receive",
                "onSuccess": on_success,
                "onFailure": on_failure
            }]},
            {"workflowId": "callee", "steps": []}
        ]
    }))
    .expect("test spec is valid JSON")
}

fn message_with(accepted: bool) -> step_context::MessageSnapshot {
    step_context::MessageSnapshot {
        headers: BTreeMap::from([("x-tag".to_owned(), "reply".to_owned())]),
        payload: Some(json!({
            "id": 42,
            "accepted": accepted,
            "object": {"ok": true},
            "target": "callee"
        })),
    }
}

fn message() -> step_context::MessageSnapshot {
    message_with(true)
}

fn execution_context() -> Arc<ExecutionContext> {
    let (event_tx, _event_rx) = mpsc::channel(16);
    Arc::new(ExecutionContext {
        event_tx,
        role: ContextRole::Invocation,
        trace_seq: AtomicU64::new(0),
        run_seq: AtomicU64::new(0),
        execution_event_seq: AtomicU64::new(0),
        step_attempts: Mutex::new(BTreeMap::new()),
        cancel: CancellationToken::new(),
        is_timeout: Arc::new(AtomicBool::new(false)),
        completed_workflows: Mutex::new(BTreeSet::new()),
        message_receivers: Arc::default(),
    })
}

async fn finish(
    engine: &Engine,
    workflow: &Workflow,
    context: &ExecutionContext,
    vars: &VarStore,
    success: bool,
    message: Option<step_context::MessageSnapshot>,
) -> (SettledAttempt, FlowDecision) {
    let step = &workflow.steps[0];
    let begun = engine
        .begin_attempt(context, &workflow.workflow_id, step)
        .await;
    let run = Ok(StepExecution {
        result: StepResult {
            success,
            response: None,
            message,
            err: (!success).then(|| "criteria failed".to_owned()),
            err_kind: (!success).then_some(RuntimeErrorKind::SuccessCriteriaFailed),
        },
        outputs: BTreeMap::new(),
        dry_run_request: None,
        trace: StepTraceData::default(),
    });
    let retry_count = BTreeMap::new();
    let scope = RouteScope {
        workflow_id: &workflow.workflow_id,
        workflow,
        step_idx: 0,
        vars,
        depth: 0,
        retry_count: &retry_count,
        cancel: &context.cancel,
        is_timeout: &context.is_timeout,
    };
    engine
        .finish_attempt(context, step, begun, run, scope)
        .await
}

fn action(action_type: &str, target: Value, condition: &str) -> Value {
    let mut result = json!({
        "name": "route",
        "type": action_type,
        "criteria": [{"condition": condition}]
    });
    if let (Some(object), Value::Object(target)) = (result.as_object_mut(), target) {
        object.extend(target);
    }
    result
}

#[tokio::test]
async fn success_and_failure_criteria_route_with_the_current_message() {
    let success = action(
        "end",
        json!({}),
        "$message.payload#/id == 42 && $message.payload#/accepted == true && $message.header.x-tag == 'reply'",
    );
    let engine = EngineBuilder::new(spec(json!([success]), json!([])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    let context = execution_context();
    let vars = VarStore::default();

    let (settled, flow) = finish(&engine, workflow, &context, &vars, true, Some(message())).await;
    assert!(matches!(flow, FlowDecision::Done));
    assert_eq!(settled.execution.result.message, Some(message()));
    assert_eq!(settled.decision.action_type, "end");
    assert_eq!(settled.decision.path, TraceDecisionPath::Done);

    // Reuse the invocation and variables to prove a later message-free attempt
    // cannot inherit the first attempt's message.
    let (settled, flow) = finish(&engine, workflow, &context, &vars, true, None).await;
    assert!(matches!(flow, FlowDecision::Next(1)));
    assert_eq!(settled.decision.path, TraceDecisionPath::Next);

    let (settled, flow) = finish(
        &engine,
        workflow,
        &context,
        &vars,
        true,
        Some(message_with(false)),
    )
    .await;
    assert!(matches!(flow, FlowDecision::Next(1)));
    assert_eq!(settled.decision.path, TraceDecisionPath::Next);

    let output_equivalent = action("end", json!({}), "$steps.receive.outputs.id == 42");
    let engine = EngineBuilder::new(spec(json!([output_equivalent]), json!([])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    let context = execution_context();
    let mut vars = VarStore::default();
    vars.set_step_output("receive", "id", json!(42));
    let (settled, flow) = finish(&engine, workflow, &context, &vars, true, Some(message())).await;
    assert!(matches!(flow, FlowDecision::Done));
    assert_eq!(settled.decision.action_type, "end");
    assert_eq!(settled.decision.path, TraceDecisionPath::Done);

    let failure = action("end", json!({}), "$message.payload#/accepted == true");
    let engine = EngineBuilder::new(spec(json!([]), json!([failure])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    let (settled, flow) = finish(
        &engine,
        workflow,
        &execution_context(),
        &VarStore::default(),
        false,
        Some(message()),
    )
    .await;
    assert!(matches!(flow, FlowDecision::Error(_)));
    assert_eq!(settled.execution.result.message, Some(message()));
    assert_eq!(settled.decision.action_type, "end");
    assert_eq!(settled.decision.path, TraceDecisionPath::Done);
}

#[tokio::test]
async fn goto_parameters_keep_typed_message_values_without_message_targets() {
    let goto = json!({
        "name": "goto_callee",
        "type": "goto",
        "workflowId": "callee",
        "parameters": [
            {"name": "id", "value": "$message.payload#/id"},
            {"name": "accepted", "value": "$message.payload#/accepted"},
            {"name": "object", "value": "$message.payload#/object"},
            {"name": "tag", "value": "$message.header.x-tag"}
        ]
    });
    let engine = EngineBuilder::new(spec(json!([goto]), json!([])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    match finish(
        &engine,
        workflow,
        &execution_context(),
        &VarStore::default(),
        true,
        Some(message()),
    )
    .await
    .1
    {
        FlowDecision::GotoWorkflow {
            workflow_id,
            inputs,
        } => {
            assert_eq!(workflow_id, "callee");
            assert_eq!(
                inputs.expect("mapped inputs"),
                BTreeMap::from([
                    ("id".to_owned(), json!(42)),
                    ("accepted".to_owned(), json!(true)),
                    ("object".to_owned(), json!({"ok": true})),
                    ("tag".to_owned(), json!("reply")),
                ])
            );
        }
        other => panic!("expected workflow goto, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_reference_parameters_read_message_while_target_does_not() {
    let retry = json!({
        "name": "retry_through_callee",
        "type": "retry",
        "workflowId": "callee",
        "parameters": [
            {"name": "id", "value": "$message.payload#/id"},
            {"name": "accepted", "value": "$message.payload#/accepted"},
            {"name": "tag", "value": "$message.header.x-tag"}
        ]
    });
    let engine = EngineBuilder::new(spec(json!([]), json!([retry])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    match finish(
        &engine,
        workflow,
        &execution_context(),
        &VarStore::default(),
        false,
        Some(message()),
    )
    .await
    .1
    {
        FlowDecision::Retry {
            reference:
                Some(RetryReference::Workflow {
                    workflow_id,
                    inputs,
                }),
            ..
        } => {
            assert_eq!(workflow_id, "callee");
            assert_eq!(
                inputs.expect("mapped inputs"),
                BTreeMap::from([
                    ("id".to_owned(), json!(42)),
                    ("accepted".to_owned(), json!(true)),
                    ("tag".to_owned(), json!("reply")),
                ])
            );
        }
        other => panic!("expected workflow retry, got {other:?}"),
    }
}

#[tokio::test]
async fn message_context_does_not_expand_action_target_ids() {
    let goto = json!({
        "name": "goto_from_message",
        "type": "goto",
        "workflowId": "{$message.payload#/target}"
    });
    let engine = EngineBuilder::new(spec(json!([goto]), json!([])))
        .build()
        .expect("engine builds");
    let workflow = &engine.inner.index.spec.workflows[0];
    match finish(
        &engine,
        workflow,
        &execution_context(),
        &VarStore::default(),
        true,
        Some(message()),
    )
    .await
    .1
    {
        FlowDecision::GotoWorkflow { workflow_id, .. } => assert_ne!(workflow_id, "callee"),
        other => panic!("expected workflow goto, got {other:?}"),
    }
}

#[test]
fn settlement_and_context_preserve_absence_and_explicit_null() {
    let failed = step_attempt::settle(Err(RuntimeError::new(
        RuntimeErrorKind::HttpRequest,
        "synthetic early error",
    )));
    assert!(failed.result.message.is_none());

    let absent = step_context::MessageSnapshot::default();
    let null = step_context::MessageSnapshot {
        payload: Some(Value::Null),
        ..step_context::MessageSnapshot::default()
    };
    let vars = VarStore::default();
    let engine = EngineBuilder::new(spec(json!([]), json!([])))
        .build()
        .expect("engine builds");
    let absent_eval =
        step_context::with_message(engine.make_eval_context(&vars, None), Some(&absent));
    let null_eval = step_context::with_message(engine.make_eval_context(&vars, None), Some(&null));
    assert_eq!(absent_eval.message_payload, None);
    assert_eq!(null_eval.message_payload, Some(Value::Null));
}
