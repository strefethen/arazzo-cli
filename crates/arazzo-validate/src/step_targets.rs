use std::collections::{HashMap, HashSet};

use arazzo_spec::{
    classify_operation_path, classify_workflow_dependency, SourceType, Step, StepTarget,
    WorkflowDependency, SUPPORTED_OPERATION_PATH_FORMS,
};

use crate::{Diagnostic, Severity, ValidationErrorKind};

pub(crate) fn check(
    step: &Step,
    step_path: &str,
    source_names: &HashSet<&str>,
    workflow_ids: &HashSet<&str>,
    source_types: &HashMap<&str, SourceType>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &step.target {
        Some(StepTarget::OperationPath(operation_path)) => {
            let classified = classify_operation_path(operation_path);
            if let Some(source_name) = classified.source_name() {
                if !source_names.contains(source_name) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidReference,
                        path: format!("{step_path}.operationPath"),
                        message: format!(
                            "{step_path}.operationPath references unknown sourceDescription \"{source_name}\""
                        ),
                    });
                }
            }
            if let Some(reason) = classified.malformed() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::UnsupportedOperationPath,
                    path: format!("{step_path}.operationPath"),
                    message: format!(
                        "{step_path}.operationPath {reason}; expected {SUPPORTED_OPERATION_PATH_FORMS}"
                    ),
                });
            }
            if step.action.is_some() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidAsyncStep,
                    path: format!("{step_path}.action"),
                    message: format!(
                        "{step_path}.action is only supported with operationId or channelPath async targets"
                    ),
                });
            }
        }
        Some(StepTarget::ChannelPath(_)) if step.action.is_none() => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidAsyncStep,
                path: format!("{step_path}.action"),
                message: format!("{step_path}.channelPath requires action send or receive"),
            });
        }
        Some(StepTarget::WorkflowId(value)) => {
            check_workflow(value, step_path, workflow_ids, source_types, diagnostics);
            if step.action.is_some() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidAsyncStep,
                    path: format!("{step_path}.action"),
                    message: format!("{step_path}.action is not applicable to workflowId targets"),
                });
            }
        }
        _ => {}
    }
}

fn check_workflow(
    value: &str,
    step_path: &str,
    workflow_ids: &HashSet<&str>,
    source_types: &HashMap<&str, SourceType>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = format!("{step_path}.workflowId");
    let message = match classify_workflow_dependency(value) {
        WorkflowDependency::Local(id) if workflow_ids.contains(id) => return,
        WorkflowDependency::Local(id) => format!("{path} references unknown local workflow \"{id}\""),
        WorkflowDependency::External { source_name, workflow_id } => match source_types.get(source_name) {
            None => format!("{path} references unknown sourceDescription \"{source_name}\""),
            Some(SourceType::Arazzo) => {
                diagnostics.push(Diagnostic::warning(
                    ValidationErrorKind::UnsupportedWorkflowId,
                    path.clone(),
                    format!("{path} external workflow \"{workflow_id}\" from Arazzo source \"{source_name}\" is valid Arazzo, but this runtime does not load external Arazzo documents; running this step will fail"),
                ));
                return;
            }
            Some(source_type) => format!("{path} sourceDescription \"{source_name}\" has type {source_type}, expected type arazzo"),
        },
        WorkflowDependency::Invalid => {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidStepTarget,
                path: path.clone(),
                message: format!("{path} contains invalid workflow reference \"{value}\""),
            });
            return;
        }
    };
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind: ValidationErrorKind::InvalidReference,
        path,
        message,
    });
}
