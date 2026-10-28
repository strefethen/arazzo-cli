    #[test]
    fn validate_step_no_operation() {
        let mut spec = valid_spec();
        spec.workflows[0].steps[0].target = None;
        let errs = expect_validation_errors(validate(&spec));
        assert_eq!(errs[0].kind, ValidationErrorKind::InvalidStepTarget);
        assert!(errs[0]
            .message
            .contains("must have operationId, operationPath, channelPath, or workflowId"));
    }

    #[test]
    fn validate_async_source_applicability_matrix() {
        for source_type in [SourceType::OpenApi, SourceType::Arazzo, SourceType::AsyncApi] {
            for (target, action, correlation, fields) in [
                (StepTarget::OperationId("$sourceDescriptions.api.receive".into()),
                 Some(StepAction::Receive), None, vec!["action"]),
                (StepTarget::OperationId("$sourceDescriptions.api.receive".into()),
                 Some(StepAction::Receive), Some("fixed-id".into()),
                 vec!["action", "correlationId"]),
                (StepTarget::OperationId("$sourceDescriptions.api.send".into()),
                 Some(StepAction::Send), None, vec!["action"]),
                (StepTarget::ChannelPath("{$sourceDescriptions.api.url}#/channels/events".into()),
                 Some(StepAction::Receive), Some("fixed-id".into()), vec!["channelPath"]),
            ] {
                let mut spec = valid_spec();
                spec.arazzo = "1.1.0".into();
                spec.source_descriptions[0].type_ = source_type;
                let step = &mut spec.workflows[0].steps[0];
                step.target = Some(target);
                step.action = action;
                step.correlation_id = correlation;
                if source_type == SourceType::AsyncApi {
                    assert!(validate(&spec).is_ok());
                    continue;
                }
                let diagnostics = expect_validation_errors(validate(&spec));
                assert_eq!(diagnostics.len(), fields.len(), "{diagnostics:?}");
                for (diagnostic, field) in diagnostics.iter().zip(fields) {
                    assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidAsyncStep);
                    assert_eq!(diagnostic.severity, Severity::Error);
                    assert_eq!(diagnostic.path, format!("workflow \"wf1\" > step \"s1\".{field}"));
                    let rule = if field == "channelPath" {
                        "channelPath must reference an asyncapi source".to_string()
                    } else {
                        format!("{field} is only applicable to asyncapi steps")
                    };
                    assert_eq!(diagnostic.message,
                        format!("{rule}; source \"api\" is type {source_type}"));
                }
            }
        }
    }

    #[test]
    fn validate_async_source_applicability_preserves_unresolved_targets() {
        for target in [
            StepTarget::OperationId("publish".into()),
            StepTarget::OperationId("$sourceDescriptions.undeclared.publish".into()),
            StepTarget::OperationId("$sourceDescriptions.api.".into()),
            StepTarget::ChannelPath("{$sourceDescriptions.undeclared.url}#/channels/events".into()),
            StepTarget::ChannelPath("{$sourceDescriptions.api.url}#/channels/~2bad".into()),
        ] {
            let mut spec = valid_spec();
            spec.workflows[0].steps[0].target = Some(target);
            spec.workflows[0].steps[0].action = Some(StepAction::Send);
            let diagnostics = match validate_diagnostics(&spec) {
                Ok(warnings) => warnings,
                Err(Error::Validation(report)) => report.errors,
                Err(error) => panic!("unexpected error: {error}"),
            };
            assert!(!diagnostics.iter().any(|d| d.kind == ValidationErrorKind::InvalidAsyncStep),
                "{diagnostics:?}");
            if spec.workflows[0].steps[0].target == Some(StepTarget::OperationId("publish".into())) {
                assert!(validate(&spec).is_ok());
            }
        }
    }

    #[test]
    fn validate_async_source_applicability_keeps_existing_errors_first() {
        let mut spec = valid_spec();
        let step = &mut spec.workflows[0].steps[0];
        step.target = Some(StepTarget::OperationId("$sourceDescriptions.api.receive".into()));
        step.correlation_id = Some("fixed-id".into());
        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors.len(), 2, "{errors:?}");
        for error in &errors {
            assert_eq!(error.kind, ValidationErrorKind::InvalidAsyncStep);
            assert_eq!(error.path, "workflow \"wf1\" > step \"s1\".correlationId");
        }
        assert!(errors[0].message.ends_with("only applicable to async steps with action receive"));
        assert_eq!(errors[1].message,
            "correlationId is only applicable to asyncapi steps; source \"api\" is type openapi");

        spec.workflows[0].steps[0].target = Some(StepTarget::ChannelPath(
            "{$sourceDescriptions.api.url}#/channels/events".into(),
        ));
        spec.workflows[0].steps[0].correlation_id = None;
        let errors = expect_validation_errors(validate(&spec));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].path.ends_with(".action"));
        assert!(errors[0].message.ends_with("requires action send or receive"));
        assert!(errors[1].path.ends_with(".channelPath"));
    }

    #[test]
    fn validate_async_channel_step_and_timeout_boundaries() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::AsyncApi;
        spec.workflows[0].steps[0].target = Some(StepTarget::ChannelPath(
            "{$sourceDescriptions.api.url}#/channels/events".to_string(),
        ));
        spec.workflows[0].steps[0].action = Some(StepAction::Receive);
        spec.workflows[0].steps[0].correlation_id = Some("$inputs.eventId".to_string());

        for timeout in [0, u64::MAX] {
            spec.workflows[0].steps[0].timeout = Some(timeout);
            if let Err(err) = validate(&spec) {
                panic!("expected unsigned timeout {timeout} to validate: {err}");
            }
        }
    }

    #[test]
    fn validate_async_operation_id_send_step() {
        let mut spec = valid_spec();
        spec.source_descriptions[0].type_ = SourceType::AsyncApi;
        let step = &mut spec.workflows[0].steps[0];
        step.target = Some(StepTarget::OperationId(
            "$sourceDescriptions.api.publish".to_string(),
        ));
        step.action = Some(StepAction::Send);

        if let Err(err) = validate(&spec) {
            panic!("expected operationId async send step to validate: {err}");
        }
    }

    #[test]
    fn validate_async_field_combinations_fail_with_specific_diagnostics() {
        let mut channel_without_action = valid_spec();
        channel_without_action.workflows[0].steps[0].target =
            Some(StepTarget::ChannelPath("/events".to_string()));

        let mut operation_path_with_action = valid_spec();
        operation_path_with_action.workflows[0].steps[0].action = Some(StepAction::Send);

        let mut workflow_with_action = valid_spec();
        workflow_with_action.workflows[0].steps[0].target =
            Some(StepTarget::WorkflowId("wf1".to_string()));
        workflow_with_action.workflows[0].steps[0].action = Some(StepAction::Receive);

        let mut workflow_with_correlation = valid_spec();
        workflow_with_correlation.workflows[0].steps[0].target =
            Some(StepTarget::WorkflowId("wf1".to_string()));
        workflow_with_correlation.workflows[0].steps[0].correlation_id =
            Some("order-1".to_string());

        let mut send_with_correlation = valid_spec();
        send_with_correlation.workflows[0].steps[0].target =
            Some(StepTarget::OperationId("publish".to_string()));
        send_with_correlation.workflows[0].steps[0].action = Some(StepAction::Send);
        send_with_correlation.workflows[0].steps[0].correlation_id = Some("order-1".to_string());

        for (name, spec, expected_path) in [
            ("channel without action", channel_without_action, ".action"),
            (
                "operationPath with action",
                operation_path_with_action,
                ".action",
            ),
            ("workflow with action", workflow_with_action, ".action"),
            (
                "workflow with correlationId",
                workflow_with_correlation,
                ".correlationId",
            ),
            (
                "send with correlationId",
                send_with_correlation,
                ".correlationId",
            ),
        ] {
            let errors = expect_validation_errors(validate(&spec));
            assert!(
                errors.iter().any(|error| {
                    error.kind == ValidationErrorKind::InvalidAsyncStep
                        && error.path.ends_with(expected_path)
                }),
                "{name} should produce a specific async validation diagnostic: {errors:?}"
            );
        }
    }

