//! Source routing of a step's `operationId`.
//!
//! Arazzo 1.1.0 Step Object, `operationId`: *"If multiple (non arazzo type)
//! sourceDescriptions are defined, then the operationId MUST be specified
//! using a Runtime Expression (e.g., $sourceDescriptions.<name>.<operationId>)
//! to avoid ambiguity or potential clashes."*
//!
//! The same section describes a step as *"a call to an API operation (OpenAPI
//! Operation Object), (AysncAPI Operations Object) or another Workflow
//! Object"*, and its async step example routes `operationId:
//! $sourceDescriptions.asyncOrderApi.placeOrder` to an AsyncAPI source. Only
//! an `arazzo` source, which describes workflows rather than operations, is
//! refused as an `operationId` target.

use std::collections::HashMap;

use arazzo_spec::{
    classify_operation_id, OperationIdTarget, SourceType, SUPPORTED_OPERATION_ID_FORMS,
};

use crate::{Diagnostic, Severity, ValidationErrorKind};

const QUALIFICATION_REQUIREMENT: &str = "If multiple (non arazzo type) sourceDescriptions are \
     defined, then the operationId MUST be specified using a Runtime Expression (e.g., \
     $sourceDescriptions.<name>.<operationId>) to avoid ambiguity or potential clashes.";

pub(crate) fn check(
    value: &str,
    step_path: &str,
    source_types: &HashMap<&str, SourceType>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = format!("{step_path}.operationId");
    let (kind, message) = match classify_operation_id(value) {
        OperationIdTarget::Bare(name) => {
            let operation_sources = source_types
                .values()
                .filter(|source_type| **source_type != SourceType::Arazzo)
                .count();
            if operation_sources < 2 {
                return;
            }
            (
                ValidationErrorKind::InvalidStepTarget,
                format!(
                    "{path} \"{name}\" must name its source as \
                     $sourceDescriptions.<name>.{name} because {operation_sources} non-arazzo \
                     sourceDescriptions are defined: \"{QUALIFICATION_REQUIREMENT}\""
                ),
            )
        }
        OperationIdTarget::SourceQualified { source_name, .. } => {
            match source_types.get(source_name) {
                None => (
                    ValidationErrorKind::InvalidReference,
                    format!(
                        "{path} \"{value}\" references unknown sourceDescription \"{source_name}\""
                    ),
                ),
                Some(SourceType::Arazzo) => (
                    ValidationErrorKind::InvalidReference,
                    format!(
                        "{path} \"{value}\" names sourceDescription \"{source_name}\", which has \
                         type arazzo and describes workflows rather than operations; use \
                         workflowId to reference its workflows"
                    ),
                ),
                Some(SourceType::OpenApi | SourceType::AsyncApi) => return,
            }
        }
        OperationIdTarget::Malformed(reason) => (
            ValidationErrorKind::InvalidStepTarget,
            format!("{path} \"{value}\" {reason}; expected {SUPPORTED_OPERATION_ID_FORMS}"),
        ),
    };
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind,
        path,
        message,
    });
}
