//! Source-type applicability of asynchronous step fields.

use arazzo_spec::{
    classify_operation_id, classify_operation_path, ArazzoSpec, OperationIdTarget,
    OperationPathTarget, SourceType, Step, StepTarget,
};

use crate::{Diagnostic, Severity, ValidationErrorKind};

pub(crate) fn check(
    spec: &ArazzoSpec,
    _workflow_index: usize,
    _step_index: usize,
    step: &Step,
    step_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let source_name = match &step.target {
        Some(StepTarget::OperationId(value)) => match classify_operation_id(value) {
            OperationIdTarget::SourceQualified { source_name, .. } => source_name,
            _ => return,
        },
        Some(StepTarget::ChannelPath(value)) => match classify_operation_path(value) {
            OperationPathTarget::SourcePointer { source_name, .. } => source_name,
            _ => return,
        },
        _ => return,
    };
    let Some(source) = spec
        .source_descriptions
        .iter()
        .find(|source| source.name == source_name)
    else {
        return;
    };
    if source.type_ == SourceType::AsyncApi {
        return;
    }

    if matches!(step.target, Some(StepTarget::ChannelPath(_))) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidAsyncStep,
            path: format!("{step_path}.channelPath"),
            message: format!(
                "channelPath must reference an asyncapi source; source \"{source_name}\" is type {}",
                source.type_
            ),
        });
    } else {
        for (field, present) in [
            ("action", step.action.is_some()),
            ("correlationId", step.correlation_id.is_some()),
        ] {
            if present {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidAsyncStep,
                    path: format!("{step_path}.{field}"),
                    message: format!(
                        "{field} is only applicable to asyncapi steps; source \"{source_name}\" is type {}",
                        source.type_
                    ),
                });
            }
        }
    }
}
