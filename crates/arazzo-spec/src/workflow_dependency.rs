//! Classification of workflow-level `dependsOn` and Step `workflowId` references.

use crate::source_reference::parse_source_reference;

/// A workflow-level dependency reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowDependency<'a> {
    /// A workflow defined in the current Arazzo Description.
    Local(&'a str),
    /// A workflow defined in a separate Arazzo Description.
    External {
        source_name: &'a str,
        workflow_id: &'a str,
    },
    /// Empty values and malformed runtime expressions.
    Invalid,
}

/// Classifies an Arazzo workflow-level `dependsOn` or Step `workflowId` value.
///
/// The specification permits either a local `workflowId` or the exact
/// `$sourceDescriptions.<name>.<workflowId>` runtime-expression form. Other
/// `$`-prefixed forms are not workflow dependency references.
pub fn classify_workflow_dependency(value: &str) -> WorkflowDependency<'_> {
    if value.is_empty() {
        return WorkflowDependency::Invalid;
    }
    if !value.starts_with('$') {
        return WorkflowDependency::Local(value);
    }

    let Some(reference) = value.strip_prefix("$sourceDescriptions.") else {
        return WorkflowDependency::Invalid;
    };
    // The `source-reference` production itself lives in `source_reference.rs`;
    // a step's `operationId` reads the same one.
    match parse_source_reference(reference) {
        Ok(parsed) => WorkflowDependency::External {
            source_name: parsed.source_name,
            workflow_id: parsed.reference_id,
        },
        Err(_) => WorkflowDependency::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_workflow_dependency, WorkflowDependency};

    #[test]
    fn step_and_depends_on_forms_share_classification() -> Result<(), Box<dyn std::error::Error>> {
        for value in [
            "local",
            "$sourceDescriptions.child.flow",
            "$workflows.local",
        ] {
            let document = format!(
                r#"{{"arazzo":"1.1.0","info":{{"title":"Refs","version":"1"}},"sourceDescriptions":[],"workflows":[{{"workflowId":"parent","dependsOn":["{value}"],"steps":[{{"stepId":"call","workflowId":"{value}"}}]}}]}}"#
            );
            let spec = crate::parse_unvalidated_bytes(document.as_bytes())?;
            let workflow = &spec.workflows[0];
            let Some(crate::StepTarget::WorkflowId(step_value)) = &workflow.steps[0].target else {
                panic!("workflow target");
            };
            assert_eq!(
                classify_workflow_dependency(step_value),
                classify_workflow_dependency(&workflow.depends_on[0])
            );
        }
        Ok(())
    }

    #[test]
    fn classifies_local_and_external_references() {
        assert_eq!(
            classify_workflow_dependency("prepare"),
            WorkflowDependency::Local("prepare")
        );
        assert_eq!(
            classify_workflow_dependency("$sourceDescriptions.shared.ready"),
            WorkflowDependency::External {
                source_name: "shared",
                workflow_id: "ready"
            }
        );
    }

    #[test]
    fn rejects_empty_malformed_and_step_only_forms() {
        for value in [
            "",
            "$workflows.ready",
            "$sourceDescriptions.shared",
            "$sourceDescriptions..ready",
            "$sourceDescriptions.shared.",
        ] {
            assert_eq!(
                classify_workflow_dependency(value),
                WorkflowDependency::Invalid,
                "{value}"
            );
        }
    }

    #[test]
    fn accepts_dotted_external_workflow_ids_and_rejects_non_strict_source_names() {
        assert_eq!(
            classify_workflow_dependency("$sourceDescriptions.shared.ready.extra"),
            WorkflowDependency::External {
                source_name: "shared",
                workflow_id: "ready.extra"
            }
        );
        assert_eq!(
            classify_workflow_dependency("$sourceDescriptions.shared.ready\\n"),
            WorkflowDependency::External {
                source_name: "shared",
                workflow_id: "ready\\n"
            }
        );
        for value in [
            "$sourceDescriptions.shared name.ready",
            "$sourceDescriptions.shared/name.ready",
            "$sourceDescriptions.shared.",
            "$sourceDescriptions.shared.ready{bad}",
            "$sourceDescriptions.shared.ready}bad",
            "$sourceDescriptions.shared.ready\n",
            "$sourceDescriptions.shared.ready\t",
            "$sourceDescriptions.shared.ready\\q",
            "$sourceDescriptions.shared.ready\\u12",
            "$sourceDescriptions.shared.ready\\",
        ] {
            assert_eq!(
                classify_workflow_dependency(value),
                WorkflowDependency::Invalid,
                "{value}"
            );
        }
    }
}
