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

    let Some(reference) = value.strip_prefix("$sourceDescriptions.") else {
        return WorkflowDependency::Invalid;
    };
    let Some((source_name, workflow_id)) = reference.split_once('.') else {
        return WorkflowDependency::Invalid;
    };
    if source_name.is_empty()
        || workflow_id.is_empty()
        || !source_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return WorkflowDependency::Invalid;
    }
    WorkflowDependency::External {
        source_name,
        workflow_id,
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
        for value in [
            "$sourceDescriptions.shared name.ready",
            "$sourceDescriptions.shared/name.ready",
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
