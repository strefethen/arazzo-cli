//! Classification of workflow-level `dependsOn` references.

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

/// Classifies an Arazzo workflow-level `dependsOn` value.
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

    let parts = value.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["$sourceDescriptions", source_name, workflow_id]
            if !source_name.is_empty() && !workflow_id.is_empty() =>
        {
            WorkflowDependency::External {
                source_name,
                workflow_id,
            }
        }
        _ => WorkflowDependency::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_workflow_dependency, WorkflowDependency};

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
            "$sourceDescriptions.shared.ready.extra",
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
}
