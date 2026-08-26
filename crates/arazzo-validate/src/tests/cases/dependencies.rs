    #[test]
    fn validate_local_depends_on_accepts_existing_ids_and_rejects_missing_ids() {
        let mut spec = valid_spec();
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/second".to_string())),
            depends_on: vec!["s1".to_string()],
            ..Step::default()
        });
        if let Err(err) = validate(&spec) {
            panic!("existing local dependsOn ID should validate: {err}");
        }

        spec.workflows[0].steps[1].depends_on = vec!["missing".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with(".dependsOn[0]")
                && error.message.contains("unknown local step \"missing\"")
        }));
    }

    #[test]
    fn validate_workflow_depends_on_references_and_external_scope() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            depends_on: vec!["wf1".to_string()],
            steps: vec![Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/second".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        assert!(validate(&spec).is_ok());

        spec.workflows[1].depends_on = vec!["WF1".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with("dependsOn[0]")
        }));

        spec.workflows[1].depends_on = vec!["$sourceDescriptions.api.remote".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.path.ends_with("dependsOn[0]")
                && error.message.contains("expected type arazzo")
        }));

        spec.source_descriptions.push(SourceDescription {
            name: "shared".to_string(),
            url: "https://example.com/shared.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[1].depends_on = vec!["$sourceDescriptions.shared.remote".to_string()];
        let warnings = match validate_diagnostics(&spec) {
            Ok(warnings) => warnings,
            Err(err) => panic!("known Arazzo source should warn, not fail: {err}"),
        };
        assert!(warnings.iter().any(|warning| {
            warning.kind == ValidationErrorKind::UnsupportedDependencyScope
                && warning.severity == Severity::Warning
                && warning.path.ends_with("dependsOn[0]")
        }));
    }

    #[test]
    fn validate_workflow_depends_on_rejects_unknown_and_malformed_external_references() {
        let mut spec = valid_spec();
        spec.workflows[0].depends_on = vec![
            "$sourceDescriptions.missing.remote".to_string(),
            "$sourceDescriptions.bad name.ready".to_string(),
        ];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.message.contains("unknown sourceDescription")
        }));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::InvalidReference
                && error.message.contains("invalid workflow reference")
        }));

        spec.workflows[0].depends_on = vec!["$sourceDescriptions.api.ready.extra".to_string()];
        assert!(
            validate(&spec).is_err(),
            "OpenAPI source must remain unsupported"
        );

        spec.source_descriptions.push(SourceDescription {
            name: "shared".to_string(),
            url: "https://example.com/shared.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[0].depends_on = vec!["$sourceDescriptions.shared.ready.extra".to_string()];
        let warnings = match validate_diagnostics(&spec) {
            Ok(warnings) => warnings,
            Err(error) => panic!("known Arazzo source is warning-only: {error}"),
        };
        let warning = match warnings
            .iter()
            .find(|warning| warning.kind == ValidationErrorKind::UnsupportedDependencyScope)
        {
            Some(warning) => warning,
            None => panic!("expected external dependency warning"),
        };
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.clone().into_error().severity, Severity::Error);
    }

    #[test]
    fn validate_workflow_depends_on_cycles_are_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].depends_on = vec!["wf1".to_string()];
        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::DependencyCycle && error.path.ends_with("dependsOn")
        }));

        let mut two_node = valid_spec();
        two_node.workflows[0].depends_on = vec!["wf2".to_string()];
        two_node.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            depends_on: vec!["wf1".to_string()],
            steps: vec![Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/second".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        let errors = expect_validation_errors(validate(&two_node));
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.kind == ValidationErrorKind::DependencyCycle)
                .count(),
            2
        );
    }

    #[test]
    fn validate_local_depends_on_cycles_are_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].depends_on = vec!["s2".to_string()];
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/second".to_string())),
            depends_on: vec!["s1".to_string()],
            ..Step::default()
        });

        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::DependencyCycle
                && error.message.contains("dependency cycle")
        }));
    }

    #[test]
    fn validate_cross_scope_depends_on_references_fail_as_unsupported() {
        for dependency in [
            "$workflows.audit.steps.record",
            "$sourceDescriptions.external.archive.steps.store",
        ] {
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].depends_on = vec![dependency.to_string()];
            let errors = expect_validation_errors(validate(&spec));
            assert!(errors.iter().any(|error| {
                error.kind == ValidationErrorKind::UnsupportedDependencyScope
                    && error.path.ends_with(".dependsOn[0]")
                    && error.message.contains("valid Arazzo 1.1 syntax")
            }));
        }
    }

