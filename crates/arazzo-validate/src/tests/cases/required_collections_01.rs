    fn required_lists_spec(source_descriptions: &str, workflows: &str) -> String {
        format!(
            r#"arazzo: "1.1.0"
info:
  title: Required lists
  version: "1.0.0"
{source_descriptions}{workflows}
"#
        )
    }

    fn assert_missing_required_path(yaml: &str, path: &str) {
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected {path} to fail validation");
        };
        let Some(error) = report.errors.iter().find(|error| error.path == path) else {
            panic!("expected a diagnostic at {path}, got {:?}", report.errors);
        };
        assert_eq!(error.kind, ValidationErrorKind::MissingRequiredField);
        assert_eq!(error.severity, Severity::Error);
    }

    #[test]
    fn required_source_descriptions_and_workflows_reject_empty_and_absent_lists() {
        let valid_workflows = "workflows:\n  - workflowId: wf1\n    steps: []\n";
        let valid_sources =
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n";

        for source_descriptions in ["sourceDescriptions: []\n", ""] {
            let yaml = required_lists_spec(source_descriptions, valid_workflows);
            assert_missing_required_path(&yaml, "sourceDescriptions");
        }
        for workflows in ["workflows: []\n", ""] {
            let yaml = required_lists_spec(valid_sources, workflows);
            assert_missing_required_path(&yaml, "workflows");
        }
    }

    #[test]
    fn required_source_descriptions_and_workflows_report_both_absent_lists() {
        let yaml = required_lists_spec("", "");
        let Err(Error::Validation(report)) = parse_bytes(yaml.as_bytes()) else {
            panic!("expected both required lists to fail validation");
        };
        for path in ["sourceDescriptions", "workflows"] {
            assert!(
                report.errors.iter().any(|error| {
                    error.path == path
                        && error.kind == ValidationErrorKind::MissingRequiredField
                        && error.severity == Severity::Error
                }),
                "expected required-list error at {path}, got {:?}",
                report.errors
            );
        }
    }

    #[test]
    fn required_lists_null_values_remain_parse_errors() {
        let source_null = required_lists_spec(
            "sourceDescriptions: null\n",
            "workflows:\n  - workflowId: wf1\n    steps: []\n",
        );
        let workflow_null = required_lists_spec(
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n",
            "workflows: null\n",
        );
        let steps_null = required_lists_spec(
            "sourceDescriptions:\n  - name: api\n    url: https://example.com\n    type: openapi\n",
            "workflows:\n  - workflowId: wf1\n    steps: null\n",
        );

        for yaml in [source_null, workflow_null, steps_null] {
            assert!(
                matches!(expect_parse_error(yaml.as_bytes()), Error::ParseYaml(_)),
                "expected typed null to remain a parseYaml error"
            );
        }
    }

