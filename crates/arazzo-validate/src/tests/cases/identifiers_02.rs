    /// The Goal probe document: after this ticket it fails, reporting both
    /// violations together in the same run — Design's "report every
    /// violation, not the first" bullet.
    #[test]
    fn goal_probe_document_reports_both_violations_together() {
        let yaml = r#"arazzo: 1.1.0
info:
  title: Gap probe
  version: 1.0.0
sourceDescriptions:
  - name: probe-api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: probe
    steps:
      - stepId: getThing
        operationPath: /things
        successCriteria: []
        outputs:
          "bad key!": $response.body
"#;
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected the probe document to fail validation");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.kind == ValidationErrorKind::InvalidIdentifier),
            "missing InvalidIdentifier error; errors={:?}",
            report.errors
        );
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.path.ends_with(".successCriteria")),
            "missing successCriteria error; errors={:?}",
            report.errors
        );
        assert_eq!(report.errors.len(), 2, "errors={:?}", report.errors);
    }

    /// A document whose only violations are SHOULD-level identifier
    /// positions (`workflowId`, `stepId`, `sourceDescriptions[].name`) must
    /// still validate: this is ac-0379b's scope, and rejecting them would
    /// collapse the MUST/SHOULD split and make `validate` refuse
    /// specification-conformant documents.
    #[test]
    fn should_level_identifier_violations_alone_still_validate() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: "not an identifier!"
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "not an identifier!"
    steps:
      - stepId: "not an identifier!"
        operationPath: /test
"#;
        if let Err(err) = parse_bytes(yaml.as_bytes()) {
            panic!("SHOULD-level-only violations must not fail validation, got: {err}");
        }
    }

    /// Table-driven SHOULD-level identifier check (ac-0379b Acceptance
    /// Criteria): each of the three `IdentifierClass::Identifier` definition
    /// sites produces a warning — never an error — when the value violates
    /// `^[A-Za-z0-9_\-]+$`, and stays silent when the value is legal
    /// (including `-` and `_`).
    #[test]
    fn should_level_identifier_positions_warn_on_invalid_and_stay_silent_on_valid() {
        const VALID_ID: &str = "Az9-_";
        const INVALID_ID: &str = "not identifier shaped";

        type PositionBuilder = fn(&str) -> String;
        let positions: &[(&str, PositionBuilder)] = &[
            ("workflowId", workflow_id_spec),
            ("stepId", step_id_spec),
            ("sourceDescriptions[].name", source_description_name_spec),
        ];

        for (label, build) in positions {
            let valid_yaml = build(VALID_ID);
            let (_, warnings) = match parse_bytes_with_diagnostics(valid_yaml.as_bytes()) {
                Ok(result) => result,
                Err(err) => {
                    panic!("{label}: expected {VALID_ID:?} to validate cleanly, got: {err}")
                }
            };
            assert!(
                !warnings
                    .iter()
                    .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
                "{label}: expected no identifier warning for {VALID_ID:?}, got: {warnings:?}"
            );

            let invalid_yaml = build(INVALID_ID);
            let (_, warnings) = match parse_bytes_with_diagnostics(invalid_yaml.as_bytes()) {
                Ok(result) => result,
                Err(err) => {
                    panic!("{label}: a SHOULD violation must warn, not fail validation: {err}")
                }
            };
            assert!(
                warnings.iter().any(|w| w.kind
                    == ValidationErrorKind::InvalidIdentifier
                    && w.severity == Severity::Warning),
                "{label}: expected an invalidIdentifier warning for {INVALID_ID:?}, got: {warnings:?}"
            );
        }
    }

    /// Anchoring regression: `IdentifierClass::is_valid` walks every
    /// character with `chars().all(..)`, so it has no unanchored form to
    /// regress to — this proves the SHOULD-level check actually fires.
    #[test]
    fn should_level_bad_name_with_spaces_warns() {
        let yaml = workflow_id_spec("bad name with spaces!");
        let (_, warnings) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(result) => result,
            Err(err) => panic!("\"bad name with spaces!\" must warn, not fail validation: {err}"),
        };
        assert!(
            warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier
                    && w.path.contains("workflowId")),
            "warnings={warnings:?}"
        );
    }

    /// Pins the class difference against ac-4a71f's `DottedKey`: a `.` is
    /// illegal in the `Identifier` class (SHOULD-level `workflowId`) but
    /// legal in `DottedKey` (MUST-level `outputs` keys). The same literal
    /// value warns at one position and validates cleanly at the other.
    #[test]
    fn dot_in_identifier_warns_but_is_legal_in_dotted_key() {
        const VALUE_WITH_DOT: &str = "wf.1";

        let identifier_yaml = workflow_id_spec(VALUE_WITH_DOT);
        let (_, warnings) = match parse_bytes_with_diagnostics(identifier_yaml.as_bytes()) {
            Ok(result) => result,
            Err(err) => panic!("dotted workflowId must warn, not fail validation: {err}"),
        };
        assert!(
            warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
            "warnings={warnings:?}"
        );

        let dotted_key_yaml = step_outputs_spec(VALUE_WITH_DOT);
        if let Err(err) = parse_bytes(dotted_key_yaml.as_bytes()) {
            panic!("the same value as a DottedKey key must validate cleanly, got: {err}");
        }
    }

    /// Double-report guard: an empty `workflowId` produces only the existing
    /// "is required" error, never an additional character-class warning —
    /// `IdentifierClass::is_valid` rejects the empty string, but the empty
    /// branch must short-circuit before that check runs.
    #[test]
    fn empty_identifier_produces_only_the_required_error_no_identifier_warning() {
        let yaml = workflow_id_spec("");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected empty workflowId to fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::MissingRequiredField
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|w| w.kind == ValidationErrorKind::InvalidIdentifier),
            "warnings={:?}",
            report.warnings
        );
    }

    /// Double-report guard: a duplicate, invalid-shaped `workflowId` produces
    /// exactly one `DuplicateIdentifier` error (for the repeat) and exactly
    /// one `InvalidIdentifier` warning (for the first occurrence) — the
    /// duplicate branch takes precedence over the character-class check for
    /// the repeat, so the repeat is never reported twice.
    #[test]
    fn duplicate_invalid_shaped_identifier_is_not_double_reported() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: "bad id!"
    steps:
      - stepId: s1
        operationPath: /test
  - workflowId: "bad id!"
    steps:
      - stepId: s2
        operationPath: /test2
"#;
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected duplicate workflowId to fail validation");
        };
        let dup_errors = report
            .errors
            .iter()
            .filter(|e| e.kind == ValidationErrorKind::DuplicateIdentifier)
            .count();
        assert_eq!(dup_errors, 1, "errors={:?}", report.errors);

        let identifier_warnings = report
            .warnings
            .iter()
            .filter(|w| w.kind == ValidationErrorKind::InvalidIdentifier)
            .count();
        assert_eq!(
            identifier_warnings, 1,
            "expected exactly one identifier warning, from the first occurrence only; \
             warnings={:?}",
            report.warnings
        );
    }

    /// Pins the `components.<field>."<key>"` diagnostic path form: Components
    /// map keys sit outside any workflow or step, so the crate's
    /// `workflow "<id>" > step "<id>"` convention does not apply to them.
    #[test]
    fn components_path_uses_field_and_quoted_key_form() {
        let yaml = components_parameters_spec("bad key!");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected components.parameters bad key to fail validation");
        };
        let issue = match report
            .errors
            .iter()
            .find(|item| item.kind == ValidationErrorKind::InvalidIdentifier)
        {
            Some(issue) => issue,
            None => panic!(
                "expected an InvalidIdentifier error, got: {:?}",
                report.errors
            ),
        };
        assert_eq!(issue.path, "components.parameters.\"bad key!\"");
    }

