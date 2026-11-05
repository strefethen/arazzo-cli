use arazzo_validate::{parse_bytes_with_diagnostics, Error, Severity};

fn document(target: &str) -> String {
    format!(
        r#"arazzo: 1.1.0
info: {{title: Targets, version: '1'}}
sourceDescriptions:
  - {{name: child, url: child.yaml, type: arazzo}}
  - {{name: api, url: api.yaml, type: openapi}}
  - {{name: events, url: events.yaml, type: asyncapi}}
workflows:
  - workflowId: parent
    steps:
      - stepId: call
        workflowId: '{target}'
  - workflowId: local
    steps:
      - stepId: operation
        operationId: ping
"#
    )
}

#[test]
fn step_workflow_target_classifications() {
    for (target, expected) in [
        ("local", None),
        ("missing", Some((Severity::Error, "invalidReference"))),
        ("Local", Some((Severity::Error, "invalidReference"))),
        (
            "$sourceDescriptions.absent.flow",
            Some((Severity::Error, "invalidReference")),
        ),
        (
            "$sourceDescriptions.api.flow",
            Some((Severity::Error, "invalidReference")),
        ),
        (
            "$sourceDescriptions.events.flow",
            Some((Severity::Error, "invalidReference")),
        ),
        (
            "$sourceDescriptions.child.flow",
            Some((Severity::Warning, "unsupportedWorkflowId")),
        ),
        (
            "$workflows.local",
            Some((Severity::Error, "invalidStepTarget")),
        ),
        (
            "$sourceDescriptions.child.",
            Some((Severity::Error, "invalidStepTarget")),
        ),
    ] {
        let result = parse_bytes_with_diagnostics(document(target).as_bytes());
        let diagnostics = match result {
            Ok((_, diagnostics)) => {
                assert!(
                    expected.is_none_or(|(severity, _)| severity == Severity::Warning),
                    "{target} must fail"
                );
                diagnostics
            }
            Err(Error::Validation(report)) => {
                assert!(
                    expected.is_some_and(|(severity, _)| severity == Severity::Error),
                    "{target} must stay valid"
                );
                report.errors
            }
            Err(error) => panic!("{target}: {error}"),
        };
        match expected {
            None => assert!(diagnostics.is_empty(), "{diagnostics:?}"),
            Some((severity, kind)) => {
                assert_eq!(diagnostics.len(), 1, "{target}: {diagnostics:?}");
                assert_eq!(diagnostics[0].severity, severity);
                assert_eq!(diagnostics[0].kind.name(), kind);
                assert_eq!(
                    diagnostics[0].path,
                    "workflow \"parent\" > step \"call\".workflowId"
                );
            }
        }
    }
}
