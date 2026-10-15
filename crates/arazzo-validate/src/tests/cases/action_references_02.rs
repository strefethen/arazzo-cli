    // Action names are strings, not Components map keys or references.
    #[test]
    fn on_action_reference_looking_name_is_literal() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: T
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    standardRetry:
      name: standardRetry
      type: retry
      retryAfter: 1
      retryLimit: 3
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationId: fixturetest
        onFailure:
          - name: "$components.failureActions.standardRetry"
            type: end
"#;
        if let Err(err) = parse_bytes(yaml.as_bytes()) {
            panic!("expected $components.* OnAction.name to validate cleanly, got: {err}");
        }
    }

    #[test]
    fn name_only_component_shorthand_requires_its_own_type_at_all_action_positions() {
        for (position, namespace) in [
            ("workflow success", "successActions"),
            ("workflow failure", "failureActions"),
            ("step success", "successActions"),
            ("step failure", "failureActions"),
        ] {
            let action = format!("{{name: '$components.{namespace}.component'}}");
            let document = action_position_document(position, &action, false).replace(
                "workflows:",
                &format!("components:\n  {namespace}:\n    component: {{name: reusable, type: end}}\nworkflows:"),
            );
            let expected_path = format!("{}.type", action_position_path(position));
            let raw_report = action_validation_report(&document);
            let Err(Error::Validation(typed_report)) = validate(&parse_unvalidated(&document)) else {
                panic!("{position}: direct validation must reject a missing action type");
            };
            for report in [raw_report, typed_report] {
                assert_eq!(report.errors.len(), 1, "{position}: {report:?}");
                assert_eq!(report.errors[0].path, expected_path);
                assert_eq!(report.errors[0].kind, ValidationErrorKind::MissingRequiredField);
            }
        }
    }

    #[test]
    fn reference_looking_action_names_keep_only_their_own_fields() {
        for (position, namespace, action_type) in [
            ("workflow success", "successActions", "goto"),
            ("workflow failure", "failureActions", "retry"),
            ("step success", "successActions", "goto"),
            ("step failure", "failureActions", "retry"),
        ] {
            let name = format!("$components.{namespace}.component");
            let action = format!("{{name: '{name}', type: {action_type}, workflowId: wf}}");
            let document = action_position_document(position, &action, false);
            // An absent component map, a same-type component, and a different-type
            // component must all leave the concrete action unchanged.
            for component in ["", action_type, "end"] {
                let document = if component.is_empty() {
                    document.clone()
                } else {
                    let fields = if component == "end" {
                        ""
                    } else {
                        ", workflowId: wf, parameters: [{name: inherited, value: component}], criteria: [{condition: 'true'}]"
                    };
                    let retry = if component == "retry" {
                        ", retryAfter: 2, retryLimit: 5"
                    } else {
                        ""
                    };
                    document.replace("workflows:", &format!("components:\n  {namespace}:\n    component: {{name: reusable, type: {component}{fields}{retry}}}\nworkflows:"))
                };
                let spec = expect_parsed(document.as_bytes());
                if let Err(error) = validate(&parse_unvalidated(&document)) {
                    panic!("{position} component={component}: {error}");
                }
                let workflow = &spec.workflows[0];
                let action = match position {
                    "workflow success" => &workflow.success_actions[0],
                    "workflow failure" => &workflow.failure_actions[0],
                    "step success" => &workflow.steps[0].on_success[0],
                    "step failure" => &workflow.steps[0].on_failure[0],
                    _ => unreachable!(),
                };
                assert_eq!(action.name, name);
                assert_eq!(action.workflow_id, "wf");
                assert_eq!(action.action_type().to_string(), action_type);
                assert_eq!(action.retry_after, 0.0);
                assert_eq!(action.retry_limit, None);
                assert!(action.criteria.is_empty());
                assert!(action.parameters.is_empty());
            }
        }
    }
