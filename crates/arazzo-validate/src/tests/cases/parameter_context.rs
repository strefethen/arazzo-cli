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

        direct_validation_component_reference_positive_matrix();
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
            1,
            "action identity is the case-sensitive name when `in` is prohibited: {:?}",
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

        unused_component_actions_enforce_parameter_contract();
        workflow_input_duplicates_use_name_only_and_report_every_in();
        async_operation_parameters_fail_closed();
        direct_validation_component_reference_negative_matrix();
        inherited_parameter_context_diagnostics_have_stable_cardinality();
        component_action_reference_provenance_is_stable();
    }

    #[test]
    fn unused_component_actions_enforce_parameter_contract() {
        let yaml = parameter_context_document(
            r#"components:
  successActions:
    unusedSuccess:
      reference: $components.successActions.other
      name: unusedSuccess
      type: goto
      parameters:
        - name: duplicate
          in: header
          value: first
        - name: duplicate
          in: query
          value: second
  failureActions:
    unusedFailure:
      name: unusedFailure
      type: retry
      workflowId: child
      parameters:
        - name: duplicate
          in: header
          value: first
        - name: duplicate
          in: query
          value: second
"#,
            "    steps: []\n",
            "    steps: []\n",
        );
        let report = parameter_context_report(&yaml);

        for prefix in [
            "components.successActions.unusedSuccess",
            "components.failureActions.unusedFailure",
        ] {
            for index in 0..2 {
                assert!(
                    report.errors.iter().any(|error| {
                        error.path == format!("{prefix}.parameters[{index}].in")
                            && error.kind == ValidationErrorKind::InvalidParameterLocation
                    }),
                    "missing forbidden `in` at {prefix}[{index}]: {:?}",
                    report.errors
                );
            }
            assert!(
                report.errors.iter().any(|error| {
                    error.path == format!("{prefix}.parameters[1].name")
                        && error.kind == ValidationErrorKind::DuplicateIdentifier
                        && error
                            .message
                            .contains(&format!("{prefix}.parameters[0].name"))
                }),
                "missing name-only duplicate at {prefix}: {:?}",
                report.errors
            );
        }
        assert!(
            report.errors.iter().any(|error| {
                error.path == "components.successActions.unusedSuccess.parameters"
                    && error.kind == ValidationErrorKind::InvalidParameterLocation
                    && error.message.contains("declares no workflowId")
            }),
            "component success action must require workflowId despite invalid reference: {:?}",
            report.errors
        );
        assert!(!report.errors.iter().any(|error| {
            error.path == "components.failureActions.unusedFailure.parameters"
                && error.message.contains("declares no workflowId")
        }));
    }

    #[test]
    fn workflow_input_duplicates_use_name_only_and_report_every_in() {
        let yaml = parameter_context_document(
            "",
            r#"    steps:
      - stepId: child
        workflowId: child
        parameters:
          - name: duplicate
            in: header
            value: first
          - name: duplicate
            in: query
            value: second
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&yaml);
        assert_eq!(
            report
                .errors
                .iter()
                .filter(|error| error.path.ends_with(".parameters[0].in")
                    || error.path.ends_with(".parameters[1].in"))
                .count(),
            2,
            "every prohibited location must be reported: {:?}",
            report.errors
        );
        let duplicate = report
            .errors
            .iter()
            .find(|error| error.kind == ValidationErrorKind::DuplicateIdentifier)
            .unwrap_or_else(|| panic!("missing workflow-input duplicate: {:?}", report.errors));
        assert_eq!(
            duplicate.path,
            "workflow \"parent\" > step \"child\".parameters[1].name"
        );
        assert!(duplicate
            .message
            .contains("workflow \"parent\" > step \"child\".parameters[0].name"));
    }

    #[test]
    fn async_operation_parameters_fail_closed() {
        let yaml = parameter_context_document(
            "",
            r#"    steps:
      - stepId: async-id
        operationId: publish
        action: send
        parameters:
          - name: header
            in: header
            value: value
      - stepId: async-path
        operationPath: /subscribe
        action: receive
        parameters:
          - name: header
            in: header
            value: value
      - stepId: channel
        channelPath: /events
        action: send
        parameters:
          - name: header
            in: header
            value: value
      - stepId: ordinary-id
        operationId: getItem
        parameters:
          - name: header
            in: header
            value: value
      - stepId: ordinary-path
        operationPath: /items
        parameters:
          - name: header
            in: header
            value: value
"#,
            "    steps: []\n",
        );
        let report = parameter_context_report(&yaml);
        let invalid_paths = report
            .errors
            .iter()
            .filter(|error| error.kind == ValidationErrorKind::InvalidParameterLocation)
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            invalid_paths,
            vec![
                "workflow \"parent\" > step \"async-id\".parameters[0]",
                "workflow \"parent\" > step \"async-path\".parameters[0]",
                "workflow \"parent\" > step \"channel\".parameters[0]",
            ]
        );
        for step in ["async-id", "async-path"] {
            assert!(
                report.errors.iter().any(|error| {
                    error.path.contains(&format!("step \"{step}\""))
                        && error.message.contains("asynchronous operation")
                }),
                "missing asynchronous-operation failure for {step}: {:?}",
                report.errors
            );
        }
        assert!(report.errors.iter().any(|error| {
            error.path.contains("step \"channel\"") && error.message.contains("channelPath")
        }));
    }

    fn direct_component_reference_document(
        missing_operation_in: bool,
        workflow_and_action_have_in: bool,
    ) -> ArazzoSpec {
        let operation_in = if missing_operation_in {
            ""
        } else {
            "      in: header\n"
        };
        let workflow_in = if workflow_and_action_have_in {
            "      in: header\n"
        } else {
            ""
        };
        let yaml = parameter_context_document(
            &format!(
                r#"components:
  parameters:
    operation:
      name: operation
{operation_in}      value: operation
    workflowInput:
      name: workflowInput
{workflow_in}      value: workflow
"#
            ),
            r#"    steps:
      - stepId: operation
        operationId: invoke
        parameters:
          - reference: $components.parameters.operation
        onSuccess:
          - name: handoff
            type: goto
            workflowId: child
            parameters:
              - reference: $components.parameters.workflowInput
      - stepId: workflow
        workflowId: child
        parameters:
          - reference: $components.parameters.workflowInput
"#,
            "    steps: []\n",
        );
        parse_unvalidated(&yaml)
    }

    #[test]
    fn direct_validation_component_reference_positive_matrix() {
        let spec = direct_component_reference_document(false, false);
        let original = spec.clone();
        if let Err(err) = validate(&spec) {
            panic!("direct validate must resolve operation/workflow/action parameters: {err}");
        }
        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("direct validate_diagnostics must resolve components: {err}"),
        }
        assert_eq!(
            spec, original,
            "direct validation must not mutate its public model"
        );
        assert!(!spec.workflows[0].steps[0].parameters[0]
            .reference
            .is_empty());
    }

    #[test]
    fn direct_validation_component_reference_negative_matrix() {
        let mut spec = direct_component_reference_document(true, true);
        let operation_duplicate = spec.workflows[0].steps[0].parameters[0].clone();
        spec.workflows[0].steps[0]
            .parameters
            .push(operation_duplicate);
        let action_duplicate = spec.workflows[0].steps[0].on_success[0].parameters[0].clone();
        spec.workflows[0].steps[0].on_success[0]
            .parameters
            .push(action_duplicate);
        let workflow_duplicate = spec.workflows[0].steps[1].parameters[0].clone();
        spec.workflows[0].steps[1]
            .parameters
            .push(workflow_duplicate);
        let original = spec.clone();
        let validate_errors = expect_validation_errors(validate(&spec));
        let diagnostic_errors = match validate_diagnostics(&spec) {
            Err(Error::Validation(report)) => report.errors,
            Ok(_) => panic!("direct validate_diagnostics must reject invalid component contexts"),
            Err(other) => panic!("expected validation diagnostics, got: {other}"),
        };
        assert_eq!(validate_errors, diagnostic_errors);
        assert_eq!(
            spec, original,
            "direct validation must not mutate its public model"
        );

        for path in [
            "workflow \"parent\" > step \"operation\".parameters[0].in",
            "workflow \"parent\" > step \"operation\".onSuccess[0].parameters[0].in",
            "workflow \"parent\" > step \"workflow\".parameters[0].in",
        ] {
            assert!(
                diagnostic_errors.iter().any(|error| {
                    error.path == path
                        && error.kind == ValidationErrorKind::InvalidParameterLocation
                }),
                "missing direct API diagnostic {path}: {diagnostic_errors:?}"
            );
        }
        for path in [
            "workflow \"parent\" > step \"operation\".parameters[1].name",
            "workflow \"parent\" > step \"operation\".onSuccess[0].parameters[1].name",
            "workflow \"parent\" > step \"workflow\".parameters[1].name",
        ] {
            assert!(
                diagnostic_errors.iter().any(|error| {
                    error.path == path && error.kind == ValidationErrorKind::DuplicateIdentifier
                }),
                "missing direct API identity diagnostic {path}: {diagnostic_errors:?}"
            );
        }
    }

