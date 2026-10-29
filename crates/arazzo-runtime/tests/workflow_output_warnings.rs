mod common;

use arazzo_runtime::{
    DebugController, EngineEvent, ExecutionEventKind, ExecutionResult, ObserverEvent,
    RuntimeErrorKind, WorkflowOutputWarning,
};
use arazzo_spec::{
    ActionType, OnAction, OutputValue, SelectorObject, SelectorType, Step, StepTarget, Workflow,
};
use common::{fixture_engine_builder, make_spec_with_base, start_server, MockHttpResponse};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

fn http_step(id: &str) -> Step {
    Step {
        step_id: id.to_string(),
        target: Some(StepTarget::OperationPath(format!(
            "{{$sourceDescriptions.test.url}}#/paths/~1{id}/get"
        ))),
        ..Step::default()
    }
}

fn skipping_workflow(id: &str) -> Workflow {
    let mut first = http_step("first");
    first.on_success = vec![OnAction {
        name: "stop-early".to_string(),
        type_: Some(ActionType::End),
        ..OnAction::default()
    }];
    Workflow {
        workflow_id: id.to_string(),
        steps: vec![first, http_step("skipped")],
        outputs: BTreeMap::from([(
            "skippedStatus".to_string(),
            "$steps.skipped.outputs.status".into(),
        )]),
        ..Workflow::default()
    }
}

#[tokio::test]
async fn serial_skipped_output_is_null_and_emits_warning() {
    assert_skipped_output(false, true).await;
}

#[tokio::test]
async fn workflow_warning_is_emitted_without_tracing() {
    assert_skipped_output(false, false).await;
}

#[tokio::test]
async fn dry_run_emits_workflow_warning_and_preserves_null() {
    assert_skipped_output(true, false).await;
}

async fn assert_skipped_output(dry_run: bool, trace: bool) {
    let server = start_server(|_, _, _, _| MockHttpResponse::json(200, "{}"));
    let engine = fixture_engine_builder(make_spec_with_base(
        &server.base_url,
        vec![skipping_workflow("serial")],
    ))
    .dry_run(dry_run)
    .trace(trace)
    .build()
    .unwrap_or_else(|err| panic!("building engine: {err}"));
    let result = engine.execute_collect("serial", BTreeMap::new()).await;
    assert_eq!(
        result
            .outputs
            .as_ref()
            .unwrap_or_else(|err| panic!("execution: {err}"))["skippedStatus"],
        Value::Null
    );
    assert_eq!(
        result.workflow_output_warnings(),
        vec![&skipped_warning("serial")]
    );
    assert_warning_before_completion(&result, "serial");
    assert_eq!(result.trace_steps().len(), usize::from(trace));
    assert_eq!(result.dry_run_requests().len(), usize::from(dry_run));
}

fn skipped_warning(workflow: &str) -> WorkflowOutputWarning {
    WorkflowOutputWarning {
        workflow_id: workflow.to_string(),
        output: "skippedStatus".to_string(),
        expression: "$steps.skipped.outputs.status".to_string(),
        message: "step \"skipped\" not found in context".to_string(),
    }
}

fn assert_warning_before_completion(result: &ExecutionResult, workflow: &str) {
    let warning = result.events.iter().position(|event| matches!(
        event, EngineEvent::WorkflowOutputWarning(warning) if warning.workflow_id == workflow
    )).unwrap_or_else(|| panic!("missing warning for {workflow}"));
    let completed = result.events.iter().position(|event| matches!(
        event, EngineEvent::Observer(ObserverEvent::WorkflowCompleted { workflow_id, error: None, .. }) if workflow_id == workflow
    )).unwrap_or_else(|| panic!("missing completion for {workflow}"));
    assert!(
        warning < completed,
        "warning must precede its invocation's completion"
    );
}

#[tokio::test]
async fn parallel_levels_emit_warning_after_running_both_steps() {
    let server = start_server(|_, _, _, _| MockHttpResponse::json(200, "{}"));
    let mut second = http_step("second");
    second.outputs = BTreeMap::from([("status".to_string(), "$statusCode".into())]);
    let workflow = Workflow {
        workflow_id: "parallel".to_string(),
        steps: vec![http_step("first"), second],
        outputs: BTreeMap::from([("missing".to_string(), "$steps.second.outputs.nope".into())]),
        ..Workflow::default()
    };
    let engine = fixture_engine_builder(make_spec_with_base(&server.base_url, vec![workflow]))
        .parallel(true)
        .trace(true)
        .build()
        .unwrap_or_else(|err| panic!("building parallel engine: {err}"));
    let result = engine.execute_collect("parallel", BTreeMap::new()).await;
    assert_eq!(
        result
            .outputs
            .as_ref()
            .unwrap_or_else(|err| panic!("parallel execution: {err}"))["missing"],
        Value::Null
    );
    assert_eq!(
        result.workflow_output_warnings(),
        vec![&WorkflowOutputWarning {
            workflow_id: "parallel".to_string(),
            output: "missing".to_string(),
            expression: "$steps.second.outputs.nope".to_string(),
            message: "output \"nope\" not found in step \"second\"".to_string(),
        }]
    );
    assert!(result.sequential_fallbacks().is_empty());
    let lifecycle = result.execution_events();
    assert_eq!(lifecycle.len(), 4);
    assert_eq!(lifecycle[0].kind, ExecutionEventKind::BeforeStep);
    assert_eq!(lifecycle[0].step_id, "first");
    assert_eq!(lifecycle[1].kind, ExecutionEventKind::BeforeStep);
    assert_eq!(lifecycle[1].step_id, "second");
    assert_eq!(lifecycle[2].kind, ExecutionEventKind::AfterStep);
    assert_warning_before_completion(&result, "parallel");
}

fn child_call(id: &str) -> Step {
    Step {
        step_id: id.to_string(),
        target: Some(StepTarget::WorkflowId("child".to_string())),
        ..Step::default()
    }
}

#[tokio::test]
async fn repeated_child_warnings_keep_event_order_before_own_and_parent_completion() {
    let server = start_server(|_, _, _, _| MockHttpResponse::json(200, "{}"));
    let parent = Workflow {
        workflow_id: "parent".to_string(),
        steps: vec![child_call("one"), child_call("two")],
        outputs: BTreeMap::from([(
            "parentMissing".to_string(),
            "$steps.one.outputs.nope".into(),
        )]),
        ..Workflow::default()
    };
    let engine = fixture_engine_builder(make_spec_with_base(
        &server.base_url,
        vec![parent, skipping_workflow("child")],
    ))
    .build()
    .unwrap_or_else(|err| panic!("building parent engine: {err}"));
    let result = engine.execute_collect("parent", BTreeMap::new()).await;
    assert!(result.outputs.is_ok());
    let warnings = result.workflow_output_warnings();
    assert_eq!(warnings.len(), 3);
    assert_eq!(warnings[0], &skipped_warning("child"));
    assert_eq!(
        warnings[1], warnings[0],
        "each invocation must retain its warning"
    );
    assert_eq!(warnings[2].workflow_id, "parent");
    let milestones: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::WorkflowOutputWarning(warning) => {
                Some(format!("warning:{}", warning.workflow_id))
            }
            EngineEvent::Observer(ObserverEvent::WorkflowCompleted { workflow_id, .. }) => {
                Some(format!("complete:{workflow_id}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        milestones,
        [
            "warning:child",
            "complete:child",
            "warning:child",
            "complete:child",
            "warning:parent",
            "complete:parent"
        ]
    );
}

#[tokio::test]
async fn child_warning_survives_later_parent_criteria_failure() {
    let server = start_server(|_, _, _, _| MockHttpResponse::json(200, "{}"));
    let mut failing = http_step("second");
    failing.success_criteria = vec![arazzo_spec::SuccessCriterion {
        condition: "$statusCode == 201".to_string(),
        ..Default::default()
    }];
    let parent = Workflow {
        workflow_id: "parent".to_string(),
        steps: vec![child_call("one"), failing],
        ..Workflow::default()
    };
    let engine = fixture_engine_builder(make_spec_with_base(
        &server.base_url,
        vec![parent, skipping_workflow("child")],
    ))
    .build()
    .unwrap_or_else(|err| panic!("building failing parent: {err}"));
    let result = engine.execute_collect("parent", BTreeMap::new()).await;
    assert_eq!(
        result.outputs.as_ref().err().map(|err| &err.kind),
        Some(&RuntimeErrorKind::SuccessCriteriaFailed)
    );
    assert_eq!(
        result.workflow_output_warnings(),
        vec![&skipped_warning("child")]
    );
    assert_warning_before_completion(&result, "child");
}

#[tokio::test]
async fn selector_warning_and_output_map_evaluation_order_are_retained() {
    let mut workflow = skipping_workflow("selectors");
    workflow.outputs = BTreeMap::from([
        (
            "zSelector".to_string(),
            OutputValue::Selector(SelectorObject {
                context: "$outputs.aMissing".to_string(),
                selector: "/value".to_string(),
                type_: SelectorType::Name("jsonpointer".to_string()),
                extensions: BTreeMap::new(),
            }),
        ),
        (
            "aMissing".to_string(),
            "$steps.skipped.outputs.status".into(),
        ),
        ("bCopy".to_string(), "$outputs.aMissing".into()),
    ]);
    let engine = fixture_engine_builder(make_spec_with_base("http://127.0.0.1:1", vec![workflow]))
        .dry_run(true)
        .build()
        .unwrap_or_else(|err| panic!("building selector engine: {err}"));
    let result = engine.execute_collect("selectors", BTreeMap::new()).await;
    assert_eq!(
        result
            .outputs
            .as_ref()
            .unwrap_or_else(|err| panic!("selector execution: {err}")),
        &BTreeMap::from([
            ("aMissing".to_string(), Value::Null),
            ("bCopy".to_string(), Value::Null),
            ("zSelector".to_string(), Value::Null),
        ])
    );
    let warnings = result.workflow_output_warnings();
    assert_eq!(warnings.len(), 2);
    assert_eq!(warnings[0].output, "aMissing");
    assert_eq!(warnings[1].output, "zSelector");
    assert_eq!(warnings[1].message, "selector matched no values");
    assert_warning_before_completion(&result, "selectors");
}

#[tokio::test]
async fn earlier_warning_survives_later_debugger_output_callback_error() {
    // A YAML extension with a sequence key cannot be serialized as JSON by
    // report_workflow_output. The unresolved selector reaches that callback
    // after the earlier output has already produced its ordinary warning.
    let bad_extension = serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::from_iter([(
        serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::String("key".to_string())]),
        serde_yaml_ng::Value::Null,
    )]));
    let mut workflow = skipping_workflow("debug-error");
    workflow.outputs = BTreeMap::from([
        (
            "aWarning".to_string(),
            "$steps.skipped.outputs.status".into(),
        ),
        (
            "bDebuggerError".to_string(),
            OutputValue::Selector(SelectorObject {
                context: "$steps.skipped.outputs.status".to_string(),
                selector: "/value".to_string(),
                type_: SelectorType::Name("jsonpointer".to_string()),
                extensions: BTreeMap::from([("x-bad-json-key".to_string(), bad_extension)]),
            }),
        ),
        (
            "zNeverEvaluated".to_string(),
            "$steps.skipped.outputs.later".into(),
        ),
    ]);
    let controller = Arc::new(DebugController::new());
    let engine = fixture_engine_builder(make_spec_with_base("http://127.0.0.1:1", vec![workflow]))
        .dry_run(true)
        .debug_controller(Arc::clone(&controller))
        .build()
        .unwrap_or_else(|err| panic!("building debug engine: {err}"));
    let result = engine.execute_collect("debug-error", BTreeMap::new()).await;
    let err = result
        .outputs
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("debug callback must fail"));
    assert_eq!(err.kind, RuntimeErrorKind::DebugController);
    assert_eq!(err.message, "reporting workflow output \"bDebuggerError\": formatting workflow output \"bDebuggerError\": key must be a string");
    let warnings = result.workflow_output_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].output, "aWarning");
    assert_eq!(warnings[0].expression, "$steps.skipped.outputs.status");
    assert!(!result.events.iter().any(|event| matches!(event,
        EngineEvent::Observer(ObserverEvent::WorkflowCompleted { workflow_id, .. }) if workflow_id == "debug-error"
    )), "output callback failure preserves the existing absence of completion");
    let diagnostics = controller
        .take_output_diagnostics()
        .unwrap_or_else(|err| panic!("debug diagnostics: {err}"));
    assert_eq!(
        diagnostics.len(),
        1,
        "later callbacks must not run after the failure"
    );
    assert!(diagnostics[0].contains("aWarning"));
}

#[test]
fn public_warning_serialization_uses_camel_case_and_round_trips() {
    let warning = skipped_warning("workflow");
    let value =
        serde_json::to_value(&warning).unwrap_or_else(|err| panic!("serializing warning: {err}"));
    assert_eq!(
        value,
        json!({
            "workflowId": "workflow",
            "output": "skippedStatus",
            "expression": "$steps.skipped.outputs.status",
            "message": "step \"skipped\" not found in context",
        })
    );
    assert_eq!(
        serde_json::from_value::<WorkflowOutputWarning>(value)
            .unwrap_or_else(|err| panic!("deserializing warning: {err}")),
        warning.clone()
    );
}
