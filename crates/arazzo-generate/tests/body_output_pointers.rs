#![forbid(unsafe_code)]
#![allow(clippy::expect_used)]

//! Generated CRUD response ID outputs keep the selected property as one
//! canonical JSON Pointer token, including when its name contains punctuation.

use arazzo_generate::crud::generate_crud;
use arazzo_spec::OutputValue;
use serde_json::{json, Value as JsonValue};

fn generated_create_output(property: &str) -> String {
    let property_key = serde_json::to_string(property).expect("property name is valid JSON");
    let openapi_yaml = format!(
        r#"
openapi: "3.0.3"
info:
  title: Widgets
  version: "1.0.0"
servers:
  - url: https://widgets.example.test
paths:
  /widgets:
    post:
      responses:
        "201":
          description: Created
          content:
            application/json:
              schema:
                type: object
                properties:
                  {property_key}: {{ type: string }}
  /widgets/{{widgetId}}:
    get:
      responses:
        "200":
          description: Found
"#
    );
    let openapi: openapiv3::OpenAPI =
        serde_yaml_ng::from_str(&openapi_yaml).expect("minimal OpenAPI fixture parses");
    let generated = generate_crud(&openapi, "widgets.openapi.yaml", "widgets.openapi.yaml")
        .expect("CRUD generation succeeds");

    let workflow = generated
        .spec
        .workflows
        .iter()
        .find(|workflow| workflow.workflow_id == "crud-widgets")
        .expect("widgets workflow is generated");
    let create_step = workflow
        .steps
        .iter()
        .find(|step| step.step_id == "create-widgets")
        .expect("create step is generated");

    match create_step.outputs.get(property) {
        Some(OutputValue::RuntimeExpression(expression)) => expression.clone(),
        Some(OutputValue::Selector(_)) => panic!("generated output must be an expression"),
        None => panic!("selected response property remains the output key"),
    }
}

#[test]
fn response_id_output_is_one_canonical_pointer_token() {
    let body = json!({
        "id": "ordinary-id",
        "a.bId": "literal-dot-key",
        "a": { "bId": "nested-decoy" },
        "a/bId": "literal-slash-key",
        "a~bId": "literal-tilde-key",
        "a~/bId": "literal-combined-key",
        "a~": { "bId": "escaped-nested-decoy" }
    });
    let cases = [
        ("id", "$response.body#/id", "ordinary-id"),
        ("a.bId", "$response.body#/a.bId", "literal-dot-key"),
        ("a/bId", "$response.body#/a~1bId", "literal-slash-key"),
        ("a~bId", "$response.body#/a~0bId", "literal-tilde-key"),
        ("a~/bId", "$response.body#/a~0~1bId", "literal-combined-key"),
    ];

    for (property, expected_expression, expected_value) in cases {
        let expression = generated_create_output(property);
        assert_eq!(expression, expected_expression, "property {property:?}");
        assert!(!expression.starts_with("$response.body."));

        let pointer = expression
            .strip_prefix("$response.body#")
            .expect("generated response body expression uses canonical # suffix");
        assert_eq!(
            pointer.split('/').count() - 1,
            1,
            "one token for {property:?}"
        );
        assert_eq!(
            body.pointer(pointer),
            Some(&JsonValue::String(expected_value.to_string())),
            "pointer resolves the literal property {property:?}"
        );
    }

    let dotted_expression = generated_create_output("a.bId");
    let dotted_pointer = dotted_expression
        .strip_prefix("$response.body#")
        .expect("dotted property still uses a JSON Pointer");
    assert_eq!(
        body.pointer(dotted_pointer),
        body.get("a.bId"),
        "the dotted name is one property, not nested traversal"
    );
    assert_ne!(
        body.pointer("/a/bId"),
        body.get("a.bId"),
        "the dotted name is not split into nested pointer tokens"
    );
}
