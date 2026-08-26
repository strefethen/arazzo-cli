    /// One `x-*` extension (must not warn) and one misspelled/unrecognized
    /// field (must warn) at every position `arazzo-spec` captures leftover
    /// fields for, except `SchemaObject`/`PropertyDef` (JSON Schema
    /// carve-out, covered separately). Exercises: root, `info`,
    /// `sourceDescriptions[]`, `components`, `components.parameters.<name>`,
    /// `components.successActions.<name>`, workflow, workflow parameter,
    /// step, step parameter, `requestBody`, `requestBody.replacements[]`,
    /// `successCriteria[]`, and `onSuccess[]`.
    const UNKNOWN_FIELD_COVERAGE_YAML: &str = r#"arazzo: "1.1.0"
x-root-ext: keep
rootUnknown: warn
info:
  title: Unknown Field Coverage
  version: "1.0.0"
  x-info-ext: keep
  infoUnknown: warn
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
    x-source-ext: keep
    sourceUnknown: warn
components:
  x-components-ext: keep
  componentsUnknown: warn
  parameters:
    sharedParam:
      name: q
      in: query
      value: "1"
      x-param-ext: keep
      paramUnknown: warn
  successActions:
    sharedSuccess:
      name: terminate
      type: end
      x-action-ext: keep
      actionUnknown: warn
workflows:
  - workflowId: wf1
    x-workflow-ext: keep
    workflowUnknown: warn
    parameters:
      - name: p1
        in: query
        value: "1"
        x-param-ext: keep
        paramUnknown: warn
    steps:
      - stepId: s1
        operationPath: /test
        x-step-ext: keep
        stepUnknown: warn
        parameters:
          - name: p2
            in: query
            value: "2"
            x-param-ext: keep
            paramUnknown: warn
        requestBody:
          contentType: application/json
          payload: "{}"
          x-body-ext: keep
          bodyUnknown: warn
          replacements:
            - target: /a
              value: 1
              x-replacement-ext: keep
              replacementUnknown: warn
        successCriteria:
          - condition: "$statusCode == 200"
            x-criterion-ext: keep
            criterionUnknown: warn
        onSuccess:
          - name: finish
            type: end
            x-action-ext: keep
            actionUnknown: warn
"#;

    #[test]
    fn unknown_field_warns_at_every_captured_position_x_extension_does_not() {
        let (_, diagnostics) =
            match parse_bytes_with_diagnostics(UNKNOWN_FIELD_COVERAGE_YAML.as_bytes()) {
                Ok(v) => v,
                Err(err) => panic!("expected a warning-only document, got: {err}"),
            };

        let unknown_field: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.kind == ValidationErrorKind::UnknownField)
            .collect();

        let expected: &[(&str, &str)] = &[
            ("", "rootUnknown"),
            ("info", "infoUnknown"),
            ("sourceDescriptions[0]", "sourceUnknown"),
            ("components", "componentsUnknown"),
            ("components.parameters.sharedParam", "paramUnknown"),
            ("components.successActions.sharedSuccess", "actionUnknown"),
            ("workflow \"wf1\"", "workflowUnknown"),
            ("workflow \"wf1\".parameters[0]", "paramUnknown"),
            ("workflow \"wf1\" > step \"s1\"", "stepUnknown"),
            (
                "workflow \"wf1\" > step \"s1\".parameters[0]",
                "paramUnknown",
            ),
            ("workflow \"wf1\" > step \"s1\".requestBody", "bodyUnknown"),
            (
                "workflow \"wf1\" > step \"s1\".requestBody.replacements[0]",
                "replacementUnknown",
            ),
            (
                "workflow \"wf1\" > step \"s1\".successCriteria[0]",
                "criterionUnknown",
            ),
            (
                "workflow \"wf1\" > step \"s1\".onSuccess[0]",
                "actionUnknown",
            ),
        ];

        for (path, key) in expected {
            assert!(
                unknown_field.iter().any(|d| d.severity == Severity::Warning
                    && d.path == *path
                    && d.message.contains(&format!("\"{key}\""))),
                "missing unknownField warning at path {path:?} for {key:?}; got={unknown_field:?}"
            );
        }
        assert_eq!(
            unknown_field.len(),
            expected.len(),
            "unexpected extra/missing unknownField warnings: {unknown_field:?}"
        );

        // Every `x-*` field in the document is a no-op: none of them appear
        // in any diagnostic, at any severity.
        for ext_key in [
            "x-root-ext",
            "x-info-ext",
            "x-source-ext",
            "x-components-ext",
            "x-param-ext",
            "x-action-ext",
            "x-workflow-ext",
            "x-step-ext",
            "x-body-ext",
            "x-replacement-ext",
            "x-criterion-ext",
        ] {
            assert!(
                diagnostics.iter().all(|d| !d.message.contains(ext_key)),
                "vendor extension {ext_key} must never be reported; diagnostics={diagnostics:?}"
            );
        }
    }

    /// Payload Replacement Object (`Replacement`) had no capture before
    /// ac-bd441 — the one genuine blind spot the design calls out. Confirms
    /// it now warns on an unrecognized field and stays silent on `x-*`.
    #[test]
    fn replacement_unknown_field_warns_and_x_extension_does_not() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Replacement Coverage
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        requestBody:
          contentType: application/json
          payload: "{}"
          replacements:
            - target: /a
              value: 1
              x-replacement-ext: keep
            - target: /b
              value: 2
              replacementTypo: warn
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a warning-only document, got: {err}"),
        };

        let unknown_field: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.kind == ValidationErrorKind::UnknownField)
            .collect();
        assert_eq!(unknown_field.len(), 1, "diagnostics={unknown_field:?}");
        assert_eq!(
            unknown_field[0].path,
            "workflow \"wf1\" > step \"s1\".requestBody.replacements[1]"
        );
        assert!(unknown_field[0].message.contains("\"replacementTypo\""));
        assert!(
            diagnostics
                .iter()
                .all(|d| !d.message.contains("x-replacement-ext")),
            "diagnostics={diagnostics:?}"
        );
    }

    /// Arazzo 1.1.0 workflow `inputs` (and `components.inputs`) are JSON
    /// Schema 2020-12 objects. `items`, `enum`, `minimum`,
    /// `additionalProperties`, and `oneOf` are legitimate keywords `arazzo-spec`
    /// does not model as typed fields — not unrecognized fields — so this
    /// must produce zero warnings. Written before the detection landed, per
    /// the ticket's testing obligations: without the `SchemaObject`/
    /// `PropertyDef` carve-out, this fails.
    #[test]
    fn json_schema_positions_produce_no_unknown_field_warnings() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Schema Carveout
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    inputs:
      type: object
      items:
        type: string
      enum:
        - a
        - b
      minimum: 1
      additionalProperties: false
      oneOf:
        - type: string
    steps:
      - stepId: s1
        operationPath: /test
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a clean document, got: {err}"),
        };
        assert!(diagnostics.is_empty(), "diagnostics={diagnostics:?}");
    }

    /// Pins the suggestion decision from the ticket's Design: detection only,
    /// no "did you mean" text — a hand-maintained field-name registry would
    /// be required to offer one, which the design explicitly rules out.
    #[test]
    fn unknown_field_message_is_the_bare_form_no_suggestion() {
        let yaml = r#"arazzo: "1.1.0"
info:
  title: Bare Message
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        sucessCriteria: []
"#;

        let (_, diagnostics) = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
            Ok(v) => v,
            Err(err) => panic!("expected a warning-only document, got: {err}"),
        };

        assert_eq!(diagnostics.len(), 1, "diagnostics={diagnostics:?}");
        assert_eq!(diagnostics[0].kind, ValidationErrorKind::UnknownField);
        assert_eq!(diagnostics[0].severity, Severity::Warning);
        assert_eq!(
            diagnostics[0].message,
            "unrecognized field \"sucessCriteria\"; only `x-` prefixed extension fields are \
             permitted here"
        );
        assert!(
            !diagnostics[0].message.contains("successCriteria"),
            "message must not suggest the correctly spelled field name: {}",
            diagnostics[0].message
        );
    }

