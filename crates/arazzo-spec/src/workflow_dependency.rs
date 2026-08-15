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
        || !source_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !is_source_reference_id(workflow_id)
    {
        return WorkflowDependency::Invalid;
    }
    WorkflowDependency::External {
        source_name,
        workflow_id,
    }
}

/// Validates the vendored `CHAR` rule used by `source-reference-id`.
///
/// The rule permits Unicode characters except `{`, `}`, `"`, and `\\`, with
/// JSON-style escapes for those characters and for controls. The
/// source-reference-id itself must contain at least one CHAR token.
fn is_source_reference_id(value: &str) -> bool {
    let mut chars = value.chars();
    let mut token_count = 0;
    while let Some(ch) = chars.next() {
        token_count += 1;
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                return false;
            };
            match escaped {
                '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => {}
                'u' => {
                    if (0..4).any(|_| chars.next().is_none_or(|hex| !hex.is_ascii_hexdigit())) {
                        return false;
                    }
                }
                _ => return false,
            }
        } else if !matches!(
            ch as u32,
            0x20..=0x21 | 0x23..=0x5b | 0x5d..=0x7a | 0x7c | 0x7e..=0x10ffff
        ) {
            return false;
        }
    }
    token_count > 0
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
