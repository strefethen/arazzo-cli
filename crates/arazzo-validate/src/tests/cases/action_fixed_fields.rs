    fn action_position_document(position: &str, action: &str, json: bool) -> String {
        if json {
            let document = match position {
                "component success" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"components":{{"successActions":{{"component":{action}}}}},"workflows":[{{"workflowId":"wf","steps":[]}}]}}"#
                ),
                "component failure" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"components":{{"failureActions":{{"component":{action}}}}},"workflows":[{{"workflowId":"wf","steps":[]}}]}}"#
                ),
                "workflow success" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"workflows":[{{"workflowId":"wf","successActions":[{action}],"steps":[]}}]}}"#
                ),
                "workflow failure" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"workflows":[{{"workflowId":"wf","failureActions":[{action}],"steps":[]}}]}}"#
                ),
                "step success" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"workflows":[{{"workflowId":"wf","steps":[{{"stepId":"step","operationPath":"/test","onSuccess":[{action}]}}]}}]}}"#
                ),
                "step failure" => format!(
                    r#"{{"arazzo":"1.1.0","info":{{"title":"T","version":"1"}},"sourceDescriptions":[{{"name":"api","url":"https://example.com","type":"openapi"}}],"workflows":[{{"workflowId":"wf","steps":[{{"stepId":"step","operationPath":"/test","onFailure":[{action}]}}]}}]}}"#
                ),
                other => panic!("unknown action position {other}"),
            };
            return document;
        }

        match position {
            "component success" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\ncomponents:\n  successActions:\n    component: {action}\nworkflows:\n  - workflowId: wf\n    steps: []\n"
            ),
            "component failure" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\ncomponents:\n  failureActions:\n    component: {action}\nworkflows:\n  - workflowId: wf\n    steps: []\n"
            ),
            "workflow success" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\nworkflows:\n  - workflowId: wf\n    successActions: [{action}]\n    steps: []\n"
            ),
            "workflow failure" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\nworkflows:\n  - workflowId: wf\n    failureActions: [{action}]\n    steps: []\n"
            ),
            "step success" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: step\n        operationPath: /test\n        onSuccess: [{action}]\n"
            ),
            "step failure" => format!(
                "arazzo: \"1.1.0\"\ninfo: {{title: T, version: \"1\"}}\nsourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: step\n        operationPath: /test\n        onFailure: [{action}]\n"
            ),
            other => panic!("unknown action position {other}"),
        }
    }

    fn action_position_path(position: &str) -> &'static str {
        match position {
            "component success" => "components.successActions.component",
            "component failure" => "components.failureActions.component",
            "workflow success" => "workflow \"wf\".successActions[0]",
            "workflow failure" => "workflow \"wf\".failureActions[0]",
            "step success" => "workflow \"wf\" > step \"step\".onSuccess[0]",
            "step failure" => "workflow \"wf\" > step \"step\".onFailure[0]",
            other => panic!("unknown action position {other}"),
        }
    }

    fn action_validation_report(document: &str) -> super::ValidationReport {
        let Err(Error::Validation(report)) = parse_bytes(document.as_bytes()) else {
            panic!("expected action validation failure for document:\n{document}");
        };
        report
    }

    fn assert_action_error(document: &str, path: &str, kind: ValidationErrorKind) {
        let report = action_validation_report(document);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.path == path && error.kind == kind),
            "expected {kind:?} at {path}; errors={:?}",
            report.errors
        );
    }

    /// Scanner-visible positive evidence exercises every concrete container
    /// in both YAML and JSON. An empty action name is valid; raw validation
    /// cares about its string shape and presence, not its length.
    pub(super) fn action_fixed_fields_positive_matrix() {
        const POSITIONS: [&str; 6] = [
            "component success",
            "component failure",
            "workflow success",
            "workflow failure",
            "step success",
            "step failure",
        ];
        for json in [false, true] {
            for position in POSITIONS {
                let document =
                    action_position_document(position, r#"{"name":"","type":"end"}"#, json);
                if let Err(error) = parse_bytes(document.as_bytes()) {
                    panic!("json={json} position={position}: {error}");
                }
            }
        }

        for (position, action) in [
            (
                "step success",
                r#"{"name":"goto","type":"goto","stepId":"step"}"#,
            ),
            (
                "step failure",
                r#"{"name":"goto","type":"goto","stepId":"step"}"#,
            ),
            (
                "step failure",
                r#"{"name":"retry","type":"retry","retryAfter":0,"retryLimit":0}"#,
            ),
        ] {
            let document = action_position_document(position, action, false);
            if let Err(error) = parse_bytes(document.as_bytes()) {
                panic!("valid {position} action {action} failed: {error}");
            }
        }
    }

    /// Scanner-visible negative evidence covers raw absence/null shape,
    /// serde-owned malformed containers, type-field applicability, reusable
    /// exemptions, and component diagnostic provenance.
    pub(super) fn action_fixed_fields_negative_matrix() {
        const POSITIONS: [&str; 6] = [
            "component success",
            "component failure",
            "workflow success",
            "workflow failure",
            "step success",
            "step failure",
        ];
        for json in [false, true] {
            for position in POSITIONS {
                let path = action_position_path(position);
                for (field, action) in [
                    ("name", r#"{"type":"end"}"#),
                    ("name", r#"{"name":null,"type":"end"}"#),
                    ("type", r#"{"name":"action"}"#),
                    ("type", r#"{"name":"action","type":null}"#),
                ] {
                    assert_action_error(
                        &action_position_document(position, action, json),
                        &format!("{path}.{field}"),
                        ValidationErrorKind::MissingRequiredField,
                    );
                }

                for action in [
                    r#"[]"#,
                    r#"{"name":[],"type":"end"}"#,
                    r#"{"name":"action","type":[]}"#,
                ] {
                    let result =
                        parse_bytes(action_position_document(position, action, json).as_bytes());
                    assert!(
                        matches!(result, Err(Error::ParseYaml(_))),
                        "json={json} position={position} action={action}: malformed action shape must remain serde-owned, got {result:?}"
                    );
                }
            }
        }

        for (position, action, path, kind) in [
            (
                "step success",
                r#"{"name":"retry","type":"retry"}"#,
                "workflow \"wf\" > step \"step\".onSuccess[0].type",
                ValidationErrorKind::InvalidRetryField,
            ),
            (
                "step success",
                r#"{"name":"end","type":"end","retryAfter":0}"#,
                "workflow \"wf\" > step \"step\".onSuccess[0].retryAfter",
                ValidationErrorKind::InvalidRetryField,
            ),
            (
                "step success",
                r#"{"name":"end","type":"end","retryLimit":0}"#,
                "workflow \"wf\" > step \"step\".onSuccess[0].retryLimit",
                ValidationErrorKind::InvalidRetryField,
            ),
            (
                "step failure",
                r#"{"name":"end","type":"end","retryAfter":0}"#,
                "workflow \"wf\" > step \"step\".onFailure[0].retryAfter",
                ValidationErrorKind::InvalidRetryField,
            ),
            (
                "step failure",
                r#"{"name":"goto","type":"goto","retryLimit":0,"stepId":"step"}"#,
                "workflow \"wf\" > step \"step\".onFailure[0].retryLimit",
                ValidationErrorKind::InvalidRetryField,
            ),
            (
                "step success",
                r#"{"name":"end","type":"end","workflowId":""}"#,
                "workflow \"wf\" > step \"step\".onSuccess[0].workflowId",
                ValidationErrorKind::InvalidReference,
            ),
        ] {
            assert_action_error(
                &action_position_document(position, action, false),
                path,
                kind,
            );
        }

        let missing_type = action_position_document("step success", r#"{"name":"action"}"#, false);
        let parsed_missing_type = action_validation_report(&missing_type);
        assert_eq!(
            parsed_missing_type.errors.len(),
            1,
            "raw parse must own exactly one missing-type diagnostic: {:?}",
            parsed_missing_type.errors
        );
        assert_eq!(
            parsed_missing_type.errors[0].path,
            "workflow \"wf\" > step \"step\".onSuccess[0].type"
        );
        let direct_missing_type =
            expect_validation_errors(validate(&parse_unvalidated(&missing_type)));
        assert_eq!(
            direct_missing_type.len(),
            1,
            "direct validation must own the same defaulted type state: {direct_missing_type:?}"
        );
        assert_eq!(
            direct_missing_type[0].path,
            "workflow \"wf\" > step \"step\".onSuccess[0].type"
        );

        for reference in ["\"\"", "\"not-a-runtime-expression\"", "null"] {
            let document = action_position_document(
                "step success",
                &format!(r#"{{"reference":{reference},"type":"retry"}}"#),
                false,
            );
            let report = action_validation_report(&document);
            assert_eq!(
                report.errors.len(),
                1,
                "reference={reference}: canonical reference must produce one error, got {:?}",
                report.errors
            );
            assert_eq!(report.errors[0].kind, ValidationErrorKind::InvalidReference);
            assert_eq!(
                report.errors[0].path,
                "workflow \"wf\" > step \"step\".onSuccess[0]"
            );
        }

        let retry_limit_null = action_position_document(
            "step failure",
            r#"{"name":"retry","type":"retry","retryLimit":null}"#,
            false,
        );
        let report = action_validation_report(&retry_limit_null);
        assert_eq!(
            report.errors.len(),
            1,
            "retryLimit null must be diagnosed once, got {:?}",
            report.errors
        );
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::InvalidRetryField
        );
        assert_eq!(
            report.errors[0].path,
            "workflow \"wf\" > step \"step\".onFailure[0].retryLimit"
        );
        // `Option<u64>` collapses raw null to `None`; this parse-boundary
        // diagnostic is intentionally the only place that can reject it
        // without changing the public Action model.
        assert!(validate(&parse_unvalidated(&retry_limit_null)).is_ok());

        let retry_limit_zero = action_validation_report(&action_position_document(
            "step success",
            r#"{"name":"end","type":"end","retryLimit":0}"#,
            false,
        ));
        assert_eq!(
            retry_limit_zero.errors.len(),
            1,
            "raw zero retryLimit must not duplicate the typed applicability error: {:?}",
            retry_limit_zero.errors
        );

        let reusable = r#"arazzo: "1.1.0"
info: {title: T, version: "1"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    good: {name: good, type: end}
    bad: {name: bad, type: retry}
workflows:
  - workflowId: wf
    steps:
      - stepId: step
        operationPath: /test
        onSuccess:
          - reference: $components.successActions.good
            type: retry
          - name: $components.successActions.bad
"#;
        let report = action_validation_report(reusable);
        let component_errors = report
            .errors
            .iter()
            .filter(|error| {
                error.path == "components.successActions.bad.type"
                    && error.kind == ValidationErrorKind::InvalidRetryField
            })
            .collect::<Vec<_>>();
        assert_eq!(component_errors.len(), 1, "errors={:?}", report.errors);
        assert!(
            report
                .errors
                .iter()
                .all(|error| !error.path.contains("onSuccess")),
            "canonical and legacy reusable actions must not duplicate component diagnostics: {:?}",
            report.errors
        );

        let component_reference = r#"arazzo: "1.1.0"
info: {title: T, version: "1"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  successActions:
    invalid: {reference: $components.successActions.good, type: end}
workflows:
  - workflowId: wf
    steps: []
"#;
        assert_action_error(
            component_reference,
            "components.successActions.invalid.name",
            ValidationErrorKind::MissingRequiredField,
        );
    }
