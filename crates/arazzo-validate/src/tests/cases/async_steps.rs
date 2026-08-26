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

