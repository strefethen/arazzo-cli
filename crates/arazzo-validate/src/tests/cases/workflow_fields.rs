    #[test]
    fn parse_bytes_workflow_level_param_invalid_in() {
        let yaml = r#"arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:
      - name: q
        in: body
        value: x
    steps:
      - stepId: s1
        operationPath: /test
"#;
        let result = parse_bytes(yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error for invalid workflow param location"),
            Err(err) => {
                let msg = err.to_string();
                if !msg.contains("parsing arazzo yaml") {
                    panic!("unexpected error: {msg}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_workflow_level_fields() {
        let spec_yaml = r#"
arazzo: "1.0.0"
info:
  title: Test
  summary: A summary
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:
      - name: Authorization
        in: header
        value: "Bearer token"
    successActions:
      - name: finish
        type: end
    failureActions:
      - name: retry
        type: retry
        retryAfter: 5
        retryLimit: 3
    steps:
      - stepId: s1
        description: First step
        operationPath: /test
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        assert_eq!(spec.info.summary, "A summary");
        let wf = &spec.workflows[0];
        assert_eq!(wf.parameters.len(), 1);
        assert_eq!(wf.parameters[0].name, "Authorization");
        assert_eq!(wf.success_actions.len(), 1);
        assert_eq!(wf.success_actions[0].action_type(), ActionType::End);
        assert_eq!(wf.failure_actions.len(), 1);
        assert_eq!(wf.failure_actions[0].action_type(), ActionType::Retry);
        assert_eq!(wf.failure_actions[0].retry_after, 5.0);
        assert_eq!(wf.steps[0].description, "First step");
    }

    /// Workflow Object Fixed Fields (spec/arazzo/v1.1.0.html): `dependsOn` is
    /// `[string]` on the Workflow Object itself, not only on Step. Before
    /// this, a conformant `dependsOn` on a workflow warned as an unrecognized
    /// field and failed `--strict`.
    #[test]
    fn workflow_depends_on_parses_round_trips_and_produces_zero_unknown_field_warnings() {
        let spec_yaml = r#"arazzo: "1.1.0"
info:
  title: Workflow dependsOn
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: first
    steps:
      - stepId: s1
        operationPath: /test
  - workflowId: second
    dependsOn:
      - first
    steps:
      - stepId: s2
        operationPath: /test
"#;

        let (spec, diagnostics) = match parse_bytes_with_diagnostics(spec_yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a clean document, got: {err}"),
        };
        assert_eq!(diagnostics, Vec::new(), "diagnostics={diagnostics:?}");
        assert_eq!(
            spec.workflows[1].depends_on,
            vec!["first".to_string()],
            "workflows={:?}",
            spec.workflows
        );

        let serialized =
            serde_yaml_ng::to_string(&spec).unwrap_or_else(|err| panic!("serialize: {err}"));
        assert!(serialized.contains("dependsOn"), "{serialized}");
        let reparsed = match arazzo_spec::parse_unvalidated_bytes(serialized.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("reparsing serialized spec: {err}"),
        };
        assert_eq!(
            reparsed.workflows[1].depends_on,
            spec.workflows[1].depends_on
        );
    }

