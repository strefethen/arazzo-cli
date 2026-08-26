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
      name: retryPolicy
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
        assert_eq!(
            actions[0].retry_after, 10.0,
            "retryAfter overridden locally"
        );
    }

    #[test]
    fn legacy_component_retry_zero_values_keep_compatibility() {
        // `ac-80a8f` owns the unresolved distinction between an omitted
        // retryAfter and a legacy local `retryAfter: 0`. Preserve the shipped
        // merge behavior here; raw presence is used only for fixed-field
        // applicability diagnostics in this ticket.
        let spec_yaml = r#"
arazzo: "1.0.0"
info: {title: Test, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  failureActions:
    retryPolicy: {name: retryPolicy, type: retry, retryAfter: 2, retryLimit: 5}
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
            retryAfter: 0
            retryLimit: 0
"#;

        let (spec, diagnostics) = match parse_bytes_with_diagnostics(spec_yaml.as_bytes()) {
            Ok(result) => result,
            Err(error) => panic!("failure retry zero values must remain valid: {error}"),
        };
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
        assert_eq!(
            spec.workflows[0].steps[0].on_failure[0].retry_after, 2.0,
            "legacy retryAfter: 0 remains the historical no-override form"
        );
        assert_eq!(
            spec.workflows[0].steps[0].on_failure[0].retry_limit,
            Some(0),
            "legacy retryLimit: 0 remains an explicit valid retry limit"
        );
    }

    /// Scanner-visible positive evidence covers direct and component decimal
    /// retryAfter declarations only. Legacy name-form resolution is a
    /// compatibility extension, not conformance evidence.
    pub(super) fn decimal_retry_after_positive_matrix() {
        for (position, retry_after) in [
            ("component failure", "0"),
            ("component failure", "0.25"),
            ("workflow failure", "1"),
            ("workflow failure", "1.75"),
            ("step failure", "0"),
            ("step failure", "0.25"),
            ("step failure", "1.75"),
        ] {
            let action = format!(r#"{{name: retry, type: retry, retryAfter: {retry_after}}}"#);
            let document = action_position_document(position, &action, false);
            if let Err(error) = parse_bytes(document.as_bytes()) {
                panic!("{position} retryAfter={retry_after} must validate: {error}");
            }
        }
    }

    #[test]
    fn legacy_decimal_retry_after_resolution_keeps_compatibility() {
        let component =
            legacy_action_fixed_field_document("failure", "retry", "      retryAfter: 0.25", "");
        let inherited = match parse_bytes(component.as_bytes()) {
            Ok(spec) => spec,
            Err(error) => panic!("fractional component retryAfter must validate: {error}"),
        };
        assert_eq!(
            inherited.workflows[0].steps[0].on_failure[0].retry_after, 0.25,
            "legacy reference must inherit a fractional component retryAfter"
        );

        let local_override = legacy_action_fixed_field_document(
            "failure",
            "retry",
            "      retryAfter: 0.25",
            "            retryAfter: 1.75",
        );
        let overridden = match parse_bytes(local_override.as_bytes()) {
            Ok(spec) => spec,
            Err(error) => panic!("fractional legacy local override must validate: {error}"),
        };
        assert_eq!(
            overridden.workflows[0].steps[0].on_failure[0].retry_after, 1.75,
            "nonzero legacy decimal retryAfter must override its component"
        );

        let local_zero = legacy_action_fixed_field_document(
            "failure",
            "retry",
            "      retryAfter: 0.25",
            "            retryAfter: 0",
        );
        let inherited_zero = match parse_bytes(local_zero.as_bytes()) {
            Ok(spec) => spec,
            Err(error) => panic!("legacy retryAfter: 0 must remain compatible: {error}"),
        };
        assert_eq!(
            inherited_zero.workflows[0].steps[0].on_failure[0].retry_after, 0.25,
            "legacy retryAfter: 0 remains the no-override sentinel"
        );
    }

    /// Scanner-visible negative evidence covers direct and component decimal
    /// declarations only. Legacy overlays and ignored reference siblings stay
    /// in ordinary compatibility tests below.
    pub(super) fn decimal_retry_after_negative_matrix() {
        for (position, retry_after) in [
            ("component failure", "-0.25"),
            ("workflow failure", "-0.25"),
            ("step failure", "-0.25"),
            ("component failure", ".nan"),
            ("workflow failure", ".nan"),
            ("step failure", ".nan"),
            ("component failure", ".inf"),
            ("workflow failure", ".inf"),
            ("step failure", ".inf"),
            ("component failure", "-.inf"),
            ("workflow failure", "-.inf"),
            ("step failure", "-.inf"),
        ] {
            let action = format!(r#"{{name: retry, type: retry, retryAfter: {retry_after}}}"#);
            let document = action_position_document(position, &action, false);
            assert_exact_action_errors(
                &document,
                &[(
                    &format!("{}.retryAfter", action_position_path(position)),
                    ValidationErrorKind::InvalidRetryField,
                )],
            );
        }
    }

    #[test]
    fn legacy_decimal_retry_after_validation_and_reference_siblings_keep_compatibility() {
        for retry_after in ["-0.25", ".nan", ".inf", "-.inf"] {
            let component = legacy_action_fixed_field_document(
                "failure",
                "retry",
                &format!("      retryAfter: {retry_after}"),
                "",
            );
            assert_exact_action_errors(
                &component,
                &[(
                    "components.failureActions.policy.retryAfter",
                    ValidationErrorKind::InvalidRetryField,
                )],
            );

            let local = legacy_action_fixed_field_document(
                "failure",
                "retry",
                "      retryAfter: 0.25",
                &format!("            retryAfter: {retry_after}"),
            );
            assert_exact_action_errors(
                &local,
                &[(
                    "workflow \"wf\" > step \"s\".onFailure[0].retryAfter",
                    ValidationErrorKind::InvalidRetryField,
                )],
            );
        }

        for retry_after in [".nan", "-0.25"] {
            let local_zero = legacy_action_fixed_field_document(
                "failure",
                "retry",
                &format!("      retryAfter: {retry_after}"),
                "            retryAfter: 0",
            );
            assert_exact_action_errors(
                &local_zero,
                &[(
                    "components.failureActions.policy.retryAfter",
                    ValidationErrorKind::InvalidRetryField,
                )],
            );
        }

        let canonical_sibling = r#"
arazzo: "1.1.0"
info: {title: Canonical sibling, version: "1"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  failureActions:
    retry: {name: retry, type: retry, retryAfter: 0.25}
workflows:
  - workflowId: wf
    steps:
      - stepId: step
        operationPath: /test
        onFailure:
          - reference: $components.failureActions.retry
            retryAfter: .nan
"#;
        if let Err(error) = parse_bytes(canonical_sibling.as_bytes()) {
            panic!("canonical reusable siblings must remain ignored: {error}");
        }
    }

    fn legacy_action_fixed_field_document(
        kind: &str,
        component_type: &str,
        component_fields: &str,
        local_fields: &str,
    ) -> String {
        let (component_section, action_list) = match kind {
            "success" => ("successActions", "onSuccess"),
            "failure" => ("failureActions", "onFailure"),
            other => panic!("unknown action kind {other}"),
        };
        format!(
            r#"arazzo: "1.1.0"
info: {{title: Test, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
components:
  {component_section}:
    policy:
      name: policy
      type: {component_type}
{component_fields}
workflows:
  - workflowId: wf
    steps:
      - stepId: s
        operationPath: /test
        {action_list}:
          - name: "$components.{component_section}.policy"
{local_fields}
"#
        )
    }

    fn assert_exact_action_errors(document: &str, expected: &[(&str, ValidationErrorKind)]) {
        let report = action_validation_report(document);
        assert_eq!(
            report.errors.len(),
            expected.len(),
            "unexpected diagnostics: {:?}",
            report.errors
        );
        for (error, (path, kind)) in report.errors.iter().zip(expected) {
            assert_eq!(error.path, *path, "errors={:?}", report.errors);
            assert_eq!(error.kind, *kind, "errors={:?}", report.errors);
        }
    }

    #[test]
    fn legacy_inherited_end_and_goto_reject_local_zero_retry_fields() {
        for (kind, component_type, component_fields, list) in [
            ("success", "end", "", "onSuccess"),
            ("success", "goto", "      stepId: s", "onSuccess"),
            ("failure", "end", "", "onFailure"),
            ("failure", "goto", "      stepId: s", "onFailure"),
        ] {
            let document = legacy_action_fixed_field_document(
                kind,
                component_type,
                component_fields,
                "            retryAfter: 0\n            retryLimit: 0",
            );
            let base = format!("workflow \"wf\" > step \"s\".{list}[0]");
            let retry_after = format!("{base}.retryAfter");
            let retry_limit = format!("{base}.retryLimit");
            assert_exact_action_errors(
                &document,
                &[
                    (&retry_after, ValidationErrorKind::InvalidRetryField),
                    (&retry_limit, ValidationErrorKind::InvalidRetryField),
                ],
            );
        }
    }

    #[test]
    fn legacy_inherited_retry_rejects_local_null_retry_limit_once() {
        let document = legacy_action_fixed_field_document(
            "failure",
            "retry",
            "",
            "            retryLimit: null",
        );
        assert_exact_action_errors(
            &document,
            &[(
                "workflow \"wf\" > step \"s\".onFailure[0].retryLimit",
                ValidationErrorKind::InvalidRetryField,
            )],
        );
    }

    #[test]
    fn legacy_inherited_end_rejects_local_empty_targets_for_both_kinds() {
        for (kind, list) in [("success", "onSuccess"), ("failure", "onFailure")] {
            let document = legacy_action_fixed_field_document(
                kind,
                "end",
                "",
                "            workflowId: \"\"\n            stepId: \"\"",
            );
            let base = format!("workflow \"wf\" > step \"s\".{list}[0]");
            let workflow_id = format!("{base}.workflowId");
            let step_id = format!("{base}.stepId");
            assert_exact_action_errors(
                &document,
                &[
                    (&workflow_id, ValidationErrorKind::InvalidReference),
                    (&step_id, ValidationErrorKind::InvalidReference),
                ],
            );
        }
    }

    #[test]
    fn legacy_local_type_override_checks_inherited_default_field_presence() {
        let retry_fields = legacy_action_fixed_field_document(
            "failure",
            "retry",
            "      retryAfter: 0\n      retryLimit: 0",
            "            type: end",
        );
        assert_exact_action_errors(
            &retry_fields,
            &[
                (
                    "workflow \"wf\" > step \"s\".onFailure[0].retryAfter",
                    ValidationErrorKind::InvalidRetryField,
                ),
                (
                    "workflow \"wf\" > step \"s\".onFailure[0].retryLimit",
                    ValidationErrorKind::InvalidRetryField,
                ),
            ],
        );

        let empty_target = legacy_action_fixed_field_document(
            "failure",
            "retry",
            "      workflowId: \"\"",
            "            type: end",
        );
        assert_exact_action_errors(
            &empty_target,
            &[(
                "workflow \"wf\" > step \"s\".onFailure[0].workflowId",
                ValidationErrorKind::InvalidReference,
            )],
        );
    }

    #[test]
    fn parse_bytes_component_action_explicit_end_overrides_component_type() {
        // Component defines retry; the local reference explicitly sets type: end.
        // The explicit end must survive the merge instead of being treated as
        // an omitted (default) type, but its inherited retry fields then make
        // the effective Failure Action Object invalid.
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
      name: retryPolicy
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

        let mut resolved = parse_unvalidated(spec_yaml);
        if let Err(error) = super::resolve_components(&mut resolved, None) {
            panic!("expected component resolution to preserve the local type: {error}");
        }
        let actions = &resolved.workflows[0].steps[0].on_failure;
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].action_type(),
            ActionType::End,
            "explicit type: end must override the component's retry"
        );

        let report = action_validation_report(spec_yaml);
        assert!(
            report.errors.iter().any(|error| {
                error.path == "workflow \"wf1\" > step \"s1\".onFailure[0].retryAfter"
                    && error.kind == ValidationErrorKind::InvalidRetryField
            }),
            "errors={:?}",
            report.errors
        );
        assert!(
            report.errors.iter().any(|error| {
                error.path == "workflow \"wf1\" > step \"s1\".onFailure[0].retryLimit"
                    && error.kind == ValidationErrorKind::InvalidRetryField
            }),
            "errors={:?}",
            report.errors
        );
    }

