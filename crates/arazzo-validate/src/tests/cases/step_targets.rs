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
        operationPath: '{$sourceDescriptions.api.url}#/paths/~1test/get'
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
        spec.workflows[0].steps[0].target = Some(StepTarget::OperationPath(
            "{$sourceDescriptions.missing.url}#/paths/~1items/get".to_string(),
        ));

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
    fn validate_step_operation_path_method_prefix_is_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target = Some(StepTarget::OperationPath(
            "POST {$sourceDescriptions.api.url}#/paths/~1pet/post".to_string(),
        ));
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors
            .iter()
            .any(|error| error.kind == ValidationErrorKind::UnsupportedOperationPath));
    }

    #[test]
    fn validate_step_operation_path_known_source_reference() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target = Some(StepTarget::OperationPath(
            "{$sourceDescriptions.api.url}#/paths/~1items/get".to_string(),
        ));

        if let Err(err) = validate(&spec) {
            panic!("expected sourceDescription reference to validate, got: {err}");
        }
    }

    #[test]
    fn canonical_operation_path_has_no_capability_warning() {
        let mut spec = valid_spec();
        for pointer in ["/paths/~1status/get", "/paths/~1a~0b%20c/query"] {
            spec.workflows[0].steps[0].target = Some(StepTarget::OperationPath(format!(
                "{{$sourceDescriptions.api.url}}#{pointer}"
            )));
            let warnings = validate_diagnostics(&spec)
                .unwrap_or_else(|error| panic!("standard pointer syntax should validate: {error}"));
            assert!(!warnings
                .iter()
                .any(|item| item.kind == ValidationErrorKind::UnsupportedOperationPath));
        }
    }

    #[test]
    fn custom_and_incomplete_operation_paths_are_errors() {
        for target in [
            "/items",
            "GET /items",
            "{api}./items",
            "POST {api}./items",
            "https://api.example.com/items",
            "{$sourceDescriptions.api.url}",
            "#/paths/~1items/get",
            "{$sourceDescriptions.api.url}#/paths/~2/get",
        ] {
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].target = Some(StepTarget::OperationPath(target.to_string()));
            let errors = expect_validation_errors(validate(&spec));
            assert!(
                errors
                    .iter()
                    .any(|item| item.kind == ValidationErrorKind::UnsupportedOperationPath),
                "{target}: {errors:?}"
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
        operationPath: '{$sourceDescriptions.api.url}#/paths/~1test/get'
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
