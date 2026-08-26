    /// Decision 1 (ac-bd441): the accepted `type`/`version` pairs from the
    /// Arazzo v1.1.0 §5.8.12.1 table, shared by every validation site.
    const SCHEMA_TYPE_VERSIONS: [(&str, &str); 7] = [
        ("jsonpath", "rfc9535"),
        ("jsonpath", "draft-goessner-dispatch-jsonpath-00"),
        ("xpath", "xpath-10"),
        ("xpath", "xpath-20"),
        ("xpath", "xpath-30"),
        ("xpath", "xpath-31"),
        ("jsonpointer", "rfc6901"),
    ];

    #[test]
    fn validate_selector_accepts_schema_type_version_combinations() {
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            let mut spec = valid_spec();
            spec.workflows[0].outputs = BTreeMap::from([(
                "selected".to_string(),
                OutputValue::Selector(SelectorObject {
                    context: "$inputs.document".to_string(),
                    selector: if type_name == "jsonpointer" {
                        "/items".to_string()
                    } else {
                        "$.items".to_string()
                    },
                    type_: SelectorType::ExpressionType(CriterionExpressionType {
                        type_: type_name.to_string(),
                        version: version.to_string(),
                        ..CriterionExpressionType::default()
                    }),
                    extensions: BTreeMap::new(),
                }),
            )]);

            if let Err(error) = validate(&spec) {
                panic!("expected {type_name} {version} selector to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_selector_rejects_unsupported_type_version_combination() {
        let mut spec = valid_spec();
        spec.workflows[0].outputs = BTreeMap::from([(
            "selected".to_string(),
            OutputValue::Selector(SelectorObject {
                context: "$inputs.document".to_string(),
                selector: "$.items".to_string(),
                type_: SelectorType::ExpressionType(CriterionExpressionType {
                    type_: "jsonpath".to_string(),
                    version: "xpath-10".to_string(),
                    ..CriterionExpressionType::default()
                }),
                extensions: BTreeMap::new(),
            }),
        )]);

        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
        assert!(errors[0].message.contains("not supported for jsonpath"));
    }

    #[test]
    fn validate_criterion_type_requires_context() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains("context is required when type is specified"));
    }

    #[test]
    fn validate_criterion_type_object_is_accepted() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            context: "$response.body".to_string(),
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
                ..CriterionExpressionType::default()
            })),
            ..SuccessCriterion::default()
        }];

        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error, got: {err}");
        }
    }

    #[test]
    fn validate_criterion_type_object_rejects_invalid_version() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
            context: "$response.body".to_string(),
            condition: "$.pets[0]".to_string(),
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "invalid-version".to_string(),
                ..CriterionExpressionType::default()
            })),
            ..SuccessCriterion::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidCriterionType);
        assert!(errs[0]
            .message
            .contains("type.version \"invalid-version\" is not supported for jsonpath"));
    }

    #[test]
    fn validate_criterion_accepts_schema_type_version_combinations() {
        // Decision 1: the criterion site shares the §5.8.12.1 table —
        // notably `rfc9535` and `xpath-31`, which it used to reject.
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            if type_name == "jsonpointer" {
                // Criterion object types are jsonpath | xpath only.
                continue;
            }
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].success_criteria = vec![SuccessCriterion {
                context: "$response.body".to_string(),
                condition: if type_name == "xpath" {
                    "//pets".to_string()
                } else {
                    "$.pets[0]".to_string()
                },
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: type_name.to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                })),
                ..SuccessCriterion::default()
            }];

            if let Err(error) = validate(&spec) {
                panic!("expected criterion {type_name} {version} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_selector_rejects_bare_number_xpath_versions() {
        // Decision 1 negative: the pre-ticket bare-number tokens are not in
        // the §5.8.12.1 table and no longer validate.
        for version in ["10", "30"] {
            let mut spec = valid_spec();
            spec.workflows[0].outputs = BTreeMap::from([(
                "selected".to_string(),
                OutputValue::Selector(SelectorObject {
                    context: "$inputs.document".to_string(),
                    selector: "//items".to_string(),
                    type_: SelectorType::ExpressionType(CriterionExpressionType {
                        type_: "xpath".to_string(),
                        version: version.to_string(),
                        ..CriterionExpressionType::default()
                    }),
                    extensions: BTreeMap::new(),
                }),
            )]);

            let errors = expect_validation_errors(validate(&spec));
            assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
            assert!(
                errors[0].message.contains("not supported for xpath"),
                "{version}: {}",
                errors[0].message
            );
        }
    }

    fn replacement_with_type(type_: SelectorType, target: &str) -> Replacement {
        Replacement {
            target: target.to_string(),
            target_selector_type: Some(type_),
            value: serde_yaml_ng::Value::String("v".to_string()).into(),
            ..Replacement::default()
        }
    }

    fn spec_with_replacement(replacement: Replacement) -> ArazzoSpec {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].request_body = Some(RequestBody {
            replacements: vec![replacement],
            ..RequestBody::default()
        });
        spec
    }

    #[test]
    fn validate_target_selector_type_accepts_schema_type_version_combinations() {
        for (type_name, version) in SCHEMA_TYPE_VERSIONS {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::ExpressionType(CriterionExpressionType {
                    type_: type_name.to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                }),
                "/foo",
            ));

            if let Err(error) = validate(&spec) {
                panic!("expected targetSelectorType {type_name} {version} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_target_selector_type_accepts_plain_names() {
        // Decision 3: plain `xpath` (meaning XPath 3.1) is conformant Arazzo
        // and must validate even though the runtime engine is XPath 1.0.
        for name in ["jsonpath", "xpath", "jsonpointer"] {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::Name(name.to_string()),
                "/foo",
            ));

            if let Err(error) = validate(&spec) {
                panic!("expected targetSelectorType {name} to validate: {error}");
            }
        }
    }

    #[test]
    fn validate_target_selector_type_rejects_unknown_name() {
        let spec = spec_with_replacement(replacement_with_type(
            SelectorType::Name("regex".to_string()),
            "/foo",
        ));

        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
        assert!(
            errors[0].path.contains("targetSelectorType"),
            "{}",
            errors[0].path
        );
        assert!(errors[0]
            .message
            .contains("must be one of jsonpath, xpath, or jsonpointer"));
    }

    #[test]
    fn validate_target_selector_type_rejects_bare_number_and_unknown_versions() {
        for version in ["10", "30", "rfc9536"] {
            let spec = spec_with_replacement(replacement_with_type(
                SelectorType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: version.to_string(),
                    ..CriterionExpressionType::default()
                }),
                "//foo",
            ));

            let errors = expect_validation_errors(validate(&spec));
            assert_eq!(errors[0].kind, ValidationErrorKind::InvalidSelectorType);
            assert!(
                errors[0].message.contains("not supported for xpath"),
                "{version}: {}",
                errors[0].message
            );
        }
    }

    #[test]
    fn validate_action_criteria_follow_criterion_rules() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            criteria: vec![SuccessCriterion {
                condition: "//item[1]".to_string(),
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: "xpath".to_string(),
                    version: "xpath-10".to_string(),
                    ..CriterionExpressionType::default()
                })),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }];

        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::MissingRequiredField);
        assert!(errs[0]
            .message
            .contains(".onFailure[0].criteria[0].context is required"));
    }

    #[test]
    fn validate_multiple_errors() {
        let spec = ArazzoSpec::default();
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs.len() >= 2);
        let messages: Vec<&str> = errs.iter().map(|e| e.message.as_str()).collect();
        assert!(messages
            .iter()
            .any(|m| m.contains("arazzo version is required")));
        assert!(messages
            .iter()
            .any(|m| m.contains("info.title is required")));
    }

