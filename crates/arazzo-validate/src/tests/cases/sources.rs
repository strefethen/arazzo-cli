    #[test]
    fn validate_source_duplicate_name() {
        let mut spec = valid_spec();
        spec.source_descriptions.push(SourceDescription {
            name: "api".to_string(),
            url: "https://other.example.com".to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        });
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::DuplicateIdentifier);
        assert!(errs[0].message.contains("is duplicate"));
    }

    #[test]
    fn validate_source_missing_url() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].url.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains("sourceDescriptions[0].url is required"));
    }

    #[test]
    fn parse_bytes_source_invalid_type() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: invalid
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for invalid source type"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("parsing arazzo yaml") {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn validate_source_type_arazzo() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::Arazzo;
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error, got: {err}");
        }
    }

