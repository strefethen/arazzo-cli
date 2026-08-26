    // --- ac-4a71f: MUST-level identifier and successCriteria rules ---

    fn workflow_outputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
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
    outputs:
      "{key}": $response.body
"#
        )
    }

    fn step_outputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
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
        outputs:
          "{key}": $response.body
"#
        )
    }

    fn components_inputs_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    "{key}":
      type: object
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_parameters_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    "{key}":
      name: q
      in: query
      value: v
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_success_actions_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    "{key}":
      name: action
      type: end
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn components_failure_actions_spec(key: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    "{key}":
      name: action
      type: end
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    /// Spec builders for ac-0379b's three SHOULD-level `IdentifierClass::Identifier`
    /// definition sites: Workflow Object `workflowId`, Step Object `stepId`, and
    /// `sourceDescriptions[].name`.
    fn workflow_id_spec(id: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "{id}"
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    fn step_id_spec(id: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: "{id}"
        operationPath: /test
"#
        )
    }

    fn source_description_name_spec(name: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: "{name}"
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#
        )
    }

    /// Table-driven MUST-level identifier check (Acceptance Criteria: every
    /// DottedKey position produces an error when violated, and stays valid
    /// when the key is legal). Covers all six positions in one pass: workflow
    /// outputs, step outputs, and the four Components maps.
    #[test]
    fn dotted_key_positions_accept_valid_and_reject_invalid_keys() {
        const VALID_KEY: &str = "Az9.-_";
        const INVALID_KEY: &str = "not identifier shaped";

        type PositionBuilder = fn(&str) -> String;
        let positions: &[(&str, PositionBuilder)] = &[
            ("workflow outputs", workflow_outputs_spec),
            ("step outputs", step_outputs_spec),
            ("components.inputs", components_inputs_spec),
            ("components.parameters", components_parameters_spec),
            ("components.successActions", components_success_actions_spec),
            ("components.failureActions", components_failure_actions_spec),
        ];

        for (label, build) in positions {
            let valid_yaml = build(VALID_KEY);
            if let Err(err) = parse_bytes(valid_yaml.as_bytes()) {
                panic!("{label}: expected {VALID_KEY:?} to validate cleanly, got: {err}");
            }

            let invalid_yaml = build(INVALID_KEY);
            let Err(Error::Validation(report)) = parse_bytes(invalid_yaml.as_bytes()) else {
                panic!("{label}: expected {INVALID_KEY:?} to fail validation");
            };
            assert!(
                report
                    .errors
                    .iter()
                    .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier),
                "{label}: expected an invalidIdentifier error, got: {:?}",
                report.errors
            );
        }
    }

    /// Anchoring regression: the literal `bad key!` from the specification's
    /// MUST clause (and the ticket's Goal probe document) must be rejected.
    /// `IdentifierClass::is_valid` walks every character with
    /// `chars().all(..)`, so it has no unanchored form to regress to — this
    /// test is what proves the check actually fires, not merely compiles.
    #[test]
    fn bad_key_exclamation_is_rejected() {
        let yaml = step_outputs_spec("bad key!");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected \"bad key!\" to be rejected");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier
                    && item.path.contains("bad key!")),
            "errors={:?}",
            report.errors
        );
    }

