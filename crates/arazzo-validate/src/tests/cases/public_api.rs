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
            (
                ValidationErrorKind::UnsupportedXpathVersion,
                "unsupportedXpathVersion",
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

    /// Structurally valid: its only finding is an unknown-field warning.
    const WARNING_ONLY_YAML: &str = r#"arazzo: "1.0.0"
unknownField: true
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
"#;

    /// Missing `info.title` (error) plus one unknown-field warning.
    const MIXED_YAML: &str = r#"arazzo: "1.0.0"
unknownField: true
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
"#;

    #[test]
    fn warning_only_document_parses_ok_and_returns_diagnostics() {
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(WARNING_ONLY_YAML.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("warnings must not fail validation, got: {err}"),
        };
        assert_eq!(spec.info.title, "Warning Only");
        assert_eq!(diagnostics.len(), 1, "diagnostics={diagnostics:?}");
        assert!(diagnostics.iter().all(|d| d.severity == Severity::Warning));
        assert!(diagnostics
            .iter()
            .all(|d| d.kind == ValidationErrorKind::UnknownField));
        assert_eq!(diagnostics[0].path, "");
        assert_eq!(
            diagnostics[0].message,
            "unrecognized field \"unknownField\"; only `x-` prefixed extension fields are permitted here"
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
                    "unrecognized field \"unknownField\"; only `x-` prefixed extension fields are permitted here"
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

