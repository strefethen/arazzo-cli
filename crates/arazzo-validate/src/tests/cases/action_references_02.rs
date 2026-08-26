    /// Pins the "do not over-apply the regex" boundary from the design:
    /// `OnAction.name` carries `$components.*` reference values in real
    /// documents (e.g. `examples/httpbin-components.arazzo.yaml`) and must
    /// not be constrained by the DottedKey check — only the Components map
    /// *key* is checked, never an `OnAction.name` value that references one.
    #[test]
    fn on_action_name_component_reference_is_accepted() {
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
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.standardRetry"
"#;
        if let Err(err) = parse_bytes(yaml.as_bytes()) {
            panic!("expected $components.* OnAction.name to validate cleanly, got: {err}");
        }
    }

