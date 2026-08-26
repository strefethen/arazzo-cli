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

