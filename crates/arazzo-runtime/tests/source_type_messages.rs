#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{ArazzoSpec, SourceDescription, SourceType, Step, StepTarget, Workflow};

#[tokio::test]
async fn arazzo_operation_error_does_not_recommend_external_workflow() {
    let spec = ArazzoSpec {
        source_descriptions: vec![SourceDescription {
            name: "child".into(),
            url: "child.yaml".into(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "parent".into(),
            steps: vec![Step {
                step_id: "call".into(),
                target: Some(StepTarget::OperationId(
                    "$sourceDescriptions.child.flow".into(),
                )),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    };
    let engine = EngineBuilder::new(spec)
        .dry_run(true)
        .build()
        .expect("engine");
    let result = engine.execute_collect("parent", BTreeMap::new()).await;
    let error = result
        .outputs
        .as_ref()
        .expect_err("arazzo source cannot supply operations");
    assert_eq!(
        error.kind,
        RuntimeErrorKind::UnsupportedSourceDescriptionType
    );
    assert!(error.message.contains("workflowId"));
    assert!(error
        .message
        .contains("cross-document workflow references are unsupported"));
    assert!(!error.message.contains("name it from workflowId"));
    assert!(!error.message.contains("<workflowId>"));
    assert_eq!(error.message.matches("$sourceDescriptions.").count(), 1);
    assert!(result.dry_run_requests().is_empty());
}
