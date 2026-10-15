    #[test]
    fn validate_local_depends_on_accepts_existing_ids_and_rejects_missing_ids() {
        let mut spec = valid_spec();
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("{$sourceDescriptions.api.url}#/paths/~1second/get".to_string())),
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
                target: Some(StepTarget::OperationPath("{$sourceDescriptions.api.url}#/paths/~1second/get".to_string())),
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
                target: Some(StepTarget::OperationPath("{$sourceDescriptions.api.url}#/paths/~1second/get".to_string())),
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
            target: Some(StepTarget::OperationPath("{$sourceDescriptions.api.url}#/paths/~1second/get".to_string())),
            depends_on: vec!["s1".to_string()],
            ..Step::default()
        });

        let errors = expect_validation_errors(validate(&spec));
        assert!(errors.iter().any(|error| {
            error.kind == ValidationErrorKind::DependencyCycle
                && error.message.contains("dependency cycle")
        }));
    }

    const DEEP_CHAIN_LEN: usize = 20_000;

    /// Runs `body` on a thread with a fixed small stack, so a recursive
    /// traversal overflows regardless of the platform's default stack size.
    fn run_on_small_stack(body: impl FnOnce() + Send + 'static) {
        let handle = match std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(body)
        {
            Ok(handle) => handle,
            Err(err) => panic!("spawn small-stack thread: {err}"),
        };
        if handle.join().is_err() {
            panic!("deep-chain check panicked on the small-stack thread");
        }
    }

    fn deep_workflow_chain(back_edge: bool) -> ArazzoSpec {
        let mut spec = valid_spec();
        spec.workflows = (0..DEEP_CHAIN_LEN)
            .map(|index| Workflow {
                workflow_id: format!("wf{index}"),
                depends_on: if index + 1 < DEEP_CHAIN_LEN {
                    vec![format!("wf{}", index + 1)]
                } else if back_edge {
                    vec!["wf0".to_string()]
                } else {
                    Vec::new()
                },
                ..Workflow::default()
            })
            .collect();
        spec
    }

    fn deep_step_chain(back_edge: bool) -> Workflow {
        Workflow {
            workflow_id: "wf1".to_string(),
            steps: (0..DEEP_CHAIN_LEN)
                .map(|index| Step {
                    step_id: format!("s{index}"),
                    depends_on: if index + 1 < DEEP_CHAIN_LEN {
                        vec![format!("s{}", index + 1)]
                    } else if back_edge {
                        vec!["s0".to_string()]
                    } else {
                        Vec::new()
                    },
                    ..Step::default()
                })
                .collect(),
            ..Workflow::default()
        }
    }

    #[test]
    fn deep_workflow_depends_on_chain_does_not_overflow_the_stack() {
        run_on_small_stack(|| {
            let acyclic = deep_workflow_chain(false);
            assert!(!crate::step_dependencies::local_workflow_depends_on_has_cycle(&acyclic, 0));
            assert!(!crate::step_dependencies::local_workflow_depends_on_has_cycle(
                &acyclic,
                DEEP_CHAIN_LEN - 1
            ));

            let cyclic = deep_workflow_chain(true);
            for start in [0, DEEP_CHAIN_LEN / 2, DEEP_CHAIN_LEN - 1] {
                assert!(
                    crate::step_dependencies::local_workflow_depends_on_has_cycle(&cyclic, start),
                    "workflow {start} reaches the cycle"
                );
            }
        });
    }

    #[test]
    fn deep_step_depends_on_chain_does_not_overflow_the_stack() {
        run_on_small_stack(|| {
            assert!(!crate::step_dependencies::local_step_depends_on_has_cycle(&deep_step_chain(false)));
            assert!(crate::step_dependencies::local_step_depends_on_has_cycle(&deep_step_chain(true)));
        });
    }

    #[test]
    fn deep_step_depends_on_cycle_reports_one_diagnostic_per_workflow() {
        run_on_small_stack(|| {
            let mut spec = valid_spec();
            spec.workflows[0] = deep_step_chain(true);
            let errors = expect_validation_errors(validate(&spec));
            let cycles = errors
                .iter()
                .filter(|error| error.kind == ValidationErrorKind::DependencyCycle)
                .count();
            assert_eq!(cycles, 1);
        });
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

