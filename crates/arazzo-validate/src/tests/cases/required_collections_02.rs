    #[test]
    fn workflow_steps_are_required_but_empty_steps_are_valid() {
        let source =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";
        let absent_steps = required_lists_spec(
            source,
            "workflows:\n  - workflowId: wf1\n    operationPath: /test\n",
        );
        assert_missing_required_path(&absent_steps, "workflow \"wf1\".steps");

        let empty_steps =
            required_lists_spec(source, "workflows:\n  - workflowId: wf1\n    steps: []\n");
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(empty_steps.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("steps: [] must remain valid, got: {err}"),
        };
        assert!(spec.workflows[0].steps.is_empty());
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    #[test]
    fn anonymous_workflow_missing_steps_uses_indexed_path() {
        let source =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";
        let yaml = required_lists_spec(source, "workflows:\n  - operationPath: /test\n");
        assert_missing_required_path(&yaml, "workflows[0].steps");
    }

    #[test]
    fn fully_populated_document_remains_clean() {
        let (spec, diagnostics) = match parse_bytes_with_diagnostics(VALID_YAML.as_bytes()) {
            Ok(value) => value,
            Err(err) => panic!("fully populated document must remain valid, got: {err}"),
        };
        assert_eq!(spec.source_descriptions.len(), 1);
        assert_eq!(spec.workflows.len(), 1);
        assert_eq!(spec.workflows[0].steps.len(), 1);
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    /// `successCriteria` absent, populated, bare-null, explicit-null, tilde,
    /// and empty, exercised through
    /// `parse_bytes` — the only entry point that can see the distinction
    /// between an absent key and an explicit `[]`. `Step.success_criteria` is
    /// `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so both
    /// collapse to the same typed `Vec::new()`; `validate(&ArazzoSpec)` /
    /// `validate_diagnostics(&ArazzoSpec)` cannot enforce this rule for the
    /// same reason (see the comment on `check_raw_success_criteria`).
    #[test]
    fn success_criteria_absent_and_populated_are_valid_empty_is_rejected() {
        fn spec_with(success_criteria: &str) -> String {
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
{success_criteria}
"#
            )
        }

        // Absent: no `successCriteria` key at all.
        if let Err(err) = parse_bytes(spec_with("").as_bytes()) {
            panic!("absent successCriteria must validate, got: {err}");
        }

        // Populated: at least one Criterion Object.
        let populated =
            spec_with("        successCriteria:\n          - condition: $statusCode == 200");
        if let Err(err) = parse_bytes(populated.as_bytes()) {
            panic!("populated successCriteria must validate, got: {err}");
        }

        // Bare null: a present key with no value is still provided and must
        // contain at least one Criterion Object.
        let bare_null = spec_with("        successCriteria:");
        let Err(Error::Validation(report)) = parse_bytes(bare_null.as_bytes()) else {
            panic!("expected bare successCriteria: to fail validation");
        };
        assert!(
            report.errors.iter().any(|item| {
                item.kind == ValidationErrorKind::MissingRequiredField
                    && item.path.ends_with(".successCriteria")
            }),
            "errors={:?}",
            report.errors
        );

        // Explicit null spellings fail during typed parsing and must remain
        // rejected even though the raw check handles the implicit null above.
        for spelling in ["null", "~"] {
            let explicit_null = spec_with(&format!("        successCriteria: {spelling}"));
            assert!(
                parse_bytes(explicit_null.as_bytes()).is_err(),
                "expected successCriteria: {spelling} to be rejected"
            );
        }

        // Empty: Step Object — "If successCriteria is provided, it MUST
        // contain at least one Criterion Object."
        let empty = spec_with("        successCriteria: []");
        let Err(Error::Validation(report)) = parse_bytes(empty.as_bytes()) else {
            panic!("expected successCriteria: [] to fail validation");
        };
        assert!(
            report
                .errors
                .iter()
                .any(|item| item.path.ends_with(".successCriteria")),
            "errors={:?}",
            report.errors
        );
    }

