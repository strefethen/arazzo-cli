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

