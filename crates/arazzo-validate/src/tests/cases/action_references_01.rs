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

    fn reusable_action_document(value: Option<&str>, json: bool) -> String {
        if json {
            let value = value.map_or_else(String::new, |value| format!(r#", "value": {value}"#));
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
      "onSuccess": [{{"reference": "$components.successActions.notify"{value}}}]
    }}]
  }}]
}}"#
            );
        }

        let value = value.map_or_else(String::new, |value| format!("\n            value: {value}"));
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
          - reference: "$components.successActions.notify"{value}
"#
        )
    }

    #[test]
    fn reusable_action_values_reject_non_string_classes_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for value in ["null", "false", "0", "{}", "[]"] {
                let document = reusable_action_document(Some(value), json);
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
      name: retryAll
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
        assert_eq!(wf.failure_actions[0].retry_after, 1.0);
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
            assert_eq!(action.retry_after, 1.0);
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
        let Err(Error::Validation(report)) = parse_bytes(non_expression.as_bytes()) else {
            panic!("a non-runtime-expression action reference must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(report.errors[0].kind, ValidationErrorKind::InvalidReference);
        assert_eq!(
            report.errors[0].path,
            "workflow \"wf\" > step \"s1\".onSuccess[0]"
        );
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
            let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
                panic!("shape={shape} must fail validation");
            };
            assert!(
                report.errors.len() == 1
                    && report.errors[0].kind == ValidationErrorKind::InvalidReference
                    && report.errors[0]
                        .message
                        .contains("reference must be a non-empty runtime expression string"),
                "shape={shape}, errors={:?}",
                report.errors
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

