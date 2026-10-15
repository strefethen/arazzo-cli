//! Sub-workflow (`workflowId`) Step parameter values reaching the child's
//! `$inputs`. Arazzo 1.1.0 Parameter Object `value`: "The value can be a
//! constant, a Runtime Expression, or a Selector Object".

mod common;

use arazzo_spec::{Parameter, Step, StepTarget, Workflow};
use common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Runs `parent` → `child`, passing `value` as the child's `v` input, and
/// returns what the child saw as `$inputs.v`.
async fn child_input(value: Value, parent_inputs: BTreeMap<String, Value>) -> Value {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "parent".to_string(),
            steps: vec![Step {
                step_id: "call-child".to_string(),
                target: Some(StepTarget::WorkflowId("child".to_string())),
                parameters: vec![Parameter {
                    name: "v".to_string(),
                    value: to_yaml(value).into(),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "result".to_string(),
                "$steps.call-child.outputs.received".to_string().into(),
            )]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "noop".to_string(),
                target: Some(StepTarget::OperationPath(
                    "{$sourceDescriptions.test.url}#/paths/~1ok/get".to_string(),
                )),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("received".to_string(), "$inputs.v".to_string().into())]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let outputs = match engine
        .execute_collect("parent", parent_inputs)
        .await
        .outputs
    {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };
    outputs.get("result").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn constant_integer_reaches_child() {
    assert_eq!(child_input(json!(42), BTreeMap::new()).await, json!(42));
}

#[tokio::test]
async fn constant_string_reaches_child() {
    assert_eq!(
        child_input(json!("us-east"), BTreeMap::new()).await,
        json!("us-east")
    );
}

#[tokio::test]
async fn constant_boolean_reaches_child() {
    assert_eq!(child_input(json!(true), BTreeMap::new()).await, json!(true));
}

#[tokio::test]
async fn constant_mapping_reaches_child() {
    let value = json!({"region": "us-east", "limit": 5, "tags": ["a", "b"]});
    assert_eq!(child_input(value.clone(), BTreeMap::new()).await, value);
}

#[tokio::test]
async fn constant_sequence_reaches_child() {
    let value = json!([1, "two", false, {"k": null}]);
    assert_eq!(child_input(value.clone(), BTreeMap::new()).await, value);
}

#[tokio::test]
async fn dollar_prefixed_non_expression_reaches_child_as_text() {
    assert_eq!(
        child_input(json!("$USD"), BTreeMap::new()).await,
        json!("$USD")
    );
}

#[tokio::test]
async fn exact_runtime_expression_preserves_type() {
    let inputs = BTreeMap::from([("x".to_string(), json!(7))]);
    assert_eq!(child_input(json!("$inputs.x"), inputs).await, json!(7));
}

#[tokio::test]
async fn unresolved_runtime_expression_yields_null_not_text() {
    assert_eq!(
        child_input(json!("$inputs.absent"), BTreeMap::new()).await,
        Value::Null
    );
}
