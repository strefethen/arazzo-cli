    fn reusable_parameter_document(value: Option<&str>, json: bool) -> String {
        if json {
            let value = value.map_or_else(String::new, |value| format!(r#", "value": {value}"#));
            return format!(
                r#"{{
  "arazzo": "1.1.0",
  "info": {{"title": "Reusable parameter value", "version": "1.0.0"}},
  "sourceDescriptions": [{{"name": "api", "url": "https://example.com", "type": "openapi"}}],
  "components": {{"parameters": {{"shared": {{"name": "shared", "in": "header", "value": "component default"}}}}}},
  "workflows": [{{
    "workflowId": "wf",
    "steps": [{{
      "stepId": "request",
      "operationPath": "/request",
      "parameters": [{{"reference": "$components.parameters.shared"{value}}}]
    }}]
  }}]
}}"#
            );
        }

        let value = value.map_or_else(String::new, |value| format!("\n            value: {value}"));
        format!(
            r#"arazzo: "1.1.0"
info: {{title: Reusable parameter value, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
components:
  parameters:
    shared: {{name: shared, in: header, value: "component default"}}
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationPath: /request
        parameters:
          - reference: "$components.parameters.shared"{value}
"#
        )
    }

    #[test]
    fn reusable_parameter_values_distinguish_omitted_and_empty_string_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            let inherited = expect_parsed(reusable_parameter_document(None, json).as_bytes());
            assert_eq!(
                inherited.workflows[0].steps[0].parameters[0].value,
                "component default".into(),
                "{format} omitted value must inherit"
            );

            let overridden =
                expect_parsed(reusable_parameter_document(Some("\"\""), json).as_bytes());
            assert_eq!(
                overridden.workflows[0].steps[0].parameters[0].value,
                serde_yaml_ng::Value::String(String::new()).into(),
                "{format} empty string must override"
            );
        }
    }

    #[test]
    fn reusable_parameter_values_reject_non_string_classes_for_yaml_and_json() {
        for (format, json) in [("YAML", false), ("JSON", true)] {
            for value in ["null", "false", "0", "{}", "[]"] {
                let document = reusable_parameter_document(Some(value), json);
                let Err(Error::Validation(report)) =
                    parse_bytes_with_diagnostics(document.as_bytes())
                else {
                    panic!("{format} Reusable Object value {value} must fail validation");
                };
                assert_eq!(report.errors.len(), 1, "{format} value={value}: {report:?}");
                let error = &report.errors[0];
                assert_eq!(error.kind, ValidationErrorKind::InvalidReference);
                assert_eq!(
                    error.path,
                    "workflow \"wf\" > step \"request\".parameters[0].value"
                );
                assert_eq!(
                    error.message,
                    "workflow \"wf\" > step \"request\".parameters[0].value must be a string when used on a Reusable Object"
                );
            }
        }
    }

    #[test]
    fn component_parameter_definition_with_reference_still_requires_value() {
        let yaml = r#"arazzo: "1.1.0"
info: {title: Component parameter, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
components:
  parameters:
    shared: {name: shared, reference: "$components.parameters.other"}
workflows:
  - workflowId: wf
    steps: [{stepId: request, operationPath: /request}]
"#;
        let json = r#"{
  "arazzo": "1.1.0",
  "info": {"title": "Component parameter", "version": "1.0.0"},
  "sourceDescriptions": [{"name": "api", "url": "https://example.com", "type": "openapi"}],
  "components": {"parameters": {"shared": {"name": "shared", "reference": "$components.parameters.other"}}},
  "workflows": [{"workflowId": "wf", "steps": [{"stepId": "request", "operationPath": "/request"}]}]
}"#;

        for (format, document) in [("YAML", yaml), ("JSON", json)] {
            let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(document.as_bytes())
            else {
                panic!("{format} component Parameter Object must require value");
            };
            assert_eq!(report.errors.len(), 1, "{format}: {report:?}");
            assert_eq!(
                report.errors[0].kind,
                ValidationErrorKind::MissingRequiredField
            );
            assert_eq!(report.errors[0].path, "components.parameters.shared.value");
        }
    }

    fn required_value_fixture(values: [&str; 15]) -> String {
        format!(
            r#"arazzo: "1.1.0"
info: {{title: Required value presence, version: "1.0.0"}}
sourceDescriptions:
  - {{name: api, url: https://example.com, type: openapi}}
components:
  parameters:
    componentParam: {{name: componentParam, value: {}}}
  successActions:
    componentSuccess:
      name: componentSuccess
      type: goto
      workflowId: target
      parameters:
        - {{name: componentSuccessParam, value: {}}}
  failureActions:
    componentFailure:
      name: componentFailure
      type: goto
      workflowId: target
      parameters:
        - {{name: componentFailureParam, value: {}}}
workflows:
  - workflowId: wf
    parameters:
      - {{name: workflowParam, in: header, value: {}}}
    successActions:
      - name: workflowSuccess
        type: goto
        workflowId: target
        parameters:
          - {{name: workflowSuccessParam, value: {}}}
    failureActions:
      - name: workflowFailure
        type: goto
        workflowId: target
        parameters:
          - {{name: workflowFailureParam, value: {}}}
    steps:
      - stepId: request
        operationPath: /request
        parameters:
          - {{name: stepParam, in: query, value: {}}}
        onSuccess:
          - name: stepSuccess
            type: goto
            workflowId: target
            parameters:
              - {{name: stepSuccessParam, value: {}}}
        onFailure:
          - name: stepFailure
            type: goto
            workflowId: target
            parameters:
              - {{name: stepFailureParam, value: {}}}
        requestBody:
          contentType: application/json
          payload: {{}}
          replacements:
            - {{target: /a, value: {}}}
            - {{target: /b, value: {}}}
            - {{target: /c, value: {}}}
            - {{target: /d, value: {}}}
            - {{target: /e, value: {}}}
            - {{target: /f, value: {}}}
  - workflowId: target
    steps:
      - stepId: targetStep
        operationPath: /target
"#,
            values[0],
            values[1],
            values[2],
            values[3],
            values[4],
            values[5],
            values[6],
            values[7],
            values[8],
            values[9],
            values[10],
            values[11],
            values[12],
            values[13],
            values[14],
        )
    }

    fn missing_required_value_fixture() -> String {
        required_value_fixture([
            "null", "null", "null", "null", "null", "null", "null", "null", "null", "null", "null",
            "null", "null", "null", "null",
        ])
        .replace(", value: null", "")
    }

    fn required_value_json(values: [&str; 15]) -> String {
        format!(
            r#"{{
  "arazzo": "1.1.0",
  "info": {{"title": "Required value presence", "version": "1.0.0"}},
  "sourceDescriptions": [{{"name": "api", "url": "https://example.com", "type": "openapi"}}],
  "components": {{
    "parameters": {{"componentParam": {{"name": "componentParam", "value": {}}}}},
    "successActions": {{"componentSuccess": {{"name": "componentSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "componentSuccessParam", "value": {}}}]}}}},
    "failureActions": {{"componentFailure": {{"name": "componentFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "componentFailureParam", "value": {}}}]}}}}
  }},
  "workflows": [
    {{
      "workflowId": "wf",
      "parameters": [{{"name": "workflowParam", "in": "header", "value": {}}}],
      "successActions": [{{"name": "workflowSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "workflowSuccessParam", "value": {}}}]}}],
      "failureActions": [{{"name": "workflowFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "workflowFailureParam", "value": {}}}]}}],
      "steps": [{{
        "stepId": "request",
        "operationPath": "/request",
        "parameters": [{{"name": "stepParam", "in": "query", "value": {}}}],
        "onSuccess": [{{"name": "stepSuccess", "type": "goto", "workflowId": "target", "parameters": [{{"name": "stepSuccessParam", "value": {}}}]}}],
        "onFailure": [{{"name": "stepFailure", "type": "goto", "workflowId": "target", "parameters": [{{"name": "stepFailureParam", "value": {}}}]}}],
        "requestBody": {{"contentType": "application/json", "payload": {{}}, "replacements": [
          {{"target": "/a", "value": {}}}, {{"target": "/b", "value": {}}}, {{"target": "/c", "value": {}}},
          {{"target": "/d", "value": {}}}, {{"target": "/e", "value": {}}}, {{"target": "/f", "value": {}}}
        ]}}
      }}]
    }},
    {{"workflowId": "target", "steps": [{{"stepId": "targetStep", "operationPath": "/target"}}]}}
  ]
}}"#,
            values[0],
            values[1],
            values[2],
            values[3],
            values[4],
            values[5],
            values[6],
            values[7],
            values[8],
            values[9],
            values[10],
            values[11],
            values[12],
            values[13],
            values[14],
        )
    }

    fn missing_required_value_json() -> String {
        required_value_json([
            "null", "null", "null", "null", "null", "null", "null", "null", "null", "null", "null",
            "null", "null", "null", "null",
        ])
        .replace(", \"value\": null", "")
    }

    #[test]
    pub(super) fn raw_required_values_report_exact_paths_for_yaml_and_json() {
        let yaml = missing_required_value_fixture();
        let json = missing_required_value_json();
        let expected = [
            "components.parameters.componentParam.value",
            "components.successActions.componentSuccess.parameters[0].value",
            "components.failureActions.componentFailure.parameters[0].value",
            "workflow \"wf\".parameters[0].value",
            "workflow \"wf\".successActions[0].parameters[0].value",
            "workflow \"wf\".failureActions[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".parameters[0].value",
            "workflow \"wf\" > step \"request\".onSuccess[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".onFailure[0].parameters[0].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[0].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[1].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[2].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[3].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[4].value",
            "workflow \"wf\" > step \"request\".requestBody.replacements[5].value",
        ];

        for (format, document) in [("YAML", yaml.as_bytes()), ("JSON", json.as_bytes())] {
            let Err(Error::Validation(report)) = parse_bytes_with_diagnostics(document) else {
                panic!("{format} document with omitted values must fail validation");
            };
            let actual = report
                .errors
                .iter()
                .filter(|diagnostic| diagnostic.kind == ValidationErrorKind::MissingRequiredField)
                .map(|diagnostic| diagnostic.path.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{format} errors={:?}", report.errors);
        }
    }

    #[test]
    pub(super) fn raw_required_values_accept_concrete_any_values_for_yaml_and_json() {
        let yaml = required_value_fixture([
            "null", "\"\"", "false", "0", "{}", "[]", "null", "\"\"", "false", "null", "\"\"",
            "false", "0", "{}", "[]",
        ]);
        let json = required_value_json([
            "null", "\"\"", "false", "0", "{}", "[]", "null", "\"\"", "false", "null", "\"\"",
            "false", "0", "{}", "[]",
        ]);
        for (format, document) in [("YAML", yaml.as_bytes()), ("JSON", json.as_bytes())] {
            let result = parse_bytes_with_diagnostics(document);
            assert!(
                result.is_ok(),
                "{format} explicit Any values must pass: {result:?}"
            );
        }
    }

    #[test]
    fn malformed_value_containers_do_not_add_raw_missing_value_diagnostics() {
        for malformed in [
            "parameters: {name: not-a-list}",
            "requestBody: {replacements: {target: /a}}",
        ] {
            let yaml = format!(
                r#"arazzo: "1.1.0"
info: {{title: malformed container, version: "1.0.0"}}
sourceDescriptions: [{{name: api, url: https://example.com, type: openapi}}]
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationPath: /request
        {malformed}
"#
            );
            let result = parse_bytes_with_diagnostics(yaml.as_bytes());
            let Err(Error::ParseYaml(error)) = result else {
                panic!("malformed container must remain a serde parse error");
            };
            assert!(
                !error.to_string().contains("value is required"),
                "raw pass must not duplicate malformed-container diagnostics: {error}"
            );
        }
    }

