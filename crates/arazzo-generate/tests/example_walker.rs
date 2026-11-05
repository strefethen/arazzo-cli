#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use arazzo_generate::crud::{generate_crud, GenerateOutput};
use serde_json::{json, Value};

fn generate(schema: Value, schemas: Value) -> GenerateOutput {
    try_generate(schema, schemas).unwrap()
}

fn try_generate(schema: Value, schemas: Value) -> Result<GenerateOutput, String> {
    let operation = json!({
        "requestBody": {"content": {"application/json": {"schema": schema}}},
        "responses": {"200": {"description": "OK"}}
    });
    let openapi = serde_json::from_value(json!({
        "openapi": "3.0.3", "info": {"title": "Example", "version": "1"},
        "servers": [{"url": "https://example.com"}],
        "paths": {
            "/items": {"post": operation},
            "/items/{itemId}": {"put": operation}
        },
        "components": {"schemas": schemas}
    }))
    .unwrap();
    generate_crud(&openapi, "example.yaml", "example.yaml")
}

fn cost(value: &Value) -> usize {
    match value {
        Value::String(value) => 1 + value.chars().count(),
        Value::Array(values) => 1 + values.iter().map(cost).sum::<usize>(),
        Value::Object(values) => {
            1 + values
                .iter()
                .map(|(key, value)| key.chars().count() + cost(value))
                .sum::<usize>()
        }
        _ => 1,
    }
}

fn null_key_allowance(value: &Value) -> usize {
    match value {
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                if value.is_null() {
                    key.chars().count() + 1
                } else {
                    null_key_allowance(value)
                }
            })
            .sum(),
        Value::Array(values) => values.iter().map(null_key_allowance).sum(),
        _ => 0,
    }
}

#[test]
fn wide_reference_bodies_warn_once_for_each_step() {
    let names: Vec<String> = (0..16).map(|i| format!("field{i}")).collect();
    let mut schemas = serde_json::Map::new();
    for level in 0..6 {
        let properties: serde_json::Map<String, Value> = names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    if level == 5 {
                        json!({"type": "string"})
                    } else {
                        json!({"$ref": format!("#/components/schemas/Level{}", level + 1)})
                    },
                )
            })
            .collect();
        schemas.insert(
            format!("Level{level}"),
            json!({"type": "object", "required": names, "properties": properties}),
        );
    }
    let result = generate(
        json!({"$ref": "#/components/schemas/Level0"}),
        Value::Object(schemas),
    );
    assert_eq!(result.warnings, vec![
        "request body example for step \"create-items\" exceeded the example size limit (1000) and was trimmed; fields past the limit are null",
        "request body example for step \"update-items\" exceeded the example size limit (1000) and was trimmed; fields past the limit are null",
    ]);
    assert_eq!(result.spec.workflows[0].steps.len(), 2);
    for step in &result.spec.workflows[0].steps {
        let body = serde_json::to_value(
            step.request_body
                .as_ref()
                .unwrap()
                .payload
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert!(body.is_object());
        assert!(cost(&body) <= 1000 + null_key_allowance(&body));
    }
}

#[test]
fn petstore_bodies_need_no_trimming() {
    let openapi =
        serde_yaml_ng::from_str(include_str!("../../../testdata/petstore.openapi.yaml")).unwrap();
    let output = generate_crud(&openapi, "petstore.openapi.yaml", "petstore.openapi.yaml").unwrap();
    assert!(output.warnings.is_empty(), "{:?}", output.warnings);
}

#[test]
fn typed_password_body_uses_the_shared_format_table() {
    let result = generate(json!({"type": "string", "format": "password"}), json!({}));
    assert!(result.warnings.is_empty());
    for step in &result.spec.workflows[0].steps {
        let body = serde_json::to_value(
            step.request_body
                .as_ref()
                .unwrap()
                .payload
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body, json!("P@ssw0rd123"));
    }
}

#[test]
fn unrepresentable_numeric_bound_fails_generation_with_step_context() {
    let error = try_generate(
        json!({"type": "number", "minimum": f64::MAX, "exclusiveMinimum": true}),
        json!({}),
    )
    .err()
    .unwrap();
    assert_eq!(
        error,
        "request body example for step \"create-items\": cannot synthesize a JSON number for \"body\" satisfying minimum 1.7976931348623157e308 with exclusiveMinimum: true"
    );
    let error = try_generate(
        json!({"$ref": "#/components/schemas/Item"}),
        json!({"Item": {"type": "object", "required": ["count"], "properties": {
            "count": {"type": "integer", "maximum": i64::MIN, "exclusiveMaximum": true},
            "price": {"type": "number", "maximum": -f64::MAX, "exclusiveMaximum": true}
        }}}),
    )
    .err()
    .unwrap();
    assert_eq!(
        error,
        "request body example for step \"create-items\": cannot synthesize a JSON number for \"price\" satisfying maximum -1.7976931348623157e308 with exclusiveMaximum: true"
    );
}

#[test]
fn representable_strict_neighbors_still_generate() {
    let result = generate(
        json!({"type": "object", "required": ["count", "price"], "properties": {
            "count": {"type": "integer", "minimum": i64::MAX, "exclusiveMinimum": true},
            "price": {"type": "number", "minimum": f64::MAX.next_down(), "exclusiveMinimum": true}
        }}),
        json!({}),
    );
    assert!(result.warnings.is_empty());
    let body = serde_json::to_value(
        result.spec.workflows[0].steps[0]
            .request_body
            .as_ref()
            .unwrap()
            .payload
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body,
        json!({"count": 9_223_372_036_854_775_808_u64, "price": f64::MAX})
    );
}
