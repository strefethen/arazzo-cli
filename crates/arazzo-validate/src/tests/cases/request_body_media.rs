    const LITERAL_JSON_TEXT_YAML: &str = r#"arazzo: "1.1.0"
info:
  title: Literal JSON text
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        x:
          type: string
    steps:
      - stepId: notJson
        operationId: send
        requestBody:
          contentType: application/json
          payload: "hello"
      - stepId: omitted
        operationId: send
        requestBody:
          payload: "{a: 1}"
      - stepId: suffix
        operationId: send
        requestBody:
          contentType: application/problem+json; charset=utf-8
          payload: "{\"a\": 1,}"
      - stepId: validText
        operationId: send
        requestBody:
          contentType: application/json
          payload: "{\"a\": [1, true, null]}"
      - stepId: interpolated
        operationId: send
        requestBody:
          contentType: application/json
          payload: "{\"a\": {$inputs.x}}"
      - stepId: wholeExpression
        operationId: send
        requestBody:
          contentType: application/json
          payload: $inputs.x
      - stepId: otherType
        operationId: send
        requestBody:
          contentType: text/plain
          payload: "hello"
      - stepId: structured
        operationId: send
        requestBody:
          contentType: application/json
          payload:
            a: "not { json"
"#;

    fn request_body_warnings() -> Vec<Diagnostic> {
        let (_, diagnostics) = parse_bytes_with_diagnostics(LITERAL_JSON_TEXT_YAML.as_bytes())
            .unwrap_or_else(|err| panic!("expected a warning-only document, got: {err}"));
        diagnostics
            .into_iter()
            .filter(|d| d.kind == ValidationErrorKind::InvalidRequestBody)
            .collect()
    }

    #[test]
    fn literal_json_typed_payload_that_is_not_json_text_warns() {
        let warnings = request_body_warnings();
        let found: Vec<(&str, &str)> = warnings
            .iter()
            .map(|d| (d.path.as_str(), d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            vec![
                (
                    "workflow \"wf\" > step \"notJson\".requestBody.payload",
                    "workflow \"wf\" > step \"notJson\".requestBody.payload is a literal string sent as written under contentType \"application/json\", but it is not JSON text: expected value at line 1 column 1",
                ),
                (
                    "workflow \"wf\" > step \"omitted\".requestBody.payload",
                    "workflow \"wf\" > step \"omitted\".requestBody.payload is a literal string sent as written under an omitted contentType, which sends application/json, but it is not JSON text: key must be a string at line 1 column 2",
                ),
                (
                    "workflow \"wf\" > step \"suffix\".requestBody.payload",
                    "workflow \"wf\" > step \"suffix\".requestBody.payload is a literal string sent as written under contentType \"application/problem+json; charset=utf-8\", but it is not JSON text: trailing comma at line 1 column 9",
                ),
            ]
        );
        assert!(warnings.iter().all(|d| d.severity == Severity::Warning));
    }

    #[test]
    fn strict_promotes_the_literal_json_text_warning_to_an_error() {
        let warnings = request_body_warnings();
        assert!(!warnings.is_empty());
        for warning in warnings {
            assert!(!warning.is_error());
            assert_eq!(warning.into_error().severity, Severity::Error);
        }
    }

    #[test]
    fn valid_json_text_interpolation_expressions_other_types_and_structures_do_not_warn() {
        let warnings = request_body_warnings();
        for step in [
            "validText",
            "interpolated",
            "wholeExpression",
            "otherType",
            "structured",
        ] {
            assert!(
                !warnings
                    .iter()
                    .any(|d| d.path.contains(&format!("step \"{step}\""))),
                "{step}: {warnings:?}"
            );
        }
    }

