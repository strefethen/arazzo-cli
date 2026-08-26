    #[test]
    fn parse_bytes_component_inputs_ref() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      properties:
        name:
          type: string
      required:
        - name
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = match spec.workflows[0].inputs.as_ref() {
            Some(i) => i,
            None => panic!("inputs should be present"),
        };
        assert!(
            inputs.ref_.is_empty(),
            "ref_ should be cleared after resolution"
        );
        assert!(inputs.properties.contains_key("name"));
        assert_eq!(inputs.required, vec!["name".to_string()]);
    }

    #[test]
    fn component_input_ref_preserves_component_extensions() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      x-input:
        origin: component
      properties:
        name:
          type: string
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = spec.workflows[0]
            .inputs
            .as_ref()
            .unwrap_or_else(|| panic!("inputs should be present"));
        assert!(inputs.extensions.contains_key("x-input"));
    }

    #[test]
    fn non_vendor_unknown_fields_do_not_validate_or_survive_serialization() {
        let spec_yaml = r#"
arazzo: "1.0.0"
unknownRoot: dropped
info:
  title: Test
  version: "1.0.0"
  unknownInfo: dropped
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        unknownStep: dropped
"#;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let serialized =
            serde_yaml_ng::to_string(&spec).unwrap_or_else(|err| panic!("serialize: {err}"));

        assert!(!serialized.contains("unknownRoot"));
        assert!(!serialized.contains("unknownInfo"));
        assert!(!serialized.contains("unknownStep"));
    }

    #[test]
    fn parse_bytes_component_inputs_ref_not_found() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    other:
      type: object
workflows:
  - workflowId: wf1
    inputs:
      $ref: "#/components/inputs/missing"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let result = parse_bytes(spec_yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err
                    .to_string()
                    .contains("component input \"missing\" not found")
                {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_component_inputs_ref_invalid_prefix() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    foo:
      type: object
workflows:
  - workflowId: wf1
    inputs:
      $ref: "http://external/schema"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let result = parse_bytes(spec_yaml.as_bytes());
        match result {
            Ok(_) => panic!("expected error"),
            Err(err) => {
                if !err.to_string().contains("unsupported input $ref") {
                    panic!("unexpected error: {err}");
                }
            }
        }
    }

    #[test]
    fn parse_bytes_component_inputs_ref_full_replacement() {
        let spec_yaml = r##"
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  inputs:
    shared:
      type: object
      properties:
        age:
          type: integer
      required:
        - age
workflows:
  - workflowId: wf1
    inputs:
      type: string
      $ref: "#/components/inputs/shared"
    steps:
      - stepId: s1
        operationPath: /test
"##;

        let spec = match parse_bytes(spec_yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("expected no error, got: {err}"),
        };
        let inputs = match spec.workflows[0].inputs.as_ref() {
            Some(i) => i,
            None => panic!("inputs should be present"),
        };
        // The inline `type: string` should be fully replaced by the component
        assert_eq!(
            inputs.type_,
            Some(arazzo_spec::JsonSchemaType::Object),
            "inline type should be replaced by component's type"
        );
        assert!(inputs.properties.contains_key("age"));
        assert_eq!(inputs.required, vec!["age".to_string()]);
    }

