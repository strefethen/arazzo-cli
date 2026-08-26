#![forbid(unsafe_code)]

//! Validation layer for parsed Arazzo specifications.

// The conformance manifest's hermetic source scanner does not recognize the
// nested tests in this large library module. Keep these minimal executable
// adapters before the library items and delegate all assertions to the direct,
// comprehensive YAML/JSON validation tests below.
#[cfg(test)]
#[test]
fn conformance_required_any_values_positive_evidence() {
    tests::raw_required_values_accept_concrete_any_values_for_yaml_and_json();
}

#[cfg(test)]
#[test]
fn conformance_required_any_values_negative_evidence() {
    tests::raw_required_values_report_exact_paths_for_yaml_and_json();
}

// See the note above the required-Any adapters. These two execute the complete
// Parameter context matrix while keeping the manifest's evidence references
// stable and scanner-visible.
#[cfg(test)]
#[test]
fn conformance_parameter_context_positive_evidence() {
    tests::parameter_context_positive_matrix();
}

#[cfg(test)]
#[test]
fn conformance_parameter_context_negative_evidence() {
    tests::parameter_context_negative_matrix();
}

#[cfg(test)]
#[test]
fn conformance_action_fixed_fields_positive_evidence() {
    tests::action_fixed_fields_positive_matrix();
}

#[cfg(test)]
#[test]
fn conformance_action_fixed_fields_negative_evidence() {
    tests::action_fixed_fields_negative_matrix();
}

#[cfg(test)]
#[test]
fn conformance_decimal_retry_after_positive_evidence() {
    tests::decimal_retry_after_positive_matrix();
}

#[cfg(test)]
#[test]
fn conformance_decimal_retry_after_negative_evidence() {
    tests::decimal_retry_after_negative_matrix();
}

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use arazzo_spec::{
    classify_operation_path, classify_workflow_dependency, parse_unvalidated_bytes,
    unrecognized_fields, ActionType, ArazzoSpec, OnAction, OutputValue, ParamLocation, Parameter,
    SelectorObject, SelectorType, SourceType, Step, StepAction, StepTarget, SuccessCriterion,
    ValueSource, VendorExtensions, Workflow, WorkflowDependency, SUPPORTED_OPERATION_PATH_FORMS,
};
use iri_string::types::UriReferenceStr;

mod xpath_advisory;

/// Parser/validation error type for Arazzo specs.
#[derive(Debug)]
pub enum Error {
    ReadFile(std::io::Error),
    ParseYaml(serde_yaml_ng::Error),
    Validation(ValidationReport),
    /// Component resolution error (pre-validation).
    ComponentResolution(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFile(err) => write!(f, "reading arazzo file: {err}"),
            Self::ParseYaml(err) => write!(f, "parsing arazzo yaml: {err}"),
            Self::Validation(report) => write!(f, "{report}"),
            Self::ComponentResolution(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFile(err) => Some(err),
            Self::Validation(report) => Some(report),
            Self::ParseYaml(err) => Some(err),
            Self::ComponentResolution(_) => None,
        }
    }
}

/// A collection of structural validation findings for an Arazzo spec.
///
/// `errors` are fatal; `warnings` never fail validation on their own and are
/// carried alongside so callers on the failure path can still surface them.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "validation errors:")?;
        for err in &self.errors {
            write!(f, "\n  - {err}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationReport {}

/// Whether a diagnostic fails validation or is merely advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A single validation finding with severity, kind, spec path, and message.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: ValidationErrorKind,
    pub path: String,
    pub message: String,
}

impl Diagnostic {
    /// Builds a warning-severity diagnostic.
    pub fn warning(
        kind: ValidationErrorKind,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            kind,
            path: path.into(),
            message: message.into(),
        }
    }

    /// Returns true when this diagnostic fails validation.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Reclassifies this diagnostic as an error, for strict callers that
    /// promote every warning.
    pub fn into_error(self) -> Self {
        Self {
            severity: Severity::Error,
            ..self
        }
    }
}

/// Retained name for the error-severity view of a [`Diagnostic`].
pub type ValidationError = Diagnostic;

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for Diagnostic {}

/// Classification of validation errors for programmatic matching.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValidationErrorKind {
    MissingRequiredField,
    DuplicateIdentifier,
    InvalidStepTarget,
    UnsupportedVersion,
    InvalidParameterLocation,
    MissingParameterValue,
    InvalidExpression,
    InvalidReference,
    InvalidAsyncStep,
    UnsupportedDependencyScope,
    DependencyCycle,
    InvalidRetryField,
    InvalidCriterionType,
    InvalidSelectorType,
    /// A specification-valid `operationPath` that this runtime cannot resolve.
    /// Warning severity: the document is conformant, the executor is not.
    UnsupportedOperationPath,
    /// A specification-valid XPath `type`/`version` declaration that this
    /// runtime will not execute: only an explicit `xpath-10` reaches the
    /// XPath 1.0 engine, while the §5.8.12.1 table also allows `xpath-31`
    /// (the omitted-version default), `xpath-30`, and `xpath-20`. Warning
    /// severity, promoted to error only under `--strict`: the document is
    /// conformant, the executor is not. See [`xpath_advisory`].
    UnsupportedXpathVersion,
    /// A workflow/step `outputs` key or Components map key that violates the
    /// specification's `^[a-zA-Z0-9\.\-_]+$` MUST-level regular expression
    /// (error severity), or a `workflowId`/`stepId`/`sourceDescriptions[].name`
    /// value that violates the SHOULD-level `^[A-Za-z0-9_\-]+$` recommendation
    /// (warning severity, promoted to error only under `--strict`). Severity
    /// distinguishes the two; the kind is shared.
    InvalidIdentifier,
    /// A field that is neither a modeled field nor a `x-*` Specification
    /// Extension. Warning severity, promoted to error only under `--strict`:
    /// the specification reserves `x-` for extensions but states no
    /// MUST-reject for an unrecognized field, so this is advisory rather than
    /// fatal by default. Not raised inside a JSON Schema position (workflow
    /// or `components.inputs`), where an unmodeled keyword is legitimate JSON
    /// Schema, not a mistake.
    UnknownField,
}

impl ValidationErrorKind {
    /// Returns the stable camelCase name used in the CLI `--json` contract.
    /// Renaming a value is a breaking contract change, like a `RUNTIME_*` code.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::MissingRequiredField => "missingRequiredField",
            Self::DuplicateIdentifier => "duplicateIdentifier",
            Self::InvalidStepTarget => "invalidStepTarget",
            Self::UnsupportedVersion => "unsupportedVersion",
            Self::InvalidParameterLocation => "invalidParameterLocation",
            Self::MissingParameterValue => "missingParameterValue",
            Self::InvalidExpression => "invalidExpression",
            Self::InvalidReference => "invalidReference",
            Self::InvalidAsyncStep => "invalidAsyncStep",
            Self::UnsupportedDependencyScope => "unsupportedDependencyScope",
            Self::DependencyCycle => "dependencyCycle",
            Self::InvalidRetryField => "invalidRetryField",
            Self::InvalidCriterionType => "invalidCriterionType",
            Self::InvalidSelectorType => "invalidSelectorType",
            Self::UnsupportedOperationPath => "unsupportedOperationPath",
            Self::UnsupportedXpathVersion => "unsupportedXpathVersion",
            Self::InvalidIdentifier => "invalidIdentifier",
            Self::UnknownField => "unknownField",
        }
    }
}

/// Parses and validates an Arazzo spec file from disk, discarding warnings.
pub fn parse(path: impl AsRef<Path>) -> Result<ArazzoSpec, Error> {
    parse_with_diagnostics(path).map(|(spec, _)| spec)
}

/// Parses and validates an Arazzo spec from raw YAML bytes, discarding warnings.
pub fn parse_bytes(data: &[u8]) -> Result<ArazzoSpec, Error> {
    parse_bytes_with_diagnostics(data).map(|(spec, _)| spec)
}

/// Parses and validates an Arazzo spec file from disk, returning any
/// non-fatal diagnostics alongside the spec.
///
/// Errors still fail; warnings never do.
pub fn parse_with_diagnostics(
    path: impl AsRef<Path>,
) -> Result<(ArazzoSpec, Vec<Diagnostic>), Error> {
    let bytes = fs::read(path).map_err(Error::ReadFile)?;
    parse_bytes_with_diagnostics(&bytes)
}

/// Parses and validates an Arazzo spec from raw YAML bytes, returning any
/// non-fatal diagnostics alongside the spec.
pub fn parse_bytes_with_diagnostics(data: &[u8]) -> Result<(ArazzoSpec, Vec<Diagnostic>), Error> {
    // The typed model intentionally keeps its public `value` fields as the
    // declared `ValueSource` types. It cannot retain whether a defaulted null
    // came from an omitted key or from an explicit Any value, so retain the
    // parsed document only inside this boundary pass.
    let raw = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(data).map_err(Error::ParseYaml)?;
    let mut spec = parse_unvalidated_bytes(data).map_err(Error::ParseYaml)?;
    let mut provenance =
        resolve_components(&mut spec, Some(&raw)).map_err(Error::ComponentResolution)?;

    // `successCriteria` emptiness cannot be seen in the typed model — see the
    // comment on `check_raw_success_criteria` — so it is checked against the
    // raw bytes here, the one place both the document text and the rest of
    // the diagnostics pipeline are in hand. `validate`/`validate_diagnostics`
    // take only `&ArazzoSpec` and therefore cannot enforce this rule.
    let mut diagnostics = collect_diagnostics(&spec, &provenance);
    diagnostics.append(&mut provenance.raw_legacy_action_diagnostics);
    diagnostics.extend(check_raw_required_values(&raw));
    diagnostics.extend(check_raw_action_field_boundaries(&raw));
    diagnostics.extend(check_raw_success_criteria(&raw));
    let warnings = partition_diagnostics(diagnostics)?;
    Ok((spec, warnings))
}

/// Applies structural validation rules to an Arazzo spec.
///
/// Warnings are dropped; use [`validate_diagnostics`] to receive them.
pub fn validate(spec: &ArazzoSpec) -> Result<(), Error> {
    validate_diagnostics(spec).map(|_| ())
}

/// Applies structural validation rules, returning warnings on success and
/// failing with a report that also carries them on error.
pub fn validate_diagnostics(spec: &ArazzoSpec) -> Result<Vec<Diagnostic>, Error> {
    // Component references affect Parameter name, location, and action context.
    // Resolve them on a private clone so the direct validation APIs enforce the
    // same contract as the parse entry points without mutating the caller's
    // public model. Only omitted-vs-explicit-null `value` presence requires raw
    // document bytes and remains a parse-boundary distinction.
    let mut resolved = spec.clone();
    let mut provenance =
        resolve_components(&mut resolved, None).map_err(Error::ComponentResolution)?;
    // Resolution records invalid typed wrapper values before it replaces a
    // Parameter value or discards an action value. The raw parse entry point
    // performs the equivalent presence-aware pass, so this vector is populated
    // only for direct typed APIs.
    let mut diagnostics = std::mem::take(&mut provenance.reusable_value_diagnostics);
    diagnostics.extend(collect_diagnostics(&resolved, &provenance));
    partition_diagnostics(diagnostics)
}

/// Splits a diagnostics list into a success (warnings only) or failure
/// (report carrying both errors and warnings) result. Shared by
/// [`validate_diagnostics`] and [`parse_bytes_with_diagnostics`], which feeds
/// it an extra diagnostic source ([`check_raw_success_criteria`]) that only
/// it can see.
fn partition_diagnostics(diagnostics: Vec<Diagnostic>) -> Result<Vec<Diagnostic>, Error> {
    let (errors, warnings) = diagnostics
        .into_iter()
        .partition::<Vec<Diagnostic>, _>(Diagnostic::is_error);

    if errors.is_empty() {
        return Ok(warnings);
    }
    Err(Error::Validation(ValidationReport { errors, warnings }))
}

fn raw_mapping_field<'a>(
    value: &'a serde_yaml_ng::Value,
    key: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    let serde_yaml_ng::Value::Mapping(mapping) = value else {
        return None;
    };
    mapping.get(serde_yaml_ng::Value::String(key.to_string()))
}

fn raw_string_field<'a>(value: &'a serde_yaml_ng::Value, key: &str) -> Option<&'a str> {
    match raw_mapping_field(value, key) {
        Some(serde_yaml_ng::Value::String(value)) => Some(value),
        _ => None,
    }
}

fn raw_mapping_has_field(value: &serde_yaml_ng::Value, key: &str) -> bool {
    raw_mapping_field(value, key).is_some()
}

fn raw_parameter_value_is_present(
    parameters: Option<&serde_yaml_ng::Value>,
    parameter_index: usize,
) -> bool {
    parameters
        .and_then(serde_yaml_ng::Value::as_sequence)
        .and_then(|parameters| parameters.get(parameter_index))
        .is_some_and(|parameter| raw_mapping_has_field(parameter, "value"))
}

fn raw_component_action<'a>(
    components: Option<&'a serde_yaml_ng::Value>,
    field: &str,
    name: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    raw_mapping_field(components?, field)?
        .as_mapping()?
        .get(name)
}

/// Reusable Object `value` is an optional string, unlike the required Any
/// `value` on a concrete Parameter Object or Payload Replacement Object.
fn check_raw_reusable_value(
    reusable: &serde_yaml_ng::Value,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(value) = raw_mapping_field(reusable, "value") else {
        return;
    };
    if matches!(value, serde_yaml_ng::Value::String(_)) {
        return;
    }

    diagnostics.push(invalid_reusable_value_diagnostic(path));
}

fn invalid_reusable_value_diagnostic(path: &str) -> Diagnostic {
    let value_path = format!("{path}.value");
    Diagnostic {
        severity: Severity::Error,
        kind: ValidationErrorKind::InvalidReference,
        path: value_path.clone(),
        message: format!("{value_path} must be a string when used on a Reusable Object"),
    }
}

fn check_raw_parameter_values(
    parameters: Option<&serde_yaml_ng::Value>,
    path: &str,
    permits_reusable_objects: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(parameters) = parameters.and_then(serde_yaml_ng::Value::as_sequence) else {
        return;
    };
    for (index, parameter) in parameters.iter().enumerate() {
        let Some(_) = parameter.as_mapping() else {
            continue;
        };
        // Workflow, Step, and Action lists are Parameter Object | Reusable
        // Object unions. Components.parameters is a map of Parameter Objects,
        // so a stray `reference` there cannot waive Parameter.value.
        if permits_reusable_objects && raw_mapping_has_field(parameter, "reference") {
            check_raw_reusable_value(parameter, &format!("{path}[{index}]"), diagnostics);
            continue;
        }
        if !raw_mapping_has_field(parameter, "value") {
            let value_path = format!("{path}[{index}].value");
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: value_path.clone(),
                message: format!("{value_path} is required"),
            });
        }
    }
}

fn check_raw_action_parameter_values(
    actions: Option<&serde_yaml_ng::Value>,
    path: &str,
    skip_reusable_actions: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(actions) = actions.and_then(serde_yaml_ng::Value::as_sequence) else {
        return;
    };
    for (index, action) in actions.iter().enumerate() {
        let Some(_) = action.as_mapping() else {
            continue;
        };
        // Workflow/Step action lists are Action Object | Reusable Object
        // unions. A reusable action ignores extra fields, including a nested
        // parameters list, so do not report through that ignored shape.
        if skip_reusable_actions && raw_mapping_has_field(action, "reference") {
            continue;
        }
        check_raw_parameter_values(
            raw_mapping_field(action, "parameters"),
            &format!("{path}[{index}].parameters"),
            true,
            diagnostics,
        );
    }
}

fn check_raw_replacement_values(
    request_body: Option<&serde_yaml_ng::Value>,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(replacements) = request_body
        .and_then(|body| raw_mapping_field(body, "replacements"))
        .and_then(serde_yaml_ng::Value::as_sequence)
    else {
        return;
    };
    for (index, replacement) in replacements.iter().enumerate() {
        let Some(_) = replacement.as_mapping() else {
            continue;
        };
        if !raw_mapping_has_field(replacement, "value") {
            let value_path = format!("{path}.replacements[{index}].value");
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: value_path.clone(),
                message: format!("{value_path} is required"),
            });
        }
    }
}

/// Required `value` fields accept Any, including YAML null and empty values.
/// The typed defaults intentionally erase that presence bit, so inspect only
/// well-formed container shapes at the parse boundary. Serde owns malformed
/// maps and lists; this pass must not add a second, less-specific diagnostic.
fn check_raw_required_values(root: &serde_yaml_ng::Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let components = raw_mapping_field(root, "components");
    if let Some(parameters) = components
        .and_then(|components| raw_mapping_field(components, "parameters"))
        .and_then(serde_yaml_ng::Value::as_mapping)
    {
        for (name, parameter) in parameters {
            let Some(name) = name.as_str() else {
                continue;
            };
            if parameter.as_mapping().is_none() {
                continue;
            }
            if !raw_mapping_has_field(parameter, "value") {
                let value_path = format!("components.parameters.{name}.value");
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: value_path.clone(),
                    message: format!("{value_path} is required"),
                });
            }
        }
    }
    if let Some(components) = components {
        for (field, path) in [
            ("successActions", "components.successActions"),
            ("failureActions", "components.failureActions"),
        ] {
            let Some(actions) =
                raw_mapping_field(components, field).and_then(serde_yaml_ng::Value::as_mapping)
            else {
                continue;
            };
            for (name, action) in actions {
                let Some(name) = name.as_str() else {
                    continue;
                };
                // Component maps contain Action Objects, not Reusable Object
                // wrappers, so inspect parameters even when an invalid
                // `reference` field appears on the action.
                check_raw_parameter_values(
                    raw_mapping_field(action, "parameters"),
                    &format!("{path}.{name}.parameters"),
                    true,
                    &mut diagnostics,
                );
            }
        }
    }

    let Some(workflows) =
        raw_mapping_field(root, "workflows").and_then(serde_yaml_ng::Value::as_sequence)
    else {
        return diagnostics;
    };
    for (workflow_index, workflow) in workflows.iter().enumerate() {
        let workflow_path = raw_string_field(workflow, "workflowId")
            .map(|id| format!("workflow \"{id}\""))
            .unwrap_or_else(|| format!("workflows[{workflow_index}]"));
        check_raw_parameter_values(
            raw_mapping_field(workflow, "parameters"),
            &format!("{workflow_path}.parameters"),
            true,
            &mut diagnostics,
        );
        for field in ["successActions", "failureActions"] {
            check_raw_action_parameter_values(
                raw_mapping_field(workflow, field),
                &format!("{workflow_path}.{field}"),
                true,
                &mut diagnostics,
            );
        }
        let Some(steps) =
            raw_mapping_field(workflow, "steps").and_then(serde_yaml_ng::Value::as_sequence)
        else {
            continue;
        };
        for (step_index, step) in steps.iter().enumerate() {
            let step_path = raw_string_field(step, "stepId")
                .map(|id| format!("{workflow_path} > step \"{id}\""))
                .unwrap_or_else(|| format!("{workflow_path} > steps[{step_index}]"));
            check_raw_parameter_values(
                raw_mapping_field(step, "parameters"),
                &format!("{step_path}.parameters"),
                true,
                &mut diagnostics,
            );
            for field in ["onSuccess", "onFailure"] {
                check_raw_action_parameter_values(
                    raw_mapping_field(step, field),
                    &format!("{step_path}.{field}"),
                    true,
                    &mut diagnostics,
                );
            }
            check_raw_replacement_values(
                raw_mapping_field(step, "requestBody"),
                &format!("{step_path}.requestBody"),
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ActionKind {
    Success,
    Failure,
}

impl ActionKind {
    const fn component_prefix(self) -> &'static str {
        match self {
            Self::Success => "$components.successActions.",
            Self::Failure => "$components.failureActions.",
        }
    }
}

fn raw_unknown_action_field(path: &str, field: &str, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.push(Diagnostic::warning(
        ValidationErrorKind::UnknownField,
        path,
        format!(
            "unrecognized field \"{field}\"; only `x-` prefixed extension fields are permitted here"
        ),
    ));
}

fn raw_action_is_legacy_component_reference(
    action: &serde_yaml_ng::Value,
    kind: ActionKind,
) -> bool {
    raw_string_field(action, "name").is_some_and(|name| name.starts_with(kind.component_prefix()))
}

fn check_raw_action_required_field(
    path: &str,
    action: &serde_yaml_ng::Value,
    field: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if matches!(
        raw_mapping_field(action, field),
        Some(serde_yaml_ng::Value::String(_))
    ) {
        return;
    }

    let field_path = format!("{path}.{field}");
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind: ValidationErrorKind::MissingRequiredField,
        path: field_path.clone(),
        message: format!("{field_path} is required and must be a string"),
    });
}

fn raw_defaulted_action_field_is_present(action: &serde_yaml_ng::Value, field: &str) -> bool {
    match raw_mapping_field(action, field) {
        Some(serde_yaml_ng::Value::Null) => true,
        Some(serde_yaml_ng::Value::String(value)) => value.is_empty(),
        Some(value) if field == "retryAfter" => value.as_f64() == Some(0.0),
        Some(value) if field == "retryLimit" => value.as_u64() == Some(0),
        _ => false,
    }
}

fn raw_retry_field_is_null(action: &serde_yaml_ng::Value, field: &str) -> bool {
    matches!(
        raw_mapping_field(action, field),
        Some(serde_yaml_ng::Value::Null)
    )
}

fn raw_action_type(action: &serde_yaml_ng::Value) -> Option<ActionType> {
    match raw_string_field(action, "type")? {
        "end" => Some(ActionType::End),
        "goto" => Some(ActionType::Goto),
        "retry" => Some(ActionType::Retry),
        _ => None,
    }
}

fn raw_action_field_not_applicable(
    path: &str,
    field: &str,
    kind: ValidationErrorKind,
    message: String,
    diagnostics: &mut Vec<Diagnostic>,
) {
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind,
        path: format!("{path}.{field}"),
        message,
    });
}

/// Fields defaulted by the public action model need a narrow raw-presence
/// check. A typed `retry_after: 0.0` or empty target is indistinguishable from
/// omission, but an explicitly supplied field remains non-conformant outside
/// its action context.
fn check_raw_defaulted_action_field_applicability(
    path: &str,
    action: &serde_yaml_ng::Value,
    kind: ActionKind,
    action_type: ActionType,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let retry_is_applicable = kind == ActionKind::Failure && action_type == ActionType::Retry;
    if !retry_is_applicable {
        for field in ["retryAfter", "retryLimit"] {
            if raw_defaulted_action_field_is_present(action, field) {
                raw_action_field_not_applicable(
                    path,
                    field,
                    ValidationErrorKind::InvalidRetryField,
                    format!("{path}.{field} is only applicable to a failure retry action"),
                    diagnostics,
                );
            }
        }
    } else {
        for field in ["retryAfter", "retryLimit"] {
            if raw_retry_field_is_null(action, field) {
                let expected = if field == "retryAfter" {
                    "a non-negative number"
                } else {
                    "a non-negative integer"
                };
                raw_action_field_not_applicable(
                    path,
                    field,
                    ValidationErrorKind::InvalidRetryField,
                    format!("{path}.{field} must be {expected}"),
                    diagnostics,
                );
            }
        }
    }

    if !action_allows_target(kind, action_type) {
        for field in ["workflowId", "stepId"] {
            if raw_defaulted_action_field_is_present(action, field) {
                raw_action_field_not_applicable(
                    path,
                    field,
                    ValidationErrorKind::InvalidReference,
                    format!("{path}.{field} is only applicable to a goto action or failure retry action"),
                    diagnostics,
                );
            }
        }
    }
}

fn check_raw_action_boundary(
    path: &str,
    action: &serde_yaml_ng::Value,
    component: bool,
    kind: ActionKind,
    diagnostics: &mut Vec<Diagnostic>,
) {
    // Action lists and component maps have independently typed containers.
    // Let serde own a malformed entry rather than adding field-level noise.
    if action.as_mapping().is_none() {
        return;
    }

    // Only workflow and Step lists admit a Reusable Object. A canonical
    // `reference` object ignores its siblings; the shipped name-form
    // component extension similarly sources name/type from its component.
    // Components maps always hold concrete Action Objects.
    let canonical_reusable = !component && raw_mapping_has_field(action, "reference");
    let legacy_reusable = !component && raw_action_is_legacy_component_reference(action, kind);
    if component || (!canonical_reusable && !legacy_reusable) {
        check_raw_action_required_field(path, action, "name", diagnostics);
        check_raw_action_required_field(path, action, "type", diagnostics);
        if let Some(action_type) = raw_action_type(action) {
            // Legacy name-form fields are checked after component resolution so
            // omitted local `type` can inherit the effective component context.
            // Canonical Reusable Object siblings remain ignored.
            check_raw_defaulted_action_field_applicability(
                path,
                action,
                kind,
                action_type,
                diagnostics,
            );
        }
    }

    let reference = raw_mapping_field(action, "reference");
    let value = raw_mapping_field(action, "value");
    let reusable_reference_is_invalid = !component
        && reference.is_some_and(|reference| match reference {
            serde_yaml_ng::Value::String(reference) => {
                reference.is_empty() || !reference.starts_with('$')
            }
            _ => true,
        });
    let reference_is_non_string =
        reference.is_some_and(|reference| !matches!(reference, serde_yaml_ng::Value::String(_)));
    if component
        && (reference_is_non_string
            || matches!(reference, Some(serde_yaml_ng::Value::String(reference)) if reference.is_empty()))
    {
        raw_unknown_action_field(path, "reference", diagnostics);
    }
    if component && matches!(value, Some(serde_yaml_ng::Value::Null)) {
        raw_unknown_action_field(path, "value", diagnostics);
    }
    if reusable_reference_is_invalid {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidReference,
            path: path.to_string(),
            message: "reference must be a non-empty runtime expression string".to_string(),
        });
    }
    if !component && reference.is_some() && !reusable_reference_is_invalid {
        check_raw_reusable_value(action, path, diagnostics);
    }
    if !component && matches!(value, Some(serde_yaml_ng::Value::Null)) && reference.is_none() {
        raw_unknown_action_field(path, "value", diagnostics);
    }
}

fn check_raw_action_list(
    value: Option<&serde_yaml_ng::Value>,
    path: &str,
    component: bool,
    kind: ActionKind,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(serde_yaml_ng::Value::Sequence(actions)) = value else {
        return;
    };
    for (index, action) in actions.iter().enumerate() {
        check_raw_action_boundary(
            &format!("{path}[{index}]"),
            action,
            component,
            kind,
            diagnostics,
        );
    }
}

/// Typed `Option<Value>` fields cannot distinguish an omitted `value` from an
/// explicit YAML null, and an empty reference is the typed default. Inspect
/// only these boundary shapes at the parse boundary so diagnostics retain the
/// document's field presence without adding wire metadata to the public model.
fn check_raw_action_field_boundaries(root: &serde_yaml_ng::Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(components) = raw_mapping_field(root, "components") {
        for (field, path, kind) in [
            (
                "successActions",
                "components.successActions",
                ActionKind::Success,
            ),
            (
                "failureActions",
                "components.failureActions",
                ActionKind::Failure,
            ),
        ] {
            let Some(serde_yaml_ng::Value::Mapping(actions)) = raw_mapping_field(components, field)
            else {
                continue;
            };
            for (name, action) in actions {
                let Some(name) = name.as_str() else { continue };
                check_raw_action_boundary(
                    &format!("{path}.{name}"),
                    action,
                    true,
                    kind,
                    &mut diagnostics,
                );
            }
        }
    }

    let Some(serde_yaml_ng::Value::Sequence(workflows)) = raw_mapping_field(root, "workflows")
    else {
        return diagnostics;
    };
    for (workflow_index, workflow) in workflows.iter().enumerate() {
        let workflow_path = raw_string_field(workflow, "workflowId")
            .map(|id| format!("workflow \"{id}\""))
            .unwrap_or_else(|| format!("workflows[{workflow_index}]"));
        for (field, kind) in [
            ("successActions", ActionKind::Success),
            ("failureActions", ActionKind::Failure),
        ] {
            check_raw_action_list(
                raw_mapping_field(workflow, field),
                &format!("{workflow_path}.{field}"),
                false,
                kind,
                &mut diagnostics,
            );
        }
        let Some(serde_yaml_ng::Value::Sequence(steps)) = raw_mapping_field(workflow, "steps")
        else {
            continue;
        };
        for (step_index, step) in steps.iter().enumerate() {
            let step_path = raw_string_field(step, "stepId")
                .map(|id| format!("{workflow_path} > step \"{id}\""))
                .unwrap_or_else(|| format!("{workflow_path} > steps[{step_index}]"));
            for (field, kind) in [
                ("onSuccess", ActionKind::Success),
                ("onFailure", ActionKind::Failure),
            ] {
                check_raw_action_list(
                    raw_mapping_field(step, field),
                    &format!("{step_path}.{field}"),
                    false,
                    kind,
                    &mut diagnostics,
                );
            }
        }
    }
    diagnostics
}

fn typed_reusable_parameter_value_is_valid(value: &ValueSource) -> bool {
    matches!(
        value,
        ValueSource::Literal(serde_yaml_ng::Value::Null | serde_yaml_ng::Value::String(_))
    )
}

fn typed_reusable_action_value_is_valid(value: Option<&serde_yaml_ng::Value>) -> bool {
    matches!(value, None | Some(serde_yaml_ng::Value::String(_)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DeclarationScope {
    ComponentAction {
        kind: ActionKind,
        action_index: usize,
    },
    WorkflowParameters {
        workflow_index: usize,
    },
    StepParameters {
        workflow_index: usize,
        step_index: usize,
    },
    WorkflowAction {
        kind: ActionKind,
        workflow_index: usize,
        action_index: usize,
    },
    StepAction {
        kind: ActionKind,
        workflow_index: usize,
        step_index: usize,
        action_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ActionListScope {
    kind: ActionKind,
    workflow_index: usize,
    step_index: Option<usize>,
}

impl ActionListScope {
    const fn declaration(self, action_index: usize) -> DeclarationScope {
        match self.step_index {
            Some(step_index) => DeclarationScope::StepAction {
                kind: self.kind,
                workflow_index: self.workflow_index,
                step_index,
                action_index,
            },
            None => DeclarationScope::WorkflowAction {
                kind: self.kind,
                workflow_index: self.workflow_index,
                action_index,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ParameterDeclarationKey {
    scope: DeclarationScope,
    parameter_index: usize,
}

impl DeclarationScope {
    const fn declaration(self, parameter_index: usize) -> ParameterDeclarationKey {
        ParameterDeclarationKey {
            scope: self,
            parameter_index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ComponentParameterKey {
    parameter_index: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActionLocalFields {
    type_: bool,
    workflow_id: bool,
    step_id: bool,
    retry_after: bool,
    retry_limit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionFixedField {
    Type,
    WorkflowId,
    StepId,
    RetryAfter,
    RetryLimit,
}

impl ActionLocalFields {
    const fn contains(self, field: ActionFixedField) -> bool {
        match field {
            ActionFixedField::Type => self.type_,
            ActionFixedField::WorkflowId => self.workflow_id,
            ActionFixedField::StepId => self.step_id,
            ActionFixedField::RetryAfter => self.retry_after,
            ActionFixedField::RetryLimit => self.retry_limit,
        }
    }
}

/// Checks default-valued component fields that become invalid only because a
/// legacy wrapper supplies a different action type. Positive/non-empty values
/// remain visible to typed validation; only zero and empty-string presence
/// needs the raw component declaration here.
fn check_raw_inherited_component_field_applicability(
    path: &str,
    component: &serde_yaml_ng::Value,
    kind: ActionKind,
    component_type: ActionType,
    action_type: ActionType,
    local_fields: ActionLocalFields,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let component_allows_retry = kind == ActionKind::Failure && component_type == ActionType::Retry;
    let action_allows_retry = kind == ActionKind::Failure && action_type == ActionType::Retry;
    if component_allows_retry && !action_allows_retry {
        for (field, fixed_field) in [
            ("retryAfter", ActionFixedField::RetryAfter),
            ("retryLimit", ActionFixedField::RetryLimit),
        ] {
            let is_default_value = raw_mapping_field(component, field).is_some_and(|value| {
                if field == "retryAfter" {
                    value.as_f64() == Some(0.0)
                } else {
                    value.as_u64() == Some(0)
                }
            });
            if !local_fields.contains(fixed_field) && is_default_value {
                raw_action_field_not_applicable(
                    path,
                    field,
                    ValidationErrorKind::InvalidRetryField,
                    format!("{path}.{field} is only applicable to a failure retry action"),
                    diagnostics,
                );
            }
        }
    }

    if action_allows_target(kind, component_type) && !action_allows_target(kind, action_type) {
        for (field, fixed_field) in [
            ("workflowId", ActionFixedField::WorkflowId),
            ("stepId", ActionFixedField::StepId),
        ] {
            if !local_fields.contains(fixed_field)
                && matches!(
                    raw_mapping_field(component, field),
                    Some(serde_yaml_ng::Value::String(value)) if value.is_empty()
                )
            {
                raw_action_field_not_applicable(
                    path,
                    field,
                    ValidationErrorKind::InvalidReference,
                    format!(
                        "{path}.{field} is only applicable to a goto action or failure retry action"
                    ),
                    diagnostics,
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ComponentActionFixedFieldOrigin {
    /// The range source after legacy merge. This deliberately differs from
    /// `local_fields.retry_after`: an explicit local zero remains visible for
    /// applicability, but does not override the component value.
    local_retry_after_overrides: bool,
    local_fields: ActionLocalFields,
}

/// Validation-only metadata retained while component references are expanded.
/// Structural destination and source keys use deterministic collection
/// indices; rendered diagnostic paths never participate in identity. The
/// public model stays unchanged for direct `validate*` callers.
#[derive(Debug, Default, PartialEq)]
struct ResolutionProvenance {
    /// The byte-parse entry point runs the raw Action boundary pass. It owns
    /// presence diagnostics that a defaulted typed model cannot distinguish;
    /// direct typed validation owns the equivalent `type_: None` diagnostic.
    raw_action_boundaries_owned: bool,
    /// Parameter declarations whose intrinsic fields came from a reusable
    /// component, mapped to their source in `components.parameters`.
    component_parameter_origins: HashMap<ParameterDeclarationKey, ComponentParameterKey>,
    /// Action declarations whose entire parameter list came from a component
    /// action through canonical or supported legacy resolution, mapped to the
    /// structural source component action.
    component_action_origins: HashMap<DeclarationScope, DeclarationScope>,
    /// Fixed action fields inherited from a component action. Canonical
    /// Reusable Objects have no local fields; legacy name-form references can
    /// override individual fixed fields and must validate those at the use.
    component_action_fixed_field_origins:
        HashMap<DeclarationScope, ComponentActionFixedFieldOrigin>,
    /// Direct-API-only findings collected immediately before resolution would
    /// erase the invalid typed wrapper value.
    reusable_value_diagnostics: Vec<Diagnostic>,
    /// Parse-boundary findings for default-valued legacy wrapper fields. These
    /// are collected only after the component Action type is resolved, because
    /// the public model cannot retain zero/null/empty field presence.
    raw_legacy_action_diagnostics: Vec<Diagnostic>,
}

fn collect_diagnostics(spec: &ArazzoSpec, provenance: &ResolutionProvenance) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::<Diagnostic>::new();

    check_unknown_fields("", &spec.extensions, &mut diagnostics);
    check_unknown_fields("info", &spec.info.extensions, &mut diagnostics);

    if spec.arazzo.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: "arazzo".to_string(),
            message: "arazzo version is required".to_string(),
        });
    } else if !spec.arazzo.starts_with("1.") {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::UnsupportedVersion,
            path: "arazzo".to_string(),
            message: format!("unsupported arazzo version: {} (expected 1.x)", spec.arazzo),
        });
    }

    if let Some(self_uri) = &spec.self_uri {
        match UriReferenceStr::new(self_uri) {
            Ok(uri) if uri.fragment().is_some() => diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidReference,
                path: "$self".to_string(),
                message: "$self must not contain a fragment identifier".to_string(),
            }),
            Ok(_) => {}
            Err(err) => diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidReference,
                path: "$self".to_string(),
                message: format!("$self must be a valid RFC 3986 URI-reference: {err}"),
            }),
        }
    }

    if spec.info.title.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: "info.title".to_string(),
            message: "info.title is required".to_string(),
        });
    }
    if spec.info.version.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: "info.version".to_string(),
            message: "info.version is required".to_string(),
        });
    }

    if spec.source_descriptions.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: "sourceDescriptions".to_string(),
            message: "sourceDescriptions is required and MUST have at least one entry".to_string(),
        });
    }
    if spec.workflows.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: "workflows".to_string(),
            message: "workflows is required and MUST have at least one entry".to_string(),
        });
    }

    let mut source_names = HashSet::<&str>::new();
    let mut source_types = HashMap::<&str, SourceType>::new();
    for (idx, src) in spec.source_descriptions.iter().enumerate() {
        let path = format!("sourceDescriptions[{idx}]");
        check_unknown_fields(&path, &src.extensions, &mut diagnostics);
        if src.name.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.name"),
                message: format!("{path}.name is required"),
            });
        } else if !source_names.insert(&src.name) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::DuplicateIdentifier,
                path: format!("{path}.name"),
                message: format!("{path}.name '{}' is duplicate", src.name),
            });
        } else if let Some(diag) = check_identifier_warning(
            &format!("{path}.name"),
            &src.name,
            IdentifierClass::Identifier,
        ) {
            diagnostics.push(diag);
        }
        source_types.insert(src.name.as_str(), src.type_);
        if src.url.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.url"),
                message: format!("{path}.url is required"),
            });
        }
    }

    if let Some(components) = &spec.components {
        check_unknown_fields("components", &components.extensions, &mut diagnostics);
        // components.inputs is excluded: each entry is a JSON Schema object
        // (SchemaObject), and unmodeled JSON Schema keywords are legitimate,
        // not unrecognized fields.
        validate_component_keys(
            "components.inputs",
            components.inputs.keys(),
            &mut diagnostics,
        );
        validate_component_keys(
            "components.parameters",
            components.parameters.keys(),
            &mut diagnostics,
        );
        validate_component_keys(
            "components.successActions",
            components.success_actions.keys(),
            &mut diagnostics,
        );
        validate_component_keys(
            "components.failureActions",
            components.failure_actions.keys(),
            &mut diagnostics,
        );
        for (name, param) in &components.parameters {
            validate_parameter(
                &format!("components.parameters.{name}"),
                param,
                &spec.arazzo,
                false,
                &mut diagnostics,
            );
        }
        for (action_index, (name, action)) in components.success_actions.iter().enumerate() {
            let action_path = format!("components.successActions.{name}");
            check_unknown_fields(&action_path, &action.extensions, &mut diagnostics);
            warn_action_reusable_fields(&action_path, action, &mut diagnostics);
            validate_action_fixed_fields(
                &action_path,
                action,
                ActionKind::Success,
                None,
                provenance.raw_action_boundaries_owned,
                &mut diagnostics,
            );
            // Components hold Success Action Objects, not Reusable Objects.
            // Their complete parameter contract applies even when unused, and
            // an invalid `reference` field must not suppress it.
            validate_action_parameters(
                &action_path,
                action,
                &spec.arazzo,
                provenance,
                DeclarationScope::ComponentAction {
                    kind: ActionKind::Success,
                    action_index,
                },
                false,
                &mut diagnostics,
            );
        }
        for (action_index, (name, action)) in components.failure_actions.iter().enumerate() {
            let action_path = format!("components.failureActions.{name}");
            check_unknown_fields(&action_path, &action.extensions, &mut diagnostics);
            warn_action_reusable_fields(&action_path, action, &mut diagnostics);
            validate_action_fixed_fields(
                &action_path,
                action,
                ActionKind::Failure,
                None,
                provenance.raw_action_boundaries_owned,
                &mut diagnostics,
            );
            validate_action_parameters(
                &action_path,
                action,
                &spec.arazzo,
                provenance,
                DeclarationScope::ComponentAction {
                    kind: ActionKind::Failure,
                    action_index,
                },
                false,
                &mut diagnostics,
            );
        }
    }

    // Collect all workflow IDs upfront for cross-workflow goto validation.
    let workflow_ids: HashSet<&str> = spec
        .workflows
        .iter()
        .filter(|wf| !wf.workflow_id.is_empty())
        .map(|wf| wf.workflow_id.as_str())
        .collect();

    let mut seen_workflow_ids = HashSet::<&str>::new();
    let mut seen_parameter_contexts =
        HashSet::<(ParameterDeclarationKey, ParameterContextConstraint)>::new();

    for (wf_idx, wf) in spec.workflows.iter().enumerate() {
        let path = if wf.workflow_id.is_empty() {
            format!("workflows[{wf_idx}]")
        } else {
            format!("workflow \"{}\"", wf.workflow_id)
        };
        check_unknown_fields(&path, &wf.extensions, &mut diagnostics);

        if wf.workflow_id.is_empty() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.workflowId"),
                message: format!("{path}.workflowId is required"),
            });
        } else if !seen_workflow_ids.insert(wf.workflow_id.as_str()) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::DuplicateIdentifier,
                path: format!("{path}.workflowId"),
                message: format!("{path}.workflowId '{}' is duplicate", wf.workflow_id),
            });
        } else if let Some(diag) = check_identifier_warning(
            &format!("{path}.workflowId"),
            &wf.workflow_id,
            IdentifierClass::Identifier,
        ) {
            diagnostics.push(diag);
        }

        for (dependency_idx, dependency) in wf.depends_on.iter().enumerate() {
            let dependency_path = format!("{path}.dependsOn[{dependency_idx}]");
            match classify_workflow_dependency(dependency) {
                WorkflowDependency::Local(workflow_id) => {
                    if !workflow_ids.contains(workflow_id) {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            kind: ValidationErrorKind::InvalidReference,
                            path: dependency_path,
                            message: format!(
                                "{path}.dependsOn references unknown local workflow \"{workflow_id}\""
                            ),
                        });
                    }
                }
                WorkflowDependency::External {
                    source_name,
                    workflow_id,
                } => match source_types.get(source_name) {
                    None => diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidReference,
                        path: dependency_path,
                        message: format!(
                            "{path}.dependsOn references unknown sourceDescription \"{source_name}\""
                        ),
                    }),
                    Some(SourceType::Arazzo) => diagnostics.push(Diagnostic::warning(
                        ValidationErrorKind::UnsupportedDependencyScope,
                        dependency_path,
                        format!(
                            "{path}.dependsOn external workflow \"{workflow_id}\" from Arazzo source \"{source_name}\" is valid but cannot be checked because external Arazzo documents are not loaded"
                        ),
                    )),
                    Some(source_type) => diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidReference,
                        path: dependency_path,
                        message: format!(
                            "{path}.dependsOn sourceDescription \"{source_name}\" has type {source_type}, expected type arazzo"
                        ),
                    }),
                },
                WorkflowDependency::Invalid => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidReference,
                    path: dependency_path,
                    message: format!(
                        "{path}.dependsOn contains invalid workflow reference \"{dependency}\""
                    ),
                }),
            }
        }

        validate_parameters(
            &format!("{path}.parameters"),
            &wf.parameters,
            &spec.arazzo,
            provenance,
            DeclarationScope::WorkflowParameters {
                workflow_index: wf_idx,
            },
            &mut diagnostics,
        );
        validate_parameter_identities(
            &format!("{path}.parameters"),
            &wf.parameters,
            ParameterIdentityMode::NameAndLocation,
            &mut diagnostics,
        );

        // Collect step IDs for this workflow before validating actions.
        let step_ids: HashSet<&str> = wf
            .steps
            .iter()
            .filter(|s| !s.step_id.is_empty())
            .map(|s| s.step_id.as_str())
            .collect();

        validate_actions(
            &format!("{path}.successActions"),
            &wf.success_actions,
            (&step_ids, &workflow_ids),
            &spec.arazzo,
            provenance,
            ActionListScope {
                kind: ActionKind::Success,
                workflow_index: wf_idx,
                step_index: None,
            },
            &mut diagnostics,
        );
        validate_actions(
            &format!("{path}.failureActions"),
            &wf.failure_actions,
            (&step_ids, &workflow_ids),
            &spec.arazzo,
            provenance,
            ActionListScope {
                kind: ActionKind::Failure,
                workflow_index: wf_idx,
                step_index: None,
            },
            &mut diagnostics,
        );

        let mut seen_step_ids = HashSet::<&str>::new();
        for (step_idx, step) in wf.steps.iter().enumerate() {
            let step_path = if step.step_id.is_empty() {
                format!("{path} > steps[{step_idx}]")
            } else {
                format!("{path} > step \"{}\"", step.step_id)
            };
            check_unknown_fields(&step_path, &step.extensions, &mut diagnostics);

            if step.step_id.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{step_path}.stepId"),
                    message: format!("{step_path}.stepId is required"),
                });
            } else if !seen_step_ids.insert(step.step_id.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::DuplicateIdentifier,
                    path: format!("{step_path}.stepId"),
                    message: format!("{step_path}.stepId '{}' is duplicate", step.step_id),
                });
            } else if let Some(diag) = check_identifier_warning(
                &format!("{step_path}.stepId"),
                &step.step_id,
                IdentifierClass::Identifier,
            ) {
                diagnostics.push(diag);
            }

            if step.target.is_none() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidStepTarget,
                    path: step_path.clone(),
                    message: format!(
                        "{step_path} must have operationId, operationPath, channelPath, or workflowId"
                    ),
                });
            }
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
                    // A warning, not an error: the specification form is valid
                    // Arazzo, this executor simply does not resolve it. Saying
                    // "invalid" would blame the document for our gap — but
                    // reporting nothing at all is how a conformant step used to
                    // reach the runtime and turn into a nonsense request URL.
                    if let Some(reason) = classified.unsupported() {
                        diagnostics.push(Diagnostic::warning(
                            ValidationErrorKind::UnsupportedOperationPath,
                            format!("{step_path}.operationPath"),
                            format!(
                                "{step_path}.operationPath \"{operation_path}\" carries {reason}; \
                                 this runtime does not resolve the specification form (source \
                                 reference plus JSON Pointer), so running this step will fail. \
                                 Supported forms are {SUPPORTED_OPERATION_PATH_FORMS}."
                            ),
                        ));
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
                Some(StepTarget::WorkflowId(_)) if step.action.is_some() => {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidAsyncStep,
                        path: format!("{step_path}.action"),
                        message: format!(
                            "{step_path}.action is not applicable to workflowId targets"
                        ),
                    });
                }
                _ => {}
            }

            let supports_async_fields = matches!(
                &step.target,
                Some(StepTarget::OperationId(_) | StepTarget::ChannelPath(_))
            );
            if step.correlation_id.is_some()
                && (step.action != Some(StepAction::Receive) || !supports_async_fields)
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidAsyncStep,
                    path: format!("{step_path}.correlationId"),
                    message: format!(
                        "{step_path}.correlationId is only applicable to async steps with action receive"
                    ),
                });
            }

            for (dependency_idx, dependency) in step.depends_on.iter().enumerate() {
                let dependency_path = format!("{step_path}.dependsOn[{dependency_idx}]");
                match classify_step_dependency(dependency) {
                    StepDependency::Local(step_id) if !step_ids.contains(step_id) => {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            kind: ValidationErrorKind::InvalidReference,
                            path: dependency_path,
                            message: format!(
                                "{step_path}.dependsOn references unknown local step \"{step_id}\""
                            ),
                        });
                    }
                    StepDependency::CrossWorkflow => {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            kind: ValidationErrorKind::UnsupportedDependencyScope,
                            path: dependency_path,
                            message: format!(
                                "{step_path}.dependsOn cross-workflow references are valid Arazzo 1.1 syntax but are not supported by this runtime"
                            ),
                        });
                    }
                    StepDependency::ExternalSource => {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            kind: ValidationErrorKind::UnsupportedDependencyScope,
                            path: dependency_path,
                            message: format!(
                                "{step_path}.dependsOn external-source references are valid Arazzo 1.1 syntax but are not supported by this runtime"
                            ),
                        });
                    }
                    StepDependency::Invalid => {
                        diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            kind: ValidationErrorKind::InvalidReference,
                            path: dependency_path,
                            message: format!(
                                "{step_path}.dependsOn contains invalid step reference \"{dependency}\""
                            ),
                        });
                    }
                    StepDependency::Local(_) => {}
                }
            }

            validate_parameters(
                &format!("{step_path}.parameters"),
                &step.parameters,
                &spec.arazzo,
                provenance,
                DeclarationScope::StepParameters {
                    workflow_index: wf_idx,
                    step_index: step_idx,
                },
                &mut diagnostics,
            );
            validate_parameter_identities(
                &format!("{step_path}.parameters"),
                &step.parameters,
                if matches!(&step.target, Some(StepTarget::WorkflowId(_))) {
                    ParameterIdentityMode::NameOnly
                } else {
                    ParameterIdentityMode::NameAndLocation
                },
                &mut diagnostics,
            );
            let parameter_context = StepParameterContext {
                workflow_path: &path,
                workflow_index: wf_idx,
                workflow: wf,
                step_path: &step_path,
                step_index: step_idx,
                step,
            };
            validate_effective_parameter_context(
                &parameter_context,
                &mut seen_parameter_contexts,
                &mut diagnostics,
            );
            validate_querystring_exclusivity(&parameter_context, &mut diagnostics);
            for (name, output) in &step.outputs {
                let output_path = format!("{step_path}.outputs.{name}");
                validate_output_value(&output_path, output, &spec.arazzo, &mut diagnostics);
                if let Some(diag) = check_identifier(&output_path, name, IdentifierClass::DottedKey)
                {
                    diagnostics.push(diag);
                }
            }
            if let Some(request_body) = &step.request_body {
                check_unknown_fields(
                    &format!("{step_path}.requestBody"),
                    &request_body.extensions,
                    &mut diagnostics,
                );
                if let Some(payload) = &request_body.payload {
                    validate_value_source(
                        &format!("{step_path}.requestBody.payload"),
                        payload,
                        &spec.arazzo,
                        &mut diagnostics,
                    );
                }
                validate_replacements(
                    &format!("{step_path}.requestBody.replacements"),
                    &request_body.replacements,
                    &spec.arazzo,
                    &mut diagnostics,
                );
            }

            for (criterion_idx, criterion) in step.success_criteria.iter().enumerate() {
                validate_criterion(
                    &format!("{step_path}.successCriteria[{criterion_idx}]"),
                    criterion,
                    &spec.arazzo,
                    &mut diagnostics,
                );
            }

            validate_actions(
                &format!("{step_path}.onFailure"),
                &step.on_failure,
                (&step_ids, &workflow_ids),
                &spec.arazzo,
                provenance,
                ActionListScope {
                    kind: ActionKind::Failure,
                    workflow_index: wf_idx,
                    step_index: Some(step_idx),
                },
                &mut diagnostics,
            );
            validate_actions(
                &format!("{step_path}.onSuccess"),
                &step.on_success,
                (&step_ids, &workflow_ids),
                &spec.arazzo,
                provenance,
                ActionListScope {
                    kind: ActionKind::Success,
                    workflow_index: wf_idx,
                    step_index: Some(step_idx),
                },
                &mut diagnostics,
            );
        }

        if local_step_depends_on_has_cycle(wf) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::DependencyCycle,
                path: format!("{path}.steps"),
                message: format!(
                    "{path} contains a dependency cycle in local dependsOn references"
                ),
            });
        }

        if local_workflow_depends_on_has_cycle(spec, wf_idx) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::DependencyCycle,
                path: format!("{path}.dependsOn"),
                message: format!(
                    "{path} contains a dependency cycle in local dependsOn references"
                ),
            });
        }

        for (name, output) in &wf.outputs {
            let output_path = format!("{path}.outputs.{name}");
            validate_output_value(&output_path, output, &spec.arazzo, &mut diagnostics);
            validate_output_step_reference(&output_path, output, &step_ids, &mut diagnostics);
            if let Some(diag) = check_identifier(&output_path, name, IdentifierClass::DottedKey) {
                diagnostics.push(diag);
            }
        }
    }

    diagnostics
}

fn local_workflow_depends_on_has_cycle(spec: &ArazzoSpec, start: usize) -> bool {
    let positions = spec
        .workflows
        .iter()
        .enumerate()
        .map(|(index, workflow)| (workflow.workflow_id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut states = vec![0_u8; spec.workflows.len()];

    fn visit(
        index: usize,
        spec: &ArazzoSpec,
        positions: &HashMap<&str, usize>,
        states: &mut [u8],
    ) -> bool {
        if states[index] == 1 {
            return true;
        }
        if states[index] == 2 {
            return false;
        }
        states[index] = 1;
        for dependency in &spec.workflows[index].depends_on {
            let WorkflowDependency::Local(workflow_id) = classify_workflow_dependency(dependency)
            else {
                continue;
            };
            let Some(&dependency_index) = positions.get(workflow_id) else {
                continue;
            };
            if visit(dependency_index, spec, positions, states) {
                return true;
            }
        }
        states[index] = 2;
        false
    }

    start < spec.workflows.len() && visit(start, spec, &positions, &mut states)
}

fn local_step_depends_on_has_cycle(workflow: &Workflow) -> bool {
    let positions = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.step_id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut states = vec![0_u8; workflow.steps.len()];

    fn visit(
        index: usize,
        workflow: &Workflow,
        positions: &HashMap<&str, usize>,
        states: &mut [u8],
    ) -> bool {
        if states[index] == 1 {
            return true;
        }
        if states[index] == 2 {
            return false;
        }
        states[index] = 1;
        for dependency in &workflow.steps[index].depends_on {
            let StepDependency::Local(step_id) = classify_step_dependency(dependency) else {
                continue;
            };
            let Some(&dependency_index) = positions.get(step_id) else {
                continue;
            };
            if visit(dependency_index, workflow, positions, states) {
                return true;
            }
        }
        states[index] = 2;
        false
    }

    (0..workflow.steps.len()).any(|index| visit(index, workflow, &positions, &mut states))
}

enum StepDependency<'a> {
    Local(&'a str),
    CrossWorkflow,
    ExternalSource,
    Invalid,
}

fn classify_step_dependency(value: &str) -> StepDependency<'_> {
    if value.is_empty() {
        return StepDependency::Invalid;
    }
    if !value.starts_with('$') {
        return StepDependency::Local(value);
    }

    let parts = value.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["$workflows", workflow_id, "steps", step_id]
            if !workflow_id.is_empty() && !step_id.is_empty() =>
        {
            StepDependency::CrossWorkflow
        }
        ["$sourceDescriptions", source_name, workflow_id, "steps", step_id]
            if !source_name.is_empty() && !workflow_id.is_empty() && !step_id.is_empty() =>
        {
            StepDependency::ExternalSource
        }
        _ => StepDependency::Invalid,
    }
}

/// Raw required-list checks for Workflow and Step Objects.
///
/// The Workflow Object's `steps` field is REQUIRED, while the Arazzo
/// Specification Object's `sourceDescriptions` and `workflows` fields are
/// checked in [`collect_diagnostics`] because their typed lists need no
/// presence distinction. This raw walk rejects an absent `steps` key while
/// preserving the specification-valid `steps: []` spelling.
///
/// `successCriteria` presence check: Step Object — *"If `successCriteria` is
/// provided, it MUST contain at least one Criterion Object."*
///
/// `Step.success_criteria` is `Vec<SuccessCriterion>` with
/// `#[serde(default, skip_serializing_if = "Vec::is_empty")]`
/// (`crates/arazzo-spec/src/lib.rs`), so an absent `successCriteria` key and
/// an explicit `successCriteria: []` collapse to the same typed value by the
/// time a `&ArazzoSpec` exists — `validate`/`validate_diagnostics` cannot
/// enforce this rule. This walks the raw YAML document text instead, which is
/// the only place the distinction survives, and is why the call lives in
/// `parse_bytes_with_diagnostics` rather than `collect_diagnostics`.
///
/// The implicit null spelling (`successCriteria:` with no value) is a provided
/// field and is represented as `Value::Null` by the raw YAML parse, so it must
/// produce the same diagnostic as an empty sequence. Explicit `null` and `~`
/// spellings currently fail earlier during typed parsing with a `parseYaml`
/// type error; the null arm remains correct if that serde behavior changes.
///
/// Deliberately tolerant of shapes it does not recognize (non-mapping root,
/// missing `workflows`, non-mapping workflow/step entries, and non-null
/// non-sequence `successCriteria`): those are either not-yet-parseable YAML
/// (already rejected earlier in the pipeline with a clearer error) or someone
/// else's diagnostic to raise, not this check's. A missing `steps` key is the
/// exception: it is a required field and is diagnosed here because the typed
/// model collapses absent and empty lists.
fn check_raw_success_criteria(root: &serde_yaml_ng::Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(workflows) =
        raw_mapping_field(root, "workflows").and_then(serde_yaml_ng::Value::as_sequence)
    else {
        return diagnostics;
    };

    for (wf_idx, wf_value) in workflows.iter().enumerate() {
        let Some(wf_mapping) = wf_value.as_mapping() else {
            continue;
        };
        let workflow_id = wf_mapping
            .get("workflowId")
            .and_then(serde_yaml_ng::Value::as_str)
            .unwrap_or_default();
        let wf_path = if workflow_id.is_empty() {
            format!("workflows[{wf_idx}]")
        } else {
            format!("workflow \"{workflow_id}\"")
        };
        let Some(steps_value) = wf_mapping.get("steps") else {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{wf_path}.steps"),
                message: format!("{wf_path}.steps is required"),
            });
            continue;
        };
        let Some(steps) = steps_value.as_sequence() else {
            continue;
        };
        for (step_idx, step_value) in steps.iter().enumerate() {
            let Some(step_mapping) = step_value.as_mapping() else {
                continue;
            };
            let step_id = step_mapping
                .get("stepId")
                .and_then(serde_yaml_ng::Value::as_str)
                .unwrap_or_default();
            let step_path = if step_id.is_empty() {
                format!("{wf_path} > steps[{step_idx}]")
            } else {
                format!("{wf_path} > step \"{step_id}\"")
            };
            let provided_without_criteria = match step_mapping.get("successCriteria") {
                Some(serde_yaml_ng::Value::Null) => true,
                Some(value) => value
                    .as_sequence()
                    .is_some_and(|criteria| criteria.is_empty()),
                None => false,
            };
            if provided_without_criteria {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{step_path}.successCriteria"),
                    message: format!(
                        "{step_path}.successCriteria, if provided, MUST contain at least one \
                         Criterion Object"
                    ),
                });
            }
        }
    }

    diagnostics
}

/// Character class for [`check_identifier`]. Implemented as a byte-class
/// predicate rather than `regex`: `arazzo-validate`'s `Cargo.toml` does not
/// depend on `regex`, and the workspace denies `unwrap_used`/`expect_used`,
/// which rules out the usual `LazyLock::new(|| Regex::new(..).unwrap())`.
/// Anchored by construction — `chars().all(..)` covers the entire string end
/// to end, so there is no unanchored-regex failure mode to introduce later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentifierClass {
    /// `^[A-Za-z0-9_\-]+$` — the SHOULD-level `workflowId`/`stepId`/
    /// `sourceDescriptions[].name` definition-site positions. Checked via
    /// [`check_identifier_warning`], never [`check_identifier`]: a SHOULD
    /// violation is a warning, not an error.
    Identifier,
    /// `^[a-zA-Z0-9\.\-_]+$` — the MUST-level `outputs` keys and Components
    /// map keys this ticket enforces.
    DottedKey,
}

impl IdentifierClass {
    fn is_valid(self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        value.chars().all(|c| match self {
            Self::Identifier => c.is_ascii_alphanumeric() || c == '-' || c == '_',
            Self::DottedKey => c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_',
        })
    }

    const fn regex_label(self) -> &'static str {
        match self {
            Self::Identifier => "^[A-Za-z0-9_\\-]+$",
            Self::DottedKey => "^[a-zA-Z0-9\\.\\-_]+$",
        }
    }
}

/// Shared body for [`check_identifier`] and [`check_identifier_warning`].
/// Returns `None` when `value` is valid under `class`, otherwise a
/// diagnostic at `severity` naming the offending value and the regular
/// expression it should or must match.
fn identifier_diagnostic(
    path: &str,
    value: &str,
    class: IdentifierClass,
    severity: Severity,
) -> Option<Diagnostic> {
    if class.is_valid(value) {
        return None;
    }
    let verb = match severity {
        Severity::Error => "must",
        Severity::Warning => "should",
    };
    Some(Diagnostic {
        severity,
        kind: ValidationErrorKind::InvalidIdentifier,
        path: path.to_string(),
        message: format!(
            "{path} value {value:?} {verb} match the regular expression {}",
            class.regex_label()
        ),
    })
}

/// Validates `value` at `path` against `class`. Returns `None` when valid,
/// otherwise an error-severity [`ValidationError`] naming the offending value
/// and the regular expression it must match. MUST-level positions only.
fn check_identifier(path: &str, value: &str, class: IdentifierClass) -> Option<ValidationError> {
    identifier_diagnostic(path, value, class, Severity::Error)
}

/// Validates `value` at `path` against `class`. Returns `None` when valid,
/// otherwise a warning-severity [`Diagnostic`] naming the offending value and
/// the regular expression it should match. SHOULD-level positions only —
/// never fails validation on its own; promoted to an error by the shared
/// `--strict` mechanism.
fn check_identifier_warning(path: &str, value: &str, class: IdentifierClass) -> Option<Diagnostic> {
    identifier_diagnostic(path, value, class, Severity::Warning)
}

/// Warns on every field captured at `path` that is neither a modeled field
/// nor a `x-*` Specification Extension — most often a misspelling of a real
/// field, such as `sucessCriteria`, that would otherwise silently disable the
/// behavior the author intended. Warning severity, promoted to error only by
/// the shared `--strict` mechanism: the specification reserves `x-` for
/// extensions but states no MUST-reject for an unrecognized field.
///
/// Must not be called on a `SchemaObject` or `PropertyDef` capture: their
/// leftover keys are legitimate JSON Schema 2020-12 keywords (`items`,
/// `enum`, `minimum`, ...), not typos.
fn check_unknown_fields(
    path: &str,
    extensions: &VendorExtensions,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (key, _) in unrecognized_fields(extensions) {
        diagnostics.push(Diagnostic::warning(
            ValidationErrorKind::UnknownField,
            path.to_string(),
            format!(
                "unrecognized field \"{key}\"; only `x-` prefixed extension fields are \
                 permitted here"
            ),
        ));
    }
}

fn warn_action_reusable_fields(path: &str, action: &OnAction, diagnostics: &mut Vec<Diagnostic>) {
    for field in [
        (!action.reference.is_empty(), "reference"),
        (action.value.is_some(), "value"),
    ] {
        if field.0 {
            diagnostics.push(Diagnostic::warning(
                ValidationErrorKind::UnknownField,
                path.to_string(),
                format!(
                    "unrecognized field \"{}\"; only `x-` prefixed extension fields are permitted here",
                    field.1
                ),
            ));
        }
    }
}

/// Components Object: *"All the fixed fields declared above are objects that
/// MUST use keys that match the regular expression: `^[a-zA-Z0-9\.\-_]+$`."*
///
/// Path form `components.<field>."<key>"`: Components map keys sit outside
/// any workflow or step, so the crate's `workflow "<id>" > step "<id>"`
/// convention does not apply, and `{key:?}` (`Debug` for `&String`) produces
/// exactly the quoted-key form the design calls for.
fn validate_component_keys<'a>(
    field_path: &str,
    keys: impl Iterator<Item = &'a String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for key in keys {
        let path = format!("{field_path}.{key:?}");
        if let Some(diag) = check_identifier(&path, key, IdentifierClass::DottedKey) {
            diagnostics.push(diag);
        }
    }
}

/// True for a document declaring an Arazzo version older than 1.1.0, which is
/// where `in: querystring` was introduced.
fn declares_pre_1_1(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    matches!((parts.next(), parts.next()), (Some("1"), Some("0")))
}

/// One Parameter after the same inheritance/override selection the runtime
/// applies, retaining its declaration path for diagnostics. A Step overrides
/// only an exact `(name, in)` identity; case differences and locations remain
/// distinct.
struct EffectiveParameter<'a> {
    parameter: &'a Parameter,
    path: String,
    declaration: ParameterDeclarationKey,
}

struct StepParameterContext<'a> {
    workflow_path: &'a str,
    workflow_index: usize,
    workflow: &'a Workflow,
    step_path: &'a str,
    step_index: usize,
    step: &'a Step,
}

/// Mirrors `merge_workflow_params` in `arazzo-runtime`: workflow-level
/// parameters are inherited only by non-workflow targets, and a Step replaces
/// the same `(name, in)` identity instead of being a duplicate across lists.
fn effective_parameters<'a>(context: &StepParameterContext<'a>) -> Vec<EffectiveParameter<'a>> {
    let step_keys: HashSet<(&str, Option<ParamLocation>)> = context
        .step
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter.in_))
        .collect();
    let mut params = Vec::new();
    if !matches!(&context.step.target, Some(StepTarget::WorkflowId(_))) {
        params.extend(
            context
                .workflow
                .parameters
                .iter()
                .enumerate()
                .filter(|(_, parameter)| {
                    !step_keys.contains(&(parameter.name.as_str(), parameter.in_))
                })
                .map(|(index, parameter)| EffectiveParameter {
                    parameter,
                    path: format!("{}.parameters[{index}]", context.workflow_path),
                    declaration: DeclarationScope::WorkflowParameters {
                        workflow_index: context.workflow_index,
                    }
                    .declaration(index),
                }),
        );
    }
    params.extend(
        context
            .step
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| EffectiveParameter {
                parameter,
                path: format!("{}.parameters[{index}]", context.step_path),
                declaration: DeclarationScope::StepParameters {
                    workflow_index: context.workflow_index,
                    step_index: context.step_index,
                }
                .declaration(index),
            }),
    );
    params
}

/// The distinct context constraints that can apply to one structural
/// Parameter declaration. The declaration key plus this class is the internal
/// identity: one workflow Parameter inherited by equivalent Steps reports
/// once, while operation and asynchronous failures remain distinct. Rendered
/// paths are diagnostic output only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ParameterContextConstraint {
    OperationRequiresIn,
    WorkflowProhibitsIn,
    AsyncOperationUnsupported,
    ChannelPathUnsupported,
}

fn push_parameter_context_diagnostic(
    declaration: ParameterDeclarationKey,
    path: String,
    constraint: ParameterContextConstraint,
    message: String,
    seen: &mut HashSet<(ParameterDeclarationKey, ParameterContextConstraint)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if seen.insert((declaration, constraint)) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidParameterLocation,
            path,
            message,
        });
    }
}

/// Parameter Object §5.8.6.1 is context-sensitive: HTTP operation targets
/// require `in`, while a `workflowId` target maps every parameter to workflow
/// inputs and therefore prohibits it. An `action: send|receive` marks an
/// AsyncAPI operation target, and `channelPath` is also AsyncAPI; parameter
/// transport for both fails closed because this executor does not implement it.
fn validate_effective_parameter_context(
    context: &StepParameterContext<'_>,
    seen: &mut HashSet<(ParameterDeclarationKey, ParameterContextConstraint)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let parameters = effective_parameters(context);
    let Some(target) = &context.step.target else {
        return;
    };

    match target {
        StepTarget::OperationId(_) | StepTarget::OperationPath(_)
            if context.step.action.is_some() =>
        {
            for parameter in parameters {
                push_parameter_context_diagnostic(
                    parameter.declaration,
                    parameter.path.clone(),
                    ParameterContextConstraint::AsyncOperationUnsupported,
                    format!(
                        "{} cannot be used with an asynchronous operation target: AsyncAPI \
                         parameter transport is not supported by this executor",
                        parameter.path
                    ),
                    seen,
                    diagnostics,
                );
            }
        }
        StepTarget::OperationId(_) | StepTarget::OperationPath(_) => {
            for parameter in parameters {
                if parameter.parameter.in_.is_none() {
                    push_parameter_context_diagnostic(
                        parameter.declaration,
                        format!("{}.in", parameter.path),
                        ParameterContextConstraint::OperationRequiresIn,
                        format!(
                            "{}.in is required when the Step targets an operation",
                            parameter.path
                        ),
                        seen,
                        diagnostics,
                    );
                }
            }
        }
        StepTarget::WorkflowId(_) => {
            for parameter in parameters {
                if parameter.parameter.in_.is_some() {
                    push_parameter_context_diagnostic(
                        parameter.declaration,
                        format!("{}.in", parameter.path),
                        ParameterContextConstraint::WorkflowProhibitsIn,
                        format!(
                            "{}.in must not be set when the Step targets a workflow: \
                             parameters map to workflow inputs",
                            parameter.path
                        ),
                        seen,
                        diagnostics,
                    );
                }
            }
        }
        StepTarget::ChannelPath(_) => {
            for parameter in parameters {
                push_parameter_context_diagnostic(
                    parameter.declaration,
                    parameter.path.clone(),
                    ParameterContextConstraint::ChannelPathUnsupported,
                    format!(
                        "{} cannot be used with a channelPath target: AsyncAPI parameter \
                         transport is not supported by this executor",
                        parameter.path
                    ),
                    seen,
                    diagnostics,
                );
            }
        }
    }
}

/// Parameter Object: *"The `querystring` location cannot coexist with `query`
/// parameters in the same operation per OpenAPI constraints."* The OpenAPI
/// 3.2.0 Parameter Object those constraints point at says of `querystring`, in
/// the same sentence, that it *"MUST NOT appear more than once"*. Both halves
/// are enforced here.
///
/// Checked on the effective set rather than the step-declared one: a
/// workflow-level `query` and a step-level `querystring` have different merge
/// keys, so both reach the same request.
fn validate_querystring_exclusivity(
    context: &StepParameterContext<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let params = effective_parameters(context);
    let named = |location: ParamLocation| {
        params
            .iter()
            .filter(|param| param.parameter.in_ == Some(location))
            .map(|param| format!("{:?}", param.parameter.name))
            .collect::<Vec<_>>()
    };
    let querystring = named(ParamLocation::Querystring);
    if querystring.is_empty() {
        return;
    }
    // `merge_workflow_params` dedups on `(name, in)`, so a workflow-level and a
    // step-level `querystring` sharing a name collapse into one parameter —
    // the documented override path, not a duplicate. Distinct names do not
    // collapse, and every one of them reaches the same operation.
    let mut distinct = Vec::<&String>::new();
    for name in &querystring {
        if !distinct.contains(&name) {
            distinct.push(name);
        }
    }
    if distinct.len() > 1 {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidParameterLocation,
            path: format!("{}.parameters", context.step_path),
            message: format!(
                "{} carries more than one in: querystring parameter [{}]; the \
                 querystring location supplies the entire query component and must not \
                 appear more than once in the same operation. Workflow-level parameters \
                 are inherited by this step and count here.",
                context.step_path,
                distinct
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    let query = named(ParamLocation::Query);
    if query.is_empty() {
        return;
    }
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind: ValidationErrorKind::InvalidParameterLocation,
        path: format!("{}.parameters", context.step_path),
        message: format!(
            "{} carries in: querystring parameter(s) [{}] alongside in: query \
             parameter(s) [{}]; the querystring location supplies the entire query \
             component and cannot coexist with query parameters in the same operation. \
             Workflow-level parameters are inherited by this step and count here.",
            context.step_path,
            querystring.join(", "),
            query.join(", ")
        ),
    });
}

/// Intrinsic Parameter Object validation. `in` location rules deliberately do
/// not live here because a component definition has no target context.
fn validate_parameter(
    param_path: &str,
    param: &Parameter,
    arazzo_version: &str,
    permits_reusable_object: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_unknown_fields(param_path, &param.extensions, diagnostics);
    if param.in_ == Some(ParamLocation::Querystring) && declares_pre_1_1(arazzo_version) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::UnsupportedVersion,
            path: format!("{param_path}.in"),
            message: format!(
                "{param_path}.in \"querystring\" was introduced in Arazzo 1.1.0, but this \
                 document declares arazzo: {arazzo_version}, whose vocabulary has no such \
                 parameter location; declare arazzo: 1.1.0 to use it"
            ),
        });
    }
    if param.name.is_empty() && (!permits_reusable_object || param.reference.is_empty()) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{param_path}.name"),
            message: format!("{param_path}.name is required (unless using reference)"),
        });
    }
    validate_value_source(
        &format!("{param_path}.value"),
        &param.value,
        arazzo_version,
        diagnostics,
    );
}

fn validate_parameters(
    path_prefix: &str,
    params: &[Parameter],
    arazzo_version: &str,
    provenance: &ResolutionProvenance,
    list_origin: DeclarationScope,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (param_idx, param) in params.iter().enumerate() {
        let param_path = format!("{path_prefix}[{param_idx}]");
        // A source Reusable Object has already expanded to its component
        // definition. Validate that definition once at components.* and only
        // apply target context at this consuming path below.
        if !provenance
            .component_parameter_origins
            .contains_key(&list_origin.declaration(param_idx))
        {
            validate_parameter(&param_path, param, arazzo_version, true, diagnostics);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParameterIdentityMode {
    /// Operation-context Parameters use the specification identity `(name, in)`.
    NameAndLocation,
    /// Workflow-input and action Parameters prohibit `in`, so identity is the
    /// case-sensitive name even when two invalid entries carry different `in`s.
    NameOnly,
}

/// Parameter identity is checked per declared list. A Step override lives in
/// another list and was already selected into the effective set above.
fn validate_parameter_identities(
    path_prefix: &str,
    params: &[Parameter],
    mode: ParameterIdentityMode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut first_declarations = HashMap::<(String, Option<ParamLocation>), String>::new();
    for (index, parameter) in params.iter().enumerate() {
        if parameter.name.is_empty() {
            continue;
        }
        let parameter_path = format!("{path_prefix}[{index}]");
        let identity = (
            parameter.name.clone(),
            match mode {
                ParameterIdentityMode::NameAndLocation => parameter.in_,
                ParameterIdentityMode::NameOnly => None,
            },
        );
        if let Some(first_path) = first_declarations.get(&identity) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::DuplicateIdentifier,
                path: format!("{parameter_path}.name"),
                message: format!(
                    "{parameter_path}.name \"{}\" is a duplicate: the parameter list \
                     \"MUST NOT include duplicate parameters\"; first declaration is at \
                     {first_path}.name",
                    parameter.name
                ),
            });
        } else {
            first_declarations.insert(identity, parameter_path);
        }
    }
}

fn validate_output_value(
    path: &str,
    output: &OutputValue,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let OutputValue::Selector(selector) = output {
        validate_selector(path, selector, arazzo_version, diagnostics);
    }
}

fn validate_output_step_reference(
    path: &str,
    output: &OutputValue,
    step_ids: &HashSet<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expression = match output {
        OutputValue::RuntimeExpression(expression) => expression,
        OutputValue::Selector(selector) => &selector.context,
    };
    if let Some(after) = expression.strip_prefix("$steps.") {
        let step_name = after.split('.').next().unwrap_or_default();
        if !step_ids.contains(step_name) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidReference,
                path: path.to_string(),
                message: format!("{path} references unknown step '{step_name}'"),
            });
        }
    }
}

fn validate_value_source(
    path: &str,
    value: &ValueSource,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        ValueSource::Selector(selector) => {
            validate_selector(path, selector, arazzo_version, diagnostics)
        }
        ValueSource::Literal(serde_yaml_ng::Value::Sequence(values)) => {
            for (index, value) in values.iter().enumerate() {
                validate_value_source(
                    &format!("{path}[{index}]"),
                    &value.clone().into(),
                    arazzo_version,
                    diagnostics,
                );
            }
        }
        ValueSource::Literal(serde_yaml_ng::Value::Mapping(values)) => {
            for (key, value) in values {
                let key = key.as_str().unwrap_or("<non-string-key>");
                validate_value_source(
                    &format!("{path}.{key}"),
                    &value.clone().into(),
                    arazzo_version,
                    diagnostics,
                );
            }
        }
        ValueSource::Literal(_) => {}
    }
}

fn validate_selector(
    path: &str,
    selector: &SelectorObject,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_unknown_fields(path, &selector.extensions, diagnostics);
    if selector.context.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.context"),
            message: format!("{path}.context is required"),
        });
    }

    let type_name = selector.type_.resolved_name();
    if selector.selector.trim().is_empty() && type_name != "jsonpointer" {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.selector"),
            message: format!("{path}.selector is required"),
        });
    }

    validate_expression_type(
        &format!("{path}.type"),
        &selector.type_,
        &SELECTOR_TYPE_RULES,
        arazzo_version,
        diagnostics,
    );
}

/// Per-site policy for [`validate_expression_type`]: which type names each
/// declaration form accepts and which diagnostic kind the site reports.
/// The version table itself is shared and never varies by site.
struct ExpressionTypeRules {
    /// Type names accepted in the plain-name form, plus the label used in
    /// the diagnostic message.
    allowed_names: &'static [&'static str],
    names_label: &'static str,
    /// Type names accepted in the Expression Type Object form, plus label.
    allowed_object_types: &'static [&'static str],
    object_types_label: &'static str,
    kind: ValidationErrorKind,
    /// Site identity for the xpath advisory's pre-1.1 wording.
    advisory_site: xpath_advisory::XpathSite,
}

const SELECTOR_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["jsonpath", "xpath", "jsonpointer"],
    names_label: "jsonpath, xpath, or jsonpointer",
    allowed_object_types: &["jsonpath", "xpath", "jsonpointer"],
    object_types_label: "jsonpath, xpath, or jsonpointer",
    kind: ValidationErrorKind::InvalidSelectorType,
    advisory_site: xpath_advisory::XpathSite::Selector,
};

const CRITERION_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["simple", "regex", "jsonpath", "xpath"],
    names_label: "simple, regex, jsonpath, xpath",
    allowed_object_types: &["jsonpath", "xpath"],
    object_types_label: "jsonpath or xpath",
    kind: ValidationErrorKind::InvalidCriterionType,
    advisory_site: xpath_advisory::XpathSite::Criterion,
};

/// Arazzo v1.1.0 §5.8.12.1 version table, one accepted set for every site
/// (Selector Object `type`, Criterion `type`, `targetSelectorType`):
/// `jsonpath` → `rfc9535` | `draft-goessner-dispatch-jsonpath-00`; `xpath` →
/// `xpath-31` | `xpath-30` | `xpath-20` | `xpath-10`; `jsonpointer` →
/// `rfc6901`. The field description under §5.8.12.1 omits `xpath-31`, but the
/// table allows it and makes it the default, and the object's own Effective
/// Boolean Value list names "XPath 3.1 (default)"; the table is implemented.
fn expression_type_version_supported(type_name: &str, version: &str) -> bool {
    match type_name {
        "jsonpath" => matches!(version, "rfc9535" | "draft-goessner-dispatch-jsonpath-00"),
        "xpath" => matches!(version, "xpath-31" | "xpath-30" | "xpath-20" | "xpath-10"),
        "jsonpointer" => version == "rfc6901",
        _ => false,
    }
}

/// Validates a selector/criterion expression type declaration rooted at
/// `base_path` (the path of the `type`/`targetSelectorType` field itself).
/// Name-form errors report at `base_path`, object-form type errors at
/// `{base_path}.type`, and version errors at `{base_path}.version`.
fn validate_expression_type(
    base_path: &str,
    selector_type: &SelectorType,
    rules: &ExpressionTypeRules,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match selector_type {
        SelectorType::Name(name) => {
            let normalized = name.trim().to_lowercase();
            if !rules.allowed_names.contains(&normalized.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: base_path.to_string(),
                    message: format!("{base_path} must be one of {}", rules.names_label),
                });
            } else if normalized == "xpath" {
                diagnostics.extend(xpath_advisory::unexecutable_xpath_version(
                    base_path,
                    None,
                    arazzo_version,
                    rules.advisory_site,
                ));
            }
        }
        SelectorType::ExpressionType(expression_type) => {
            check_unknown_fields(
                &format!("{base_path}.type"),
                &expression_type.extensions,
                diagnostics,
            );
            let normalized = expression_type.type_.trim().to_lowercase();
            if !rules.allowed_object_types.contains(&normalized.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: format!("{base_path}.type"),
                    message: format!(
                        "{base_path}.type must be one of {}",
                        rules.object_types_label
                    ),
                });
                return;
            }

            let version = expression_type.version.trim();
            if version.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{base_path}.version"),
                    message: format!("{base_path}.version is required"),
                });
                return;
            }

            if !expression_type_version_supported(&normalized, version) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: format!("{base_path}.version"),
                    message: format!(
                        "{base_path}.version {version:?} is not supported for {normalized}"
                    ),
                });
            } else if normalized == "xpath" {
                diagnostics.extend(xpath_advisory::unexecutable_xpath_version(
                    base_path,
                    Some(version),
                    arazzo_version,
                    rules.advisory_site,
                ));
            }
        }
    }
}

fn validate_replacements(
    path_prefix: &str,
    replacements: &[arazzo_spec::Replacement],
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (replacement_idx, replacement) in replacements.iter().enumerate() {
        check_unknown_fields(
            &format!("{path_prefix}[{replacement_idx}]"),
            &replacement.extensions,
            diagnostics,
        );
        if replacement.target.trim().is_empty() {
            let path = format!("{path_prefix}[{replacement_idx}].target");
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: path.clone(),
                message: format!("{path} is required"),
            });
        }
        if let Some(selector_type) = &replacement.target_selector_type {
            validate_expression_type(
                &format!("{path_prefix}[{replacement_idx}].targetSelectorType"),
                selector_type,
                &SELECTOR_TYPE_RULES,
                arazzo_version,
                diagnostics,
            );
        }
        validate_value_source(
            &format!("{path_prefix}[{replacement_idx}].value"),
            &replacement.value,
            arazzo_version,
            diagnostics,
        );
    }
}

fn validate_actions(
    path_prefix: &str,
    actions: &[OnAction],
    target_ids: (&HashSet<&str>, &HashSet<&str>),
    arazzo_version: &str,
    provenance: &ResolutionProvenance,
    list_key: ActionListScope,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (action_idx, action) in actions.iter().enumerate() {
        let action_path = format!("{path_prefix}[{action_idx}]");
        let action_key = list_key.declaration(action_idx);
        let action_is_component_owned = provenance
            .component_action_origins
            .contains_key(&action_key);
        let fixed_field_origin = provenance
            .component_action_fixed_field_origins
            .get(&action_key)
            .copied();
        check_unknown_fields(&action_path, &action.extensions, diagnostics);
        if action.reference.is_empty() && action.value.is_some() {
            diagnostics.push(Diagnostic::warning(
                ValidationErrorKind::UnknownField,
                action_path.clone(),
                "unrecognized field \"value\"; only `x-` prefixed extension fields are permitted here"
                    .to_string(),
            ));
        }
        validate_action_parameters(
            &action_path,
            action,
            arazzo_version,
            provenance,
            action_key,
            action_is_component_owned,
            diagnostics,
        );
        validate_action_fixed_fields(
            &action_path,
            action,
            list_key.kind,
            fixed_field_origin,
            provenance.raw_action_boundaries_owned,
            diagnostics,
        );
        validate_action_target_references(
            &action_path,
            action,
            list_key.kind,
            target_ids,
            diagnostics,
        );
        for (criterion_idx, criterion) in action.criteria.iter().enumerate() {
            validate_criterion(
                &format!("{action_path}.criteria[{criterion_idx}]"),
                criterion,
                arazzo_version,
                diagnostics,
            );
        }
    }
}

fn action_allows_target(kind: ActionKind, action_type: ActionType) -> bool {
    action_type == ActionType::Goto
        || (kind == ActionKind::Failure && action_type == ActionType::Retry)
}

fn action_fields_are_component_owned(
    origin: Option<ComponentActionFixedFieldOrigin>,
    fields: &[ActionFixedField],
) -> bool {
    origin.is_some_and(|origin| {
        fields
            .iter()
            .all(|field| !origin.local_fields.contains(*field))
    })
}

/// Legacy name-form `retryAfter: 0` is a merge sentinel, not an override.
/// Keep raw local presence for applicability diagnostics, but assign range
/// validation to the component when the resolved value still comes from it.
fn retry_after_range_is_component_owned(origin: Option<ComponentActionFixedFieldOrigin>) -> bool {
    origin.is_some_and(|origin| !origin.local_retry_after_overrides)
}

/// Checks intrinsic Success/Failure Action fixed-field applicability. Target
/// existence is intentionally separate: a component action's `stepId` is
/// relative to the workflow that consumes it, while its type and supplied
/// fields have one component-owned definition site.
fn validate_action_fixed_fields(
    action_path: &str,
    action: &OnAction,
    kind: ActionKind,
    origin: Option<ComponentActionFixedFieldOrigin>,
    raw_action_boundaries_owned: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let action_type = action.action_type();
    if (!action.retry_after.is_finite() || action.retry_after < 0.0)
        && !retry_after_range_is_component_owned(origin)
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidRetryField,
            path: format!("{action_path}.retryAfter"),
            message: format!("{action_path}.retryAfter must be a non-negative finite number"),
        });
    }
    if !action.has_declared_type()
        && !raw_action_boundaries_owned
        && !action_fields_are_component_owned(origin, &[ActionFixedField::Type])
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{action_path}.type"),
            message: format!("{action_path}.type is required and must be a string"),
        });
    }
    if kind == ActionKind::Success
        && action_type == ActionType::Retry
        && !action_fields_are_component_owned(origin, &[ActionFixedField::Type])
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidRetryField,
            path: format!("{action_path}.type"),
            message: format!("{action_path}.type must be end or goto for a success action"),
        });
    }

    let retry_is_applicable = kind == ActionKind::Failure && action_type == ActionType::Retry;
    if !retry_is_applicable {
        if action.retry_limit.is_some()
            && !(raw_action_boundaries_owned && action.retry_limit == Some(0))
            && !action_fields_are_component_owned(
                origin,
                &[ActionFixedField::Type, ActionFixedField::RetryLimit],
            )
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidRetryField,
                path: format!("{action_path}.retryLimit"),
                message: format!(
                    "{action_path}.retryLimit is only applicable to a failure retry action"
                ),
            });
        }
        if action.retry_after.is_finite()
            && action.retry_after > 0.0
            && !action_fields_are_component_owned(
                origin,
                &[ActionFixedField::Type, ActionFixedField::RetryAfter],
            )
        {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidRetryField,
                path: format!("{action_path}.retryAfter"),
                message: format!(
                    "{action_path}.retryAfter is only applicable to a failure retry action"
                ),
            });
        }
    }

    if !action_allows_target(kind, action_type) {
        for (field, value) in [
            ("workflowId", &action.workflow_id),
            ("stepId", &action.step_id),
        ] {
            let fixed_field = match field {
                "workflowId" => ActionFixedField::WorkflowId,
                "stepId" => ActionFixedField::StepId,
                _ => unreachable!("the target field list is fixed"),
            };
            if !value.is_empty()
                && !action_fields_are_component_owned(
                    origin,
                    &[ActionFixedField::Type, fixed_field],
                )
            {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidReference,
                    path: format!("{action_path}.{field}"),
                    message: format!(
                        "{action_path}.{field} is only applicable to a goto action or failure retry action"
                    ),
                });
            }
        }
        return;
    }

    let has_step = !action.step_id.is_empty();
    let has_workflow = !action.workflow_id.is_empty();
    if action_type == ActionType::Goto
        && !has_step
        && !has_workflow
        && !action_fields_are_component_owned(
            origin,
            &[
                ActionFixedField::Type,
                ActionFixedField::WorkflowId,
                ActionFixedField::StepId,
            ],
        )
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: action_path.to_string(),
            message: format!("{action_path} goto action must specify stepId or workflowId"),
        });
    }
    if has_step
        && has_workflow
        && !action_fields_are_component_owned(
            origin,
            &[
                ActionFixedField::Type,
                ActionFixedField::WorkflowId,
                ActionFixedField::StepId,
            ],
        )
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidReference,
            path: action_path.to_string(),
            message: format!(
                "{action_path} {action_type} action specifies both stepId and workflowId; use one or the other"
            ),
        });
    }
}

/// Applies current-workflow target lookup after an action is consumed. A
/// reusable component action can validly name a step that exists only in the
/// consuming workflow, so this must retain the use-site path.
fn validate_action_target_references(
    action_path: &str,
    action: &OnAction,
    kind: ActionKind,
    target_ids: (&HashSet<&str>, &HashSet<&str>),
    diagnostics: &mut Vec<Diagnostic>,
) {
    let action_type = action.action_type();
    if !action_allows_target(kind, action_type) {
        return;
    }
    let (step_ids, workflow_ids) = target_ids;
    if !action.step_id.is_empty()
        && !action.step_id.starts_with('$')
        && !step_ids.contains(action.step_id.as_str())
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidReference,
            path: format!("{action_path}.stepId"),
            message: format!(
                "{action_path}.stepId references unknown step \"{}\"",
                action.step_id
            ),
        });
    }
    if !action.workflow_id.is_empty()
        && !action.workflow_id.starts_with('$')
        && !workflow_ids.contains(action.workflow_id.as_str())
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidReference,
            path: format!("{action_path}.workflowId"),
            message: format!(
                "{action_path}.workflowId references unknown workflow \"{}\"",
                action.workflow_id
            ),
        });
    }
}

/// Validates the `parameters` list of a success or failure action.
///
/// Success/Failure Action Object (Arazzo 1.1.0): *"A list of parameters that
/// MUST be passed to a workflow as referenced by `workflowId`. ... The list
/// MUST NOT include duplicate parameters. The `in` field MUST NOT be used."*
fn validate_action_parameters(
    action_path: &str,
    action: &OnAction,
    arazzo_version: &str,
    provenance: &ResolutionProvenance,
    action_key: DeclarationScope,
    parameters_are_component_owned: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if action.parameters.is_empty() || parameters_are_component_owned {
        return;
    }
    let params_path = format!("{action_path}.parameters");
    if declares_pre_1_1(arazzo_version) {
        // Same stance as `in: querystring`: 1.1.0 vocabulary in a document
        // declaring 1.0.x is an error, and the remedy is one line.
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::UnsupportedVersion,
            path: params_path.clone(),
            message: format!(
                "{params_path} — success/failure action parameters were introduced in \
                 Arazzo 1.1.0, but this document declares arazzo: {arazzo_version}, whose \
                 vocabulary has no such field; declare arazzo: 1.1.0 to use them"
            ),
        });
    }
    if action.workflow_id.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidParameterLocation,
            path: params_path.clone(),
            message: format!(
                "{params_path} is only valid when the action targets a workflow: the \
                 parameters \"MUST be passed to a workflow as referenced by workflowId\", \
                 but this action declares no workflowId"
            ),
        });
    }
    for (param_idx, param) in action.parameters.iter().enumerate() {
        let param_path = format!("{params_path}[{param_idx}]");
        if param.in_.is_some() {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidParameterLocation,
                path: format!("{param_path}.in"),
                message: format!(
                    "{param_path}.in must not be set: action parameters map to workflow \
                     inputs and \"The in field MUST NOT be used\""
                ),
            });
        }
    }
    validate_parameter_identities(
        &params_path,
        &action.parameters,
        ParameterIdentityMode::NameOnly,
        diagnostics,
    );
    // Shared per-parameter rules: required name/value and Selector Object shape.
    validate_parameters(
        &params_path,
        &action.parameters,
        arazzo_version,
        provenance,
        action_key,
        diagnostics,
    );
}

fn validate_criterion(
    path: &str,
    criterion: &SuccessCriterion,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_unknown_fields(path, &criterion.extensions, diagnostics);
    if criterion.condition.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.condition"),
            message: format!("{path}.condition is required"),
        });
    }

    if criterion.has_declared_type() && criterion.context.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.context"),
            message: format!("{path}.context is required when type is specified"),
        });
    }

    let Some(type_) = &criterion.type_ else {
        return;
    };

    validate_expression_type(
        &format!("{path}.type"),
        type_,
        &CRITERION_TYPE_RULES,
        arazzo_version,
        diagnostics,
    );
}

/// Resolves `$ref` in workflow inputs against `components.inputs`.
fn resolve_input_refs(spec: &mut ArazzoSpec) -> Result<(), String> {
    let has_inputs = spec
        .components
        .as_ref()
        .is_some_and(|c| !c.inputs.is_empty());
    if !has_inputs {
        return Ok(());
    }
    // Borrow-checked: we need a clone of inputs because we mutate spec.workflows below.
    let input_map = spec
        .components
        .as_ref()
        .map(|c| c.inputs.clone())
        .unwrap_or_default();

    for workflow in &mut spec.workflows {
        let Some(schema) = &mut workflow.inputs else {
            continue;
        };
        if schema.ref_.is_empty() {
            continue;
        }
        let name = schema
            .ref_
            .strip_prefix("#/components/inputs/")
            .ok_or_else(|| {
                format!(
                    "workflow {}: unsupported input $ref: {} \
                     (expected #/components/inputs/<name>)",
                    workflow.workflow_id, schema.ref_
                )
            })?;
        let resolved = input_map.get(name).ok_or_else(|| {
            format!(
                "workflow {}: component input \"{}\" not found",
                workflow.workflow_id, name
            )
        })?;
        *schema = resolved.clone();
    }
    Ok(())
}

/// Resolves `$components.*` references into a private/effective validation
/// model and records which source declaration owns intrinsic diagnostics.
fn resolve_components(
    spec: &mut ArazzoSpec,
    raw: Option<&serde_yaml_ng::Value>,
) -> Result<ResolutionProvenance, String> {
    resolve_input_refs(spec)?;

    let mut provenance = ResolutionProvenance {
        raw_action_boundaries_owned: raw.is_some(),
        ..ResolutionProvenance::default()
    };
    let mut components = spec.components.clone().unwrap_or_default();
    let component_lookup = components.clone();
    let raw_components = raw.and_then(|root| raw_mapping_field(root, "components"));

    // Component maps contain Action Objects, so their Parameter references
    // must be effective even when no workflow consumes the action. Resolve
    // only nested Parameter Reusable Objects; an action-level `reference`
    // field is invalid here and never turns the definition into a wrapper.
    for (field, kind, actions) in [
        (
            "successActions",
            ActionKind::Success,
            &mut components.success_actions,
        ),
        (
            "failureActions",
            ActionKind::Failure,
            &mut components.failure_actions,
        ),
    ] {
        for (action_index, (name, action)) in actions.iter_mut().enumerate() {
            let action_path = format!("components.{field}.{name}");
            let raw_parameters = raw_component_action(raw_components, field, name)
                .and_then(|action| raw_mapping_field(action, "parameters"));
            resolve_param_refs(
                &mut action.parameters,
                &component_lookup,
                raw_parameters,
                &format!("{action_path}.parameters"),
                &action_path,
                DeclarationScope::ComponentAction { kind, action_index },
                &mut provenance,
            )?;
        }
    }
    if spec.components.is_some() {
        spec.components = Some(components.clone());
    }

    let raw_workflows = raw
        .and_then(|root| raw_mapping_field(root, "workflows"))
        .and_then(serde_yaml_ng::Value::as_sequence);

    for (workflow_index, workflow) in spec.workflows.iter_mut().enumerate() {
        let wf_label = format!("workflow {}", workflow.workflow_id);
        let wf_path = if workflow.workflow_id.is_empty() {
            format!("workflows[{workflow_index}]")
        } else {
            format!("workflow \"{}\"", workflow.workflow_id)
        };
        let raw_workflow = raw_workflows.and_then(|workflows| workflows.get(workflow_index));
        resolve_param_refs(
            &mut workflow.parameters,
            &components,
            raw_workflow.and_then(|workflow| raw_mapping_field(workflow, "parameters")),
            &format!("{wf_path}.parameters"),
            &wf_label,
            DeclarationScope::WorkflowParameters { workflow_index },
            &mut provenance,
        )?;
        resolve_action_ref(
            &mut workflow.success_actions,
            ActionResolutionContext {
                components: &components,
                component_map: &components.success_actions,
                prefix: "$components.successActions.",
                kind: "successAction",
                raw_actions: raw_workflow
                    .and_then(|workflow| raw_mapping_field(workflow, "successActions")),
                raw_components,
                component_field: "successActions",
                entity: &wf_label,
                path_prefix: &format!("{wf_path}.successActions"),
                list_key: ActionListScope {
                    kind: ActionKind::Success,
                    workflow_index,
                    step_index: None,
                },
            },
            &mut provenance,
        )?;
        resolve_action_ref(
            &mut workflow.failure_actions,
            ActionResolutionContext {
                components: &components,
                component_map: &components.failure_actions,
                prefix: "$components.failureActions.",
                kind: "failureAction",
                raw_actions: raw_workflow
                    .and_then(|workflow| raw_mapping_field(workflow, "failureActions")),
                raw_components,
                component_field: "failureActions",
                entity: &wf_label,
                path_prefix: &format!("{wf_path}.failureActions"),
                list_key: ActionListScope {
                    kind: ActionKind::Failure,
                    workflow_index,
                    step_index: None,
                },
            },
            &mut provenance,
        )?;

        let raw_steps = raw_workflow
            .and_then(|workflow| raw_mapping_field(workflow, "steps"))
            .and_then(serde_yaml_ng::Value::as_sequence);
        for (step_index, step) in workflow.steps.iter_mut().enumerate() {
            let step_label = format!("step {}", step.step_id);
            let step_path = if step.step_id.is_empty() {
                format!("{wf_path} > steps[{step_index}]")
            } else {
                format!("{wf_path} > step \"{}\"", step.step_id)
            };
            let raw_step = raw_steps.and_then(|steps| steps.get(step_index));
            resolve_param_refs(
                &mut step.parameters,
                &components,
                raw_step.and_then(|step| raw_mapping_field(step, "parameters")),
                &format!("{step_path}.parameters"),
                &step_label,
                DeclarationScope::StepParameters {
                    workflow_index,
                    step_index,
                },
                &mut provenance,
            )?;
            resolve_action_ref(
                &mut step.on_success,
                ActionResolutionContext {
                    components: &components,
                    component_map: &components.success_actions,
                    prefix: "$components.successActions.",
                    kind: "successAction",
                    raw_actions: raw_step.and_then(|step| raw_mapping_field(step, "onSuccess")),
                    raw_components,
                    component_field: "successActions",
                    entity: &step_label,
                    path_prefix: &format!("{step_path}.onSuccess"),
                    list_key: ActionListScope {
                        kind: ActionKind::Success,
                        workflow_index,
                        step_index: Some(step_index),
                    },
                },
                &mut provenance,
            )?;
            resolve_action_ref(
                &mut step.on_failure,
                ActionResolutionContext {
                    components: &components,
                    component_map: &components.failure_actions,
                    prefix: "$components.failureActions.",
                    kind: "failureAction",
                    raw_actions: raw_step.and_then(|step| raw_mapping_field(step, "onFailure")),
                    raw_components,
                    component_field: "failureActions",
                    entity: &step_label,
                    path_prefix: &format!("{step_path}.onFailure"),
                    list_key: ActionListScope {
                        kind: ActionKind::Failure,
                        workflow_index,
                        step_index: Some(step_index),
                    },
                },
                &mut provenance,
            )?;
        }
    }

    Ok(provenance)
}

fn resolve_param_refs(
    params: &mut Vec<Parameter>,
    components: &arazzo_spec::Components,
    raw_parameters: Option<&serde_yaml_ng::Value>,
    path_prefix: &str,
    entity: &str,
    list_origin: DeclarationScope,
    provenance: &mut ResolutionProvenance,
) -> Result<(), String> {
    let mut resolved = Vec::with_capacity(params.len());
    for (parameter_index, mut param) in params.drain(..).enumerate() {
        if !param.reference.is_empty() {
            if raw_parameters.is_none() && !typed_reusable_parameter_value_is_valid(&param.value) {
                provenance
                    .reusable_value_diagnostics
                    .push(invalid_reusable_value_diagnostic(&format!(
                        "{path_prefix}[{parameter_index}]"
                    )));
            }
            let Some(name) = param.reference.strip_prefix("$components.parameters.") else {
                return Err(format!(
                    "{entity}: unsupported parameter reference: {}",
                    param.reference
                ));
            };
            let Some((component_parameter_index, component_param)) =
                component_entry(&components.parameters, name)
            else {
                return Err(format!(
                    "{entity}: component parameter \"{name}\" not found"
                ));
            };
            if param.name.is_empty() {
                param.name = component_param.name.clone();
            }
            if param.in_.is_none() {
                param.in_ = component_param.in_;
            }
            // A Reusable Object may omit `value` to inherit the component
            // value, but an explicitly present null or empty string is an
            // Any value that overrides it. The public typed model intentionally
            // does not carry a wire-presence flag, so consult the matching raw
            // list entry at this parse boundary.
            let value_is_omitted = match raw_parameters {
                Some(_) => !raw_parameter_value_is_present(raw_parameters, parameter_index),
                None => matches!(
                    &param.value,
                    ValueSource::Literal(serde_yaml_ng::Value::Null)
                ),
            };
            if value_is_omitted {
                param.value = component_param.value.clone();
            }
            param.extensions = component_param.extensions.clone();
            param.reference.clear();
            provenance.component_parameter_origins.insert(
                list_origin.declaration(parameter_index),
                ComponentParameterKey {
                    parameter_index: component_parameter_index,
                },
            );
        }
        resolved.push(param);
    }
    *params = resolved;
    Ok(())
}

struct ActionResolutionContext<'a> {
    components: &'a arazzo_spec::Components,
    component_map: &'a std::collections::BTreeMap<String, OnAction>,
    prefix: &'a str,
    kind: &'a str,
    raw_actions: Option<&'a serde_yaml_ng::Value>,
    raw_components: Option<&'a serde_yaml_ng::Value>,
    component_field: &'a str,
    entity: &'a str,
    path_prefix: &'a str,
    list_key: ActionListScope,
}

fn local_action_fixed_fields(
    raw_action: Option<&serde_yaml_ng::Value>,
    action: &OnAction,
) -> ActionLocalFields {
    if let Some(action) = raw_action.filter(|action| action.as_mapping().is_some()) {
        return ActionLocalFields {
            type_: raw_mapping_has_field(action, "type"),
            workflow_id: raw_mapping_has_field(action, "workflowId"),
            step_id: raw_mapping_has_field(action, "stepId"),
            retry_after: raw_mapping_has_field(action, "retryAfter"),
            retry_limit: raw_mapping_has_field(action, "retryLimit"),
        };
    }
    ActionLocalFields {
        type_: action.type_.is_some(),
        workflow_id: !action.workflow_id.is_empty(),
        step_id: !action.step_id.is_empty(),
        retry_after: action.retry_after != 0.0,
        retry_limit: action.retry_limit.is_some(),
    }
}

fn component_entry<'a, T>(
    map: &'a std::collections::BTreeMap<String, T>,
    name: &str,
) -> Option<(usize, &'a T)> {
    map.iter()
        .enumerate()
        .find_map(|(index, (candidate, value))| {
            (candidate.as_str() == name).then_some((index, value))
        })
}

fn resolve_action_ref(
    actions: &mut [OnAction],
    context: ActionResolutionContext<'_>,
    provenance: &mut ResolutionProvenance,
) -> Result<(), String> {
    for (action_idx, action) in actions.iter_mut().enumerate() {
        let action_path = format!("{}[{action_idx}]", context.path_prefix);
        let action_key = context.list_key.declaration(action_idx);
        let raw_action = context
            .raw_actions
            .and_then(serde_yaml_ng::Value::as_sequence)
            .and_then(|actions| actions.get(action_idx));
        let mut raw_parameters =
            raw_action.and_then(|action| raw_mapping_field(action, "parameters"));
        let local_parameters_are_non_empty = !action.parameters.is_empty();
        let mut component_origin = None;
        let mut fixed_field_origin = None;
        // The raw parse boundary classifies empty/non-runtime-expression
        // references as an invalid Reusable Object. Replace its ignored
        // siblings before typed validation so the one structured reference
        // diagnostic owns the failure; direct typed callers retain the
        // existing component-resolution rule.
        if raw_action
            .and_then(|action| raw_mapping_field(action, "reference"))
            .is_some_and(|reference| match reference {
                serde_yaml_ng::Value::String(reference) => {
                    reference.is_empty() || !reference.starts_with('$')
                }
                _ => true,
            })
        {
            *action = OnAction::default();
            continue;
        }
        if !action.reference.is_empty() {
            if context.raw_actions.is_none()
                && !typed_reusable_action_value_is_valid(action.value.as_ref())
            {
                provenance
                    .reusable_value_diagnostics
                    .push(invalid_reusable_value_diagnostic(&action_path));
            }
            let Some(name) = action.reference.strip_prefix(context.prefix) else {
                return Err(format!(
                    "{}: unsupported {} reference: {}",
                    context.entity, context.kind, action.reference
                ));
            };
            let component_name = name.to_string();
            let Some((component_index, component)) =
                component_entry(context.component_map, &component_name)
            else {
                return Err(format!(
                    "{}: component {} \"{component_name}\" not found",
                    context.entity, context.kind
                ));
            };
            *action = component.clone();
            action.reference.clear();
            action.value = None;
            let source = DeclarationScope::ComponentAction {
                kind: context.list_key.kind,
                action_index: component_index,
            };
            component_origin = Some(source);
            fixed_field_origin = Some(ComponentActionFixedFieldOrigin {
                local_retry_after_overrides: false,
                local_fields: ActionLocalFields::default(),
            });
            raw_parameters = raw_component_action(
                context.raw_components,
                context.component_field,
                &component_name,
            )
            .and_then(|component| raw_mapping_field(component, "parameters"));
        } else if !action.name.is_empty() {
            let local_action_type = action.type_;
            let local_fields = local_action_fixed_fields(raw_action, action);
            let local_retry_after_overrides = action.retry_after != 0.0;
            if let Some(component_name) = resolve_one_action_ref(
                action,
                context.component_map,
                context.prefix,
                context.kind,
                context.entity,
            )? {
                let Some((component_index, component)) =
                    component_entry(context.component_map, &component_name)
                else {
                    return Err(format!(
                        "{}: component {} \"{component_name}\" not found",
                        context.entity, context.kind
                    ));
                };
                let source = DeclarationScope::ComponentAction {
                    kind: context.list_key.kind,
                    action_index: component_index,
                };
                fixed_field_origin = Some(ComponentActionFixedFieldOrigin {
                    local_retry_after_overrides,
                    local_fields,
                });
                let raw_component = raw_component_action(
                    context.raw_components,
                    context.component_field,
                    &component_name,
                );
                if let Some(raw_action) = raw_action {
                    check_raw_defaulted_action_field_applicability(
                        &action_path,
                        raw_action,
                        context.list_key.kind,
                        action.action_type(),
                        &mut provenance.raw_legacy_action_diagnostics,
                    );
                    if let (Some(local_type), Some(component_type), Some(raw_component)) =
                        (local_action_type, component.type_, raw_component)
                    {
                        if local_type != component_type {
                            check_raw_inherited_component_field_applicability(
                                &action_path,
                                raw_component,
                                context.list_key.kind,
                                component_type,
                                action.action_type(),
                                local_fields,
                                &mut provenance.raw_legacy_action_diagnostics,
                            );
                        }
                    }
                }
                // `resolve_one_action_ref` preserves a non-empty local list,
                // matching the existing merge behavior. Otherwise the copied
                // component list needs its own raw presence source.
                if !local_parameters_are_non_empty {
                    component_origin = Some(source);
                    raw_parameters = raw_component
                        .and_then(|component| raw_mapping_field(component, "parameters"));
                }
            }
        }
        if let Some(component_origin) = component_origin {
            provenance
                .component_action_origins
                .insert(action_key, component_origin);
        }
        if let Some(fixed_field_origin) = fixed_field_origin {
            provenance
                .component_action_fixed_field_origins
                .insert(action_key, fixed_field_origin);
        }
        // Action parameters may themselves be Reusable Objects pointing at
        // `$components.parameters.<name>`. Resolve them after the action-level
        // merge so parameters inherited from a component action resolve too.
        resolve_param_refs(
            &mut action.parameters,
            context.components,
            raw_parameters,
            &format!("{action_path}.parameters"),
            &format!("{} {}[{action_idx}]", context.entity, context.kind),
            action_key,
            provenance,
        )?;
    }
    Ok(())
}

fn resolve_one_action_ref(
    action: &mut OnAction,
    component_map: &std::collections::BTreeMap<String, OnAction>,
    prefix: &str,
    kind: &str,
    entity: &str,
) -> Result<Option<String>, String> {
    if let Some(name) = action.name.strip_prefix(prefix) {
        let component_name = name.to_string();
        let Some(resolved) = component_map.get(&component_name) else {
            return Err(format!(
                "{entity}: component {kind} \"{component_name}\" not found"
            ));
        };
        // Merge: start with resolved component, overlay locally declared fields
        let mut merged = resolved.clone();
        if action.type_.is_some() {
            merged.type_ = action.type_;
        }
        if !action.workflow_id.is_empty() {
            merged.workflow_id = action.workflow_id.clone();
        }
        if !action.step_id.is_empty() {
            merged.step_id = action.step_id.clone();
        }
        if action.retry_after != 0.0 {
            merged.retry_after = action.retry_after;
        }
        if action.retry_limit.is_some() {
            merged.retry_limit = action.retry_limit;
        }
        if !action.criteria.is_empty() {
            merged.criteria = action.criteria.clone();
        }
        if !action.parameters.is_empty() {
            merged.parameters = action.parameters.clone();
        }
        merged.reference.clear();
        merged.value = None;
        *action = merged;
        return Ok(Some(component_name));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
