    /// Scanner-visible positive evidence covers direct and component decimal
    /// retryAfter declarations.
    #[test]
    fn decimal_retry_after_positive_matrix() {
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

    /// Scanner-visible negative evidence covers direct and component decimal
    /// declarations.
    #[test]
    fn decimal_retry_after_negative_matrix() {
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
    fn canonical_action_reference_ignores_retry_after_siblings() {
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

