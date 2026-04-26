#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use arazzo_spec::{
    parse_unvalidated_bytes, ActionType, ArazzoSpec, CriterionType, SourceType, StepTarget,
};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn load_example_paths() -> Vec<PathBuf> {
    let read_dir = match fs::read_dir(examples_dir()) {
        Ok(entries) => entries,
        Err(err) => panic!("failed to read examples directory: {err}"),
    };
    let mut paths = Vec::new();
    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(err) => panic!("failed to read examples directory entry: {err}"),
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.ends_with(".arazzo.yaml") || file_name.ends_with(".arazzo.yml") {
            paths.push(path);
        }
    }
    paths.sort();
    assert!(!paths.is_empty(), "expected at least one example spec");
    paths
}

fn read_bytes(path: &Path) -> Vec<u8> {
    match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => panic!("failed reading {}: {err}", path.display()),
    }
}

fn parse_spec(bytes: &[u8], context: &str) -> ArazzoSpec {
    match parse_unvalidated_bytes(bytes) {
        Ok(spec) => spec,
        Err(err) => panic!("failed parsing {context}: {err}"),
    }
}

fn serialize_spec(spec: &ArazzoSpec, context: &str) -> String {
    match serde_yaml_ng::to_string(spec) {
        Ok(serialized) => serialized,
        Err(err) => panic!("failed serializing {context}: {err}"),
    }
}

#[test]
fn parse_serialize_parse_roundtrip_for_all_examples() {
    for path in load_example_paths() {
        let original = parse_spec(&read_bytes(&path), &path.display().to_string());
        let serialized = serialize_spec(&original, &path.display().to_string());
        let reparsed = parse_spec(serialized.as_bytes(), &path.display().to_string());
        assert_eq!(
            original,
            reparsed,
            "round-trip mismatch for {}",
            path.display()
        );
    }
}

#[test]
fn parse_preserves_vendor_extensions_on_root() {
    let raw = r#"
arazzo: "1.0.0"
x-arazzo-cli:
  auth:
    type: oauth2
info:
  title: Root Extension Test
  version: "1.0.0"
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
workflows:
  - workflowId: wf
    steps:
      - stepId: call
        operationPath: /get
"#;

    let spec = parse_spec(raw.as_bytes(), "spec with root extensions");
    assert!(spec.extensions.contains_key("x-arazzo-cli"));

    let serialized = serialize_spec(&spec, "spec with root extensions");
    assert!(serialized.contains("x-arazzo-cli"));

    let reparsed = parse_spec(
        serialized.as_bytes(),
        "serialized spec with root extensions",
    );
    assert_eq!(spec.extensions, reparsed.extensions);
}

#[test]
fn parse_preserves_vendor_extensions_nested() {
    let raw = r##"
arazzo: "1.0.0"
info:
  title: Nested Extension Test
  version: "1.0.0"
  x-info:
    owner: docs
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
    x-source:
      authProfile: test
components:
  x-components:
    note: preserved
  inputs:
    SharedInput:
      type: object
      x-schema:
        source: component
      properties:
        id:
          type: string
          x-property:
            pii: false
      required:
        - id
  parameters:
    RequestId:
      name: X-Request-Id
      in: header
      value: "abc"
      x-parameter:
        trace: true
  successActions:
    Done:
      type: end
      x-action:
        audit: true
workflows:
  - workflowId: wf
    x-workflow:
      owner: qa
    inputs:
      $ref: "#/components/inputs/SharedInput"
    parameters:
      - reference: "$components.parameters.RequestId"
    steps:
      - stepId: call
        operationPath: /get
        x-step:
          retries: custom
        parameters:
          - name: filter
            in: query
            value: active
            x-step-param:
              from: inline
        requestBody:
          contentType: application/json
          payload:
            id: "123"
          x-body:
            profile: create
        successCriteria:
          - context: "$response.body"
            condition: "$.ok"
            type:
              type: jsonpath
              version: draft-goessner-dispatch-jsonpath-00
              x-criterion-type:
                dialect: goessner
            x-criterion:
              expected: true
        onSuccess:
          - type: end
            x-action:
              audit: true
"##;

    let spec = parse_spec(raw.as_bytes(), "spec with nested extensions");
    let Some(components) = spec.components.as_ref() else {
        panic!("nested extension fixture should include components");
    };
    let workflow = &spec.workflows[0];
    let step = &workflow.steps[0];
    let criterion = &step.success_criteria[0];
    let criterion_type = match criterion.type_.as_ref() {
        Some(CriterionType::ExpressionType(value)) => value,
        other => panic!("expected criterion expression type, got {other:?}"),
    };

    assert!(spec.info.extensions.contains_key("x-info"));
    assert!(spec.source_descriptions[0]
        .extensions
        .contains_key("x-source"));
    assert!(components.extensions.contains_key("x-components"));
    let Some(component_input) = components.inputs.get("SharedInput") else {
        panic!("fixture should include component input");
    };
    let Some(component_input_property) = component_input.properties.get("id") else {
        panic!("fixture should include component input property");
    };
    let Some(component_parameter) = components.parameters.get("RequestId") else {
        panic!("fixture should include component parameter");
    };
    let Some(request_body) = step.request_body.as_ref() else {
        panic!("fixture should include requestBody");
    };
    let Some(component_success_action) = components.success_actions.get("Done") else {
        panic!("fixture should include component success action");
    };
    assert!(component_input.extensions.contains_key("x-schema"));
    assert!(component_input_property
        .extensions
        .contains_key("x-property"));
    assert!(component_parameter.extensions.contains_key("x-parameter"));
    assert!(workflow.extensions.contains_key("x-workflow"));
    assert!(step.extensions.contains_key("x-step"));
    assert!(step.parameters[0].extensions.contains_key("x-step-param"));
    assert!(request_body.extensions.contains_key("x-body"));
    assert!(criterion.extensions.contains_key("x-criterion"));
    assert!(criterion_type.extensions.contains_key("x-criterion-type"));
    assert!(step.on_success[0].extensions.contains_key("x-action"));
    assert!(component_success_action.extensions.contains_key("x-action"));

    let serialized = serialize_spec(&spec, "spec with nested extensions");
    let reparsed = parse_spec(
        serialized.as_bytes(),
        "serialized spec with nested extensions",
    );
    assert_eq!(spec, reparsed);
}

#[test]
fn parse_preserves_all_extension_value_shapes() {
    let raw = r#"
arazzo: "1.0.0"
x-null: null
x-string: text
x-number: 42
x-bool: true
x-array:
  - one
  - two
x-object:
  nested: true
info:
  title: Value Shape Test
  version: "1.0.0"
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
workflows:
  - workflowId: wf
    steps:
      - stepId: call
        operationPath: /get
"#;

    let spec = parse_spec(raw.as_bytes(), "spec with all extension value shapes");
    assert_eq!(
        spec.extensions.get("x-null"),
        Some(&serde_yaml_ng::Value::Null)
    );
    assert!(matches!(
        spec.extensions.get("x-string"),
        Some(serde_yaml_ng::Value::String(value)) if value == "text"
    ));
    assert!(matches!(
        spec.extensions.get("x-number"),
        Some(serde_yaml_ng::Value::Number(_))
    ));
    assert!(matches!(
        spec.extensions.get("x-bool"),
        Some(serde_yaml_ng::Value::Bool(true))
    ));
    assert!(matches!(
        spec.extensions.get("x-array"),
        Some(serde_yaml_ng::Value::Sequence(_))
    ));
    assert!(matches!(
        spec.extensions.get("x-object"),
        Some(serde_yaml_ng::Value::Mapping(_))
    ));

    let serialized = serialize_spec(&spec, "spec with all extension value shapes");
    let reparsed = parse_spec(
        serialized.as_bytes(),
        "serialized spec with all extension value shapes",
    );
    assert_eq!(spec, reparsed);
}

#[test]
fn parse_drops_non_vendor_unknown_fields() {
    let raw = r#"
arazzo: "1.0.0"
info:
  title: Unknown Field Test
  version: "1.0.0"
  unknownInfoField: false
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
workflows:
  - workflowId: wf
    steps:
      - stepId: call
        operationPath: /get
        unknownStepField:
          nested: true
unknownRootField:
  nested: true
"#;

    let spec = parse_spec(raw.as_bytes(), "spec with unknown non-extension fields");
    let serialized = serialize_spec(&spec, "spec with unknown non-extension fields");

    assert!(
        !serialized.contains("unknownRootField"),
        "non-extension root fields should not survive serialization"
    );
    assert!(
        !serialized.contains("unknownInfoField"),
        "non-extension info fields should not survive serialization"
    );
    assert!(
        !serialized.contains("unknownStepField"),
        "non-extension step fields should not survive serialization"
    );
    assert_eq!(spec.workflows.len(), 1);
    assert_eq!(
        spec.workflows[0].steps[0].target,
        Some(StepTarget::OperationPath("/get".to_string()))
    );
}

#[test]
fn step_custom_serde_preserves_vendor_extensions_and_target() {
    let raw = r#"
arazzo: "1.0.0"
info:
  title: Step Extension Test
  version: "1.0.0"
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
workflows:
  - workflowId: wf
    steps:
      - stepId: call
        operationPath: /get
        x-arazzo-cli:
          stepMode: dry
"#;

    let spec = parse_spec(raw.as_bytes(), "step extension test spec");
    let step = &spec.workflows[0].steps[0];
    assert_eq!(
        step.target,
        Some(StepTarget::OperationPath("/get".to_string()))
    );
    assert!(step.extensions.contains_key("x-arazzo-cli"));

    let serialized = serialize_spec(&spec, "step extension test spec");
    assert!(serialized.contains("operationPath: /get"));
    assert!(serialized.contains("x-arazzo-cli"));

    let reparsed = parse_spec(serialized.as_bytes(), "serialized step extension test spec");
    assert_eq!(
        reparsed.workflows[0].steps[0].target,
        Some(StepTarget::OperationPath("/get".to_string()))
    );
    assert!(reparsed.workflows[0].steps[0]
        .extensions
        .contains_key("x-arazzo-cli"));
}

#[test]
fn parse_applies_defaults_for_optional_fields() {
    let raw = r#"
arazzo: "1.0.0"
info:
  title: Defaults Test
  version: "1.0.0"
sourceDescriptions:
  - name: testApi
    type: openapi
    url: https://example.com/openapi.yaml
workflows:
  - workflowId: wf
    steps:
      - stepId: call
        operationPath: /get
        onSuccess:
          - name: done
"#;

    let spec = parse_spec(raw.as_bytes(), "defaults test spec");
    assert_eq!(spec.components, None);
    assert_eq!(spec.info.summary, "");
    assert_eq!(spec.info.description, "");
    assert_eq!(spec.source_descriptions[0].type_, SourceType::OpenApi);

    let workflow = &spec.workflows[0];
    assert_eq!(workflow.summary, "");
    assert_eq!(workflow.description, "");
    assert_eq!(workflow.inputs, None);
    assert!(workflow.outputs.is_empty());
    assert!(workflow.parameters.is_empty());
    assert!(workflow.failure_actions.is_empty());

    let step = &workflow.steps[0];
    assert_eq!(step.description, "");
    assert!(step.parameters.is_empty());
    assert_eq!(step.request_body, None);
    assert!(step.success_criteria.is_empty());
    assert!(step.on_failure.is_empty());
    assert!(step.outputs.is_empty());
    assert_eq!(step.on_success[0].type_, ActionType::End);
    assert_eq!(step.on_success[0].workflow_id, "");
    assert_eq!(step.on_success[0].step_id, "");
    assert_eq!(step.on_success[0].retry_after, 0);
    assert_eq!(step.on_success[0].retry_limit, None);
    assert!(step.on_success[0].criteria.is_empty());
}
