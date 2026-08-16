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
    resolve_components(&mut spec, &raw).map_err(Error::ComponentResolution)?;

    // `successCriteria` emptiness cannot be seen in the typed model — see the
    // comment on `check_raw_success_criteria` — so it is checked against the
    // raw bytes here, the one place both the document text and the rest of
    // the diagnostics pipeline are in hand. `validate`/`validate_diagnostics`
    // take only `&ArazzoSpec` and therefore cannot enforce this rule.
    let mut diagnostics = collect_diagnostics(&spec, Some(&raw));
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
    partition_diagnostics(collect_diagnostics(spec, None))
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

    let value_path = format!("{path}.value");
    diagnostics.push(Diagnostic {
        severity: Severity::Error,
        kind: ValidationErrorKind::InvalidReference,
        path: value_path.clone(),
        message: format!("{value_path} must be a string when used on a Reusable Object"),
    });
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

fn raw_unknown_action_field(path: &str, field: &str, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.push(Diagnostic::warning(
        ValidationErrorKind::UnknownField,
        path,
        format!(
            "unrecognized field \"{field}\"; only `x-` prefixed extension fields are permitted here"
        ),
    ));
}

fn check_raw_action_boundary(
    path: &str,
    action: &serde_yaml_ng::Value,
    component: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let reference = raw_mapping_field(action, "reference");
    let value = raw_mapping_field(action, "value");
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
    if !component && reference_is_non_string {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::InvalidReference,
            path: path.to_string(),
            message: "reference must be a runtime expression string".to_string(),
        });
    }
    if !component && reference.is_some() {
        check_raw_reusable_value(action, path, diagnostics);
    }
    if !component
        && matches!(value, Some(serde_yaml_ng::Value::Null))
        && reference.is_none_or(|reference| {
            matches!(reference, serde_yaml_ng::Value::String(reference) if reference.is_empty())
        })
    {
        raw_unknown_action_field(path, "value", diagnostics);
    }
}

fn check_raw_action_list(
    value: Option<&serde_yaml_ng::Value>,
    path: &str,
    component: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(serde_yaml_ng::Value::Sequence(actions)) = value else {
        return;
    };
    for (index, action) in actions.iter().enumerate() {
        check_raw_action_boundary(&format!("{path}[{index}]"), action, component, diagnostics);
    }
}

/// Typed `Option<Value>` fields cannot distinguish an omitted `value` from an
/// explicit YAML null, and an empty reference is the typed default. Inspect
/// only these boundary shapes at the parse boundary so diagnostics retain the
/// document's field presence without adding wire metadata to the public model.
fn check_raw_action_field_boundaries(root: &serde_yaml_ng::Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(components) = raw_mapping_field(root, "components") {
        for (field, path) in [
            ("successActions", "components.successActions"),
            ("failureActions", "components.failureActions"),
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
        for field in ["successActions", "failureActions"] {
            check_raw_action_list(
                raw_mapping_field(workflow, field),
                &format!("{workflow_path}.{field}"),
                false,
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
            for field in ["onSuccess", "onFailure"] {
                check_raw_action_list(
                    raw_mapping_field(step, field),
                    &format!("{step_path}.{field}"),
                    false,
                    &mut diagnostics,
                );
            }
        }
    }
    diagnostics
}

fn collect_diagnostics(spec: &ArazzoSpec, raw: Option<&serde_yaml_ng::Value>) -> Vec<Diagnostic> {
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

    let raw_components = raw.and_then(|root| raw_mapping_field(root, "components"));
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
        for (name, action) in &components.success_actions {
            check_unknown_fields(
                &format!("components.successActions.{name}"),
                &action.extensions,
                &mut diagnostics,
            );
            warn_action_reusable_fields(
                &format!("components.successActions.{name}"),
                action,
                &mut diagnostics,
            );
            validate_parameters(
                &format!("components.successActions.{name}.parameters"),
                &action.parameters,
                &spec.arazzo,
                raw_component_action(raw_components, "successActions", name)
                    .and_then(|action| raw_mapping_field(action, "parameters")),
                &mut diagnostics,
            );
        }
        for (name, action) in &components.failure_actions {
            check_unknown_fields(
                &format!("components.failureActions.{name}"),
                &action.extensions,
                &mut diagnostics,
            );
            warn_action_reusable_fields(
                &format!("components.failureActions.{name}"),
                action,
                &mut diagnostics,
            );
            validate_parameters(
                &format!("components.failureActions.{name}.parameters"),
                &action.parameters,
                &spec.arazzo,
                raw_component_action(raw_components, "failureActions", name)
                    .and_then(|action| raw_mapping_field(action, "parameters")),
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

    let raw_workflows = raw
        .and_then(|root| raw_mapping_field(root, "workflows"))
        .and_then(serde_yaml_ng::Value::as_sequence);
    for (wf_idx, wf) in spec.workflows.iter().enumerate() {
        let raw_workflow = raw_workflows.and_then(|workflows| workflows.get(wf_idx));
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
            raw_workflow.and_then(|workflow| raw_mapping_field(workflow, "parameters")),
            &mut diagnostics,
        );
        validate_parameter_identities(
            &format!("{path}.parameters"),
            &wf.parameters,
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
            &step_ids,
            &workflow_ids,
            &spec.arazzo,
            raw_workflow.and_then(|workflow| raw_mapping_field(workflow, "successActions")),
            &mut diagnostics,
        );
        validate_actions(
            &format!("{path}.failureActions"),
            &wf.failure_actions,
            &step_ids,
            &workflow_ids,
            &spec.arazzo,
            raw_workflow.and_then(|workflow| raw_mapping_field(workflow, "failureActions")),
            &mut diagnostics,
        );

        let mut seen_step_ids = HashSet::<&str>::new();
        let raw_steps = raw_workflow
            .and_then(|workflow| raw_mapping_field(workflow, "steps"))
            .and_then(serde_yaml_ng::Value::as_sequence);
        for (step_idx, step) in wf.steps.iter().enumerate() {
            let raw_step = raw_steps.and_then(|steps| steps.get(step_idx));
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
                raw_step.and_then(|step| raw_mapping_field(step, "parameters")),
                &mut diagnostics,
            );
            validate_parameter_identities(
                &format!("{step_path}.parameters"),
                &step.parameters,
                &mut diagnostics,
            );
            validate_effective_parameter_context(&path, wf, &step_path, step, &mut diagnostics);
            validate_querystring_exclusivity(&step_path, wf, step, &mut diagnostics);
            for (name, output) in &step.outputs {
                let output_path = format!("{step_path}.outputs.{name}");
                validate_output_value(&output_path, output, &mut diagnostics);
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
                        &mut diagnostics,
                    );
                }
                validate_replacements(
                    &format!("{step_path}.requestBody.replacements"),
                    &request_body.replacements,
                    &mut diagnostics,
                );
            }

            for (criterion_idx, criterion) in step.success_criteria.iter().enumerate() {
                validate_criterion(
                    &format!("{step_path}.successCriteria[{criterion_idx}]"),
                    criterion,
                    &mut diagnostics,
                );
            }

            validate_actions(
                &format!("{step_path}.onFailure"),
                &step.on_failure,
                &step_ids,
                &workflow_ids,
                &spec.arazzo,
                raw_step.and_then(|step| raw_mapping_field(step, "onFailure")),
                &mut diagnostics,
            );
            validate_actions(
                &format!("{step_path}.onSuccess"),
                &step.on_success,
                &step_ids,
                &workflow_ids,
                &spec.arazzo,
                raw_step.and_then(|step| raw_mapping_field(step, "onSuccess")),
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
            validate_output_value(&output_path, output, &mut diagnostics);
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
}

/// Mirrors `merge_workflow_params` in `arazzo-runtime`: workflow-level
/// parameters are inherited only by non-workflow targets, and a Step replaces
/// the same `(name, in)` identity instead of being a duplicate across lists.
fn effective_parameters<'a>(
    workflow: &'a Workflow,
    workflow_path: &str,
    step: &'a Step,
    step_path: &str,
) -> Vec<EffectiveParameter<'a>> {
    let step_keys: HashSet<(&str, Option<ParamLocation>)> = step
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter.in_))
        .collect();
    let mut params = Vec::new();
    if !matches!(&step.target, Some(StepTarget::WorkflowId(_))) {
        params.extend(
            workflow
                .parameters
                .iter()
                .enumerate()
                .filter(|(_, parameter)| {
                    !step_keys.contains(&(parameter.name.as_str(), parameter.in_))
                })
                .map(|(index, parameter)| EffectiveParameter {
                    parameter,
                    path: format!("{workflow_path}.parameters[{index}]"),
                }),
        );
    }
    params.extend(
        step.parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| EffectiveParameter {
                parameter,
                path: format!("{step_path}.parameters[{index}]"),
            }),
    );
    params
}

/// Parameter Object §5.8.6.1 is context-sensitive: operation targets require
/// `in`, while a `workflowId` target maps every parameter to workflow inputs
/// and therefore prohibits it. `channelPath` has AsyncAPI transport semantics
/// this executor does not implement, so a parameter-bearing channel step fails
/// validation rather than being assigned invented HTTP semantics.
fn validate_effective_parameter_context(
    workflow_path: &str,
    workflow: &Workflow,
    step_path: &str,
    step: &Step,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let parameters = effective_parameters(workflow, workflow_path, step, step_path);
    let Some(target) = &step.target else {
        return;
    };

    match target {
        StepTarget::OperationId(_) | StepTarget::OperationPath(_) => {
            for parameter in parameters {
                if parameter.parameter.in_.is_none() {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidParameterLocation,
                        path: format!("{}.in", parameter.path),
                        message: format!(
                            "{}.in is required when the Step targets an operation",
                            parameter.path
                        ),
                    });
                }
            }
        }
        StepTarget::WorkflowId(_) => {
            for parameter in parameters {
                if parameter.parameter.in_.is_some() {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        kind: ValidationErrorKind::InvalidParameterLocation,
                        path: format!("{}.in", parameter.path),
                        message: format!(
                            "{}.in must not be set when the Step targets a workflow: \
                             parameters map to workflow inputs",
                            parameter.path
                        ),
                    });
                }
            }
        }
        StepTarget::ChannelPath(_) => {
            for parameter in parameters {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidParameterLocation,
                    path: parameter.path.clone(),
                    message: format!(
                        "{} cannot be used with a channelPath target: AsyncAPI parameter \
                         transport is not supported by this executor",
                        parameter.path
                    ),
                });
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
    step_path: &str,
    workflow: &Workflow,
    step: &Step,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let params = effective_parameters(workflow, "", step, step_path);
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
            path: format!("{step_path}.parameters"),
            message: format!(
                "{step_path} carries more than one in: querystring parameter [{}]; the \
                 querystring location supplies the entire query component and must not \
                 appear more than once in the same operation. Workflow-level parameters \
                 are inherited by this step and count here.",
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
        path: format!("{step_path}.parameters"),
        message: format!(
            "{step_path} carries in: querystring parameter(s) [{}] alongside in: query \
             parameter(s) [{}]; the querystring location supplies the entire query \
             component and cannot coexist with query parameters in the same operation. \
             Workflow-level parameters are inherited by this step and count here.",
            querystring.join(", "),
            query.join(", ")
        ),
    });
}

fn raw_parameter_is_reusable(
    raw_parameters: Option<&serde_yaml_ng::Value>,
    parameter_index: usize,
) -> bool {
    raw_parameters
        .and_then(serde_yaml_ng::Value::as_sequence)
        .and_then(|parameters| parameters.get(parameter_index))
        .is_some_and(|parameter| raw_mapping_has_field(parameter, "reference"))
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
    validate_value_source(&format!("{param_path}.value"), &param.value, diagnostics);
}

fn validate_parameters(
    path_prefix: &str,
    params: &[Parameter],
    arazzo_version: &str,
    raw_parameters: Option<&serde_yaml_ng::Value>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (param_idx, param) in params.iter().enumerate() {
        let param_path = format!("{path_prefix}[{param_idx}]");
        // A source Reusable Object has already expanded to its component
        // definition. Validate that definition once at components.* and only
        // apply target context at this consuming path below.
        if !raw_parameter_is_reusable(raw_parameters, param_idx) {
            validate_parameter(&param_path, param, arazzo_version, true, diagnostics);
        }
    }
}

/// The Parameter Object identity is the case-sensitive `(name, in)` pair.
/// This is intentionally per declared list: a Step override lives in another
/// list and was already selected into the effective set above.
fn validate_parameter_identities(
    path_prefix: &str,
    params: &[Parameter],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut first_declarations = HashMap::<(String, Option<ParamLocation>), String>::new();
    for (index, parameter) in params.iter().enumerate() {
        if parameter.name.is_empty() {
            continue;
        }
        let parameter_path = format!("{path_prefix}[{index}]");
        let identity = (parameter.name.clone(), parameter.in_);
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

fn validate_output_value(path: &str, output: &OutputValue, diagnostics: &mut Vec<Diagnostic>) {
    if let OutputValue::Selector(selector) = output {
        validate_selector(path, selector, diagnostics);
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

fn validate_value_source(path: &str, value: &ValueSource, diagnostics: &mut Vec<Diagnostic>) {
    match value {
        ValueSource::Selector(selector) => validate_selector(path, selector, diagnostics),
        ValueSource::Literal(serde_yaml_ng::Value::Sequence(values)) => {
            for (index, value) in values.iter().enumerate() {
                validate_value_source(
                    &format!("{path}[{index}]"),
                    &value.clone().into(),
                    diagnostics,
                );
            }
        }
        ValueSource::Literal(serde_yaml_ng::Value::Mapping(values)) => {
            for (key, value) in values {
                let key = key.as_str().unwrap_or("<non-string-key>");
                validate_value_source(&format!("{path}.{key}"), &value.clone().into(), diagnostics);
            }
        }
        ValueSource::Literal(_) => {}
    }
}

fn validate_selector(path: &str, selector: &SelectorObject, diagnostics: &mut Vec<Diagnostic>) {
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
}

const SELECTOR_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["jsonpath", "xpath", "jsonpointer"],
    names_label: "jsonpath, xpath, or jsonpointer",
    allowed_object_types: &["jsonpath", "xpath", "jsonpointer"],
    object_types_label: "jsonpath, xpath, or jsonpointer",
    kind: ValidationErrorKind::InvalidSelectorType,
};

const CRITERION_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["simple", "regex", "jsonpath", "xpath"],
    names_label: "simple, regex, jsonpath, xpath",
    allowed_object_types: &["jsonpath", "xpath"],
    object_types_label: "jsonpath or xpath",
    kind: ValidationErrorKind::InvalidCriterionType,
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
            }
        }
    }
}

fn validate_replacements(
    path_prefix: &str,
    replacements: &[arazzo_spec::Replacement],
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
                diagnostics,
            );
        }
        validate_value_source(
            &format!("{path_prefix}[{replacement_idx}].value"),
            &replacement.value,
            diagnostics,
        );
    }
}

fn validate_actions(
    path_prefix: &str,
    actions: &[OnAction],
    step_ids: &HashSet<&str>,
    workflow_ids: &HashSet<&str>,
    arazzo_version: &str,
    raw_actions: Option<&serde_yaml_ng::Value>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (action_idx, action) in actions.iter().enumerate() {
        let action_path = format!("{path_prefix}[{action_idx}]");
        let raw_action = raw_actions
            .and_then(serde_yaml_ng::Value::as_sequence)
            .and_then(|actions| actions.get(action_idx));
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
            raw_action,
            diagnostics,
        );
        let action_type = action.action_type();
        // Reference checks apply to goto and retry alike: both action types
        // carry an optional stepId/workflowId reference pair. The reference is
        // required for goto (a transfer needs a destination) but optional for
        // retry (Failure Action Object: "If a stepId or workflowId are
        // specified, then the reference is executed ...").
        if matches!(action_type, ActionType::Goto | ActionType::Retry) {
            let has_step = !action.step_id.is_empty();
            let has_workflow = !action.workflow_id.is_empty();
            if action_type == ActionType::Goto && !has_step && !has_workflow {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: action_path.clone(),
                    message: format!("{action_path} goto action must specify stepId or workflowId"),
                });
            }
            if has_step && has_workflow {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::InvalidReference,
                    path: action_path.clone(),
                    message: format!(
                        "{action_path} {action_type} action specifies both stepId and workflowId; use one or the other"
                    ),
                });
            }
            if has_step
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
            if has_workflow
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
        if action.action_type() != ActionType::Retry {
            if action.retry_limit.is_some() {
                diagnostics.push(Diagnostic::warning(
                    ValidationErrorKind::InvalidRetryField,
                    format!("{action_path}.retryLimit"),
                    format!(
                        "{action_path}.retryLimit has no effect on {} action",
                        action.action_type()
                    ),
                ));
            }
            if action.retry_after > 0 {
                diagnostics.push(Diagnostic::warning(
                    ValidationErrorKind::InvalidRetryField,
                    format!("{action_path}.retryAfter"),
                    format!(
                        "{action_path}.retryAfter has no effect on {} action",
                        action.action_type()
                    ),
                ));
            }
        }
        for (criterion_idx, criterion) in action.criteria.iter().enumerate() {
            validate_criterion(
                &format!("{action_path}.criteria[{criterion_idx}]"),
                criterion,
                diagnostics,
            );
        }
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
    raw_action: Option<&serde_yaml_ng::Value>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if action.parameters.is_empty() {
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
    validate_parameter_identities(&params_path, &action.parameters, diagnostics);
    // Shared per-parameter rules: required name/value and Selector Object shape.
    // A Reusable Action replaces itself wholesale. Its component action
    // parameters have their intrinsic validation at the component source; the
    // expanded parameters above still receive action context checks.
    if !raw_action.is_some_and(|action| raw_mapping_has_field(action, "reference")) {
        validate_parameters(
            &params_path,
            &action.parameters,
            arazzo_version,
            raw_action.and_then(|action| raw_mapping_field(action, "parameters")),
            diagnostics,
        );
    }
}

fn validate_criterion(path: &str, criterion: &SuccessCriterion, diagnostics: &mut Vec<Diagnostic>) {
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

/// Resolves `$components.*` references to inline step definitions.
fn resolve_components(spec: &mut ArazzoSpec, raw: &serde_yaml_ng::Value) -> Result<(), String> {
    resolve_input_refs(spec)?;

    let components = spec.components.clone().unwrap_or_default();
    let raw_components = raw_mapping_field(raw, "components");
    let raw_workflows =
        raw_mapping_field(raw, "workflows").and_then(serde_yaml_ng::Value::as_sequence);

    for (workflow_index, workflow) in spec.workflows.iter_mut().enumerate() {
        let wf_label = format!("workflow {}", workflow.workflow_id);
        let raw_workflow = raw_workflows.and_then(|workflows| workflows.get(workflow_index));
        resolve_param_refs(
            &mut workflow.parameters,
            &components,
            raw_workflow.and_then(|workflow| raw_mapping_field(workflow, "parameters")),
            &wf_label,
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
            },
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
            },
        )?;

        let raw_steps = raw_workflow
            .and_then(|workflow| raw_mapping_field(workflow, "steps"))
            .and_then(serde_yaml_ng::Value::as_sequence);
        for (step_index, step) in workflow.steps.iter_mut().enumerate() {
            let step_label = format!("step {}", step.step_id);
            let raw_step = raw_steps.and_then(|steps| steps.get(step_index));
            resolve_param_refs(
                &mut step.parameters,
                &components,
                raw_step.and_then(|step| raw_mapping_field(step, "parameters")),
                &step_label,
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
                },
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
                },
            )?;
        }
    }

    Ok(())
}

fn resolve_param_refs(
    params: &mut Vec<Parameter>,
    components: &arazzo_spec::Components,
    raw_parameters: Option<&serde_yaml_ng::Value>,
    entity: &str,
) -> Result<(), String> {
    let mut resolved = Vec::with_capacity(params.len());
    for (parameter_index, mut param) in params.drain(..).enumerate() {
        if !param.reference.is_empty() {
            let Some(name) = param.reference.strip_prefix("$components.parameters.") else {
                return Err(format!(
                    "{entity}: unsupported parameter reference: {}",
                    param.reference
                ));
            };
            let Some(component_param) = components.parameters.get(name) else {
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
            if !raw_parameter_value_is_present(raw_parameters, parameter_index) {
                param.value = component_param.value.clone();
            }
            param.extensions = component_param.extensions.clone();
            param.reference.clear();
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
}

fn resolve_action_ref(
    actions: &mut [OnAction],
    context: ActionResolutionContext<'_>,
) -> Result<(), String> {
    for (action_idx, action) in actions.iter_mut().enumerate() {
        let raw_action = context
            .raw_actions
            .and_then(serde_yaml_ng::Value::as_sequence)
            .and_then(|actions| actions.get(action_idx));
        let mut raw_parameters =
            raw_action.and_then(|action| raw_mapping_field(action, "parameters"));
        if !action.reference.is_empty() {
            let Some(name) = action.reference.strip_prefix(context.prefix) else {
                return Err(format!(
                    "{}: unsupported {} reference: {}",
                    context.entity, context.kind, action.reference
                ));
            };
            let component_name = name.to_string();
            let Some(component) = context.component_map.get(&component_name) else {
                return Err(format!(
                    "{}: component {} \"{component_name}\" not found",
                    context.entity, context.kind
                ));
            };
            *action = component.clone();
            action.reference.clear();
            action.value = None;
            raw_parameters = raw_component_action(
                context.raw_components,
                context.component_field,
                &component_name,
            )
            .and_then(|component| raw_mapping_field(component, "parameters"));
        } else if !action.name.is_empty() {
            if let Some(component_name) = resolve_one_action_ref(
                action,
                context.component_map,
                context.prefix,
                context.kind,
                context.entity,
            )? {
                // `resolve_one_action_ref` preserves a non-empty local list,
                // matching the existing merge behavior. Otherwise the copied
                // component list needs its own raw presence source.
                let local_parameters_are_non_empty = raw_parameters
                    .and_then(serde_yaml_ng::Value::as_sequence)
                    .is_some_and(|parameters| !parameters.is_empty());
                if !local_parameters_are_non_empty {
                    raw_parameters = raw_component_action(
                        context.raw_components,
                        context.component_field,
                        &component_name,
                    )
                    .and_then(|component| raw_mapping_field(component, "parameters"));
                }
            }
        }
        // Action parameters may themselves be Reusable Objects pointing at
        // `$components.parameters.<name>`. Resolve them after the action-level
        // merge so parameters inherited from a component action resolve too.
        resolve_param_refs(
            &mut action.parameters,
            context.components,
            raw_parameters,
            &format!("{} {}[{action_idx}]", context.entity, context.kind),
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
        if action.retry_after != 0 {
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
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use arazzo_spec::{
        ActionType, CriterionExpressionType, CriterionType, Info, OnAction, OutputValue,
        ParamLocation, Replacement, RequestBody, SelectorObject, SelectorType, SourceDescription,
        SourceType, Step, StepAction, StepTarget, SuccessCriterion, Workflow,
    };

    use super::{
        parse, parse_bytes, parse_bytes_with_diagnostics, validate, validate_diagnostics,
        ArazzoSpec, Diagnostic, Error, Severity, ValidationErrorKind,
    };

    /// Unwrap a validation Error into its report errors, panicking on other variants.
    fn expect_validation_errors(result: Result<(), Error>) -> Vec<super::ValidationError> {
        match result {
            Ok(()) => panic!("expected validation error"),
            Err(Error::Validation(report)) => report.errors,
            Err(other) => panic!("expected Validation error, got: {other}"),
        }
    }

    fn expect_parse_error(data: &[u8]) -> Error {
        match parse_bytes(data) {
            Ok(_) => panic!("expected parse error"),
            Err(err) => err,
        }
    }

    fn expect_parsed(data: &[u8]) -> ArazzoSpec {
        match parse_bytes(data) {
            Ok(spec) => spec,
            Err(err) => panic!("expected parsed document, got: {err}"),
        }
    }

    const VALID_YAML: &str = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#;

    const ASYNCAPI_SOURCE_YAML: &str = r#"arazzo: "1.1.0"
info:
  title: AsyncAPI Source
  version: "1.0.0"
sourceDescriptions:
  - name: events
    url: https://example.com/asyncapi.yaml
    type: asyncapi
workflows:
  - workflowId: inspect-source
    steps:
      - stepId: inspect
        operationPath: /inspect
"#;

    fn valid_spec() -> ArazzoSpec {
        ArazzoSpec {
            arazzo: "1.0.0".to_string(),
            info: Info {
                title: "Test".to_string(),
                summary: String::new(),
                version: "1.0.0".to_string(),
                description: String::new(),
                ..Info::default()
            },
            source_descriptions: vec![SourceDescription {
                name: "api".to_string(),
                url: "https://example.com".to_string(),
                type_: SourceType::OpenApi,
                ..SourceDescription::default()
            }],
            workflows: vec![Workflow {
                workflow_id: "wf1".to_string(),
                steps: vec![Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/test".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            }],
            ..ArazzoSpec::default()
        }
    }

    #[test]
    fn validation_error_kind_names_are_frozen() {
        let cases = [
            (
                ValidationErrorKind::MissingRequiredField,
                "missingRequiredField",
            ),
            (
                ValidationErrorKind::DuplicateIdentifier,
                "duplicateIdentifier",
            ),
            (ValidationErrorKind::InvalidStepTarget, "invalidStepTarget"),
            (
                ValidationErrorKind::UnsupportedVersion,
                "unsupportedVersion",
            ),
            (
                ValidationErrorKind::InvalidParameterLocation,
                "invalidParameterLocation",
            ),
            (
                ValidationErrorKind::MissingParameterValue,
                "missingParameterValue",
            ),
            (ValidationErrorKind::InvalidExpression, "invalidExpression"),
            (ValidationErrorKind::InvalidReference, "invalidReference"),
            (ValidationErrorKind::InvalidAsyncStep, "invalidAsyncStep"),
            (
                ValidationErrorKind::UnsupportedDependencyScope,
                "unsupportedDependencyScope",
            ),
            (ValidationErrorKind::DependencyCycle, "dependencyCycle"),
            (ValidationErrorKind::InvalidRetryField, "invalidRetryField"),
            (
                ValidationErrorKind::InvalidCriterionType,
                "invalidCriterionType",
            ),
            (
                ValidationErrorKind::InvalidSelectorType,
                "invalidSelectorType",
            ),
            (
                ValidationErrorKind::UnsupportedOperationPath,
                "unsupportedOperationPath",
            ),
            (ValidationErrorKind::InvalidIdentifier, "invalidIdentifier"),
            (ValidationErrorKind::UnknownField, "unknownField"),
        ];

        for (kind, expected) in cases {
            assert_eq!(kind.name(), expected);
        }
    }

    fn temp_file_path(prefix: &str) -> PathBuf {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(v) => v.as_nanos(),
            Err(_) => 0,
        };
        std::env::temp_dir().join(format!("{prefix}-{nanos}.yaml"))
    }

    /// Structurally valid: its only findings are the two retry-field warnings.
    const WARNING_ONLY_YAML: &str = r#"arazzo: "1.0.0"
info:
  title: Warning Only
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    notify:
      name: notify
      type: end
  failureActions:
    notify:
      name: notify
      type: end
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: finish
            type: end
            retryAfter: 2
            retryLimit: 3
"#;

    /// Missing `info.title` (error) plus one retry-field warning.
    const MIXED_YAML: &str = r#"arazzo: "1.0.0"
info:
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: finish
            type: end
            retryLimit: 3
"#;

    #[test]
    fn warning_only_document_parses_ok_and_returns_diagnostics() {
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(WARNING_ONLY_YAML.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("warnings must not fail validation, got: {err}"),
        };
        assert_eq!(spec.info.title, "Warning Only");
        assert_eq!(diagnostics.len(), 2, "diagnostics={diagnostics:?}");
        assert!(diagnostics.iter().all(|d| d.severity == Severity::Warning));
        assert!(diagnostics
            .iter()
            .all(|d| d.kind == ValidationErrorKind::InvalidRetryField));
        assert_eq!(
            diagnostics[0].path,
            "workflow \"wf1\" > step \"s1\".onSuccess[0].retryLimit"
        );
        assert_eq!(
            diagnostics[0].message,
            "workflow \"wf1\" > step \"s1\".onSuccess[0].retryLimit has no effect on end action"
        );
        assert_eq!(
            diagnostics[1].path,
            "workflow \"wf1\" > step \"s1\".onSuccess[0].retryAfter"
        );

        // The signature-preserving entry point still succeeds on the same bytes.
        assert!(parse_bytes(WARNING_ONLY_YAML.as_bytes()).is_ok());
    }

    #[test]
    fn error_only_document_reports_no_warnings() {
        let spec = match arazzo_spec::parse_unvalidated_bytes(
            r#"arazzo: "1.0.0"
info:
  version: "1.0.0"
workflows: []
"#
            .as_bytes(),
        ) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(_) => panic!("expected validation error"),
            Err(Error::Validation(report)) => {
                assert!(!report.errors.is_empty());
                assert!(report.errors.iter().all(Diagnostic::is_error));
                assert!(report.warnings.is_empty(), "warnings={:?}", report.warnings);
            }
            Err(other) => panic!("expected Validation error, got: {other}"),
        }
    }

    #[test]
    fn mixed_document_fails_and_still_carries_warnings() {
        match parse_bytes_with_diagnostics(MIXED_YAML.as_bytes()) {
            Ok(_) => panic!("expected validation error"),
            Err(Error::Validation(report)) => {
                assert!(report
                    .errors
                    .iter()
                    .any(|item| item.message == "info.title is required"));
                assert_eq!(report.warnings.len(), 1, "warnings={:?}", report.warnings);
                assert_eq!(report.warnings[0].severity, Severity::Warning);
                assert_eq!(
                    report.warnings[0].message,
                    "workflow \"wf1\" > step \"s1\".onSuccess[0].retryLimit has no effect on end action"
                );
            }
            Err(other) => panic!("expected Validation error, got: {other}"),
        }
    }

    /// The public entry points keep the shapes every existing call site relies
    /// on: `parse(path)`, `parse_bytes(&[u8])`, and `validate(&spec)`.
    #[test]
    fn public_entry_points_keep_their_signatures() {
        // `parse` is generic over `impl AsRef<Path>`, so pin it through a
        // non-capturing closure that coerces to the fn pointer.
        let parse_fn: fn(&Path) -> Result<ArazzoSpec, Error> = |path| parse(path);
        let parse_bytes_fn: fn(&[u8]) -> Result<ArazzoSpec, Error> = parse_bytes;
        let validate_fn: fn(&ArazzoSpec) -> Result<(), Error> = validate;

        assert!(parse_fn(Path::new("/nonexistent/path.yaml")).is_err());
        let spec = match parse_bytes_fn(VALID_YAML.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert!(validate_fn(&spec).is_ok());
    }

    #[test]
    fn parse_file_not_found() {
        let result = parse("/nonexistent/path.yaml");
        match result {
            Ok(_) => panic!("expected error for nonexistent file"),
            Err(err) => {
                if !err.to_string().contains("reading arazzo file") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_valid_file() {
        let path = temp_file_path("arazzo-validate-parse-valid");
        if let Err(err) = std::fs::write(&path, VALID_YAML) {
            panic!("failed to write temp file: {err}");
        }

        let result = parse(&path);
        if let Err(err) = std::fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                panic!("failed to remove temp file {}: {err}", path.display());
            }
        }

        let spec = match result {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert_eq!(spec.info.title, "Test");
    }

    #[test]
    fn parse_bytes_accepts_arazzo_1_1_asyncapi_source_description() {
        let spec = match parse_bytes(ASYNCAPI_SOURCE_YAML.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected Arazzo 1.1 AsyncAPI source to validate: {err}"),
        };

        assert_eq!(spec.arazzo, "1.1.0");
        assert_eq!(spec.source_descriptions.len(), 1);
        assert_eq!(spec.source_descriptions[0].type_, SourceType::AsyncApi);
        assert_eq!(spec.workflows[0].workflow_id, "inspect-source");
    }

    #[test]
    fn parse_bytes_malformed_yaml() {
        let result = parse_bytes(b"[[[");
        match result {
            Ok(_) => panic!("expected parse error"),
            Err(err) => {
                if !err.to_string().contains("parsing arazzo yaml") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_validation_error() {
        let result = parse_bytes(b"foo: bar\n");
        match result {
            Ok(_) => panic!("expected validation error"),
            Err(Error::Validation(report)) => {
                assert!(report
                    .errors
                    .iter()
                    .any(|e| e.message.contains("arazzo version is required")));
            }
            Err(err) => panic!("expected Validation error, got: {err}"),
        }
    }

    #[test]
    fn parse_bytes_valid_spec() {
        let result = parse_bytes(VALID_YAML.as_bytes());
        let spec = match result {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert_eq!(spec.arazzo, "1.0.0");
        assert_eq!(spec.workflows.len(), 1);
        assert_eq!(spec.workflows[0].steps[0].step_id, "s1");
    }

    #[test]
    fn parse_bytes_component_parameters() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    authHeader:
      name: Authorization
      in: header
      value: "Bearer abc123"
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.authHeader"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let params = &spec.workflows[0].steps[0].parameters;
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "Authorization");
        assert_eq!(params[0].in_, Some(ParamLocation::Header));
        assert_eq!(params[0].value, "Bearer abc123".into());
        assert!(params[0].reference.is_empty());
    }

    #[test]
    fn parse_bytes_allows_vendor_extensions_without_validation_errors() {
        let spec_yaml = r#"
arazzo: "1.0.0"
x-arazzo-cli:
  root: true
info:
  title: Test
  version: "1.0.0"
  x-info:
    owner: qa
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
    x-source:
      auth: default
workflows:
  - workflowId: wf1
    x-workflow:
      env: test
    steps:
      - stepId: s1
        operationPath: /test
        x-step:
          mode: smoke
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        assert!(spec.extensions.contains_key("x-arazzo-cli"));
        assert!(spec.info.extensions.contains_key("x-info"));
        assert!(spec.source_descriptions[0]
            .extensions
            .contains_key("x-source"));
        assert!(spec.workflows[0].extensions.contains_key("x-workflow"));
        assert!(spec.workflows[0].steps[0].extensions.contains_key("x-step"));
    }

    fn reusable_parameter_document(value: Option<&str>, json: bool) -> String {
        if json {
            let value = value.map_or_else(String::new, |value| format!(r#", "value": {value}"#));
            return format!(
                r#"{{
  "arazzo": "1.1.0",
  "info": {{"title": "Reusable parameter value", "version": "1.0.0"}},
  "sourceDescriptions": [{{"name": "api", "url": "https://example.com", "type": "openapi"}}],
  "components": {{"parameters": {{"shared": {{"name": "shared", "in": "header", "value": "component default"}}}}}},
  "workflows": [{{
    "workflowId": "wf",
    "steps": [{{
      "stepId": "request",
      "operationPath": "/request",
      "parameters": [{{"reference": "$components.parameters.shared"{value}}}]
    }}]
  }}]
}}"#
            );
        }

        let value = value.map_or_else(String::new, |value| format!("\n            value: {value}"));
        format!(
            r#"arazzo: "1.1.0"
info: {{title: Reusable parameter value, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
components:
  parameters:
    shared: {{name: shared, in: header, value: "component default"}}
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationPath: /request
        parameters:
          - reference: "$components.parameters.shared"{value}
"#
        )
    }

    #[test]
    fn reusable_parameter_values_distinguish_omitted_and_empty_string_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            let inherited = expect_parsed(reusable_parameter_document(None, json).as_bytes());
            assert_eq!(
                inherited.workflows[0].steps[0].parameters[0].value,
                "component default".into(),
                "{format} omitted value must inherit"
            );

            let overridden =
                expect_parsed(reusable_parameter_document(Some("\"\""), json).as_bytes());
            assert_eq!(
                overridden.workflows[0].steps[0].parameters[0].value,
                serde_yaml_ng::Value::String(String::new()).into(),
                "{format} empty string must override"
            );
        }
    }

    #[test]
    fn reusable_parameter_values_reject_non_string_classes_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for value in ["null", "false", "0", "{}", "[]"] {
                let document = reusable_parameter_document(Some(value), json);
                let Err(Error::Validation(report)) =
                    parse_bytes_with_diagnostics(document.as_bytes())
                else {
                    panic!("{format} Reusable Object value {value} must fail validation");
                };
                assert_eq!(report.errors.len(), 1, "{format} value={value}: {report:?}");
                let error = &report.errors[0];
                assert_eq!(error.kind, ValidationErrorKind::InvalidReference);
                assert_eq!(
                    error.path,
                    "workflow \"wf\" > step \"request\".parameters[0].value"
                );
                assert_eq!(
                    error.message,
                    "workflow \"wf\" > step \"request\".parameters[0].value must be a string when used on a Reusable Object"
                );
            }
        }
    }

    #[test]
    fn component_parameter_definition_with_reference_still_requires_value() {
        let yaml = r#"arazzo: "1.1.0"
info: {title: Component parameter, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  parameters:
    shared: {name: shared, reference: "$components.parameters.other"}
workflows:
  - workflowId: wf
    steps: [{stepId: request, operationPath: /request}]
"#;
        let json = r#"{
  "arazzo": "1.1.0",
  "info": {"title": "Component parameter", "version": "1.0.0"},
  "sourceDescriptions": [{"name": "api", "url": "https://example.com", "type": "openapi"}],
  "components": {"parameters": {"shared": {"name": "shared", "reference": "$components.parameters.other"}}},
  "workflows": [{"workflowId": "wf", "steps": [{"stepId": "request", "operationPath": "/request"}]}]
}"#;

        for (format, document) in [("YAML", yaml), ("JSON", json)] {
            let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(document.as_bytes())
            else {
                panic!("{format} component Parameter Object must require value");
            };
            assert_eq!(report.errors.len(), 1, "{format}: {report:?}");
            assert_eq!(
                report.errors[0].kind,
                ValidationErrorKind::MissingRequiredField
            );
            assert_eq!(report.errors[0].path, "components.parameters.shared.value");
        }
    }

    fn required_value_fixture(values: [&str; 15]) -> String {
        format!(
            r#"arazzo: "1.1.0"
info: {{title: Required value presence, version: "1.0.0"}}
sourceDescriptions:
  - {{name: api, url: https://example.com, type: openapi}}
components:
  parameters:
    componentParam: {{name: componentParam, value: {}}}
  successActions:
    componentSuccess:
      name: componentSuccess
      type: goto
      workflowId: target
      parameters:
        - {{name: componentSuccessParam, value: {}}}
  failureActions:
    componentFailure:
      name: componentFailure
      type: goto
      workflowId: target
      parameters:
        - {{name: componentFailureParam, value: {}}}
workflows:
  - workflowId: wf
    parameters:
      - {{name: workflowParam, in: header, value: {}}}
    successActions:
      - name: workflowSuccess
        type: goto
        workflowId: target
        parameters:
          - {{name: workflowSuccessParam, value: {}}}
    failureActions:
      - name: workflowFailure
        type: goto
        workflowId: target
        parameters:
          - {{name: workflowFailureParam, value: {}}}
    steps:
      - stepId: request
        operationPath: /request
        parameters:
          - {{name: stepParam, in: query, value: {}}}
        onSuccess:
          - name: stepSuccess
            type: goto
            workflowId: target
            parameters:
              - {{name: stepSuccessParam, value: {}}}
        onFailure:
          - name: stepFailure
            type: goto
            workflowId: target
            parameters:
              - {{name: stepFailureParam, value: {}}}
        requestBody:
          contentType: application/json
          payload: {{}}
          replacements:
            - {{target: /a, value: {}}}
            - {{target: /b, value: {}}}
            - {{target: /c, value: {}}}
            - {{target: /d, value: {}}}
            - {{target: /e, value: {}}}
            - {{target: /f, value: {}}}
  - workflowId: target
    steps:
      - stepId: targetStep
        operationPath: /target
"#,
            values[0],
            values[1],
            values[2],
            values[3],
            values[4],
            values[5],
            values[6],
            values[7],
            values[8],
            values[9],
            values[10],
            values[11],
            values[12],
            values[13],
            values[14],
        )
    }

    fn missing_required_value_fixture() -> String {
        required_value_fixture([
            "null", "null", "null", "null", "null", "null", "null", "null", "null", "null", "null",
            "null", "null", "null", "null",
        ])
        .replace(", value: null", "")
    }

    fn required_value_json(values: [&str; 15]) -> String {
        format!(
            r#"{{
  "arazzo": "1.1.0",
  "info": {{"title": "Required value presence", "version": "1.0.0"}},
  "sourceDescriptions": [{{"name": "api", "url": "https://example.com", "type": "openapi"}}],
  "components": {{
    "parameters": {{"componentParam": {{"name": "componentParam", "value": {}}}}},
    "successActions": {{"componentSuccess": {{"name": "componentSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "componentSuccessParam", "value": {}}}]}}}},
    "failureActions": {{"componentFailure": {{"name": "componentFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "componentFailureParam", "value": {}}}]}}}}
  }},
  "workflows": [
    {{
      "workflowId": "wf",
      "parameters": [{{"name": "workflowParam", "in": "header", "value": {}}}],
      "successActions": [{{"name": "workflowSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "workflowSuccessParam", "value": {}}}]}}],
      "failureActions": [{{"name": "workflowFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "workflowFailureParam", "value": {}}}]}}],
      "steps": [{{
        "stepId": "request",
        "operationPath": "/request",
        "parameters": [{{"name": "stepParam", "in": "query", "value": {}}}],
        "onSuccess": [{{"name": "stepSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "stepSuccessParam", "value": {}}}]}}],
        "onFailure": [{{"name": "stepFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "stepFailureParam", "value": {}}}]}}],
        "requestBody": {{"contentType": "application/json", "payload": {{}}, "replacements": [
          {{"target": "/a", "value": {}}}, {{"target": "/b", "value": {}}}, {{"target": "/c", "value": {}}},
          {{"target": "/d", "value": {}}}, {{"target": "/e", "value": {}}}, {{"target": "/f", "value": {}}}
        ]}}
      }}]
    }},
    {{"workflowId": "target", "steps": [{{"stepId": "targetStep", "operationPath": "/target"}}]}}
  ]
}}"#,
            values[0],
            values[1],
            values[2],
            values[3],
            values[4],
            values[5],
            values[6],
            values[7],
            values[8],
            values[9],
            values[10],
            values[11],
            values[12],
            values[13],
            values[14],
        )
    }

    fn missing_required_value_json() -> String {
        required_value_json([
            "null", "null", "null", "null", "null", "null", "null", "null", "null", "null", "null",
            "null", "null", "null", "null",
        ])
        .replace(", \"value\": null", "")
    }

    #[test]
    pub(super) fn raw_required_values_report_exact_paths_for_yaml_and_json() {
        let yaml = missing_required_value_fixture();
        let json = missing_required_value_json();
        let expected = [
            "components.parameters.componentParam.value",
            "components.successActions.componentSuccess.parameters[0].value",
            "components.failureActions.componentFailure.parameters[0].value",
            "workflow \"wf\".parameters[0].value",
            "workflow \"wf\".successActions[0].parameters[0].value",
            "workflow \"wf\".failureActions[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".parameters[0].value",
            "workflow \"wf\" > step \"request\".onSuccess[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".onFailure[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[0].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[1].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[2].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[3].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[4].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[5].value",
        ];

        for (format, document) in [("YAML", yaml.as_bytes()), ("JSON", json.as_bytes())] {
            let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(document) else {
                panic!("{format} document with omitted values must fail validation");
            };
            let actual = report
                .errors
                .iter()
                .filter(|diagnostic| diagnostic.kind == ValidationErrorKind::MissingRequiredField)
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{format} errors={:?}", report.errors);
        }
    }

    #[test]
    pub(super) fn raw_required_values_accept_concrete_any_values_for_yaml_and_json() {
        let yaml = required_value_fixture([
            "null", "\"\"", "false", "0", "{}", "[]", "null", "\"\"", "false", "null", "\"\"",
            "false", "0", "{}", "[]",
        ]);
        let json = required_value_json([
            "null", "\"\"", "false", "0", "{}", "[]", "null", "\"\"", "false", "null", "\"\"",
            "false", "0", "{}", "[]",
        ]);
        for (format, document) in [("YAML", yaml.as_bytes()), ("JSON", json.as_bytes())] {
            let result = parse_bytes_with_diagnostics(document);
            assert!(
                result.is_ok(),
                "{format} explicit Any values must pass: {result:?}"
            );
        }
    }

    #[test]
    fn malformed_value_containers_do_not_add_raw_missing_value_diagnostics() {
        for malformed in [
            "parameters: {name: not-a-list}",
            "requestBody: {replacements: {target: /a}}",
        ] {
            let yaml = format!(
                r#"arazzo: "1.1.0"
info: {{title: malformed container, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationPath: /request
        {malformed}
"#
            );
            let result = parse_bytes_with_diagnostics(yaml.as_bytes());
            let Err(Error::ParseYaml(error)) = result else {
                panic!("malformed container must remain a serde parse error");
            };
            assert!(
                !error.to_string().contains("value is required"),
                "raw pass must not duplicate malformed-container diagnostics: {error}"
            );
        }
    }

    #[test]
    fn component_parameter_ref_preserves_component_extensions_when_resolved() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    authHeader:
      name: Authorization
      in: header
      value: "Bearer abc123"
      x-parameter:
        origin: component
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.authHeader"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let params = &spec.workflows[0].steps[0].parameters;
        assert!(params[0].extensions.contains_key("x-parameter"));
    }

    #[test]
    fn parse_bytes_component_parameter_not_found() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters: {}
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.missing"
"#;

        let result = parse_bytes(spec_yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err
                    .to_string()
                    .contains("component parameter \"missing\" not found")
                {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_component_success_actions() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    endWorkflow:
      type: end
      name: terminate
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: "$components.successActions.endWorkflow"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_success;
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action_type(), ActionType::End);
        assert_eq!(actions[0].name, "terminate");
    }

    #[test]
    fn component_action_ref_preserves_component_extensions_when_resolved() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    endWorkflow:
      type: end
      name: terminate
      x-action:
        origin: component
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: "$components.successActions.endWorkflow"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_success;
        assert!(actions[0].extensions.contains_key("x-action"));
    }

    #[test]
    fn parse_bytes_component_failure_actions() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    retryPolicy:
      type: retry
      retryAfter: 2
      retryLimit: 5
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_failure;
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].action_type(), ActionType::Retry);
        assert_eq!(actions[0].retry_after, 2);
        assert_eq!(actions[0].retry_limit, Some(5));
    }

    #[test]
    fn parse_bytes_component_failure_actions_multi() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    retryPolicy:
      type: retry
      retryAfter: 2
      retryLimit: 5
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
          - type: end
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_failure;
        assert_eq!(actions.len(), 2);
        // First action: resolved from component ref
        assert_eq!(actions[0].action_type(), ActionType::Retry);
        assert_eq!(actions[0].retry_after, 2);
        assert_eq!(actions[0].retry_limit, Some(5));
        // Second action: inline end
        assert_eq!(actions[1].action_type(), ActionType::End);
    }

    #[test]
    fn validate_valid_spec() {
        let spec = valid_spec();
        if let Err(err) = validate(&spec) {
            panic!("expected no error for valid spec, got: {err}");
        }
    }

    #[test]
    fn validate_missing_version() {
        let mut spec = valid_spec();
        spec.arazzo.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("arazzo version is required"));
    }

    #[test]
    fn validate_unsupported_version() {
        let mut spec = valid_spec();
        spec.arazzo = "2.0.0".to_string();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::UnsupportedVersion);
        assert!(errs[0].message.contains("unsupported arazzo version"));
    }

    #[test]
    fn validate_self_uri_accepts_absolute_and_relative_references() {
        for self_uri in [
            "https://api.example.com/workflows/purchase.arazzo.yaml",
            "workflows/purchase.arazzo.yaml",
            "../purchase.arazzo.yaml?revision=2",
        ] {
            let mut spec = valid_spec();
            spec.self_uri = Some(self_uri.to_string());
            if let Err(err) = validate(&spec) {
                panic!("expected valid $self URI-reference {self_uri:?}, got: {err}");
            }
        }
    }

    #[test]
    fn validate_self_uri_rejects_fragment_identifier() {
        let mut spec = valid_spec();
        spec.self_uri = Some("workflows/purchase.arazzo.yaml#purchase".to_string());
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert_eq!(errs[0].path, "$self");
        assert!(errs[0].message.contains("must not contain a fragment"));
    }

    #[test]
    fn validate_self_uri_rejects_invalid_uri_reference() {
        let mut spec = valid_spec();
        spec.self_uri = Some("https://example.com/%GG".to_string());
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert_eq!(errs[0].path, "$self");
        assert!(errs[0].message.contains("valid RFC 3986 URI-reference"));
    }

    #[test]
    fn validate_missing_title() {
        let mut spec = valid_spec();
        spec.info.title.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("info.title is required"));
    }

    #[test]
    fn validate_missing_info_version() {
        let mut spec = valid_spec();
        spec.info.version.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("info.version is required"));
    }

    fn required_lists_spec(source_descriptions: &str, workflows: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: Required lists
  version: "1.0.0"
{source_descriptions}{workflows}
"#
        )
    }

    fn assert_missing_required_path(yaml: &str, path: &str) {
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected {path} to fail validation");
        };
        let Some(error) = report.errors.iter().find(|error| error.path == path) else {
            panic!("expected a diagnostic at {path}, got {:?}", report.errors);
        };
        assert_eq!(error.kind, ValidationErrorKind::MissingRequiredField);
        assert_eq!(error.severity, Severity::Error);
    }

    #[test]
    fn required_source_descriptions_and_workflows_reject_empty_and_absent_lists() {
        let valid_workflows = "workflows:\n  - workflowId: wf1\n    steps: []\n";
        let valid_sources =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";

        for source_descriptions in ["sourceDescriptions: []\n", ""] {
            let yaml = required_lists_spec(source_descriptions, valid_workflows);
            assert_missing_required_path(&yaml, "sourceDescriptions");
        }
        for workflows in ["workflows: []\n", ""] {
            let yaml = required_lists_spec(valid_sources, workflows);
            assert_missing_required_path(&yaml, "workflows");
        }
    }

    #[test]
    fn required_source_descriptions_and_workflows_report_both_absent_lists() {
        let yaml = required_lists_spec("", "");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected both required lists to fail validation");
        };
        for path in ["sourceDescriptions", "workflows"] {
            assert!(
                report.errors.iter().any(|error| {
                    error.path == path
                        && error.kind == ValidationErrorKind::MissingRequiredField
                        && error.severity == Severity::Error
                }),
                "expected required-list error at {path}, got {:?}",
                report.errors
            );
        }
    }

    #[test]
    fn required_lists_null_values_remain_parse_errors() {
        let source_null = required_lists_spec(
            "sourceDescriptions: null\n",
            "workflows:\n  - workflowId: wf1\n    steps: []\n",
        );
        let workflow_null = required_lists_spec(
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n",
            "workflows: null\n",
        );
        let steps_null = required_lists_spec(
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n",
            "workflows:\n  - workflowId: wf1\n    steps: null\n",
        );

        for yaml in [source_null, workflow_null, steps_null] {
            assert!(
                matches!(expect_parse_error(yaml.as_bytes()), Error::ParseYaml(_)),
                "expected typed null to remain a parseYaml error"
            );
        }
    }

    #[test]
    fn validate_source_duplicate_name() {
        let mut spec = valid_spec();
        spec.source_descriptions.push(SourceDescription {
            name: "api".to_string(),
            url: "https://other.example.com".to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        });
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::DuplicateIdentifier);
        assert!(errs[0].message.contains("is duplicate"));
    }

    #[test]
    fn validate_source_missing_url() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].url.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains("sourceDescriptions[0].url is required"));
    }

    #[test]
    fn parse_bytes_source_invalid_type() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: invalid
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for invalid source type"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("parsing arazzo yaml") {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn validate_source_type_arazzo() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::Arazzo;
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error, got: {err}");
        }
    }

    #[test]
    fn validate_step_no_operation() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target = None;
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidStepTarget);
        assert!(errs[0]
            .message
            .contains("must have operationId, operationPath, channelPath, or workflowId"));
    }

    #[test]
    fn validate_async_channel_step_and_timeout_boundaries() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::AsyncApi;
        spec.workflows[0].steps[0].target = Some(StepTarget::ChannelPath(
            "{$sourceDescriptions.api.url}#/channels/events".to_string(),
        ));
        spec.workflows[0].steps[0].action = Some(StepAction::Receive);
        spec.workflows[0].steps[0].correlation_id = Some("$inputs.eventId".to_string());

        for timeout in [0, u64::MAX] {
            spec.workflows[0].steps[0].timeout = Some(timeout);
            if let Err(err) = validate(&spec) {
                panic!("expected unsigned timeout {timeout} to validate: {err}");
            }
        }
    }

    #[test]
    fn validate_async_operation_id_send_step() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::AsyncApi;
        let step = &mut spec.workflows[0].steps[0];
        step.target = Some(StepTarget::OperationId(
            "$sourceDescriptions.api.publish".to_string(),
        ));
        step.action = Some(StepAction::Send);

        if let Err(err) = validate(&spec) {
            panic!("expected operationId async send step to validate: {err}");
        }
    }

    #[test]
    fn validate_async_field_combinations_fail_with_specific_diagnostics() {
        let mut channel_without_action = valid_spec();
        channel_without_action.workflows[0].steps[0].target =
            Some(StepTarget::ChannelPath("/events".to_string()));

        let mut operation_path_with_action = valid_spec();
        operation_path_with_action.workflows[0].steps[0].action = Some(StepAction::Send);

        let mut workflow_with_action = valid_spec();
        workflow_with_action.workflows[0].steps[0].target =
            Some(StepTarget::WorkflowId("wf1".to_string()));
        workflow_with_action.workflows[0].steps[0].action = Some(StepAction::Receive);

        let mut workflow_with_correlation = valid_spec();
        workflow_with_correlation.workflows[0].steps[0].target =
            Some(StepTarget::WorkflowId("wf1".to_string()));
        workflow_with_correlation.workflows[0].steps[0].correlation_id =
            Some("order-1".to_string());

        let mut send_with_correlation = valid_spec();
        send_with_correlation.workflows[0].steps[0].target =
            Some(StepTarget::OperationId("publish".to_string()));
        send_with_correlation.workflows[0].steps[0].action = Some(StepAction::Send);
        send_with_correlation.workflows[0].steps[0].correlation_id = Some("order-1".to_string());

        for (name, spec, expected_path) in [
            ("channel without action", channel_without_action, ".action"),
            (
                "operationPath with action",
                operation_path_with_action,
                ".action",
            ),
            ("workflow with action", workflow_with_action, ".action"),
            (
                "workflow with correlationId",
                workflow_with_correlation,
                ".correlationId",
            ),
            (
                "send with correlationId",
                send_with_correlation,
                ".correlationId",
            ),
        ] {
            let errors = expect_validation_errors(validate(&spec));
            assert!(
                errors.iter().any(|error| {
                    error.kind == ValidationErrorKind::InvalidAsyncStep
                        && error.path.ends_with(expected_path)
                }),
                "{name} should produce a specific async validation diagnostic: {errors:?}"
            );
        }
    }

    #[test]
    fn validate_local_depends_on_accepts_existing_ids_and_rejects_missing_ids() {
        let mut spec = valid_spec();
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/second".to_string())),
            depends_on: vec!["s1".to_string()],
            ..Step::default()
        });
        if let Err(err) = validate(&spec) {
            panic!("existing local dependsOn ID should validate: {err}");
        }

        spec.workflows[0].steps[1].depends_on = vec!["missing".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with(".dependsOn[0]")
                && error.message.contains("unknown local step \"missing\"")
        }));
    }

    #[test]
    fn validate_workflow_depends_on_references_and_external_scope() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            depends_on: vec!["wf1".to_string()],
            steps: vec![Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/second".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        assert!(validate(&spec).is_ok());

        spec.workflows[1].depends_on = vec!["WF1".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with("dependsOn[0]")
        }));

        spec.workflows[1].depends_on = vec!["$sourceDescriptions.api.remote".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with("dependsOn[0]")
                && error.message.contains("expected type arazzo")
        }));

        spec.source_descriptions.push(SourceDescription {
            name: "shared".to_string(),
            url: "https://example.com/shared.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[1].depends_on = vec!["$sourceDescriptions.shared.remote".to_string()];
        let warnings = match validate_diagnostics(&spec) {
            Ok(warnings) => warnings,
            Err(err) => panic!("known Arazzo source should warn, not fail: {err}"),
        };
        assert!(warnings.iter().any(|warning| {
            warning.kind == ValidationErrorKind::UnsupportedDependencyScope
                && warning.severity == Severity::Warning
                && warning.path.ends_with("dependsOn[0]")
        }));
    }

    #[test]
    fn validate_workflow_depends_on_rejects_unknown_and_malformed_external_references() {
        let mut spec = valid_spec();
        spec.workflows[0].depends_on = vec![
            "$sourceDescriptions.missing.remote".to_string(),
            "$sourceDescriptions.bad name.ready".to_string(),
        ];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.message.contains("unknown sourceDescription")
        }));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.message.contains("invalid workflow reference")
        }));

        spec.workflows[0].depends_on = vec!["$sourceDescriptions.api.ready.extra".to_string()];
        assert!(
            validate(&spec).is_err(),
            "OpenAPI source must remain unsupported"
        );

        spec.source_descriptions.push(SourceDescription {
            name: "shared".to_string(),
            url: "https://example.com/shared.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[0].depends_on = vec!["$sourceDescriptions.shared.ready.extra".to_string()];
        let warnings = match validate_diagnostics(&spec) {
            Ok(warnings) => warnings,
            Err(error) => panic!("known Arazzo source is warning-only: {error}"),
        };
        let warning = match warnings
            .iter()
            .find(|warning| warning.kind == ValidationErrorKind::UnsupportedDependencyScope)
        {
            Some(warning) => warning,
            None => panic!("expected external dependency warning"),
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.clone().into_error().severity, Severity::Error);
    }

    #[test]
    fn validate_workflow_depends_on_cycles_are_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].depends_on = vec!["wf1".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::DependencyCycle && error.path.ends_with("dependsOn")
        }));

        let mut two_node = valid_spec();
        two_node.workflows[0].depends_on = vec!["wf2".to_string()];
        two_node.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            depends_on: vec!["wf1".to_string()],
            steps: vec![Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/second".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        let errors = expect_validation_errors(validate(&two_node));
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.kind == ValidationErrorKind::DependencyCycle)
                .count(),
            2
        );
    }

    #[test]
    fn validate_local_depends_on_cycles_are_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].depends_on = vec!["s2".to_string()];
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/second".to_string())),
            depends_on: vec!["s1".to_string()],
            ..Step::default()
        });

        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::DependencyCycle
                && error.message.contains("dependency cycle")
        }));
    }

    #[test]
    fn validate_cross_scope_depends_on_references_fail_as_unsupported() {
        for dependency in [
            "$workflows.audit.steps.record",
            "$sourceDescriptions.external.archive.steps.store",
        ] {
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].depends_on = vec![dependency.to_string()];
            let errors = expect_validation_errors(validate(&spec));
            assert!(errors.iter().any(|error| {
                error.kind == ValidationErrorKind::UnsupportedDependencyScope
                    && error.path.ends_with(".dependsOn[0]")
                    && error.message.contains("valid Arazzo 1.1 syntax")
            }));
        }
    }

    #[test]
    fn validate_replacement_empty_target_errors() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].request_body = Some(RequestBody {
            replacements: vec![Replacement {
                target: "  ".to_string(),
                value: serde_yaml_ng::Value::String("bar".to_string()).into(),
                ..Replacement::default()
            }],
            ..RequestBody::default()
        });

        let errs = expect_validation_errors(validate(&spec));
        let err = errs
            .iter()
            .find(|item| item.path.contains("requestBody.replacements[0].target"))
            .unwrap_or_else(|| panic!("expected replacement target error, got: {errs:?}"));

        assert_eq!(err.kind, ValidationErrorKind::MissingRequiredField);
    }

    #[test]
    fn validate_replacement_null_value_is_allowed() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].request_body = Some(RequestBody {
            replacements: vec![Replacement {
                target: "/foo".to_string(),
                value: serde_yaml_ng::Value::Null.into(),
                ..Replacement::default()
            }],
            ..RequestBody::default()
        });

        if let Err(err) = validate(&spec) {
            panic!("expected null replacement value to pass, got: {err}");
        }
    }

    #[test]
    fn validate_replacement_present_passes() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].request_body = Some(RequestBody {
            replacements: vec![Replacement {
                target: "/foo".to_string(),
                value: serde_yaml_ng::Value::String("bar".to_string()).into(),
                ..Replacement::default()
            }],
            ..RequestBody::default()
        });

        if let Err(err) = validate(&spec) {
            panic!("expected replacement to pass, got: {err}");
        }
    }

    #[test]
    fn parse_bytes_step_multiple_targets_rejected() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        workflowId: other-workflow
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for step with multiple targets"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains(
                    "exactly one of operationId, operationPath, channelPath, or workflowId",
                ) {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn validate_step_operation_path_unknown_source_reference() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target =
            Some(StepTarget::OperationPath("{missing}./items".to_string()));

        let errs = expect_validation_errors(validate(&spec));
        let err = errs
            .iter()
            .find(|item| item.path.contains("operationPath"))
            .unwrap_or_else(|| panic!("expected operationPath validation error, got: {errs:?}"));

        assert_eq!(err.kind, ValidationErrorKind::InvalidReference);
        assert!(err
            .message
            .contains("references unknown sourceDescription \"missing\""));
    }

    /// A leading `"<METHOD> "` token must not hide the source reference from
    /// this check. It did: the validator's own prefix parser tested
    /// `starts_with('{')` against the whole value, so the check was silently
    /// skipped for every `"<METHOD> {source}./path"` step shipped in
    /// `examples/`, while the runtime resolved those same paths fine.
    #[test]
    fn validate_step_operation_path_unknown_source_reference_behind_method_prefix() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target =
            Some(StepTarget::OperationPath("POST {missing}./pet".to_string()));

        let errs = expect_validation_errors(validate(&spec));
        let err = errs
            .iter()
            .find(|item| item.path.contains("operationPath"))
            .unwrap_or_else(|| panic!("expected operationPath validation error, got: {errs:?}"));

        assert_eq!(err.kind, ValidationErrorKind::InvalidReference);
        assert!(err
            .message
            .contains("references unknown sourceDescription \"missing\""));
    }

    #[test]
    fn validate_step_operation_path_known_source_reference() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target =
            Some(StepTarget::OperationPath("{api}./items".to_string()));

        if let Err(err) = validate(&spec) {
            panic!("expected sourceDescription reference to validate, got: {err}");
        }
    }

    fn operation_path_warnings(operation_path: &str) -> Vec<Diagnostic> {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target =
            Some(StepTarget::OperationPath(operation_path.to_string()));

        match validate_diagnostics(&spec) {
            Ok(warnings) => warnings
                .into_iter()
                .filter(|item| item.kind == ValidationErrorKind::UnsupportedOperationPath)
                .collect(),
            Err(err) => panic!("expected {operation_path:?} to validate, got: {err}"),
        }
    }

    /// The conformant specification form is valid Arazzo that this executor
    /// cannot run — a warning, so `validate` neither blames the document nor
    /// calls it clean.
    #[test]
    fn validate_warns_on_unsupported_operation_path_without_failing() {
        let operation_path = "{$sourceDescriptions.api.url}#/paths/~1status/get";
        let warnings = operation_path_warnings(operation_path);

        assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
        let warning = &warnings[0];
        assert_eq!(warning.severity, Severity::Warning);
        assert!(!warning.is_error());
        assert!(
            warning.path.ends_with(".operationPath"),
            "path was: {}",
            warning.path
        );
        assert!(
            warning.message.contains(operation_path),
            "message was: {}",
            warning.message
        );
        assert!(
            warning
                .message
                .contains("a runtime expression and a JSON Pointer fragment"),
            "message was: {}",
            warning.message
        );
    }

    #[test]
    fn validate_warns_on_each_unsupported_operation_path_shape() {
        for operation_path in [
            "{$sourceDescriptions.api.url}#/paths/~1status/get",
            "GET {$sourceDescriptions.api.url}#/paths/~1status/get",
            "{$sourceDescriptions.api.url}",
            "#/paths/~1status/get",
        ] {
            assert_eq!(
                operation_path_warnings(operation_path).len(),
                1,
                "expected one warning for {operation_path:?}"
            );
        }
    }

    #[test]
    fn validate_does_not_warn_on_supported_operation_path_forms() {
        for operation_path in [
            "/items",
            "GET /items",
            "/pets/{petId}",
            "{api}./items",
            "POST {api}./items",
            "https://api.example.com/items",
            "DELETE https://api.example.com/items/7",
        ] {
            assert!(
                operation_path_warnings(operation_path).is_empty(),
                "unexpected warning for supported form {operation_path:?}"
            );
        }
    }

    #[test]
    fn parse_bytes_param_invalid_in() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - name: q
            in: body
            value: x
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for invalid param location"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("parsing arazzo yaml") {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn validate_output_unknown_step() {
        let mut spec = valid_spec();
        spec.workflows[0].outputs = BTreeMap::from([(
            "result".to_string(),
            "$steps.nonexistent.outputs.value".to_string().into(),
        )]);
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert!(errs[0]
            .message
            .contains("references unknown step 'nonexistent'"));
    }

    /// Decision 1 (ac-bd441): the accepted `type`/`version` pairs from the
    /// Arazzo v1.1.0 §5.8.12.1 table, shared by every validation site.
    const SCHEMA_TYPE_VERSIONS: [(&str, &str); 7] = [
        ("jsonpath", "rfc9535"),
        ("jsonpath", "draft-goessner-dispatch-jsonpath-00"),
        ("xpath", "xpath-10"),
        ("xpath", "xpath-20"),
        ("xpath", "xpath-30"),
        ("xpath", "xpath-31"),
        ("jsonpointer", "rfc6901"),
    ];

    #[test]
    fn validate_selector_accepts_schema_type_version_combinations() {
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            let mut spec = valid_spec();
            spec.workflows[0].outputs = BTreeMap::from([(
                "selected".to_string(),
                OutputValue::Selector(SelectorObject {
                    context: "$inputs.document".to_string(),
                    selector: if type_name == "jsonpointer" {
                        "/items".to_string()
                    } else {
                        "$.items".to_string()
                    },
                    type_: SelectorType::ExpressionType(CriterionExpressionType {
                        type_: type_name.to_string(),
                        version: version.to_string(),
                        ..CriterionExpressionType::default()
                    }),
                    extensions: BTreeMap::new(),
                }),
            )]);

            if let Err(error) = validate(&spec) {
                panic!("expected {type_name} {version} selector to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_selector_rejects_unsupported_type_version_combination() {
        let mut spec = valid_spec();
        spec.workflows[0].outputs = BTreeMap::from([(
            "selected".to_string(),
            OutputValue::Selector(SelectorObject {
                context: "$inputs.document".to_string(),
                selector: "$.items".to_string(),
                type_: SelectorType::ExpressionType(CriterionExpressionType {
                    type_: "jsonpath".to_string(),
                    version: "xpath-10".to_string(),
                    ..CriterionExpressionType::default()
                }),
                extensions: BTreeMap::new(),
            }),
        )]);

        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
        assert!(errors[0].message.contains("not supported for jsonpath"));
    }

    #[test]
    fn validate_criterion_type_requires_context() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains("context is required when type is specified"));
    }

    #[test]
    fn validate_criterion_type_object_is_accepted() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            context: "$response.body".to_string(),
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
                ..CriterionExpressionType::default()
            })),
            ..SuccessCriterion::default()
        }];

        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error, got: {err}");
        }
    }

    #[test]
    fn validate_criterion_type_object_rejects_invalid_version() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            context: "$response.body".to_string(),
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "invalid-version".to_string(),
                ..CriterionExpressionType::default()
            })),
            ..SuccessCriterion::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidCriterionType);
        assert!(errs[0]
            .message
            .contains("type.version \"invalid-version\" is not supported for jsonpath"));
    }

    #[test]
    fn validate_criterion_accepts_schema_type_version_combinations() {
        // Decision 1: the criterion site shares the §5.8.12.1 table —
        // notably `rfc9535` and `xpath-31`, which it used to reject.
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            if type_name == "jsonpointer" {
                // Criterion object types are jsonpath | xpath only.
                continue;
            }
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
                context: "$response.body".to_string(),
                condition: if type_name == "xpath" {
                    "//pets".to_string()
                } else {
                    "$.pets[0]".to_string()
                },
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: type_name.to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                })),
                ..SuccessCriterion::default()
            }];

            if let Err(error) = validate(&spec) {
                panic!("expected criterion {type_name} {version} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_selector_rejects_bare_number_xpath_versions() {
        // Decision 1 negative: the pre-ticket bare-number tokens are not in
        // the §5.8.12.1 table and no longer validate.
        for version in ["10", "30"] {
            let mut spec = valid_spec();
            spec.workflows[0].outputs = BTreeMap::from([(
                "selected".to_string(),
                OutputValue::Selector(SelectorObject {
                    context: "$inputs.document".to_string(),
                    selector: "//items".to_string(),
                    type_: SelectorType::ExpressionType(CriterionExpressionType {
                        type_: "xpath".to_string(),
                        version: version.to_string(),
                        ..CriterionExpressionType::default()
                    }),
                    extensions: BTreeMap::new(),
                }),
            )]);

            let errors = expect_validation_errors(validate(&spec));
            assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
            assert!(
                errors[0].message.contains("not supported for xpath"),
                "{version}: {}",
                errors[0].message
            );
        }
    }

    fn replacement_with_type(type_: SelectorType, target: &str) -> Replacement {
        Replacement {
            target: target.to_string(),
            target_selector_type: Some(type_),
            value: serde_yaml_ng::Value::String("v".to_string()).into(),
            ..Replacement::default()
        }
    }

    fn spec_with_replacement(replacement: Replacement) -> ArazzoSpec {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].request_body = Some(RequestBody {
            replacements: vec![replacement],
            ..RequestBody::default()
        });
        spec
    }

    #[test]
    fn validate_target_selector_type_accepts_schema_type_version_combinations() {
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::ExpressionType(CriterionExpressionType {
                    type_: type_name.to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                }),
                "/foo",
            ));

            if let Err(error) = validate(&spec) {
                panic!("expected targetSelectorType {type_name} {version} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_target_selector_type_accepts_plain_names() {
        // Decision 3: plain `xpath` (meaning XPath 3.1) is conformant Arazzo
        // and must validate even though the runtime engine is XPath 1.0.
        for name in ["jsonpath", "xpath", "jsonpointer"] {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::Name(name.to_string()),
                "/foo",
            ));

            if let Err(error) = validate(&spec) {
                panic!("expected targetSelectorType {name} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_target_selector_type_rejects_unknown_name() {
        let spec = spec_with_replacement(replacement_with_type(
            SelectorType::Name("regex".to_string()),
            "/foo",
        ));

        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
        assert!(
            errors[0].path.contains("targetSelectorType"),
            "{}",
            errors[0].path
        );
        assert!(errors[0]
            .message
            .contains("must be one of jsonpath, xpath, or jsonpointer"));
    }

    #[test]
    fn validate_target_selector_type_rejects_bare_number_and_unknown_versions() {
        for version in ["10", "30", "rfc9536"] {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                }),
                "//foo",
            ));

            let errors = expect_validation_errors(validate(&spec));
            assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
            assert!(
                errors[0].message.contains("not supported for xpath"),
                "{version}: {}",
                errors[0].message
            );
        }
    }

    #[test]
    fn validate_action_criteria_follow_criterion_rules() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            criteria: vec![SuccessCriterion {
                condition: "//item[1]".to_string(),
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: "xpath-10".to_string(),
                    ..CriterionExpressionType::default()
                })),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains(".onFailure[0].criteria[0].context is required"));
    }

    #[test]
    fn validate_multiple_errors() {
        let spec = ArazzoSpec::default();
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs.len() >= 2);
        let messages: Vec<&str> = errs.iter().map(|e| e.message.as_str()).collect();
        assert!(messages
            .iter()
            .any(|m| m.contains("arazzo version is required")));
        assert!(messages
            .iter()
            .any(|m| m.contains("info.title is required")));
    }

    #[test]
    fn parse_bytes_workflow_level_param_invalid_in() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:
      - name: q
        in: body
        value: x
    steps:
      - stepId: s1
        operationPath: /test
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for invalid workflow param location"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("parsing arazzo yaml") {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_workflow_level_fields() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  summary: A summary
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:
      - name: Authorization
        in: header
        value: "Bearer token"
    successActions:
      - type: end
    failureActions:
      - type: retry
        retryAfter: 5
        retryLimit: 3
    steps:
      - stepId: s1
        description: First step
        operationPath: /test
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert_eq!(spec.info.summary, "A summary");
        let wf = &spec.workflows[0];
        assert_eq!(wf.parameters.len(), 1);
        assert_eq!(wf.parameters[0].name, "Authorization");
        assert_eq!(wf.success_actions.len(), 1);
        assert_eq!(wf.success_actions[0].action_type(), ActionType::End);
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].action_type(), ActionType::Retry);
        assert_eq!(wf.failure_actions[0].retry_after, 5);
        assert_eq!(wf.steps[0].description, "First step");
    }

    /// Workflow Object Fixed Fields (spec/arazzo/v1.1.0.html): `dependsOn` is
    /// `[string]` on the Workflow Object itself, not only on Step. Before
    /// this, a conformant `dependsOn` on a workflow warned as an unrecognized
    /// field and failed `--strict`.
    #[test]
    fn workflow_depends_on_parses_round_trips_and_produces_zero_unknown_field_warnings() {
        let spec_yaml = r#"arazzo: "1.1.0"
info:
  title: Workflow dependsOn
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: first
    steps:
      - stepId: s1
        operationPath: /test
  - workflowId: second
    dependsOn:
      - first
    steps:
      - stepId: s2
        operationPath: /test
"#;

        let (spec, diagnostics) = match parse_bytes_with_diagnostics(spec_yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a clean document, got: {err}"),
        };
        assert_eq!(diagnostics, Vec::new(), "diagnostics={diagnostics:?}");
        assert_eq!(
            spec.workflows[1].depends_on,
            vec!["first".to_string()],
            "workflows={:?}",
            spec.workflows
        );

        let serialized =
            serde_yaml_ng::to_string(&spec).unwrap_or_else(|err| panic!("serialize: {err}"));
        assert!(serialized.contains("dependsOn"), "{serialized}");
        let reparsed = match arazzo_spec::parse_unvalidated_bytes(serialized.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("reparsing serialized spec: {err}"),
        };
        assert_eq!(
            reparsed.workflows[1].depends_on,
            spec.workflows[1].depends_on
        );
    }

    /// Reusable Object Fixed Fields (spec/arazzo/v1.1.0.html §5.8.10.1):
    /// `reference` (required) and `value` (optional). Every position
    /// `validate_actions` covers types as `[... Action Object | Reusable
    /// Object]` (Workflow `successActions`/`failureActions`, Step
    /// `onSuccess`/`onFailure`), so a Reusable Object in `reference` form
    /// must not warn on `reference`/`value` — but a field that is neither
    /// modeled nor `reference`/`value` nor `x-*` must still warn, proving the
    /// allowlist is narrowly scoped to those two names.
    #[test]
    fn reusable_object_reference_and_string_value_do_not_warn_at_any_action_position() {
        let spec_yaml = r#"arazzo: "1.1.0"
info:
  title: Reusable Object Fields
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    notify:
      name: notify
      type: end
  failureActions:
    notify:
      name: notify
      type: end
workflows:
  - workflowId: wf1
    successActions:
      - reference: "$components.successActions.notify"
        value: ""
    failureActions:
      - reference: "$components.failureActions.notify"
        value: ""
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - reference: "$components.successActions.notify"
            value: ""
          - name: inline
            type: end
            actionTypo: warn
        onFailure:
          - reference: "$components.failureActions.notify"
            value: ""
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(spec_yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a warning-only document, got: {err}"),
        };

        let unknown_field: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.kind == ValidationErrorKind::UnknownField)
            .collect();
        // Exactly the one inline `actionTypo` field — never `reference` or
        // `value`, at any of the four positions.
        assert_eq!(unknown_field.len(), 1, "diagnostics={unknown_field:?}");
        assert!(
            unknown_field
                .iter()
                .all(|d| d.message.contains("\"actionTypo\"")),
            "diagnostics={unknown_field:?}"
        );
        assert!(unknown_field
            .iter()
            .any(|d| d.path == "workflow \"wf1\" > step \"s1\".onSuccess[1]"));
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("\"reference\"") && !d.message.contains("\"value\"")),
            "reference/value must never be reported as unrecognized; diagnostics={diagnostics:?}"
        );
    }

    fn reusable_action_document(value: &str, json: bool) -> String {
        if json {
            return format!(
                r#"{{
  "arazzo": "1.1.0",
  "info": {{"title": "Reusable action value", "version": "1.0.0"}},
  "sourceDescriptions": [{{"name": "api", "url": "https://example.com", "type": "openapi"}}],
  "components": {{"successActions": {{"notify": {{"name": "notify", "type": "end"}}}}}},
  "workflows": [{{
    "workflowId": "wf",
    "steps": [{{
      "stepId": "request",
      "operationPath": "/request",
      "onSuccess": [{{"reference": "$components.successActions.notify", "value": {value}}}]
    }}]
  }}]
}}"#
            );
        }

        format!(
            r#"arazzo: "1.1.0"
info: {{title: Reusable action value, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
components:
  successActions:
    notify: {{name: notify, type: end}}
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationPath: /request
        onSuccess:
          - reference: "$components.successActions.notify"
            value: {value}
"#
        )
    }

    #[test]
    fn reusable_action_values_reject_non_string_classes_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for value in ["null", "false", "0", "{}", "[]"] {
                let document = reusable_action_document(value, json);
                let Err(Error::Validation(report)) =
                    parse_bytes_with_diagnostics(document.as_bytes())
                else {
                    panic!("{format} Reusable Object action value {value} must fail validation");
                };
                assert_eq!(report.errors.len(), 1, "{format} value={value}: {report:?}");
                let error = &report.errors[0];
                assert_eq!(error.kind, ValidationErrorKind::InvalidReference);
                assert_eq!(
                    error.path,
                    "workflow \"wf\" > step \"request\".onSuccess[0].value"
                );
                assert_eq!(
                    error.message,
                    "workflow \"wf\" > step \"request\".onSuccess[0].value must be a string when used on a Reusable Object"
                );
            }
        }
    }

    #[test]
    fn parse_bytes_component_workflow_level_actions() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    endAll:
      type: end
      name: stop
  failureActions:
    retryAll:
      type: retry
      retryAfter: 1
      retryLimit: 2
workflows:
  - workflowId: wf1
    successActions:
      - name: "$components.successActions.endAll"
    failureActions:
      - name: "$components.failureActions.retryAll"
    steps:
      - stepId: s1
        operationPath: /test
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let wf = &spec.workflows[0];
        assert_eq!(wf.success_actions.len(), 1);
        assert_eq!(wf.success_actions[0].action_type(), ActionType::End);
        assert_eq!(wf.success_actions[0].name, "stop");
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].action_type(), ActionType::Retry);
        assert_eq!(wf.failure_actions[0].retry_after, 1);
    }

    #[test]
    fn parse_bytes_reusable_action_reference_resolves_at_all_four_positions() {
        let spec_yaml = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions:
  - {name: api, url: https://example.com, type: openapi}
components:
  successActions:
    goS3: {name: goS3, type: goto, stepId: s3}
  failureActions:
    retry: {name: retry, type: retry, retryAfter: 1, retryLimit: 2}
workflows:
  - workflowId: wf1
    successActions:
      - reference: $components.successActions.goS3
    failureActions:
      - reference: $components.failureActions.retry
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess:
          - reference: $components.successActions.goS3
        onFailure:
          - reference: $components.failureActions.retry
      - stepId: s2
        operationPath: /s2
      - stepId: s3
        operationPath: /s3
"#;
        let spec = expect_parsed(spec_yaml.as_bytes());
        let workflow = &spec.workflows[0];
        for action in [
            &workflow.success_actions[0],
            &workflow.steps[0].on_success[0],
        ] {
            assert_eq!(action.name, "goS3");
            assert_eq!(action.action_type(), ActionType::Goto);
            assert_eq!(action.step_id, "s3");
            assert!(action.reference.is_empty());
            assert!(action.value.is_none());
        }
        for action in [
            &workflow.failure_actions[0],
            &workflow.steps[0].on_failure[0],
        ] {
            assert_eq!(action.name, "retry");
            assert_eq!(action.action_type(), ActionType::Retry);
            assert_eq!(action.retry_after, 1);
            assert_eq!(action.retry_limit, Some(2));
            assert!(action.reference.is_empty());
            assert!(action.value.is_none());
        }
    }

    #[test]
    fn reusable_action_reference_errors_include_missing_components_and_namespaces() {
        let missing = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{reference: $components.successActions.missing}]
"#;
        let err = expect_parse_error(missing.as_bytes());
        assert!(format!("{err}").contains("component successAction \"missing\" not found"));

        let missing_failure = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onFailure: [{reference: $components.failureActions.missing}]
"#;
        let err = expect_parse_error(missing_failure.as_bytes());
        assert!(format!("{err}").contains("component failureAction \"missing\" not found"));

        let missing_with_components = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions: {present: {name: present, type: end}}
  failureActions: {present: {name: present, type: end}}
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{reference: $components.successActions.missing}]
"#;
        let err = expect_parse_error(missing_with_components.as_bytes());
        assert!(format!("{err}").contains("component successAction \"missing\" not found"));

        let missing_failure_with_components = missing_with_components.replace(
            "onSuccess: [{reference: $components.successActions.missing}]",
            "onFailure: [{reference: $components.failureActions.missing}]",
        );
        let err = expect_parse_error(missing_failure_with_components.as_bytes());
        assert!(format!("{err}").contains("component failureAction \"missing\" not found"));

        let wrong_namespace = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components: {successActions: {ok: {name: ok, type: end}}}
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{reference: $components.failureActions.ok}]
"#;
        let err = expect_parse_error(wrong_namespace.as_bytes());
        assert!(format!("{err}")
            .contains("unsupported successAction reference: $components.failureActions.ok"));

        let non_expression = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components: {successActions: {ok: {name: ok, type: end}}}
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{reference: not-a-components-reference}]
"#;
        let err = expect_parse_error(non_expression.as_bytes());
        assert!(format!("{err}")
            .contains("unsupported successAction reference: not-a-components-reference"));
    }

    #[test]
    fn absent_components_do_not_silently_ignore_action_or_parameter_references() {
        let action = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{name: $components.successActions.missing, type: end}]
"#;
        let err = expect_parse_error(action.as_bytes());
        assert!(format!("{err}").contains("component successAction \"missing\" not found"));

        let parameter = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    parameters: [{reference: $components.parameters.missing}]
    steps: [{stepId: s1, operationPath: /s1}]
"#;
        let err = expect_parse_error(parameter.as_bytes());
        assert!(format!("{err}").contains("component parameter \"missing\" not found"));
    }

    #[test]
    fn reusable_action_reference_takes_precedence_over_name_and_ignores_string_value() {
        let yaml = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    a: {name: a, type: end}
    b: {name: b, type: goto, stepId: s2}
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess:
          - name: $components.successActions.a
            reference: $components.successActions.b
            value: ""
      - stepId: s2
        operationPath: /s2
"#;
        let spec = expect_parsed(yaml.as_bytes());
        let action = &spec.workflows[0].steps[0].on_success[0];
        assert_eq!(action.name, "b");
        assert_eq!(action.action_type(), ActionType::Goto);
        assert_eq!(action.step_id, "s2");
        assert!(action.reference.is_empty());
        assert!(action.value.is_none());

        let reference_only =
            expect_parsed(yaml.replace("            value: \"\"\n", "").as_bytes());
        assert_eq!(
            action, &reference_only.workflows[0].steps[0].on_success[0],
            "action value must not affect resolved action semantics"
        );
    }

    #[test]
    fn inline_and_component_action_value_fields_are_warned_at_their_object_positions() {
        let yaml = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    emptyReference: {name: emptyReference, type: end, reference: ''}
    nullReference: {name: nullReference, type: end, reference: null}
    nullValue: {name: nullValue, type: end, value: null}
  failureActions:
    emptyReferenceAndNullValue: {name: emptyReferenceAndNullValue, type: end, reference: '', value: null}
    nullReference: {name: nullReference, type: end, reference: null}
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{name: inline, type: end, value: null}]
"#;
        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("value warnings must not fail normal validation: {err}"),
        };
        let value_warnings: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("\"value\""))
            .collect();
        assert_eq!(value_warnings.len(), 3, "diagnostics={diagnostics:?}");
        assert_eq!(
            value_warnings
                .iter()
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "components.successActions.nullValue",
                "components.failureActions.emptyReferenceAndNullValue",
                "workflow \"wf\" > step \"s1\".onSuccess[0]",
            ]
        );
        let reference_warnings: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("\"reference\""))
            .collect();
        assert_eq!(reference_warnings.len(), 4, "diagnostics={diagnostics:?}");
        assert_eq!(
            reference_warnings
                .iter()
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "components.successActions.emptyReference",
                "components.successActions.nullReference",
                "components.failureActions.emptyReferenceAndNullValue",
                "components.failureActions.nullReference",
            ]
        );
        assert!(diagnostics
            .iter()
            .all(|diagnostic| { diagnostic.kind == ValidationErrorKind::UnknownField }));
    }

    #[test]
    fn every_component_action_reference_shape_warns_at_its_object_position() {
        let shapes = [
            ("nullReference", "null"),
            ("emptyReference", "''"),
            ("stringReference", "$components.successActions.target"),
            ("numberReference", "42"),
            ("sequenceReference", "[bad, shape]"),
            ("mappingReference", "{bad: shape}"),
        ];
        for (name, shape) in shapes {
            for section in ["successActions", "failureActions"] {
                let yaml = format!(
                    r#"arazzo: "1.1.0"
info: {{title: Test, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
components:
  {section}:
    {name}: {{name: action, type: end, reference: {shape}}}
workflows:
  - workflowId: wf
    steps: [{{stepId: s1, operationPath: /s1}}]
"#
                );
                let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
                    Ok(value) => value,
                    Err(err) => {
                        panic!("component {section} reference shape {name} must warn: {err}")
                    }
                };
                let warnings: Vec<_> = diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.message.contains("\"reference\""))
                    .collect();
                assert_eq!(
                    warnings.len(),
                    1,
                    "shape={name}, section={section}, yaml={yaml}, diagnostics={diagnostics:?}"
                );
                assert_eq!(warnings[0].path, format!("components.{section}.{name}"));
                assert_eq!(warnings[0].severity, Severity::Warning);
            }
        }
    }

    #[test]
    fn structured_action_references_fail_before_execution() {
        for shape in ["null", "42", "[bad]", "{bad: shape}"] {
            let yaml = format!(
                r#"arazzo: "1.1.0"
info: {{title: Test, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess: [{{reference: {shape}}}]
"#
            );
            let err = expect_parse_error(yaml.as_bytes());
            assert!(
                format!("{err}").contains("reference must be a runtime expression string"),
                "shape={shape}, error={err}"
            );
        }
    }

    #[test]
    fn former_action_presence_markers_are_ordinary_unknown_fields() {
        let yaml = r#"
arazzo: "1.1.0"
info: {title: Test, version: "1.0.0"}
__arazzo_cli_internal_reference_present: true
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationPath: /s1
        onSuccess:
          - name: finish
            type: end
            __arazzo_cli_internal_value_present: true
          - name: clean
            type: end
"#;
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("marker spellings are warnings, not parse errors: {err}"),
        };
        let unknown: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == ValidationErrorKind::UnknownField)
            .collect();
        assert!(unknown.iter().any(|diagnostic| {
            diagnostic.path.is_empty()
                && diagnostic
                    .message
                    .contains("__arazzo_cli_internal_reference_present")
        }));
        assert!(unknown.iter().any(|diagnostic| {
            diagnostic.path == "workflow \"wf\" > step \"s1\".onSuccess[0]"
                && diagnostic
                    .message
                    .contains("__arazzo_cli_internal_value_present")
        }));
        assert!(spec
            .extensions
            .contains_key("__arazzo_cli_internal_reference_present"));
        assert!(spec.workflows[0].steps[0].on_success[0]
            .extensions
            .contains_key("__arazzo_cli_internal_value_present"));
        assert!(spec.workflows[0].steps[0].on_success[1]
            .extensions
            .is_empty());
    }

    #[test]
    fn validate_goto_valid_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/other".to_string())),
            ..Step::default()
        });
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "s2".to_string(),
            ..OnAction::default()
        }];
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error for valid goto, got: {err}");
        }
    }

    #[test]
    fn validate_goto_invalid_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "nonexistent".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown step \"nonexistent\"")));
    }

    #[test]
    fn validate_goto_valid_workflow_id() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test2".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "wf2".to_string(),
            ..OnAction::default()
        }];
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error for valid goto workflow, got: {err}");
        }
    }

    #[test]
    fn validate_goto_invalid_workflow_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "missing_wf".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown workflow \"missing_wf\"")));
    }

    #[test]
    fn validate_goto_source_description_workflow_ref() {
        let mut spec = valid_spec();
        // Add an Arazzo-type source description for cross-source workflow references.
        spec.source_descriptions.push(SourceDescription {
            name: "external".to_string(),
            url: "https://example.com/other.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "$sourceDescriptions.external.someWorkflow".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression workflowId should not be rejected"
        );
    }

    #[test]
    fn validate_goto_runtime_expression_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "$steps.decide.outputs.nextStep".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression stepId should not be rejected"
        );
    }

    #[test]
    fn validate_goto_missing_step_and_workflow() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::MissingRequiredField
                && e.message
                    .contains("goto action must specify stepId or workflowId")));
    }

    #[test]
    fn validate_retry_both_step_and_workflow_rejected() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test2".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "s1".to_string(),
            workflow_id: "wf2".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message
                    .contains("retry action specifies both stepId and workflowId")));
    }

    #[test]
    fn validate_retry_unknown_step_id_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "nonexistent".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown step \"nonexistent\"")));
    }

    #[test]
    fn validate_retry_unknown_workflow_id_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            workflow_id: "missing_wf".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown workflow \"missing_wf\"")));
    }

    #[test]
    fn validate_retry_runtime_expression_reference_accepted() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "$steps.decide.outputs.recoveryStep".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression retry stepId should not be rejected"
        );
    }

    /// Unlike goto, the reference is optional for retry: "If a stepId or
    /// workflowId are specified, then the reference is executed ..."
    #[test]
    fn validate_retry_without_reference_stays_valid() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "retry without stepId/workflowId must stay valid"
        );
    }

    #[test]
    fn parse_bytes_component_inputs_ref() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      properties:
        name:
          type: string
      required:
        - name
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = match spec.workflows[0].inputs.as_ref() {
            Some(i) => i,
            None => panic!("inputs should be present"),
        };
        assert!(
            inputs.ref_.is_empty(),
            "ref_ should be cleared after resolution"
        );
        assert!(inputs.properties.contains_key("name"));
        assert_eq!(inputs.required, vec!["name".to_string()]);
    }

    #[test]
    fn component_input_ref_preserves_component_extensions() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      x-input:
        origin: component
      properties:
        name:
          type: string
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = spec.workflows[0]
            .inputs
            .as_ref()
            .unwrap_or_else(|| panic!("inputs should be present"));
        assert!(inputs.extensions.contains_key("x-input"));
    }

    #[test]
    fn non_vendor_unknown_fields_do_not_validate_or_survive_serialization() {
        let spec_yaml = r#"
arazzo: "1.0.0"
unknownRoot: dropped
info:
  title: Test
  version: "1.0.0"
  unknownInfo: dropped
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        unknownStep: dropped
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let serialized =
            serde_yaml_ng::to_string(&spec).unwrap_or_else(|err| panic!("serialize: {err}"));

        assert!(!serialized.contains("unknownRoot"));
        assert!(!serialized.contains("unknownInfo"));
        assert!(!serialized.contains("unknownStep"));
    }

    #[test]
    fn parse_bytes_component_inputs_ref_not_found() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    other:
      type: object
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/missing"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let result = parse_bytes(spec_yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err
                    .to_string()
                    .contains("component input \"missing\" not found")
                {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_component_inputs_ref_invalid_prefix() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    foo:
      type: object
workflows:
  - workflowId: wf1
    inputs:
      $ref: "http://external/schema"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let result = parse_bytes(spec_yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.to_string().contains("unsupported input $ref") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_component_inputs_ref_full_replacement() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      properties:
        age:
          type: integer
      required:
        - age
workflows:
  - workflowId: wf1
    inputs:
      type: string
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = match spec.workflows[0].inputs.as_ref() {
            Some(i) => i,
            None => panic!("inputs should be present"),
        };
        // The inline `type: string` should be fully replaced by the component
        assert_eq!(
            inputs.type_,
            Some(arazzo_spec::JsonSchemaType::Object),
            "inline type should be replaced by component's type"
        );
        assert!(inputs.properties.contains_key("age"));
        assert_eq!(inputs.required, vec!["age".to_string()]);
    }

    // ── Bug #26: resolve_action_ref merges instead of replacing ───

    #[test]
    fn parse_bytes_component_action_preserves_local_overrides() {
        // Component defines retry with retryLimit=5, retryAfter=2.
        // Local action references the component but overrides retryAfter=10.
        // After resolution: retryLimit=5 (from component) + retryAfter=10 (local override).
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    retryPolicy:
      type: retry
      retryAfter: 2
      retryLimit: 5
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
            retryAfter: 10
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_failure;
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].action_type(),
            ActionType::Retry,
            "type from component"
        );
        assert_eq!(actions[0].retry_limit, Some(5), "retryLimit from component");
        assert_eq!(actions[0].retry_after, 10, "retryAfter overridden locally");
    }

    #[test]
    fn parse_bytes_component_action_explicit_end_overrides_component_type() {
        // Component defines retry; the local reference explicitly sets type: end.
        // The explicit end must survive the merge instead of being treated as
        // an omitted (default) type.
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    retryPolicy:
      type: retry
      retryAfter: 2
      retryLimit: 5
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
            type: end
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let actions = &spec.workflows[0].steps[0].on_failure;
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].action_type(),
            ActionType::End,
            "explicit type: end must override the component's retry"
        );
    }

    /// Builds a one-step document with the given Arazzo version, workflow-level
    /// parameters, and step-level parameters, rendered as YAML so the whole
    /// parse-then-validate path is exercised.
    fn querystring_doc(version: &str, workflow_params: &str, step_params: &str) -> String {
        format!(
            r#"arazzo: "{version}"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: search
    url: https://search.example.com/v1
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{workflow_params}
    steps:
      - stepId: s1
        operationPath: /index
        parameters:{step_params}
"#
        )
    }

    const NO_PARAMS: &str = " []";
    const STEP_QUERYSTRING: &str =
        "\n          - name: filter\n            in: querystring\n            value: q=red\n";
    const WORKFLOW_QUERY: &str = "\n      - name: q\n        in: query\n        value: inherited\n";
    const WORKFLOW_QUERYSTRING: &str =
        "\n      - name: inherited\n        in: querystring\n        value: a=1\n";

    /// Parameter Object: the `querystring` location cannot coexist with `query`
    /// parameters in the same operation. The two arrive from different levels
    /// here, which `merge_workflow_params` would otherwise combine into one
    /// request without complaint.
    #[test]
    fn inherited_query_conflicts_with_step_querystring() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERY, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("expected a querystring/query conflict to fail validation");
        };
        let conflicts = report
            .errors
            .iter()
            .filter(|item| item.kind == ValidationErrorKind::InvalidParameterLocation)
            .collect::<Vec<_>>();
        assert_eq!(conflicts.len(), 1, "errors={:?}", report.errors);
        assert!(
            conflicts[0].message.contains("\"filter\"") && conflicts[0].message.contains("\"q\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
        assert_eq!(
            conflicts[0].path,
            "workflow \"wf1\" > step \"s1\".parameters"
        );
    }

    /// The same conflict declared entirely at the step level.
    #[test]
    fn step_level_query_conflicts_with_step_querystring() {
        let step_params = format!(
            "{STEP_QUERYSTRING}          - name: q\n            in: query\n            value: red\n"
        );
        let yaml = querystring_doc("1.1.0", NO_PARAMS, &step_params);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("expected a querystring/query conflict to fail validation");
        };
        assert!(report
            .errors
            .iter()
            .any(|item| item.kind == ValidationErrorKind::InvalidParameterLocation));
    }

    /// A step targeting another workflow does not inherit workflow parameters,
    /// so there is no operation for the two locations to collide in.
    #[test]
    fn workflow_target_step_does_not_inherit_the_conflict() {
        let yaml = format!(
            r#"arazzo: "1.1.0"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{WORKFLOW_QUERY}
    steps:
      - stepId: s1
        workflowId: wf2
        parameters:
          - name: workflow-input
            value: accepted
  - workflowId: wf2
    steps:
      - stepId: s2
        operationPath: /index
"#
        );
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected no conflict, got: {err}"),
        }
    }

    /// Every `InvalidParameterLocation` diagnostic the document produced.
    fn location_conflicts(spec: &ArazzoSpec) -> Vec<Diagnostic> {
        let Err(Error::Validation(report)) = validate_diagnostics(spec) else {
            panic!("expected the document to fail validation");
        };
        report
            .errors
            .into_iter()
            .filter(|item| item.kind == ValidationErrorKind::InvalidParameterLocation)
            .collect()
    }

    /// OpenAPI 3.2.0 Parameter Object: `querystring` "MUST NOT appear more than
    /// once". `merge_workflow_params` keys on `(name, in)`, so a workflow-level
    /// and a step-level `querystring` with *different* names do not collapse —
    /// both reach the same operation, and the last one silently wins.
    #[test]
    fn inherited_querystring_conflicts_with_step_querystring() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERYSTRING, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let conflicts = location_conflicts(&spec);
        assert_eq!(conflicts.len(), 1, "conflicts={conflicts:?}");
        assert_eq!(conflicts[0].severity, Severity::Error);
        assert!(
            conflicts[0].message.contains("\"inherited\"")
                && conflicts[0].message.contains("\"filter\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
        assert_eq!(
            conflicts[0].path,
            "workflow \"wf1\" > step \"s1\".parameters"
        );
    }

    /// The same duplication declared entirely at the step level.
    #[test]
    fn two_step_level_querystrings_conflict() {
        let step_params = format!(
            "{STEP_QUERYSTRING}          - name: extra\n            in: querystring\n            \
             value: b=2\n"
        );
        let yaml = querystring_doc("1.1.0", NO_PARAMS, &step_params);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let conflicts = location_conflicts(&spec);
        assert_eq!(conflicts.len(), 1, "conflicts={conflicts:?}");
        assert!(
            conflicts[0].message.contains("\"filter\"")
                && conflicts[0].message.contains("\"extra\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
    }

    /// The step-overrides-workflow path: same `(name, in)` key, so
    /// `merge_workflow_params` collapses the two into the step-level one and a
    /// single `querystring` parameter reaches the operation.
    #[test]
    fn same_name_querystring_override_validates_clean() {
        let workflow_params =
            "\n      - name: filter\n        in: querystring\n        value: q=inherited\n";
        let yaml = querystring_doc("1.1.0", workflow_params, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("the override path must stay clean, got: {err}"),
        }
    }

    /// A step targeting another workflow does not inherit workflow parameters,
    /// so the workflow-level `querystring` never joins the step-level one.
    #[test]
    fn workflow_target_step_does_not_inherit_the_duplicate() {
        let yaml = format!(
            r#"arazzo: "1.1.0"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{WORKFLOW_QUERYSTRING}
    steps:
      - stepId: s1
        workflowId: wf2
        parameters:
          - name: workflow-input
            value: accepted
  - workflowId: wf2
    steps:
      - stepId: s2
        operationPath: /index
"#
        );
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected no duplicate, got: {err}"),
        }
    }

    /// A single `querystring` parameter arriving by inheritance is still one
    /// parameter: the count is over the effective set, not the step's list.
    #[test]
    fn inherited_querystring_alone_validates_clean() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERYSTRING, NO_PARAMS);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.1.0 document, got: {err}"),
        }
    }

    /// `querystring` alone is valid in a 1.1.0 document, with no diagnostics.
    #[test]
    fn querystring_alone_validates_clean_at_1_1_0() {
        let yaml = querystring_doc("1.1.0", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.1.0 document, got: {err}"),
        }
    }

    /// `querystring` was introduced in Arazzo 1.1.0, so a 1.0.x document using
    /// it is refused rather than executed: the `arazzo` field is what tooling
    /// interprets the document with, and a warning never reached `run`, which
    /// loads through the diagnostic-discarding `parse`.
    #[test]
    fn querystring_in_a_1_0_document_fails_validation() {
        let yaml = querystring_doc("1.0.1", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("a 1.0.x document using querystring must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::UnsupportedVersion
        );
        assert_eq!(report.errors[0].severity, Severity::Error);
        assert_eq!(
            report.errors[0].path,
            "workflow \"wf1\" > step \"s1\".parameters[0].in"
        );
        assert!(
            report.errors[0].message.contains("1.1.0")
                && report.errors[0].message.contains("1.0.1"),
            "the error must name both versions: {}",
            report.errors[0].message
        );
        // Rejecting is only defensible because the fix is one line, so the
        // message has to carry it rather than leave the author to infer it.
        assert!(
            report.errors[0].message.contains("declare arazzo: 1.1.0"),
            "the error must state the remedy, not only the violation: {}",
            report.errors[0].message
        );
    }

    /// `1.0.0` gates identically to `1.0.1` — the rule reads major.minor, and
    /// the message names whichever version the document actually declared.
    #[test]
    fn querystring_in_a_1_0_0_document_fails_validation() {
        let yaml = querystring_doc("1.0.0", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("a 1.0.0 document using querystring must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::UnsupportedVersion
        );
        assert!(
            report.errors[0].message.contains("arazzo: 1.0.0"),
            "the error must name the declared version: {}",
            report.errors[0].message
        );
    }

    /// The gate keys on `querystring`, not on the declared version: a 1.0.x
    /// document that stays inside the 1.0 vocabulary is untouched.
    #[test]
    fn a_1_0_document_without_querystring_is_unaffected() {
        let step_query = "\n          - name: q\n            in: query\n            value: red\n";
        let yaml = querystring_doc("1.0.1", NO_PARAMS, step_query);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.0.1 document, got: {err}"),
        }
    }

    /// The version gate reads the declared major.minor, not a string prefix:
    /// a hypothetical 1.10.0 is not 1.0.x.
    #[test]
    fn version_gate_reads_major_minor_not_a_prefix() {
        use super::declares_pre_1_1;

        assert!(declares_pre_1_1("1.0.0"));
        assert!(declares_pre_1_1("1.0.1"));
        assert!(declares_pre_1_1("1.0"));
        assert!(!declares_pre_1_1("1.1.0"));
        assert!(!declares_pre_1_1("1.10.0"));
        assert!(!declares_pre_1_1("2.0.0"));
    }

    // ── workflow / Step Parameter target context and identities ─────────

    fn parameter_context_document(components: &str, parent: &str, child: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: Parameter context
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
{components}workflows:
  - workflowId: parent
{parent}
  - workflowId: child
{child}"#
        )
    }

    fn parameter_context_report(yaml: &str) -> super::ValidationReport {
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected parameter context validation error");
        };
        report
    }

    /// Positive evidence for all supported operation locations, workflow-input
    /// shape, case-sensitive identities, exact Step overrides, and component
    /// expansion. `unused` deliberately has no `in`: a component definition
    /// has no target context until it is consumed.
    pub(super) fn parameter_context_positive_matrix() {
        let yaml = parameter_context_document(
            r#"components:
  parameters:
    reusable:
      name: reusable
      in: cookie
      value: component
    unused:
      name: unused
      value: component
"#,
            r#"    parameters:
      - name: inherited
        in: header
        value: inherited
      - name: override
        in: header
        value: workflow
    steps:
      - stepId: operation-id
        operationId: invoke
        parameters:
          - name: override
            in: header
            value: step
          - name: query-name
            in: query
            value: query
          - name: same
            in: query
            value: query
          - name: same
            in: header
            value: header
          - name: Case
            in: header
            value: upper
          - name: case
            in: header
            value: lower
          - reference: $components.parameters.reusable
      - stepId: operation-path
        operationPath: /path
        parameters:
          - name: id
            in: path
            value: path
      - stepId: querystring
        operationId: wholeQuery
        parameters:
          - name: search
            in: querystring
            value: q=blue
      - stepId: invoke-workflow
        workflowId: child
        parameters:
          - name: input
            value: passed
        onSuccess:
          - name: continue
            type: goto
            workflowId: child
            parameters:
              - name: success-input
                value: passed
        onFailure:
          - name: recover
            type: retry
            workflowId: child
            retryLimit: 1
            parameters:
              - name: failure-input
                value: passed
"#,
            "    steps: []\n",
        );

        match parse_bytes(yaml.as_bytes()) {
            Ok(_) => {}
            Err(err) => panic!("expected positive parameter matrix to validate: {err}"),
        }
    }

    /// Negative evidence pins each declaration/use path. One document uses a
    /// component reference with a bad operation context to prove that the
    /// intrinsic component check is not fabricated as a component `in` error,
    /// while the consuming Step gets the context diagnostic.
    pub(super) fn parameter_context_negative_matrix() {
        let missing_operation_in = parameter_context_document(
            "",
            r#"    parameters:
      - name: inherited
        value: inherited
    steps:
      - stepId: by-id
        operationId: invoke
      - stepId: by-path
        operationPath: /path
        parameters:
          - name: local
            value: local
      - stepId: child
        workflowId: child
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&missing_operation_in);
        let paths = report
            .errors
            .iter()
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();
        assert!(
            paths.contains(&"workflow \"parent\".parameters[0].in"),
            "errors={:?}",
            report.errors
        );
        assert!(
            paths.contains(&"workflow \"parent\" > step \"by-path\".parameters[0].in"),
            "errors={:?}",
            report.errors
        );
        assert!(
            !paths.iter().any(|path| path.contains("step \"child\"")),
            "workflow targets must not inherit the invalid global parameter: {:?}",
            report.errors
        );

        let workflow_and_channel = parameter_context_document(
            "",
            r#"    steps:
      - stepId: child
        workflowId: child
        parameters:
          - name: input
            in: header
            value: v
      - stepId: async
        channelPath: /events
        action: send
        parameters:
          - name: message
            in: header
            value: v
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&workflow_and_channel);
        assert!(report.errors.iter().any(|error| {
            error.path == "workflow \"parent\" > step \"child\".parameters[0].in"
                && error.kind == ValidationErrorKind::InvalidParameterLocation
        }));
        assert!(report.errors.iter().any(|error| {
            error.path == "workflow \"parent\" > step \"async\".parameters[0]"
                && error.message.contains("AsyncAPI parameter transport")
        }));

        let declared_duplicates = parameter_context_document(
            "",
            r#"    parameters:
      - name: duplicated
        in: header
        value: first
      - name: duplicated
        in: header
        value: second
    steps:
      - stepId: operation
        operationId: invoke
        parameters:
          - name: repeated
            in: query
            value: first
          - name: repeated
            in: query
            value: second
        onSuccess:
          - name: handoff
            type: goto
            workflowId: child
            parameters:
              - name: action
                value: first
              - name: action
                value: second
        onFailure:
          - name: recover
            type: goto
            workflowId: child
            parameters:
              - name: failure
                value: first
              - name: failure
                value: second
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&declared_duplicates);
        for (path, first_path) in [
            (
                "workflow \"parent\".parameters[1].name",
                "workflow \"parent\".parameters[0].name",
            ),
            (
                "workflow \"parent\" > step \"operation\".parameters[1].name",
                "workflow \"parent\" > step \"operation\".parameters[0].name",
            ),
            (
                "workflow \"parent\" > step \"operation\".onSuccess[0].parameters[1].name",
                "workflow \"parent\" > step \"operation\".onSuccess[0].parameters[0].name",
            ),
            (
                "workflow \"parent\" > step \"operation\".onFailure[0].parameters[1].name",
                "workflow \"parent\" > step \"operation\".onFailure[0].parameters[0].name",
            ),
        ] {
            let diagnostic = report
                .errors
                .iter()
                .find(|error| error.path == path)
                .unwrap_or_else(|| {
                    panic!("missing duplicate diagnostic {path}: {:?}", report.errors)
                });
            assert_eq!(diagnostic.kind, ValidationErrorKind::DuplicateIdentifier);
            assert!(
                diagnostic.message.contains(first_path),
                "diagnostic={diagnostic:?}"
            );
        }

        let distinct_invalid_action_locations = parameter_context_document(
            "",
            r#"    steps:
      - stepId: operation
        operationId: invoke
        onSuccess:
          - name: handoff
            type: goto
            workflowId: child
            parameters:
              - name: same
                in: header
                value: first
              - name: same
                in: query
                value: second
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&distinct_invalid_action_locations);
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|error| error.kind == ValidationErrorKind::DuplicateIdentifier)
                .count(),
            0,
            "different invalid locations must not collapse: {:?}",
            report.errors
        );
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|error| error.path.ends_with(".parameters[0].in")
                    || error.path.ends_with(".parameters[1].in"))
                .count(),
            2,
            "each invalid action location must remain visible: {:?}",
            report.errors
        );

        let component_use = parameter_context_document(
            r#"components:
  parameters:
    shared:
      name: shared
      value: component
"#,
            r#"    steps:
      - stepId: operation
        operationId: invoke
        parameters:
          - reference: $components.parameters.shared
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&component_use);
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].path,
            "workflow \"parent\" > step \"operation\".parameters[0].in"
        );

        let unused_component = parameter_context_document(
            r#"components:
  parameters:
    incomplete:
      value: component
"#,
            "    steps: []\n",
            "    steps: []\n",
        );
        let report = parameter_context_report(&unused_component);
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].path,
            "components.parameters.incomplete.name"
        );
    }

    // ── success/failure action `parameters` (Arazzo 1.1.0) ─────────────

    /// One workflow with two steps and a second workflow to target, with
    /// `action_yaml` spliced in as the first step's `onFailure` list.
    fn action_params_doc(version: &str, action_yaml: &str) -> String {
        format!(
            r#"arazzo: "{version}"
info:
  title: Action Parameters
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /index
        onFailure:
{action_yaml}
  - workflowId: fallback
    steps:
      - stepId: fb
        operationPath: /fallback
"#
        )
    }

    fn parse_unvalidated(yaml: &str) -> ArazzoSpec {
        match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        }
    }

    /// Success/Failure Action Object: parameters on a workflow-targeting
    /// action are the conformant shape — literal, runtime-expression, and
    /// Selector Object values all validate clean.
    #[test]
    fn action_parameters_on_a_workflow_target_validate_clean() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: handoff
            type: goto
            workflowId: fallback
            parameters:
              - name: fixed
                value: literal
              - name: uid
                value: $inputs.uid
              - name: reason
                value:
                  context: $response.body
                  selector: /reason
                  type: jsonpointer
"#,
        );
        let spec = parse_unvalidated(&yaml);
        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected clean validation, got: {err}"),
        }
    }

    /// Failure Action Object: `retry` with a `workflowId` may also carry
    /// parameters — the referenced workflow is a workflow target too.
    #[test]
    fn retry_action_with_a_workflow_target_accepts_parameters() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: recover
            type: retry
            workflowId: fallback
            retryLimit: 2
            parameters:
              - name: cause
                value: $inputs.cause
"#,
        );
        let spec = parse_unvalidated(&yaml);
        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected clean validation, got: {err}"),
        }
    }

    /// Success/Failure Action Object: *"The `in` field MUST NOT be used."*
    #[test]
    fn action_parameter_with_in_is_rejected() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: handoff
            type: goto
            workflowId: fallback
            parameters:
              - name: uid
                in: header
                value: v
"#,
        );
        let spec = parse_unvalidated(&yaml);
        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("an action parameter declaring `in` must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::InvalidParameterLocation
        );
        assert!(
            report.errors[0].path.ends_with(".parameters[0].in"),
            "path={}",
            report.errors[0].path
        );
    }

    /// Success/Failure Action Object: parameters *"MUST be passed to a
    /// workflow as referenced by workflowId"* — a step-targeting goto has no
    /// workflow to pass them to.
    #[test]
    fn action_parameters_on_a_step_target_are_rejected() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: bounce
            type: goto
            stepId: s1
            parameters:
              - name: uid
                value: v
"#,
        );
        let spec = parse_unvalidated(&yaml);
        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("action parameters on a step target must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::InvalidParameterLocation
        );
        assert!(
            report.errors[0].message.contains("workflowId"),
            "the error must say what parameters need: {}",
            report.errors[0].message
        );
    }

    /// An `end` action has no target at all, so parameters have nowhere to go.
    #[test]
    fn action_parameters_on_an_end_action_are_rejected() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: stop
            type: end
            parameters:
              - name: uid
                value: v
"#,
        );
        let spec = parse_unvalidated(&yaml);
        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("action parameters on an end action must fail validation");
        };
        assert!(report
            .errors
            .iter()
            .any(|item| item.kind == ValidationErrorKind::InvalidParameterLocation));
    }

    /// Success/Failure Action Object: *"The list MUST NOT include duplicate
    /// parameters."*
    #[test]
    fn duplicate_action_parameter_names_are_rejected() {
        let yaml = action_params_doc(
            "1.1.0",
            r#"          - name: handoff
            type: goto
            workflowId: fallback
            parameters:
              - name: uid
                value: a
              - name: uid
                value: b
"#,
        );
        let spec = parse_unvalidated(&yaml);
        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("duplicate action parameter names must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::DuplicateIdentifier
        );
        assert!(
            report.errors[0].message.contains("\"uid\""),
            "the duplicate must be named: {}",
            report.errors[0].message
        );
    }

    /// Action parameters are 1.1.0 vocabulary: a 1.0.x document using them is
    /// rejected the same way `in: querystring` is.
    #[test]
    fn a_1_0_document_using_action_parameters_is_rejected() {
        let yaml = action_params_doc(
            "1.0.1",
            r#"          - name: handoff
            type: goto
            workflowId: fallback
            parameters:
              - name: uid
                value: v
"#,
        );
        let spec = parse_unvalidated(&yaml);
        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("a 1.0.x document using action parameters must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::UnsupportedVersion
        );
        assert!(
            report.errors[0].message.contains("arazzo: 1.0.1"),
            "the error must name the declared version: {}",
            report.errors[0].message
        );
    }

    /// A component success action carrying parameters resolves onto the
    /// referencing action, and a Reusable Object inside the parameter list
    /// resolves against `components.parameters`.
    #[test]
    fn component_action_parameters_resolve_through_references() {
        let spec_yaml = r#"
arazzo: "1.1.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    shared:
      name: uid
      value: $inputs.uid
  successActions:
    handoff:
      name: handoff
      type: goto
      workflowId: fallback
      parameters:
        - reference: $components.parameters.shared
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: "$components.successActions.handoff"
  - workflowId: fallback
    steps:
      - stepId: fb
        operationPath: /fallback
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };

        let action = &spec.workflows[0].steps[0].on_success[0];
        assert_eq!(action.parameters.len(), 1);
        assert_eq!(action.parameters[0].name, "uid");
        assert!(action.parameters[0].reference.is_empty());
    }

    /// The `in` prohibition applies to the resolved parameter: a component
    /// parameter that carries `in` is still rejected when an action list
    /// pulls it in by reference.
    #[test]
    fn component_parameter_with_in_is_rejected_on_an_action() {
        let spec_yaml = r#"
arazzo: "1.1.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    shared:
      name: uid
      in: header
      value: v
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: handoff
            type: goto
            workflowId: fallback
            parameters:
              - reference: $components.parameters.shared
  - workflowId: fallback
    steps:
      - stepId: fb
        operationPath: /fallback
"#;

        let result = parse_bytes(spec_yaml.as_bytes());
        let Err(Error::Validation(report)) = result else {
            panic!("a referenced parameter carrying `in` must fail on an action");
        };
        assert!(
            report.errors.iter().any(|item| item.kind
                == ValidationErrorKind::InvalidParameterLocation
                && item.path.ends_with(".parameters[0].in")),
            "errors={:?}",
            report.errors
        );
    }

    // --- ac-4a71f: MUST-level identifier and successCriteria rules ---

    fn workflow_outputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
    outputs:
      "{key}": $response.body
"#
        )
    }

    fn step_outputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        outputs:
          "{key}": $response.body
"#
        )
    }

    fn components_inputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    "{key}":
      type: object
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_parameters_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    "{key}":
      name: q
      in: query
      value: v
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_success_actions_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    "{key}":
      type: end
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_failure_actions_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    "{key}":
      type: end
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    /// Spec builders for ac-0379b's three SHOULD-level `IdentifierClass::Identifier`
    /// definition sites: Workflow Object `workflowId`, Step Object `stepId`, and
    /// `sourceDescriptions[].name`.
    fn workflow_id_spec(id: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "{id}"
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn step_id_spec(id: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: "{id}"
        operationPath: /test
"#
        )
    }

    fn source_description_name_spec(name: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: "{name}"
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    /// Table-driven MUST-level identifier check (Acceptance Criteria: every
    /// DottedKey position produces an error when violated, and stays valid
    /// when the key is legal). Covers all six positions in one pass: workflow
    /// outputs, step outputs, and the four Components maps.
    #[test]
    fn dotted_key_positions_accept_valid_and_reject_invalid_keys() {
        const VALID_KEY: &str = "Az9.-_";
        const INVALID_KEY: &str = "not identifier shaped";

        type PositionBuilder = fn(&str) -> String;
        let positions: &[(&str, PositionBuilder)] = &[
            ("workflow outputs", workflow_outputs_spec),
            ("step outputs", step_outputs_spec),
            ("components.inputs", components_inputs_spec),
            ("components.parameters", components_parameters_spec),
            ("components.successActions", components_success_actions_spec),
            ("components.failureActions", components_failure_actions_spec),
        ];

        for (label, build) in positions {
            let valid_yaml = build(VALID_KEY);
            if let Err(err) = parse_bytes(valid_yaml.as_bytes()) {
                panic!("{label}: expected {VALID_KEY:?} to validate cleanly, got: {err}");
            }

            let invalid_yaml = build(INVALID_KEY);
            let Err(Error::Validation(report)) = parse_bytes(invalid_yaml.as_bytes()) else {
                panic!("{label}: expected {INVALID_KEY:?} to fail validation");
            };
            assert!(
                report
                    .errors
                    .iter()
                    .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier),
                "{label}: expected an invalidIdentifier error, got: {:?}",
                report.errors
            );
        }
    }

    /// Anchoring regression: the literal `bad key!` from the specification's
    /// MUST clause (and the ticket's Goal probe document) must be rejected.
    /// `IdentifierClass::is_valid` walks every character with
    /// `chars().all(..)`, so it has no unanchored form to regress to — this
    /// test is what proves the check actually fires, not merely compiles.
    #[test]
    fn bad_key_exclamation_is_rejected() {
        let yaml = step_outputs_spec("bad key!");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected \"bad key!\" to be rejected");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier
                    && item.path.contains("bad key!")),
            "errors={:?}",
            report.errors
        );
    }

    #[test]
    fn workflow_steps_are_required_but_empty_steps_are_valid() {
        let source =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";
        let absent_steps = required_lists_spec(
            source,
            "workflows:\n  - workflowId: wf1\n    operationPath: /test\n",
        );
        assert_missing_required_path(&absent_steps, "workflow \"wf1\".steps");

        let empty_steps =
            required_lists_spec(source, "workflows:\n  - workflowId: wf1\n    steps: []\n");
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(empty_steps.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("steps: [] must remain valid, got: {err}"),
        };
        assert!(spec.workflows[0].steps.is_empty());
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    #[test]
    fn anonymous_workflow_missing_steps_uses_indexed_path() {
        let source =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";
        let yaml = required_lists_spec(source, "workflows:\n  - operationPath: /test\n");
        assert_missing_required_path(&yaml, "workflows[0].steps");
    }

    #[test]
    fn fully_populated_document_remains_clean() {
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(VALID_YAML.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("fully populated document must remain valid, got: {err}"),
        };
        assert_eq!(spec.source_descriptions.len(), 1);
        assert_eq!(spec.workflows.len(), 1);
        assert_eq!(spec.workflows[0].steps.len(), 1);
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    /// `successCriteria` absent, populated, bare-null, explicit-null, tilde,
    /// and empty, exercised through
    /// `parse_bytes` — the only entry point that can see the distinction
    /// between an absent key and an explicit `[]`. `Step.success_criteria` is
    /// `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so both
    /// collapse to the same typed `Vec::new()`; `validate(&ArazzoSpec)` /
    /// `validate_diagnostics(&ArazzoSpec)` cannot enforce this rule for the
    /// same reason (see the comment on `check_raw_success_criteria`).
    #[test]
    fn success_criteria_absent_and_populated_are_valid_empty_is_rejected() {
        fn spec_with(success_criteria: &str) -> String {
            format!(
                r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
{success_criteria}
"#
            )
        }

        // Absent: no `successCriteria` key at all.
        if let Err(err) = parse_bytes(spec_with("").as_bytes()) {
            panic!("absent successCriteria must validate, got: {err}");
        }

        // Populated: at least one Criterion Object.
        let populated =
            spec_with("        successCriteria:\n          - condition: $statusCode == 200");
        if let Err(err) = parse_bytes(populated.as_bytes()) {
            panic!("populated successCriteria must validate, got: {err}");
        }

        // Bare null: a present key with no value is still provided and must
        // contain at least one Criterion Object.
        let bare_null = spec_with("        successCriteria:");
        let Err(Error::Validation(report)) = parse_bytes(bare_null.as_bytes()) else {
            panic!("expected bare successCriteria: to fail validation");
        };
        assert!(
            report.errors.iter().any(|item| {
                item.kind == ValidationErrorKind::MissingRequiredField
                    && item.path.ends_with(".successCriteria")
            }),
            "errors={:?}",
            report.errors
        );

        // Explicit null spellings fail during typed parsing and must remain
        // rejected even though the raw check handles the implicit null above.
        for spelling in ["null", "~"] {
            let explicit_null = spec_with(&format!("        successCriteria: {spelling}"));
            assert!(
                parse_bytes(explicit_null.as_bytes()).is_err(),
                "expected successCriteria: {spelling} to be rejected"
            );
        }

        // Empty: Step Object — "If successCriteria is provided, it MUST
        // contain at least one Criterion Object."
        let empty = spec_with("        successCriteria: []");
        let Err(Error::Validation(report)) = parse_bytes(empty.as_bytes()) else {
            panic!("expected successCriteria: [] to fail validation");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.path.ends_with(".successCriteria")),
            "errors={:?}",
            report.errors
        );
    }

    /// Pins the "do not over-apply the regex" boundary from the design:
    /// `OnAction.name` carries `$components.*` reference values in real
    /// documents (e.g. `examples/httpbin-components.arazzo.yaml`) and must
    /// not be constrained by the DottedKey check — only the Components map
    /// *key* is checked, never an `OnAction.name` value that references one.
    #[test]
    fn on_action_name_component_reference_is_accepted() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    standardRetry:
      type: retry
      retryAfter: 1
      retryLimit: 3
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.standardRetry"
"#;
        if let Err(err) = parse_bytes(yaml.as_bytes()) {
            panic!("expected $components.* OnAction.name to validate cleanly, got: {err}");
        }
    }

    /// The Goal probe document: after this ticket it fails, reporting both
    /// violations together in the same run — Design's "report every
    /// violation, not the first" bullet.
    #[test]
    fn goal_probe_document_reports_both_violations_together() {
        let yaml = r#"arazzo: 1.1.0
info:
  title: Gap probe
  version: 1.0.0
sourceDescriptions:
  - name: probe-api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: probe
    steps:
      - stepId: getThing
        operationPath: /things
        successCriteria: []
        outputs:
          "bad key!": $response.body
"#;
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected the probe document to fail validation");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier),
            "missing InvalidIdentifier error; errors={:?}",
            report.errors
        );
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.path.ends_with(".successCriteria")),
            "missing successCriteria error; errors={:?}",
            report.errors
        );
        assert_eq!(report.errors.len(), 2, "errors={:?}", report.errors);
    }

    /// A document whose only violations are SHOULD-level identifier
    /// positions (`workflowId`, `stepId`, `sourceDescriptions[].name`) must
    /// still validate: this is ac-0379b's scope, and rejecting them would
    /// collapse the MUST/SHOULD split and make `validate` refuse
    /// specification-conformant documents.
    #[test]
    fn should_level_identifier_violations_alone_still_validate() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: "not an identifier!"
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "not an identifier!"
    steps:
      - stepId: "not an identifier!"
        operationPath: /test
"#;
        if let Err(err) = parse_bytes(yaml.as_bytes()) {
            panic!("SHOULD-level-only violations must not fail validation, got: {err}");
        }
    }

    /// Table-driven SHOULD-level identifier check (ac-0379b Acceptance
    /// Criteria): each of the three `IdentifierClass::Identifier` definition
    /// sites produces a warning — never an error — when the value violates
    /// `^[A-Za-z0-9_\-]+$`, and stays silent when the value is legal
    /// (including `-` and `_`).
    #[test]
    fn should_level_identifier_positions_warn_on_invalid_and_stay_silent_on_valid() {
        const VALID_ID: &str = "Az9-_";
        const INVALID_ID: &str = "not identifier shaped";

        type PositionBuilder = fn(&str) -> String;
        let positions: &[(&str, PositionBuilder)] = &[
            ("workflowId", workflow_id_spec),
            ("stepId", step_id_spec),
            ("sourceDescriptions[].name", source_description_name_spec),
        ];

        for (label, build) in positions {
            let valid_yaml = build(VALID_ID);
            let (_, warnings) = match parse_bytes_with_diagnostics(valid_yaml.as_bytes()) {
                Ok(result) => result,
                Err(err) => {
                    panic!("{label}: expected {VALID_ID:?} to validate cleanly, got: {err}")
                }
            };
            assert!(
                !warnings
                    .iter()
                    .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
                "{label}: expected no identifier warning for {VALID_ID:?}, got: {warnings:?}"
            );

            let invalid_yaml = build(INVALID_ID);
            let (_, warnings) = match parse_bytes_with_diagnostics(invalid_yaml.as_bytes()) {
                Ok(result) => result,
                Err(err) => {
                    panic!("{label}: a SHOULD violation must warn, not fail validation: {err}")
                }
            };
            assert!(
                warnings.iter().any(|w| w.kind
                    == ValidationErrorKind::InvalidIdentifier
                    && w.severity == Severity::Warning),
                "{label}: expected an invalidIdentifier warning for {INVALID_ID:?}, got: {warnings:?}"
            );
        }
    }

    /// Anchoring regression: `IdentifierClass::is_valid` walks every
    /// character with `chars().all(..)`, so it has no unanchored form to
    /// regress to — this proves the SHOULD-level check actually fires.
    #[test]
    fn should_level_bad_name_with_spaces_warns() {
        let yaml = workflow_id_spec("bad name with spaces!");
        let (_, warnings) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(result) => result,
            Err(err) => panic!("\"bad name with spaces!\" must warn, not fail validation: {err}"),
        };
        assert!(
            warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier
                    && w.path.contains("workflowId")),
            "warnings={warnings:?}"
        );
    }

    /// Pins the class difference against ac-4a71f's `DottedKey`: a `.` is
    /// illegal in the `Identifier` class (SHOULD-level `workflowId`) but
    /// legal in `DottedKey` (MUST-level `outputs` keys). The same literal
    /// value warns at one position and validates cleanly at the other.
    #[test]
    fn dot_in_identifier_warns_but_is_legal_in_dotted_key() {
        const VALUE_WITH_DOT: &str = "wf.1";

        let identifier_yaml = workflow_id_spec(VALUE_WITH_DOT);
        let (_, warnings) = match parse_bytes_with_diagnostics(identifier_yaml.as_bytes()) {
            Ok(result) => result,
            Err(err) => panic!("dotted workflowId must warn, not fail validation: {err}"),
        };
        assert!(
            warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
            "warnings={warnings:?}"
        );

        let dotted_key_yaml = step_outputs_spec(VALUE_WITH_DOT);
        if let Err(err) = parse_bytes(dotted_key_yaml.as_bytes()) {
            panic!("the same value as a DottedKey key must validate cleanly, got: {err}");
        }
    }

    /// Double-report guard: an empty `workflowId` produces only the existing
    /// "is required" error, never an additional character-class warning —
    /// `IdentifierClass::is_valid` rejects the empty string, but the empty
    /// branch must short-circuit before that check runs.
    #[test]
    fn empty_identifier_produces_only_the_required_error_no_identifier_warning() {
        let yaml = workflow_id_spec("");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected empty workflowId to fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::MissingRequiredField
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
            "warnings={:?}",
            report.warnings
        );
    }

    /// Double-report guard: a duplicate, invalid-shaped `workflowId` produces
    /// exactly one `DuplicateIdentifier` error (for the repeat) and exactly
    /// one `InvalidIdentifier` warning (for the first occurrence) — the
    /// duplicate branch takes precedence over the character-class check for
    /// the repeat, so the repeat is never reported twice.
    #[test]
    fn duplicate_invalid_shaped_identifier_is_not_double_reported() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "bad id!"
    steps:
      - stepId: s1
        operationPath: /test
  - workflowId: "bad id!"
    steps:
      - stepId: s2
        operationPath: /test2
"#;
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected duplicate workflowId to fail validation");
        };
        let dup_errors = report
            .errors
            .iter()
            .filter(|e| e.kind == ValidationErrorKind::DuplicateIdentifier)
            .count();
        assert_eq!(dup_errors, 1, "errors={:?}", report.errors);

        let identifier_warnings = report
            .warnings
            .iter()
            .filter(|w| w.kind == ValidationErrorKind::InvalidIdentifier)
            .count();
        assert_eq!(
            identifier_warnings, 1,
            "expected exactly one identifier warning, from the first occurrence only; \
             warnings={:?}",
            report.warnings
        );
    }

    /// Pins the `components.<field>."<key>"` diagnostic path form: Components
    /// map keys sit outside any workflow or step, so the crate's
    /// `workflow "<id>" > step "<id>"` convention does not apply to them.
    #[test]
    fn components_path_uses_field_and_quoted_key_form() {
        let yaml = components_parameters_spec("bad key!");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected components.parameters bad key to fail validation");
        };
        let issue = match report
            .errors
            .iter()
            .find(|item| item.kind == ValidationErrorKind::InvalidIdentifier)
        {
            Some(issue) => issue,
            None => panic!(
                "expected an InvalidIdentifier error, got: {:?}",
                report.errors
            ),
        };
        assert_eq!(issue.path, "components.parameters.\"bad key!\"");
    }

    /// One `x-*` extension (must not warn) and one misspelled/unrecognized
    /// field (must warn) at every position `arazzo-spec` captures leftover
    /// fields for, except `SchemaObject`/`PropertyDef` (JSON Schema
    /// carve-out, covered separately). Exercises: root, `info`,
    /// `sourceDescriptions[]`, `components`, `components.parameters.<name>`,
    /// `components.successActions.<name>`, workflow, workflow parameter,
    /// step, step parameter, `requestBody`, `requestBody.replacements[]`,
    /// `successCriteria[]`, and `onSuccess[]`.
    const UNKNOWN_FIELD_COVERAGE_YAML: &str = r#"arazzo: "1.1.0"
x-root-ext: keep
rootUnknown: warn
info:
  title: Unknown Field Coverage
  version: "1.0.0"
  x-info-ext: keep
  infoUnknown: warn
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
    x-source-ext: keep
    sourceUnknown: warn
components:
  x-components-ext: keep
  componentsUnknown: warn
  parameters:
    sharedParam:
      name: q
      in: query
      value: "1"
      x-param-ext: keep
      paramUnknown: warn
  successActions:
    sharedSuccess:
      name: terminate
      type: end
      x-action-ext: keep
      actionUnknown: warn
workflows:
  - workflowId: wf1
    x-workflow-ext: keep
    workflowUnknown: warn
    parameters:
      - name: p1
        in: query
        value: "1"
        x-param-ext: keep
        paramUnknown: warn
    steps:
      - stepId: s1
        operationPath: /test
        x-step-ext: keep
        stepUnknown: warn
        parameters:
          - name: p2
            in: query
            value: "2"
            x-param-ext: keep
            paramUnknown: warn
        requestBody:
          contentType: application/json
          payload: "{}"
          x-body-ext: keep
          bodyUnknown: warn
          replacements:
            - target: /a
              value: 1
              x-replacement-ext: keep
              replacementUnknown: warn
        successCriteria:
          - condition: "$statusCode == 200"
            x-criterion-ext: keep
            criterionUnknown: warn
        onSuccess:
          - name: finish
            type: end
            x-action-ext: keep
            actionUnknown: warn
"#;

    #[test]
    fn unknown_field_warns_at_every_captured_position_x_extension_does_not() {
        let (_, diagnostics) =
            match parse_bytes_with_diagnostics(UNKNOWN_FIELD_COVERAGE_YAML.as_bytes()) {
                Ok(v) => v,
                Err(err) => panic!("expected a warning-only document, got: {err}"),
            };

        let unknown_field: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.kind == ValidationErrorKind::UnknownField)
            .collect();

        let expected: &[(&str, &str)] = &[
            ("", "rootUnknown"),
            ("info", "infoUnknown"),
            ("sourceDescriptions[0]", "sourceUnknown"),
            ("components", "componentsUnknown"),
            ("components.parameters.sharedParam", "paramUnknown"),
            ("components.successActions.sharedSuccess", "actionUnknown"),
            ("workflow \"wf1\"", "workflowUnknown"),
            ("workflow \"wf1\".parameters[0]", "paramUnknown"),
            ("workflow \"wf1\" > step \"s1\"", "stepUnknown"),
            (
                "workflow \"wf1\" > step \"s1\".parameters[0]",
                "paramUnknown",
            ),
            ("workflow \"wf1\" > step \"s1\".requestBody", "bodyUnknown"),
            (
                "workflow \"wf1\" > step \"s1\".requestBody.replacements[0]",
                "replacementUnknown",
            ),
            (
                "workflow \"wf1\" > step \"s1\".successCriteria[0]",
                "criterionUnknown",
            ),
            (
                "workflow \"wf1\" > step \"s1\".onSuccess[0]",
                "actionUnknown",
            ),
        ];

        for (path, key) in expected {
            assert!(
                unknown_field.iter().any(|d| d.severity == Severity::Warning
                    && d.path == *path
                    && d.message.contains(&format!("\"{key}\""))),
                "missing unknownField warning at path {path:?} for {key:?}; got={unknown_field:?}"
            );
        }
        assert_eq!(
            unknown_field.len(),
            expected.len(),
            "unexpected extra/missing unknownField warnings: {unknown_field:?}"
        );

        // Every `x-*` field in the document is a no-op: none of them appear
        // in any diagnostic, at any severity.
        for ext_key in [
            "x-root-ext",
            "x-info-ext",
            "x-source-ext",
            "x-components-ext",
            "x-param-ext",
            "x-action-ext",
            "x-workflow-ext",
            "x-step-ext",
            "x-body-ext",
            "x-replacement-ext",
            "x-criterion-ext",
        ] {
            assert!(
                diagnostics.iter().all(|d| !d.message.contains(ext_key)),
                "vendor extension {ext_key} must never be reported; diagnostics={diagnostics:?}"
            );
        }
    }

    /// Payload Replacement Object (`Replacement`) had no capture before
    /// ac-bd441 — the one genuine blind spot the design calls out. Confirms
    /// it now warns on an unrecognized field and stays silent on `x-*`.
    #[test]
    fn replacement_unknown_field_warns_and_x_extension_does_not() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Replacement Coverage
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        requestBody:
          contentType: application/json
          payload: "{}"
          replacements:
            - target: /a
              value: 1
              x-replacement-ext: keep
            - target: /b
              value: 2
              replacementTypo: warn
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a warning-only document, got: {err}"),
        };

        let unknown_field: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.kind == ValidationErrorKind::UnknownField)
            .collect();
        assert_eq!(unknown_field.len(), 1, "diagnostics={unknown_field:?}");
        assert_eq!(
            unknown_field[0].path,
            "workflow \"wf1\" > step \"s1\".requestBody.replacements[1]"
        );
        assert!(unknown_field[0].message.contains("\"replacementTypo\""));
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("x-replacement-ext")),
            "diagnostics={diagnostics:?}"
        );
    }

    /// Arazzo 1.1.0 workflow `inputs` (and `components.inputs`) are JSON
    /// Schema 2020-12 objects. `items`, `enum`, `minimum`,
    /// `additionalProperties`, and `oneOf` are legitimate keywords `arazzo-spec`
    /// does not model as typed fields — not unrecognized fields — so this
    /// must produce zero warnings. Written before the detection landed, per
    /// the ticket's testing obligations: without the `SchemaObject`/
    /// `PropertyDef` carve-out, this fails.
    #[test]
    fn json_schema_positions_produce_no_unknown_field_warnings() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Schema Carveout
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    inputs:
      type: object
      items:
        type: string
      enum:
        - a
        - b
      minimum: 1
      additionalProperties: false
      oneOf:
        - type: string
    steps:
      - stepId: s1
        operationPath: /test
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a clean document, got: {err}"),
        };
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    /// Pins the suggestion decision from the ticket's Design: detection only,
    /// no "did you mean" text — a hand-maintained field-name registry would
    /// be required to offer one, which the design explicitly rules out.
    #[test]
    fn unknown_field_message_is_the_bare_form_no_suggestion() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Bare Message
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        sucessCriteria: []
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a warning-only document, got: {err}"),
        };

        assert_eq!(diagnostics.len(), 1, "diagnostics={diagnostics:?}");
        assert_eq!(diagnostics[0].kind, ValidationErrorKind::UnknownField);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert_eq!(
            diagnostics[0].message,
            "unrecognized field \"sucessCriteria\"; only `x-` prefixed extension fields are \
             permitted here"
        );
        assert!(
            !diagnostics[0].message.contains("successCriteria"),
            "message must not suggest the correctly spelled field name: {}",
            diagnostics[0].message
        );
    }
}
