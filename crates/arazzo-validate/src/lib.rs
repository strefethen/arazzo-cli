#![forbid(unsafe_code)]

//! Validation layer for parsed Arazzo specifications.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;

use arazzo_spec::{
    parse_unvalidated_bytes, ActionType, ArazzoSpec, CriterionType, OnAction, Parameter,
    SuccessCriterion,
};

/// Parser/validation error type for Arazzo specs.
#[derive(Debug)]
pub enum Error {
    ReadFile(std::io::Error),
    ParseYaml(String),
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
            Self::ParseYaml(_) | Self::ComponentResolution(_) => None,
        }
    }
}

/// A collection of structural validation errors found in an Arazzo spec.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
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

/// A single structural validation error with kind, spec path, and message.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub path: String,
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for ValidationError {}

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
    InvalidRetryField,
    InvalidCriterionType,
}

/// Parses and validates an Arazzo spec file from disk.
pub fn parse(path: impl AsRef<Path>) -> Result<ArazzoSpec, Error> {
    let bytes = fs::read(path).map_err(Error::ReadFile)?;
    parse_bytes(&bytes)
}

/// Parses and validates an Arazzo spec from raw YAML bytes.
pub fn parse_bytes(data: &[u8]) -> Result<ArazzoSpec, Error> {
    let mut spec =
        parse_unvalidated_bytes(data).map_err(|err| Error::ParseYaml(err.to_string()))?;
    resolve_components(&mut spec).map_err(Error::ComponentResolution)?;
    validate(&spec)?;
    Ok(spec)
}

/// Applies structural validation rules to an Arazzo spec.
pub fn validate(spec: &ArazzoSpec) -> Result<(), Error> {
    let mut errs = Vec::<ValidationError>::new();

    if spec.arazzo.is_empty() {
        errs.push(ValidationError {
            kind: ValidationErrorKind::MissingRequiredField,
            path: "arazzo".to_string(),
            message: "arazzo version is required".to_string(),
        });
    } else if !spec.arazzo.starts_with("1.") {
        errs.push(ValidationError {
            kind: ValidationErrorKind::UnsupportedVersion,
            path: "arazzo".to_string(),
            message: format!("unsupported arazzo version: {} (expected 1.x)", spec.arazzo),
        });
    }

    if spec.info.title.is_empty() {
        errs.push(ValidationError {
            kind: ValidationErrorKind::MissingRequiredField,
            path: "info.title".to_string(),
            message: "info.title is required".to_string(),
        });
    }
    if spec.info.version.is_empty() {
        errs.push(ValidationError {
            kind: ValidationErrorKind::MissingRequiredField,
            path: "info.version".to_string(),
            message: "info.version is required".to_string(),
        });
    }

    let mut source_names = HashSet::<String>::new();
    for (idx, src) in spec.source_descriptions.iter().enumerate() {
        let path = format!("sourceDescriptions[{idx}]");
        if src.name.is_empty() {
            errs.push(ValidationError {
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.name"),
                message: format!("{path}.name is required"),
            });
        } else if !source_names.insert(src.name.clone()) {
            errs.push(ValidationError {
                kind: ValidationErrorKind::DuplicateIdentifier,
                path: format!("{path}.name"),
                message: format!("{path}.name '{}' is duplicate", src.name),
            });
        }
        if src.url.is_empty() {
            errs.push(ValidationError {
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.url"),
                message: format!("{path}.url is required"),
            });
        }
    }

    // Collect all workflow IDs upfront for cross-workflow goto validation.
    let workflow_ids: HashSet<String> = spec
        .workflows
        .iter()
        .filter(|wf| !wf.workflow_id.is_empty())
        .map(|wf| wf.workflow_id.clone())
        .collect();

    for (wf_idx, wf) in spec.workflows.iter().enumerate() {
        let path = format!("workflows[{wf_idx}]");

        if wf.workflow_id.is_empty() {
            errs.push(ValidationError {
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{path}.workflowId"),
                message: format!("{path}.workflowId is required"),
            });
        } else {
            // Check for duplicates by counting occurrences.
            let dup_count = spec
                .workflows
                .iter()
                .filter(|w| w.workflow_id == wf.workflow_id)
                .count();
            if dup_count > 1 {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::DuplicateIdentifier,
                    path: format!("{path}.workflowId"),
                    message: format!("{path}.workflowId '{}' is duplicate", wf.workflow_id),
                });
            }
        }

        validate_parameters(&format!("{path}.parameters"), &wf.parameters, &mut errs);

        // Collect step IDs for this workflow before validating actions.
        let step_ids: HashSet<String> = wf
            .steps
            .iter()
            .filter(|s| !s.step_id.is_empty())
            .map(|s| s.step_id.clone())
            .collect();

        validate_actions(
            &format!("{path}.successActions"),
            &wf.success_actions,
            &step_ids,
            &workflow_ids,
            &mut errs,
        );
        validate_actions(
            &format!("{path}.failureActions"),
            &wf.failure_actions,
            &step_ids,
            &workflow_ids,
            &mut errs,
        );

        let mut seen_step_ids = HashSet::<String>::new();
        for (step_idx, step) in wf.steps.iter().enumerate() {
            let step_path = format!("{path}.steps[{step_idx}]");

            if step.step_id.is_empty() {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{step_path}.stepId"),
                    message: format!("{step_path}.stepId is required"),
                });
            } else if !seen_step_ids.insert(step.step_id.clone()) {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::DuplicateIdentifier,
                    path: format!("{step_path}.stepId"),
                    message: format!("{step_path}.stepId '{}' is duplicate", step.step_id),
                });
            }

            if step.target.is_none() {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidStepTarget,
                    path: step_path.clone(),
                    message: format!(
                        "{step_path} must have operationId, operationPath, or workflowId"
                    ),
                });
            }

            validate_parameters(
                &format!("{step_path}.parameters"),
                &step.parameters,
                &mut errs,
            );

            for (criterion_idx, criterion) in step.success_criteria.iter().enumerate() {
                validate_criterion(
                    &format!("{step_path}.successCriteria[{criterion_idx}]"),
                    criterion,
                    &mut errs,
                );
            }

            validate_actions(
                &format!("{step_path}.onFailure"),
                &step.on_failure,
                &step_ids,
                &workflow_ids,
                &mut errs,
            );
            validate_actions(
                &format!("{step_path}.onSuccess"),
                &step.on_success,
                &step_ids,
                &workflow_ids,
                &mut errs,
            );
        }

        for (name, expr) in &wf.outputs {
            if let Some(after) = expr.strip_prefix("$steps.") {
                let step_name = after.split('.').next().unwrap_or_default();
                if !step_ids.contains(step_name) {
                    errs.push(ValidationError {
                        kind: ValidationErrorKind::InvalidReference,
                        path: format!("{path}.outputs.{name}"),
                        message: format!(
                            "{path}.outputs.{name} references unknown step '{step_name}'"
                        ),
                    });
                }
            }
        }
    }

    if errs.is_empty() {
        return Ok(());
    }
    Err(Error::Validation(ValidationReport { errors: errs }))
}

fn validate_parameters(path_prefix: &str, params: &[Parameter], errs: &mut Vec<ValidationError>) {
    for (param_idx, param) in params.iter().enumerate() {
        let param_path = format!("{path_prefix}[{param_idx}]");
        if param.name.is_empty() && param.reference.is_empty() {
            errs.push(ValidationError {
                kind: ValidationErrorKind::MissingRequiredField,
                path: format!("{param_path}.name"),
                message: format!("{param_path}.name is required (unless using reference)"),
            });
        }
        if param.is_value_empty() && param.reference.is_empty() {
            errs.push(ValidationError {
                kind: ValidationErrorKind::MissingParameterValue,
                path: param_path,
                message: format!("{path_prefix}[{param_idx}] must have value or reference"),
            });
        }
    }
}

fn validate_actions(
    path_prefix: &str,
    actions: &[OnAction],
    step_ids: &HashSet<String>,
    workflow_ids: &HashSet<String>,
    errs: &mut Vec<ValidationError>,
) {
    for (action_idx, action) in actions.iter().enumerate() {
        let action_path = format!("{path_prefix}[{action_idx}]");
        if action.retry_after < 0 {
            errs.push(ValidationError {
                kind: ValidationErrorKind::InvalidRetryField,
                path: format!("{action_path}.retryAfter"),
                message: format!("{action_path}.retryAfter must be non-negative"),
            });
        }
        if action.retry_limit < 0 {
            errs.push(ValidationError {
                kind: ValidationErrorKind::InvalidRetryField,
                path: format!("{action_path}.retryLimit"),
                message: format!("{action_path}.retryLimit must be non-negative"),
            });
        }
        if action.type_ == ActionType::Goto {
            let has_step = !action.step_id.is_empty();
            let has_workflow = !action.workflow_id.is_empty();
            if !has_step && !has_workflow {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: action_path.clone(),
                    message: format!("{action_path} goto action must specify stepId or workflowId"),
                });
            }
            if has_step && !step_ids.contains(&action.step_id) {
                errs.push(ValidationError {
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
                && !workflow_ids.contains(&action.workflow_id)
            {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidReference,
                    path: format!("{action_path}.workflowId"),
                    message: format!(
                        "{action_path}.workflowId references unknown workflow \"{}\"",
                        action.workflow_id
                    ),
                });
            }
        }
        for (criterion_idx, criterion) in action.criteria.iter().enumerate() {
            validate_criterion(
                &format!("{action_path}.criteria[{criterion_idx}]"),
                criterion,
                errs,
            );
        }
    }
}

fn validate_criterion(path: &str, criterion: &SuccessCriterion, errs: &mut Vec<ValidationError>) {
    if criterion.condition.trim().is_empty() {
        errs.push(ValidationError {
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.condition"),
            message: format!("{path}.condition is required"),
        });
    }

    if criterion.has_declared_type() && criterion.context.trim().is_empty() {
        errs.push(ValidationError {
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.context"),
            message: format!("{path}.context is required when type is specified"),
        });
    }

    let Some(type_) = &criterion.type_ else {
        return;
    };

    match type_ {
        CriterionType::Name(name) => {
            let normalized = name.trim().to_lowercase();
            if !matches!(
                normalized.as_str(),
                "simple" | "regex" | "jsonpath" | "xpath"
            ) {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidCriterionType,
                    path: format!("{path}.type"),
                    message: format!("{path}.type must be one of simple, regex, jsonpath, xpath"),
                });
            }
        }
        CriterionType::ExpressionType(expr) => {
            let normalized = expr.type_.trim().to_lowercase();
            if !matches!(normalized.as_str(), "jsonpath" | "xpath") {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidCriterionType,
                    path: format!("{path}.type.type"),
                    message: format!("{path}.type.type must be one of jsonpath or xpath"),
                });
                return;
            }

            let version = expr.version.trim().to_lowercase();
            if version.is_empty() {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{path}.type.version"),
                    message: format!("{path}.type.version is required"),
                });
                return;
            }

            if normalized == "jsonpath" && version != "draft-goessner-dispatch-jsonpath-00" {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidCriterionType,
                    path: format!("{path}.type.version"),
                    message: format!(
                        "{path}.type.version must be draft-goessner-dispatch-jsonpath-00 for jsonpath"
                    ),
                });
            }

            if normalized == "xpath"
                && !matches!(version.as_str(), "xpath-10" | "xpath-20" | "xpath-30")
            {
                errs.push(ValidationError {
                    kind: ValidationErrorKind::InvalidCriterionType,
                    path: format!("{path}.type.version"),
                    message: format!(
                        "{path}.type.version must be one of xpath-10, xpath-20, xpath-30 for xpath"
                    ),
                });
            }
        }
    }
}

/// Resolves `$components.*` references to inline step definitions.
fn resolve_components(spec: &mut ArazzoSpec) -> Result<(), String> {
    let Some(components) = spec.components.clone() else {
        return Ok(());
    };

    for workflow in &mut spec.workflows {
        let wf_label = format!("workflow {}", workflow.workflow_id);
        resolve_param_refs(&mut workflow.parameters, &components, &wf_label)?;
        resolve_action_ref(
            &mut workflow.success_actions,
            &components.success_actions,
            "$components.successActions.",
            "successAction",
            &wf_label,
        )?;
        resolve_action_ref(
            &mut workflow.failure_actions,
            &components.failure_actions,
            "$components.failureActions.",
            "failureAction",
            &wf_label,
        )?;

        for step in &mut workflow.steps {
            let step_label = format!("step {}", step.step_id);
            resolve_param_refs(&mut step.parameters, &components, &step_label)?;
            resolve_action_ref(
                &mut step.on_success,
                &components.success_actions,
                "$components.successActions.",
                "successAction",
                &step_label,
            )?;
            resolve_action_ref(
                &mut step.on_failure,
                &components.failure_actions,
                "$components.failureActions.",
                "failureAction",
                &step_label,
            )?;
        }
    }

    Ok(())
}

fn resolve_param_refs(
    params: &mut Vec<Parameter>,
    components: &arazzo_spec::Components,
    entity: &str,
) -> Result<(), String> {
    let mut resolved = Vec::with_capacity(params.len());
    for mut param in params.drain(..) {
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
            if param.is_value_empty() {
                param.value = component_param.value.clone();
            }
            param.reference.clear();
        }
        resolved.push(param);
    }
    *params = resolved;
    Ok(())
}

fn resolve_action_ref(
    actions: &mut Vec<OnAction>,
    component_map: &std::collections::BTreeMap<String, Vec<OnAction>>,
    prefix: &str,
    kind: &str,
    entity: &str,
) -> Result<(), String> {
    if actions.len() == 1 && !actions[0].name.is_empty() {
        if let Some(name) = actions[0].name.strip_prefix(prefix) {
            let Some(resolved) = component_map.get(name) else {
                return Err(format!("{entity}: component {kind} \"{name}\" not found"));
            };
            *actions = resolved.clone();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use arazzo_spec::{
        ActionType, CriterionExpressionType, CriterionType, Info, OnAction, ParamLocation,
        SourceDescription, SourceType, Step, StepTarget, SuccessCriterion, Workflow,
    };

    use super::{parse, parse_bytes, validate, ArazzoSpec, Error, ValidationErrorKind};

    /// Unwrap a validation Error into its report errors, panicking on other variants.
    fn expect_validation_errors(result: Result<(), Error>) -> Vec<super::ValidationError> {
        match result {
            Ok(()) => panic!("expected validation error"),
            Err(Error::Validation(report)) => report.errors,
            Err(other) => panic!("expected Validation error, got: {other}"),
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

    fn valid_spec() -> ArazzoSpec {
        ArazzoSpec {
            arazzo: "1.0.0".to_string(),
            info: Info {
                title: "Test".to_string(),
                summary: String::new(),
                version: "1.0.0".to_string(),
                description: String::new(),
            },
            source_descriptions: vec![SourceDescription {
                name: "api".to_string(),
                url: "https://example.com".to_string(),
                type_: SourceType::OpenApi,
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

    fn temp_file_path(prefix: &str) -> PathBuf {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(v) => v.as_nanos(),
            Err(_) => 0,
        };
        std::env::temp_dir().join(format!("{prefix}-{nanos}.yaml"))
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
        let _ = std::fs::remove_file(&path);

        let spec = match result {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert_eq!(spec.info.title, "Test");
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
        assert_eq!(params[0].value, "Bearer abc123");
        assert!(params[0].reference.is_empty());
    }

    #[test]
    fn parse_bytes_component_parameter_override() {
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
      value: "Bearer default-token"
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.authHeader"
            value: "Bearer custom-token"
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let params = &spec.workflows[0].steps[0].parameters;
        assert_eq!(params[0].value, "Bearer custom-token");
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
      - type: end
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
        assert_eq!(actions[0].type_, ActionType::End);
        assert_eq!(actions[0].name, "terminate");
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
      - type: retry
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
        assert_eq!(actions[0].type_, ActionType::Retry);
        assert_eq!(actions[0].retry_after, 2);
        assert_eq!(actions[0].retry_limit, 5);
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

    #[test]
    fn validate_source_duplicate_name() {
        let mut spec = valid_spec();
        spec.source_descriptions.push(SourceDescription {
            name: "api".to_string(),
            url: "https://other.example.com".to_string(),
            type_: SourceType::OpenApi,
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
            .contains("must have operationId, operationPath, or workflowId"));
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
            "$steps.nonexistent.outputs.value".to_string(),
        )]);
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert!(errs[0]
            .message
            .contains("references unknown step 'nonexistent'"));
    }

    #[test]
    fn validate_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![arazzo_spec::OnAction {
            type_: ActionType::Retry,
            retry_after: -1,
            ..arazzo_spec::OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidRetryField);
        assert!(errs[0].message.contains("retryAfter must be non-negative"));
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
            })),
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
            })),
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidCriterionType);
        assert!(errs[0]
            .message
            .contains("type.version must be draft-goessner-dispatch-jsonpath-00"));
    }

    #[test]
    fn validate_action_criteria_follow_criterion_rules() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: ActionType::Retry,
            criteria: vec![SuccessCriterion {
                condition: "//item[1]".to_string(),
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: "xpath-10".to_string(),
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
    fn validate_workflow_level_actions_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].success_actions = vec![OnAction {
            type_: ActionType::Retry,
            retry_after: -1,
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidRetryField);
        assert!(errs[0]
            .message
            .contains("successActions[0].retryAfter must be non-negative"));
    }

    #[test]
    fn validate_workflow_level_failure_actions_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].failure_actions = vec![OnAction {
            type_: ActionType::Retry,
            retry_limit: -1,
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidRetryField);
        assert!(errs[0]
            .message
            .contains("failureActions[0].retryLimit must be non-negative"));
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
        assert_eq!(wf.success_actions[0].type_, ActionType::End);
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].type_, ActionType::Retry);
        assert_eq!(wf.failure_actions[0].retry_after, 5);
        assert_eq!(wf.steps[0].description, "First step");
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
      - type: end
        name: stop
  failureActions:
    retryAll:
      - type: retry
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
        assert_eq!(wf.success_actions[0].type_, ActionType::End);
        assert_eq!(wf.success_actions[0].name, "stop");
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].type_, ActionType::Retry);
        assert_eq!(wf.failure_actions[0].retry_after, 1);
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
            type_: ActionType::Goto,
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
            type_: ActionType::Goto,
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
            type_: ActionType::Goto,
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
            type_: ActionType::Goto,
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
        });
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: ActionType::Goto,
            workflow_id: "$sourceDescriptions.external.someWorkflow".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression workflowId should not be rejected"
        );
    }

    #[test]
    fn validate_goto_missing_step_and_workflow() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: ActionType::Goto,
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::MissingRequiredField
                && e.message
                    .contains("goto action must specify stepId or workflowId")));
    }
}
