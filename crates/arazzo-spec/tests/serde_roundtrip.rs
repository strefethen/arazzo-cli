#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use arazzo_spec::{parse_unvalidated_bytes, ActionType, ArazzoSpec, SourceType, StepTarget};

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
fn parse_ignores_unknown_fields_and_drops_them_on_serialize() {
    let raw = r#"
arazzo: "1.0.0"
info:
  title: Unknown Field Test
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
unknownRootField:
  nested: true
"#;

    let spec = parse_spec(raw.as_bytes(), "spec with unknown root fields");
    let serialized = serialize_spec(&spec, "spec with unknown root fields");

    assert!(
        !serialized.contains("unknownRootField"),
        "unknown fields should not survive serialization"
    );
    assert_eq!(spec.workflows.len(), 1);
    assert_eq!(
        spec.workflows[0].steps[0].target,
        Some(StepTarget::OperationPath("/get".to_string()))
    );
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
