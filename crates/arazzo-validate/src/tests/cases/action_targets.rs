    #[test]
    fn validate_goto_valid_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps.push(Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath("/other".to_string())),
            ..Step::default()
        });
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "s2".to_string(),
            ..OnAction::default()
        }];
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error for valid goto, got: {err}");
        }
    }

    #[test]
    fn validate_goto_invalid_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "nonexistent".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown step \"nonexistent\"")));
    }

    #[test]
    fn validate_goto_valid_workflow_id() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test2".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "wf2".to_string(),
            ..OnAction::default()
        }];
        let result = validate(&spec);
        if let Err(err) = result {
            panic!("expected no error for valid goto workflow, got: {err}");
        }
    }

    #[test]
    fn validate_goto_invalid_workflow_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "missing_wf".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown workflow \"missing_wf\"")));
    }

    #[test]
    fn validate_goto_source_description_workflow_ref() {
        let mut spec = valid_spec();
        // Add an Arazzo-type source description for cross-source workflow references.
        spec.source_descriptions.push(SourceDescription {
            name: "external".to_string(),
            url: "https://example.com/other.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        });
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "$sourceDescriptions.external.someWorkflow".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression workflowId should not be rejected"
        );
    }

    #[test]
    fn validate_goto_runtime_expression_step_id() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            step_id: "$steps.decide.outputs.nextStep".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression stepId should not be rejected"
        );
    }

    #[test]
    fn validate_goto_missing_step_and_workflow() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_success = vec![OnAction {
            type_: Some(ActionType::Goto),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::MissingRequiredField
                && e.message
                    .contains("goto action must specify stepId or workflowId")));
    }

    #[test]
    fn validate_retry_both_step_and_workflow_rejected() {
        let mut spec = valid_spec();
        spec.workflows.push(Workflow {
            workflow_id: "wf2".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test2".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        });
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "s1".to_string(),
            workflow_id: "wf2".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message
                    .contains("retry action specifies both stepId and workflowId")));
    }

    #[test]
    fn validate_retry_unknown_step_id_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "nonexistent".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown step \"nonexistent\"")));
    }

    #[test]
    fn validate_retry_unknown_workflow_id_rejected() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            workflow_id: "missing_wf".to_string(),
            ..OnAction::default()
        }];
        let errs = expect_validation_errors(validate(&spec));
        assert!(errs
            .iter()
            .any(|e| e.kind == ValidationErrorKind::InvalidReference
                && e.message.contains("unknown workflow \"missing_wf\"")));
    }

    #[test]
    fn validate_retry_runtime_expression_reference_accepted() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            step_id: "$steps.decide.outputs.recoveryStep".to_string(),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "runtime expression retry stepId should not be rejected"
        );
    }

    /// Unlike goto, the reference is optional for retry: "If a stepId or
    /// workflowId are specified, then the reference is executed ..."
    #[test]
    fn validate_retry_without_reference_stays_valid() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].on_failure = vec![OnAction {
            type_: Some(ActionType::Retry),
            ..OnAction::default()
        }];
        assert!(
            validate(&spec).is_ok(),
            "retry without stepId/workflowId must stay valid"
        );
    }

