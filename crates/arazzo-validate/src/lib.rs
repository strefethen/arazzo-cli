#![forbid(unsafe_code)]

//! Validation layer for parsed Arazzo specifications.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;

use arazzo_spec::{parse_unvalidated_bytes, ArazzoSpec, CriterionType, OnAction, SuccessCriterion};

/// Parser/validation error type for Arazzo specs.
#[derive(Debug)]
pub enum Error {
    ReadFile(std::io::Error),
    ParseYaml(String),
    Message(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFile(err) => write!(f, "reading arazzo file: {err}"),
            Self::ParseYaml(err) => write!(f, "parsing arazzo yaml: {err}"),
            Self::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFile(err) => Some(err),
            Self::ParseYaml(_) | Self::Message(_) => None,
        }
    }
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
    resolve_components(&mut spec).map_err(Error::Message)?;
    validate(&spec).map_err(Error::Message)?;
    Ok(spec)
}

/// Applies structural validation rules to an Arazzo spec.
pub fn validate(spec: &ArazzoSpec) -> Result<(), String> {
    let mut errs = Vec::<String>::new();

    if spec.arazzo.is_empty() {
        errs.push("arazzo version is required".to_string());
    } else if !spec.arazzo.starts_with("1.") {
        errs.push(format!(
            "unsupported arazzo version: {} (expected 1.x)",
            spec.arazzo
        ));
    }

    if spec.info.title.is_empty() {
        errs.push("info.title is required".to_string());
    }
    if spec.info.version.is_empty() {
        errs.push("info.version is required".to_string());
    }

    let mut source_names = HashSet::<String>::new();
    for (idx, src) in spec.source_descriptions.iter().enumerate() {
        let path = format!("sourceDescriptions[{idx}]");
        if src.name.is_empty() {
            errs.push(format!("{path}.name is required"));
        } else if !source_names.insert(src.name.clone()) {
            errs.push(format!("{path}.name '{}' is duplicate", src.name));
        }
        if src.url.is_empty() {
            errs.push(format!("{path}.url is required"));
        }
        if src.type_ != "openapi" && src.type_ != "arazzo" {
            errs.push(format!(
                "{path}.type must be 'openapi' or 'arazzo', got '{}'",
                src.type_
            ));
        }
    }

    let mut workflow_ids = HashSet::<String>::new();
    for (wf_idx, wf) in spec.workflows.iter().enumerate() {
        let path = format!("workflows[{wf_idx}]");

        if wf.workflow_id.is_empty() {
            errs.push(format!("{path}.workflowId is required"));
        } else if !workflow_ids.insert(wf.workflow_id.clone()) {
            errs.push(format!(
                "{path}.workflowId '{}' is duplicate",
                wf.workflow_id
            ));
        }

        for (param_idx, param) in wf.parameters.iter().enumerate() {
            let param_path = format!("{path}.parameters[{param_idx}]");
            if param.name.is_empty() && param.reference.is_empty() {
                errs.push(format!(
                    "{param_path}.name is required (unless using reference)"
                ));
            }
            if param.value.is_empty() && param.reference.is_empty() {
                errs.push(format!("{param_path} must have value or reference"));
            }
            if !param.in_.is_empty()
                && param.in_ != "path"
                && param.in_ != "query"
                && param.in_ != "header"
                && param.in_ != "cookie"
            {
                errs.push(format!(
                    "{param_path}.in must be path, query, header, or cookie"
                ));
            }
        }

        validate_actions(
            &format!("{path}.successActions"),
            &wf.success_actions,
            &mut errs,
        );
        validate_actions(
            &format!("{path}.failureActions"),
            &wf.failure_actions,
            &mut errs,
        );

        let mut step_ids = HashSet::<String>::new();
        for (step_idx, step) in wf.steps.iter().enumerate() {
            let step_path = format!("{path}.steps[{step_idx}]");

            if step.step_id.is_empty() {
                errs.push(format!("{step_path}.stepId is required"));
            } else if !step_ids.insert(step.step_id.clone()) {
                errs.push(format!(
                    "{step_path}.stepId '{}' is duplicate",
                    step.step_id
                ));
            }

            let has_op = !step.operation_id.is_empty()
                || !step.operation_path.is_empty()
                || !step.workflow_id.is_empty();
            if !has_op {
                errs.push(format!(
                    "{step_path} must have operationId, operationPath, or workflowId"
                ));
            }

            for (param_idx, param) in step.parameters.iter().enumerate() {
                let param_path = format!("{step_path}.parameters[{param_idx}]");
                if param.name.is_empty() && param.reference.is_empty() {
                    errs.push(format!(
                        "{param_path}.name is required (unless using reference)"
                    ));
                }
                if param.value.is_empty() && param.reference.is_empty() {
                    errs.push(format!("{param_path} must have value or reference"));
                }
                if !param.in_.is_empty()
                    && param.in_ != "path"
                    && param.in_ != "query"
                    && param.in_ != "header"
                    && param.in_ != "cookie"
                {
                    errs.push(format!(
                        "{param_path}.in must be path, query, header, or cookie"
                    ));
                }
            }

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
                &mut errs,
            );
            validate_actions(
                &format!("{step_path}.onSuccess"),
                &step.on_success,
                &mut errs,
            );
        }

        for (name, expr) in &wf.outputs {
            if let Some(after) = expr.strip_prefix("$steps.") {
                let step_name = after.split('.').next().unwrap_or_default();
                if !step_ids.contains(step_name) {
                    errs.push(format!(
                        "{path}.outputs.{name} references unknown step '{step_name}'"
                    ));
                }
            }
        }
    }

    if errs.is_empty() {
        return Ok(());
    }
    Err(format!("validation errors:\n  - {}", errs.join("\n  - ")))
}

fn validate_actions(path_prefix: &str, actions: &[OnAction], errs: &mut Vec<String>) {
    for (action_idx, action) in actions.iter().enumerate() {
        let action_path = format!("{path_prefix}[{action_idx}]");
        if action.retry_after < 0 {
            errs.push(format!("{action_path}.retryAfter must be non-negative"));
        }
        if action.retry_limit < 0 {
            errs.push(format!("{action_path}.retryLimit must be non-negative"));
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

fn validate_criterion(path: &str, criterion: &SuccessCriterion, errs: &mut Vec<String>) {
    if criterion.condition.trim().is_empty() {
        errs.push(format!("{path}.condition is required"));
    }

    if criterion.has_declared_type() && criterion.context.trim().is_empty() {
        errs.push(format!("{path}.context is required when type is specified"));
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
                errs.push(format!(
                    "{path}.type must be one of simple, regex, jsonpath, xpath"
                ));
            }
        }
        CriterionType::ExpressionType(expr) => {
            let normalized = expr.type_.trim().to_lowercase();
            if !matches!(normalized.as_str(), "jsonpath" | "xpath") {
                errs.push(format!("{path}.type.type must be one of jsonpath or xpath"));
                return;
            }

            let version = expr.version.trim().to_lowercase();
            if version.is_empty() {
                errs.push(format!("{path}.type.version is required"));
                return;
            }

            if normalized == "jsonpath" && version != "draft-goessner-dispatch-jsonpath-00" {
                errs.push(format!(
                    "{path}.type.version must be draft-goessner-dispatch-jsonpath-00 for jsonpath"
                ));
            }

            if normalized == "xpath"
                && !matches!(version.as_str(), "xpath-10" | "xpath-20" | "xpath-30")
            {
                errs.push(format!(
                    "{path}.type.version must be one of xpath-10, xpath-20, xpath-30 for xpath"
                ));
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
        // Resolve workflow-level parameter references
        let mut resolved_wf_params = Vec::with_capacity(workflow.parameters.len());
        for mut param in workflow.parameters.drain(..) {
            if !param.reference.is_empty() {
                let Some(name) = param.reference.strip_prefix("$components.parameters.") else {
                    return Err(format!(
                        "workflow {}: unsupported parameter reference: {}",
                        workflow.workflow_id, param.reference
                    ));
                };
                let Some(component_param) = components.parameters.get(name) else {
                    return Err(format!(
                        "workflow {}: component parameter \"{}\" not found",
                        workflow.workflow_id, name
                    ));
                };
                if param.name.is_empty() {
                    param.name = component_param.name.clone();
                }
                if param.in_.is_empty() {
                    param.in_ = component_param.in_.clone();
                }
                if param.value.is_empty() {
                    param.value = component_param.value.clone();
                }
                param.reference.clear();
            }
            resolved_wf_params.push(param);
        }
        workflow.parameters = resolved_wf_params;

        // Resolve workflow-level successActions references
        if workflow.success_actions.len() == 1
            && workflow.success_actions[0].type_.is_empty()
            && !workflow.success_actions[0].name.is_empty()
        {
            if let Some(name) = workflow.success_actions[0]
                .name
                .strip_prefix("$components.successActions.")
            {
                let Some(actions) = components.success_actions.get(name) else {
                    return Err(format!(
                        "workflow {}: component successAction \"{}\" not found",
                        workflow.workflow_id, name
                    ));
                };
                workflow.success_actions = actions.clone();
            }
        }

        // Resolve workflow-level failureActions references
        if workflow.failure_actions.len() == 1
            && workflow.failure_actions[0].type_.is_empty()
            && !workflow.failure_actions[0].name.is_empty()
        {
            if let Some(name) = workflow.failure_actions[0]
                .name
                .strip_prefix("$components.failureActions.")
            {
                let Some(actions) = components.failure_actions.get(name) else {
                    return Err(format!(
                        "workflow {}: component failureAction \"{}\" not found",
                        workflow.workflow_id, name
                    ));
                };
                workflow.failure_actions = actions.clone();
            }
        }

        for step in &mut workflow.steps {
            let mut resolved_params = Vec::with_capacity(step.parameters.len());
            for mut param in step.parameters.drain(..) {
                if !param.reference.is_empty() {
                    let Some(name) = param.reference.strip_prefix("$components.parameters.") else {
                        return Err(format!(
                            "step {}: unsupported parameter reference: {}",
                            step.step_id, param.reference
                        ));
                    };
                    let Some(component_param) = components.parameters.get(name) else {
                        return Err(format!(
                            "step {}: component parameter \"{}\" not found",
                            step.step_id, name
                        ));
                    };

                    if param.name.is_empty() {
                        param.name = component_param.name.clone();
                    }
                    if param.in_.is_empty() {
                        param.in_ = component_param.in_.clone();
                    }
                    if param.value.is_empty() {
                        param.value = component_param.value.clone();
                    }
                    param.reference.clear();
                }
                resolved_params.push(param);
            }
            step.parameters = resolved_params;

            if step.on_success.len() == 1
                && step.on_success[0].type_.is_empty()
                && !step.on_success[0].name.is_empty()
            {
                if let Some(name) = step.on_success[0]
                    .name
                    .strip_prefix("$components.successActions.")
                {
                    let Some(actions) = components.success_actions.get(name) else {
                        return Err(format!(
                            "step {}: component successAction \"{}\" not found",
                            step.step_id, name
                        ));
                    };
                    step.on_success = actions.clone();
                }
            }

            if step.on_failure.len() == 1
                && step.on_failure[0].type_.is_empty()
                && !step.on_failure[0].name.is_empty()
            {
                if let Some(name) = step.on_failure[0]
                    .name
                    .strip_prefix("$components.failureActions.")
                {
                    let Some(actions) = components.failure_actions.get(name) else {
                        return Err(format!(
                            "step {}: component failureAction \"{}\" not found",
                            step.step_id, name
                        ));
                    };
                    step.on_failure = actions.clone();
                }
            }
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
        CriterionExpressionType, CriterionType, Info, OnAction, SourceDescription, Step,
        SuccessCriterion, Workflow,
    };

    use super::{parse, parse_bytes, validate, ArazzoSpec};

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
                type_: "openapi".to_string(),
            }],
            workflows: vec![Workflow {
                workflow_id: "wf1".to_string(),
                steps: vec![Step {
                    step_id: "s1".to_string(),
                    operation_path: "/test".to_string(),
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
            Err(err) => {
                if !err.to_string().contains("arazzo version is required") {
                    panic!("unexpected error: {err}");
                }
            }
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
        assert_eq!(params[0].in_, "header");
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
        assert_eq!(actions[0].type_, "end");
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
        assert_eq!(actions[0].type_, "retry");
        assert_eq!(actions[0].retry_after, 2);
        assert_eq!(actions[0].retry_limit, 5);
    }

    #[test]
    fn validate_valid_spec() {
        let spec = valid_spec();
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error for valid spec, got: {err}");
        }
    }

    #[test]
    fn validate_missing_version() {
        let mut spec = valid_spec();
        spec.arazzo.clear();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("arazzo version is required") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_unsupported_version() {
        let mut spec = valid_spec();
        spec.arazzo = "2.0.0".to_string();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("unsupported arazzo version") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_missing_title() {
        let mut spec = valid_spec();
        spec.info.title.clear();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("info.title is required") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_missing_info_version() {
        let mut spec = valid_spec();
        spec.info.version.clear();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("info.version is required") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_source_duplicate_name() {
        let mut spec = valid_spec();
        spec.source_descriptions.push(SourceDescription {
            name: "api".to_string(),
            url: "https://other.example.com".to_string(),
            type_: "openapi".to_string(),
        });
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("is duplicate") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_source_missing_url() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].url.clear();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("sourceDescriptions[0].url is required") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_source_invalid_type() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = "invalid".to_string();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("must be 'openapi' or 'arazzo'") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_source_type_arazzo() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = "arazzo".to_string();
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error, got: {err}");
        }
    }

    #[test]
    fn validate_step_no_operation() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].operation_path.clear();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("must have operationId, operationPath, or workflowId") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_param_invalid_in() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].parameters = vec![arazzo_spec::Parameter {
            name: "q".to_string(),
            value: "x".to_string(),
            in_: "body".to_string(),
            ..arazzo_spec::Parameter::default()
        }];
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("must be path, query, header, or cookie") {
                    panic!("unexpected error: {err}");
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
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("references unknown step 'nonexistent'") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![arazzo_spec::OnAction {
            type_: "retry".to_string(),
            retry_after: -1,
            ..arazzo_spec::OnAction::default()
        }];
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("retryAfter must be non-negative") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_criterion_type_requires_context() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        }];

        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("context is required when type is specified") {
                    panic!("unexpected error: {err}");
                }
            }
        }
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

        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("type.version must be draft-goessner-dispatch-jsonpath-00") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_action_criteria_follow_criterion_rules() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: "retry".to_string(),
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

        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains(".onFailure[0].criteria[0].context is required") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_multiple_errors() {
        let spec = ArazzoSpec::default();
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("arazzo version is required") {
                    panic!("missing expected version error: {err}");
                }
                if !err.contains("info.title is required") {
                    panic!("missing expected title error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_workflow_level_actions_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].success_actions = vec![OnAction {
            type_: "retry".to_string(),
            retry_after: -1,
            ..OnAction::default()
        }];
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("successActions[0].retryAfter must be non-negative") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_workflow_level_failure_actions_retry_fields() {
        let mut spec = valid_spec();
        spec.workflows[0].failure_actions = vec![OnAction {
            type_: "retry".to_string(),
            retry_limit: -1,
            ..OnAction::default()
        }];
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains("failureActions[0].retryLimit must be non-negative") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn validate_workflow_level_param_invalid_in() {
        let mut spec = valid_spec();
        spec.workflows[0].parameters = vec![arazzo_spec::Parameter {
            name: "q".to_string(),
            value: "x".to_string(),
            in_: "body".to_string(),
            ..arazzo_spec::Parameter::default()
        }];
        let result = validate(&spec);
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.contains(
                    "workflows[0].parameters[0].in must be path, query, header, or cookie",
                ) {
                    panic!("unexpected error: {err}");
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
        assert_eq!(wf.success_actions[0].type_, "end");
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].type_, "retry");
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
        assert_eq!(wf.success_actions[0].type_, "end");
        assert_eq!(wf.success_actions[0].name, "stop");
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].type_, "retry");
        assert_eq!(wf.failure_actions[0].retry_after, 1);
    }
}
