mod common;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{
    ArazzoSpec, JsonSchemaType, PropertyDef, SchemaObject, SourceDescription, SourceType, Step,
    StepTarget, Workflow,
};
use common::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn yaml_value(input: &str) -> serde_yaml_ng::Value {
    match serde_yaml_ng::from_str(input) {
        Ok(value) => value,
        Err(err) => panic!("parsing test YAML: {err}"),
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-input-validation-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.path.join(name);
        fs::write(&path, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn workflow_with_required_input() -> Workflow {
    let mut properties = BTreeMap::new();
    properties.insert(
        "name".to_string(),
        PropertyDef {
            type_: Some(JsonSchemaType::String),
            ..PropertyDef::default()
        },
    );
    Workflow {
        workflow_id: "test-inputs".to_string(),
        inputs: Some(SchemaObject {
            properties,
            required: vec!["name".to_string()],
            ..SchemaObject::default()
        }),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/test".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }
}

fn workflow_with_preference_enum(default: Option<&str>) -> Workflow {
    let mut extensions = BTreeMap::new();
    extensions.insert(
        "enum".to_string(),
        yaml_value("- sunrise\n- sunset\n- both\n"),
    );
    let preference = PropertyDef {
        type_: Some(JsonSchemaType::String),
        default: default.map(|value| serde_yaml_ng::Value::String(value.to_string())),
        extensions,
        ..PropertyDef::default()
    };
    Workflow {
        workflow_id: "test-enum-inputs".to_string(),
        inputs: Some(SchemaObject {
            properties: BTreeMap::from([("preference".to_string(), preference)]),
            ..SchemaObject::default()
        }),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationId("getPreference".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }
}

fn enum_input_spec(default: Option<&str>) -> (TempDir, ArazzoSpec) {
    let fixture = TempDir::new();
    fixture.write(
        "openapi.yaml",
        r#"
openapi: 3.0.3
info:
  title: Input validation fixture
  version: 1.0.0
servers:
  - url: https://example.invalid
paths:
  /preferences:
    get:
      operationId: getPreference
      responses:
        '200':
          description: Success
"#,
    );
    let mut spec = make_spec(vec![workflow_with_preference_enum(default)]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions = vec![SourceDescription {
        name: "local-api".to_string(),
        url: "./openapi.yaml".to_string(),
        type_: SourceType::OpenApi,
        ..SourceDescription::default()
    }];
    (fixture, spec)
}

#[tokio::test]
async fn strict_inputs_rejects_missing_required() {
    let spec = make_spec(vec![workflow_with_required_input()]);
    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .strict_inputs(true)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("test-inputs", BTreeMap::new()).await;

    match result.outputs {
        Err(err) => {
            assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
            assert!(
                err.message.contains("required"),
                "error message should mention 'required': {}",
                err.message
            );
        }
        Ok(outputs) => panic!("expected InputValidation error, got outputs: {outputs:?}"),
    }
}

#[tokio::test]
async fn non_strict_does_not_reject() {
    let spec = make_spec(vec![workflow_with_required_input()]);
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("test-inputs", BTreeMap::new()).await;

    // Should not be an InputValidation error. It may fail for other reasons
    // (e.g. dry-run produces empty outputs), but not input validation.
    if let Err(err) = &result.outputs {
        assert_ne!(
            err.kind,
            RuntimeErrorKind::InputValidation,
            "non-strict mode should not produce InputValidation error"
        );
    }
}

#[tokio::test]
async fn strict_inputs_rejects_property_enum_non_member() {
    let (fixture, spec) = enum_input_spec(None);
    let engine = match EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .dry_run(true)
        .strict_inputs(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine
        .execute_collect(
            "test-enum-inputs",
            BTreeMap::from([("preference".to_string(), json!("invalid"))]),
        )
        .await;

    let err = match result.outputs {
        Err(err) => err,
        Ok(outputs) => panic!("strict inputs should reject enum non-member: {outputs:?}"),
    };
    assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
    assert!(err
        .message
        .contains("value is not one of the declared enum values"));
    assert!(!err.message.contains("invalid"));
    assert!(!err.message.contains("sunrise"));
}

#[tokio::test]
async fn non_strict_property_enum_non_member_warns_and_dry_run_continues() {
    let (fixture, spec) = enum_input_spec(None);
    let engine = match EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .dry_run(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine
        .execute_collect(
            "test-enum-inputs",
            BTreeMap::from([("preference".to_string(), json!("invalid"))]),
        )
        .await;

    assert!(
        result.outputs.is_ok(),
        "non-strict enum validation should continue: {:?}",
        result.outputs
    );
    assert_eq!(result.dry_run_requests().len(), 1);
}

#[tokio::test]
async fn strict_property_enum_accepts_members_and_injected_default() {
    let members = ["sunrise", "sunset", "both"];
    for member in members {
        let (fixture, spec) = enum_input_spec(None);
        let engine = match EngineBuilder::new(spec)
            .source_base_dir(fixture.path())
            .dry_run(true)
            .strict_inputs(true)
            .build()
        {
            Ok(engine) => engine,
            Err(err) => panic!("building engine: {err}"),
        };
        let result = engine
            .execute_collect(
                "test-enum-inputs",
                BTreeMap::from([("preference".to_string(), json!(member))]),
            )
            .await;
        assert!(
            result.outputs.is_ok(),
            "member {member:?}: {:?}",
            result.outputs
        );
        assert_eq!(result.dry_run_requests().len(), 1);
    }

    let (fixture, spec) = enum_input_spec(Some("both"));
    let engine = match EngineBuilder::new(spec)
        .source_base_dir(fixture.path())
        .dry_run(true)
        .strict_inputs(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine
        .execute_collect("test-enum-inputs", BTreeMap::new())
        .await;
    assert!(
        result.outputs.is_ok(),
        "matching injected default should pass: {:?}",
        result.outputs
    );
    assert_eq!(result.dry_run_requests().len(), 1);
}
