    fn diagnostic_errors(result: Result<Vec<Diagnostic>, Error>) -> Vec<super::ValidationError> {
        match result {
            Err(Error::Validation(report)) => report.errors,
            Ok(_) => panic!("expected validation diagnostics"),
            Err(other) => panic!("expected Validation error, got: {other}"),
        }
    }

    #[test]
    fn direct_reusable_parameter_values_match_parse_for_distinguishable_classes() {
        let cases = [
            ("boolean", "false", "false"),
            ("number", "7", "7"),
            ("mapping", "{key: value}", r#"{"key":"value"}"#),
            ("sequence", "[one, two]", r#"["one","two"]"#),
            (
                "selector",
                "{context: $response.body, selector: /id, type: jsonpointer}",
                r#"{"context":"$response.body","selector":"/id","type":"jsonpointer"}"#,
            ),
        ];
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for (class, yaml_value, json_value) in cases {
                let value = if json { json_value } else { yaml_value };
                let document = reusable_parameter_document(Some(value), json);
                let spec = parse_unvalidated(&document);
                let original = spec.clone();
                let validate_errors = expect_validation_errors(validate(&spec));
                let diagnostic_errors = diagnostic_errors(validate_diagnostics(&spec));
                let parse_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
                    Err(Error::Validation(report)) => report.errors,
                    Ok(_) => panic!("{format} {class} reusable parameter must fail parsing"),
                    Err(other) => panic!("{format} {class}: expected Validation, got {other}"),
                };

                assert_eq!(validate_errors.len(), 1, "{format} {class}");
                assert_eq!(diagnostic_errors.len(), 1, "{format} {class}");
                assert_eq!(parse_errors.len(), 1, "{format} {class}");
                assert_eq!(validate_errors, diagnostic_errors, "{format} {class}");
                assert_eq!(diagnostic_errors, parse_errors, "{format} {class}");
                assert_eq!(
                    diagnostic_errors[0].path,
                    "workflow \"wf\" > step \"request\".parameters[0].value"
                );
                assert_eq!(spec, original, "direct validation must not mutate {class}");
            }
        }
    }

    #[test]
    fn direct_reusable_action_values_match_parse_for_distinguishable_classes() {
        let cases = [
            ("boolean", "false", "false"),
            ("number", "7", "7"),
            ("mapping", "{key: value}", r#"{"key":"value"}"#),
            ("sequence", "[one, two]", r#"["one","two"]"#),
            (
                "selector-shaped mapping",
                "{context: $response.body, selector: /id, type: jsonpointer}",
                r#"{"context":"$response.body","selector":"/id","type":"jsonpointer"}"#,
            ),
        ];
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for (class, yaml_value, json_value) in cases {
                let value = if json { json_value } else { yaml_value };
                let document = reusable_action_document(Some(value), json);
                let spec = parse_unvalidated(&document);
                let original = spec.clone();
                let validate_errors = expect_validation_errors(validate(&spec));
                let diagnostic_errors = diagnostic_errors(validate_diagnostics(&spec));
                let parse_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
                    Err(Error::Validation(report)) => report.errors,
                    Ok(_) => panic!("{format} {class} reusable action must fail parsing"),
                    Err(other) => panic!("{format} {class}: expected Validation, got {other}"),
                };

                assert_eq!(validate_errors.len(), 1, "{format} {class}");
                assert_eq!(diagnostic_errors.len(), 1, "{format} {class}");
                assert_eq!(parse_errors.len(), 1, "{format} {class}");
                assert_eq!(validate_errors, diagnostic_errors, "{format} {class}");
                assert_eq!(diagnostic_errors, parse_errors, "{format} {class}");
                assert_eq!(
                    diagnostic_errors[0].path,
                    "workflow \"wf\" > step \"request\".onSuccess[0].value"
                );
                assert_eq!(spec, original, "direct validation must not mutate {class}");
            }
        }
    }

    #[test]
    fn direct_reusable_parameter_value_checks_cover_every_resolution_scope() {
        let document = r#"arazzo: "1.1.0"
info: {title: Direct reusable Parameter scopes, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  parameters:
    input: {name: input, value: component}
    operation: {name: operation, in: header, value: component}
  successActions:
    component:
      name: component
      type: goto
      workflowId: child
      parameters:
        - reference: $components.parameters.input
          value: false
workflows:
  - workflowId: workflow-values
    parameters:
      - reference: $components.parameters.input
        value: false
    successActions:
      - name: local
        type: goto
        workflowId: child
        parameters:
          - reference: $components.parameters.input
            value: false
    steps:
      - stepId: workflow
        workflowId: child
        parameters:
          - reference: $components.parameters.input
            value: false
  - workflowId: step-values
    steps:
      - stepId: operation
        operationId: invoke
        parameters:
          - reference: $components.parameters.operation
            value: false
        onSuccess:
          - name: local
            type: goto
            workflowId: child
            parameters:
              - reference: $components.parameters.input
                value: false
  - workflowId: child
    steps: []
"#;
        let spec = parse_unvalidated(document);
        let direct_validate = expect_validation_errors(validate(&spec));
        let direct_diagnostics = diagnostic_errors(validate_diagnostics(&spec));
        let parse_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
            Err(Error::Validation(report)) => report.errors,
            Ok(_) => panic!("every non-string reusable Parameter value must fail parsing"),
            Err(other) => panic!("expected Validation, got {other}"),
        };
        let expected_paths = vec![
            "components.successActions.component.parameters[0].value",
            "workflow \"workflow-values\".parameters[0].value",
            "workflow \"workflow-values\".successActions[0].parameters[0].value",
            "workflow \"workflow-values\" > step \"workflow\".parameters[0].value",
            "workflow \"step-values\" > step \"operation\".parameters[0].value",
            "workflow \"step-values\" > step \"operation\".onSuccess[0].parameters[0].value",
        ];
        assert_eq!(direct_validate, direct_diagnostics);
        assert_eq!(direct_diagnostics, parse_errors);
        assert_eq!(direct_diagnostics.len(), expected_paths.len());
        assert_eq!(
            direct_diagnostics
                .iter()
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>(),
            expected_paths
        );
    }

    #[test]
    fn direct_reusable_action_value_checks_cover_all_four_action_lists() {
        let document = r#"arazzo: "1.1.0"
info: {title: Direct reusable Action scopes, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    imported: {name: imported-success, type: end}
  failureActions:
    imported: {name: imported-failure, type: end}
workflows:
  - workflowId: parent
    successActions:
      - {reference: $components.successActions.imported, value: false}
    failureActions:
      - {reference: $components.failureActions.imported, value: 7}
    steps:
      - stepId: operation
        operationId: invoke
        onSuccess:
          - {reference: $components.successActions.imported, value: {nested: true}}
        onFailure:
          - {reference: $components.failureActions.imported, value: [invalid]}
"#;
        let spec = parse_unvalidated(document);
        let direct_validate = expect_validation_errors(validate(&spec));
        let direct_diagnostics = diagnostic_errors(validate_diagnostics(&spec));
        let parse_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
            Err(Error::Validation(report)) => report.errors,
            Ok(_) => panic!("all four non-string reusable Action values must fail parsing"),
            Err(other) => panic!("expected Validation, got {other}"),
        };
        let expected_paths = vec![
            "workflow \"parent\".successActions[0].value",
            "workflow \"parent\".failureActions[0].value",
            "workflow \"parent\" > step \"operation\".onSuccess[0].value",
            "workflow \"parent\" > step \"operation\".onFailure[0].value",
        ];
        assert_eq!(direct_validate, direct_diagnostics);
        assert_eq!(direct_diagnostics, parse_errors);
        assert_eq!(direct_diagnostics.len(), expected_paths.len());
        assert_eq!(
            direct_diagnostics
                .iter()
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>(),
            expected_paths
        );
    }

    #[test]
    fn direct_reusable_action_null_values_fail_in_all_four_action_lists() {
        let document = r#"arazzo: "1.1.0"
info: {title: Direct reusable Action nulls, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    imported: {name: imported-success, type: end}
  failureActions:
    imported: {name: imported-failure, type: end}
workflows:
  - workflowId: parent
    successActions:
      - {reference: $components.successActions.imported, value: null}
    failureActions:
      - {reference: $components.failureActions.imported, value: null}
    steps:
      - stepId: operation
        operationId: invoke
        onSuccess:
          - {reference: $components.successActions.imported, value: null}
        onFailure:
          - {reference: $components.failureActions.imported, value: null}
"#;
        let mut spec = parse_unvalidated(document);

        // Deserialization collapses these raw nulls to `None`. Construct the
        // distinguishable typed boundary state that direct callers can supply.
        spec.workflows[0].success_actions[0].value = Some(serde_yaml_ng::Value::Null);
        spec.workflows[0].failure_actions[0].value = Some(serde_yaml_ng::Value::Null);
        spec.workflows[0].steps[0].on_success[0].value = Some(serde_yaml_ng::Value::Null);
        spec.workflows[0].steps[0].on_failure[0].value = Some(serde_yaml_ng::Value::Null);
        let original = spec.clone();

        let direct_validate = expect_validation_errors(validate(&spec));
        let direct_diagnostics = diagnostic_errors(validate_diagnostics(&spec));
        let raw_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
            Err(Error::Validation(report)) => report.errors,
            Ok(_) => panic!("all four raw null reusable Action values must fail parsing"),
            Err(other) => panic!("expected Validation, got {other}"),
        };
        let expected_paths = vec![
            "workflow \"parent\".successActions[0].value",
            "workflow \"parent\".failureActions[0].value",
            "workflow \"parent\" > step \"operation\".onSuccess[0].value",
            "workflow \"parent\" > step \"operation\".onFailure[0].value",
        ];

        assert_eq!(direct_validate, direct_diagnostics);
        assert_eq!(direct_diagnostics, raw_errors);
        assert_eq!(direct_diagnostics.len(), expected_paths.len());
        assert!(direct_diagnostics
            .iter()
            .all(|diagnostic| diagnostic.kind == ValidationErrorKind::InvalidReference));
        assert_eq!(
            direct_diagnostics
                .iter()
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>(),
            expected_paths
        );
        assert_eq!(
            spec, original,
            "direct validation must not mutate the caller"
        );
    }

    #[test]
    fn direct_reusable_string_values_preserve_parameter_overrides() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for (value, expected) in [
                (None, "component default"),
                (Some("\"override\""), "override"),
                (Some("\"\""), ""),
            ] {
                let document = reusable_parameter_document(value, json);
                let spec = parse_unvalidated(&document);
                let original = spec.clone();
                if let Err(error) = validate(&spec) {
                    panic!("{format} value={value:?} must validate directly: {error}");
                }
                match validate_diagnostics(&spec) {
                    Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
                    Err(error) => panic!("{format} value={value:?}: {error}"),
                }
                assert_eq!(
                    spec, original,
                    "direct validation must not mutate the model"
                );

                let mut resolved = spec.clone();
                if let Err(error) = super::resolve_components(&mut resolved, None) {
                    panic!("direct resolution failed: {error}");
                }
                assert_eq!(
                    resolved.workflows[0].steps[0].parameters[0].value,
                    expected.into(),
                    "{format} value={value:?}"
                );
                let parsed = expect_parsed(document.as_bytes());
                assert_eq!(
                    parsed.workflows[0].steps[0].parameters[0].value,
                    expected.into(),
                    "{format} parse value={value:?}"
                );
            }

            for value in [None, Some("\"ignored\""), Some("\"\"")] {
                let document = reusable_action_document(value, json);
                let spec = parse_unvalidated(&document);
                if let Err(error) = validate(&spec) {
                    panic!("{format} action value={value:?} must validate directly: {error}");
                }
                match validate_diagnostics(&spec) {
                    Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
                    Err(error) => panic!("{format} action value={value:?}: {error}"),
                }
                let parsed = expect_parsed(document.as_bytes());
                assert!(parsed.workflows[0].steps[0].on_success[0].value.is_none());
            }
        }
    }

    #[test]
    fn direct_reusable_null_retains_only_the_documented_presence_ambiguity() {
        for (label, document) in [
            (
                "parameter",
                reusable_parameter_document(Some("null"), false),
            ),
            ("action", reusable_action_document(Some("null"), false)),
        ] {
            let spec = parse_unvalidated(&document);
            if let Err(error) = validate(&spec) {
                panic!("typed {label} null is indistinguishable from omission: {error}");
            }
            match validate_diagnostics(&spec) {
                Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
                Err(error) => panic!("typed {label} null must retain the ambiguity: {error}"),
            }
            let parse_errors = match parse_bytes_with_diagnostics(document.as_bytes()) {
                Err(Error::Validation(report)) => report.errors,
                Ok(_) => panic!("raw explicit-null {label} must remain invalid"),
                Err(other) => panic!("expected Validation, got {other}"),
            };
            assert_eq!(parse_errors.len(), 1, "{label}: {parse_errors:?}");
            assert_eq!(parse_errors[0].kind, ValidationErrorKind::InvalidReference);
        }
    }

    #[test]
    fn concrete_component_parameter_any_value_remains_valid() {
        let document = reusable_parameter_document(None, false).replace(
            "value: \"component default\"",
            "value: {nested: [false, 7]}",
        );
        let spec = parse_unvalidated(&document);
        if let Err(error) = validate(&spec) {
            panic!("concrete Parameter Any value must validate directly: {error}");
        }
        let parsed = expect_parsed(document.as_bytes());
        assert!(matches!(
            parsed.workflows[0].steps[0].parameters[0].value,
            arazzo_spec::ValueSource::Literal(serde_yaml_ng::Value::Mapping(_))
        ));
    }

    #[test]
    fn inherited_parameter_context_diagnostics_have_stable_cardinality() {
        let yaml = parameter_context_document(
            "",
            r#"    parameters:
      - name: inherited
        value: value
    steps:
      - stepId: operation-id
        operationId: first
      - stepId: operation-path
        operationPath: /second
      - stepId: async-id
        operationId: publish
        action: send
      - stepId: channel
        channelPath: /events
        action: receive
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&yaml);
        let context_errors = report
            .errors
            .iter()
            .filter(|error| error.kind == ValidationErrorKind::InvalidParameterLocation)
            .collect::<Vec<_>>();
        assert_eq!(context_errors.len(), 3, "errors={:?}", report.errors);
        assert_eq!(
            context_errors
                .iter()
                .filter(|error| error.path == "workflow \"parent\".parameters[0].in")
                .count(),
            1,
            "equivalent operation consumers must collapse"
        );
        assert_eq!(
            context_errors
                .iter()
                .filter(|error| error.path == "workflow \"parent\".parameters[0]")
                .count(),
            2,
            "async operation and channel constraints must remain distinct"
        );
        assert!(context_errors
            .iter()
            .any(|error| error.message.contains("asynchronous operation")));
        assert!(context_errors
            .iter()
            .any(|error| error.message.contains("channelPath")));
    }

    #[test]
    fn rendered_path_collision_does_not_skip_local_parameter_provenance() {
        let yaml = r#"arazzo: "1.1.0"
info: {title: Structural provenance, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  parameters:
    imported: {name: imported, in: header, value: component}
workflows:
  - workflowId: 'a" > step "b'
    parameters:
      - reference: $components.parameters.imported
    steps:
      - stepId: source
        operationId: source
  - workflowId: a
    steps:
      - stepId: b
        operationId: target
        parameters:
          - in: header
            value: local
"#;
        let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(yaml.as_bytes()) else {
            panic!("the local Parameter without a name must fail validation");
        };
        let required_names = report
            .errors
            .iter()
            .filter(|error| {
                error.kind == ValidationErrorKind::MissingRequiredField
                    && error.path.ends_with(".parameters[0].name")
            })
            .collect::<Vec<_>>();
        assert_eq!(required_names.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            required_names[0].path,
            "workflow \"a\" > step \"b\".parameters[0].name"
        );
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
    }

    #[test]
    fn rendered_path_collision_does_not_skip_local_action_provenance() {
        let yaml = r#"arazzo: "1.1.0"
info: {title: Structural action provenance, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    imported:
      name: imported-success
      type: goto
      workflowId: child
      parameters: [{name: imported, value: component}]
  failureActions:
    imported:
      name: imported-failure
      type: retry
      workflowId: child
      parameters: [{name: imported, value: component}]
workflows:
  - workflowId: 'a" > step "b'
    steps:
      - stepId: c
        operationId: source
        onSuccess:
          - reference: $components.successActions.imported
        onFailure:
          - name: $components.failureActions.imported
  - workflowId: a
    steps:
      - stepId: 'b" > step "c'
        operationId: target
        onSuccess:
          - name: local-success
            type: goto
            workflowId: child
            parameters: [{value: local}]
        onFailure:
          - name: local-failure
            type: retry
            workflowId: child
            parameters: [{value: local}]
  - workflowId: child
    steps: []
"#;
        let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(yaml.as_bytes()) else {
            panic!("both local action Parameters without names must fail validation");
        };
        let required_names = report
            .errors
            .iter()
            .filter(|error| {
                error.kind == ValidationErrorKind::MissingRequiredField
                    && error.path.ends_with(".parameters[0].name")
            })
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            required_names,
            vec![
                "workflow \"a\" > step \"b\" > step \"c\".onFailure[0].parameters[0].name",
                "workflow \"a\" > step \"b\" > step \"c\".onSuccess[0].parameters[0].name",
            ],
            "errors={:?}",
            report.errors
        );
        assert_eq!(report.errors.len(), 2, "errors={:?}", report.errors);
    }

    #[test]
    fn rendered_path_collision_keeps_both_missing_in_diagnostics() {
        let yaml = r#"arazzo: "1.1.0"
info: {title: Structural context identity, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: 'a" > step "b'
    parameters: [{name: inherited, value: first}]
    steps: [{stepId: source, operationId: source}]
  - workflowId: a
    steps:
      - stepId: b
        operationId: target
        parameters: [{name: local, value: second}]
"#;
        let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(yaml.as_bytes()) else {
            panic!("both operation Parameters without `in` must fail validation");
        };
        let missing_in = report
            .errors
            .iter()
            .filter(|error| {
                error.kind == ValidationErrorKind::InvalidParameterLocation
                    && error.path == "workflow \"a\" > step \"b\".parameters[0].in"
            })
            .collect::<Vec<_>>();
        assert_eq!(missing_in.len(), 2, "errors={:?}", report.errors);
        assert_eq!(
            missing_in[0], missing_in[1],
            "the rendered diagnostics are intentionally byte-identical; only their structural declarations differ"
        );
        assert_eq!(report.errors.len(), 2, "errors={:?}", report.errors);
    }

    #[test]
    fn component_origin_indices_are_stable_across_btree_clones() {
        let yaml = parameter_context_document(
            r#"components:
  parameters:
    zeta: {name: zeta, in: header, value: zeta}
    alpha: {name: alpha, in: header, value: alpha}
  successActions:
    zeta: {name: zeta, type: end}
    alpha: {name: alpha, type: end}
  failureActions:
    zeta: {name: zeta, type: end}
    alpha: {name: alpha, type: end}
"#,
            r#"    steps:
      - stepId: operation
        operationId: invoke
        parameters:
          - reference: $components.parameters.zeta
        onSuccess:
          - reference: $components.successActions.zeta
          - name: $components.successActions.alpha
        onFailure:
          - reference: $components.failureActions.zeta
          - name: $components.failureActions.alpha
"#,
            "    steps: []\n",
        );
        let spec = parse_unvalidated(&yaml);
        let mut first = spec.clone();
        let first_provenance = match super::resolve_components(&mut first, None) {
            Ok(provenance) => provenance,
            Err(error) => panic!("first resolution failed: {error}"),
        };
        let mut second = spec.clone();
        let second_provenance = match super::resolve_components(&mut second, None) {
            Ok(provenance) => provenance,
            Err(error) => panic!("second resolution failed: {error}"),
        };
        assert_eq!(first, second);
        assert_eq!(first_provenance, second_provenance);

        let parameter_destination = super::DeclarationScope::StepParameters {
            workflow_index: 0,
            step_index: 0,
        }
        .declaration(0);
        assert_eq!(
            first_provenance
                .component_parameter_origins
                .get(&parameter_destination),
            Some(&super::ComponentParameterKey { parameter_index: 1 }),
            "BTreeMap order is alpha=0, zeta=1"
        );

        for (kind, action_index, expected_component_index) in [
            (super::ActionKind::Success, 0, 1),
            (super::ActionKind::Success, 1, 0),
            (super::ActionKind::Failure, 0, 1),
            (super::ActionKind::Failure, 1, 0),
        ] {
            let destination = super::DeclarationScope::StepAction {
                kind,
                workflow_index: 0,
                step_index: 0,
                action_index,
            };
            assert_eq!(
                first_provenance.component_action_origins.get(&destination),
                Some(&super::DeclarationScope::ComponentAction {
                    kind,
                    action_index: expected_component_index,
                })
            );
        }
    }

    #[test]
    fn component_action_reference_provenance_is_stable() {
        let yaml = parameter_context_document(
            r#"components:
  successActions:
    invalidSuccess:
      name: invalidSuccess
      type: goto
      workflowId: child
      parameters:
        - value: inherited
  failureActions:
    invalidFailure:
      name: invalidFailure
      type: retry
      workflowId: child
      parameters:
        - value: inherited
"#,
            r#"    successActions:
      - reference: $components.successActions.invalidSuccess
      - name: $components.successActions.invalidSuccess
    failureActions:
      - reference: $components.failureActions.invalidFailure
      - name: $components.failureActions.invalidFailure
    steps:
      - stepId: operation
        operationId: invoke
        onSuccess:
          - reference: $components.successActions.invalidSuccess
          - name: $components.successActions.invalidSuccess
          - name: $components.successActions.invalidSuccess
            parameters:
              - value: local
        onFailure:
          - reference: $components.failureActions.invalidFailure
          - name: $components.failureActions.invalidFailure
          - name: $components.failureActions.invalidFailure
            parameters:
              - value: local
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&yaml);
        let missing_names = report
            .errors
            .iter()
            .filter(|error| {
                error.kind == ValidationErrorKind::MissingRequiredField
                    && error.path.ends_with(".parameters[0].name")
            })
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            missing_names,
            vec![
                "components.successActions.invalidSuccess.parameters[0].name",
                "components.failureActions.invalidFailure.parameters[0].name",
                "workflow \"parent\" > step \"operation\".onFailure[2].parameters[0].name",
                "workflow \"parent\" > step \"operation\".onSuccess[2].parameters[0].name",
            ],
            "component-owned parameters report once at source; local legacy overrides stay consumer-owned"
        );
    }

