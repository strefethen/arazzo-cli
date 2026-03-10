mod common;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{JsonSchemaType, PropertyDef, SchemaObject, Step, StepTarget, Workflow};
use common::*;
use std::collections::BTreeMap;

fn workflow_with_required_input() -> Workflow {
    let mut properties = BTreeMap::new();
    properties.insert(
        "name".to_string(),
        PropertyDef {
            type_: Some(JsonSchemaType::String),
            ..PropertyDef::default()
        },
    );
    Workflow {
        workflow_id: "test-inputs".to_string(),
        inputs: Some(SchemaObject {
            properties,
            required: vec!["name".to_string()],
            ..SchemaObject::default()
        }),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/test".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }
}

#[tokio::test]
async fn strict_inputs_rejects_missing_required() {
    let spec = make_spec(vec![workflow_with_required_input()]);
    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .strict_inputs(true)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("test-inputs", BTreeMap::new()).await;

    match result.outputs {
        Err(err) => {
            assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
            assert!(
                err.message.contains("required"),
                "error message should mention 'required': {}",
                err.message
            );
        }
        Ok(outputs) => panic!("expected InputValidation error, got outputs: {outputs:?}"),
    }
}

#[tokio::test]
async fn non_strict_does_not_reject() {
    let spec = make_spec(vec![workflow_with_required_input()]);
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("test-inputs", BTreeMap::new()).await;

    // Should not be an InputValidation error. It may fail for other reasons
    // (e.g. dry-run produces empty outputs), but not input validation.
    if let Err(err) = &result.outputs {
        assert_ne!(
            err.kind,
            RuntimeErrorKind::InputValidation,
            "non-strict mode should not produce InputValidation error"
        );
    }
}
