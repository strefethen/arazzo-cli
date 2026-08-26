    #[test]
    fn validate_valid_spec() {
        let spec = valid_spec();
        if let Err(err) = validate(&spec) {
            panic!("expected no error for valid spec, got: {err}");
        }
    }

    #[test]
    fn validate_missing_version() {
        let mut spec = valid_spec();
        spec.arazzo.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("arazzo version is required"));
    }

    #[test]
    fn validate_unsupported_version() {
        let mut spec = valid_spec();
        spec.arazzo = "2.0.0".to_string();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::UnsupportedVersion);
        assert!(errs[0].message.contains("unsupported arazzo version"));
    }

    #[test]
    fn validate_self_uri_accepts_absolute_and_relative_references() {
        for self_uri in [
            "https://api.example.com/workflows/purchase.arazzo.yaml",
            "workflows/purchase.arazzo.yaml",
            "../purchase.arazzo.yaml?revision=2",
        ] {
            let mut spec = valid_spec();
            spec.self_uri = Some(self_uri.to_string());
            if let Err(err) = validate(&spec) {
                panic!("expected valid $self URI-reference {self_uri:?}, got: {err}");
            }
        }
    }

    #[test]
    fn validate_self_uri_rejects_fragment_identifier() {
        let mut spec = valid_spec();
        spec.self_uri = Some("workflows/purchase.arazzo.yaml#purchase".to_string());
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert_eq!(errs[0].path, "$self");
        assert!(errs[0].message.contains("must not contain a fragment"));
    }

    #[test]
    fn validate_self_uri_rejects_invalid_uri_reference() {
        let mut spec = valid_spec();
        spec.self_uri = Some("https://example.com/%GG".to_string());
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidReference);
        assert_eq!(errs[0].path, "$self");
        assert!(errs[0].message.contains("valid RFC 3986 URI-reference"));
    }

    #[test]
    fn validate_missing_title() {
        let mut spec = valid_spec();
        spec.info.title.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("info.title is required"));
    }

    #[test]
    fn validate_missing_info_version() {
        let mut spec = valid_spec();
        spec.info.version.clear();
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0].message.contains("info.version is required"));
    }

