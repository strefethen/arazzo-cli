#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use std::io::Cursor;

use arazzo_mcp::handlers::{generate_example, generate_workflow};
use arazzo_mcp::protocol;
use arazzo_mcp::state::ServerState;
use serde_json::{json, Value};

fn example(schema: Value) -> Value {
    let response = generate_example(&json!({"schema": schema, "field_name": "phone"})).unwrap();
    assert_eq!(response["isError"], false);
    assert_eq!(response["content"].as_array().unwrap().len(), 1);
    assert_eq!(response["content"][0]["type"], "text");
    serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn handler_uses_shared_31_shapes_and_literals() {
    for (schema, expected) in [
        (
            json!({"type": ["null", "string"]}),
            json!("+1-555-555-0100"),
        ),
        (
            json!({"type": "string", "examples": ["first", "second"]}),
            json!("first"),
        ),
        (json!({"const": 42}), json!(42)),
        (
            json!({"example": null, "default": "after-null"}),
            json!("after-null"),
        ),
        (
            json!({"type": "number", "exclusiveMinimum": 4.5}),
            json!(5.5),
        ),
        (
            json!({"properties": {"name": {"type": "string"}}}),
            json!({"name": "Jane Doe"}),
        ),
        (
            json!({"type": "string", "format": "email"}),
            json!("user@example.com"),
        ),
        (
            json!({"type": "string", "format": "password"}),
            json!("P@ssw0rd123"),
        ),
        (
            json!({"type": "string", "format": "byte"}),
            json!("SGVsbG8gV29ybGQ="),
        ),
    ] {
        assert_eq!(example(schema), expected);
    }
}

#[test]
fn handler_resolves_local_refs_and_stops_cycles() {
    assert_eq!(
        example(
            json!({"$ref": "#/$defs/a~1b", "$defs": {"a/b": {"type": "integer", "minimum": 12}}})
        ),
        json!(12)
    );
    assert_eq!(
        example(json!({"$ref": "#/$defs/loop", "$defs": {"loop": {"$ref": "#/$defs/loop"}}})),
        Value::Null
    );
    assert_eq!(example(json!({"$ref": "other.yaml#/A"})), Value::Null);
}

#[test]
fn handler_bounds_oversized_values_without_changing_the_envelope() {
    assert_eq!(example(json!({"example": "x".repeat(2000)})), Value::Null);
    assert_eq!(
        example(json!({"type": "string", "minLength": 4000000000u64})),
        Value::Null
    );
    assert_eq!(
        example(json!({"type": "array", "minItems": 5, "items": {"example": "x".repeat(600)}})),
        json!(["x".repeat(600)])
    );
}

#[test]
fn handler_preserves_missing_argument_error_and_default_field_name() {
    assert_eq!(
        generate_example(&json!({})),
        Err("missing required argument: schema".into())
    );
    let response = generate_example(&json!({"schema": {"type": "string"}})).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(response["content"][0]["text"].as_str().unwrap()).unwrap(),
        json!("example-value")
    );
}

#[test]
fn handler_reports_unrepresentable_numeric_bounds_through_the_tool_error_path() {
    let message = "cannot synthesize a JSON number for \"price\" satisfying exclusiveMinimum 1.7976931348623157e308";
    assert_eq!(
        generate_example(&json!({
            "schema": {"type": "number", "exclusiveMinimum": f64::MAX},
            "field_name": "price"
        })),
        Err(message.to_owned())
    );
    let call = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
        "name": "generate_example",
        "arguments": {"schema": {"type": "number", "exclusiveMinimum": f64::MAX}, "field_name": "price"}
    }});
    let mut output = Vec::new();
    protocol::serve(
        Cursor::new(format!("{call}\n")),
        &mut output,
        &ServerState::empty(),
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        response,
        json!({"jsonrpc": "2.0", "id": 7, "result": {
            "content": [{"type": "text", "text": message}],
            "isError": true
        }})
    );
    assert_eq!(
        example(json!({"type": "number", "exclusiveMinimum": f64::MAX.next_down()})),
        json!(f64::MAX)
    );
    assert_eq!(
        example(json!({"type": "number", "exclusiveMinimum": f64::MAX, "example": 5})),
        json!(5)
    );
}

#[test]
fn workflow_generation_reports_unrepresentable_bounds_as_tool_errors() {
    let dir = std::env::temp_dir().join(format!(
        "arazzo-mcp-numeric-bound-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let spec_path = dir.join("bounds.openapi.yaml");
    std::fs::write(
        &spec_path,
        "openapi: 3.0.3\ninfo: {title: Bounds, version: '1'}\nservers: [{url: 'https://example.com'}]\npaths:\n  /items:\n    post:\n      requestBody:\n        content:\n          application/json:\n            schema: {type: number, minimum: 1.7976931348623157e308, exclusiveMinimum: true}\n      responses: {'200': {description: OK}}\n  /items/{itemId}:\n    get:\n      parameters: [{name: itemId, in: path, required: true, schema: {type: string}}]\n      responses: {'200': {description: OK}}\n",
    )
    .unwrap();
    let state = ServerState::load(&[], Some(vec![dir.to_string_lossy().into_owned()])).unwrap();
    let response =
        generate_workflow(&state, &json!({"file_path": spec_path.to_string_lossy()})).unwrap();
    let response = if cfg!(target_os = "linux") {
        response
    } else {
        let error: Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(error["code"], "MCP_FILE_DENIED");
        let openapi =
            arazzo_generate::parse_openapi_bytes(&std::fs::read(&spec_path).unwrap()).unwrap();
        let message = arazzo_generate::crud::generate_crud(
            &openapi,
            spec_path.to_str().unwrap(),
            "bounds.openapi.yaml",
        )
        .err()
        .unwrap();
        json!({"content": [{"type": "text", "text": message}], "isError": true})
    };
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        response,
        json!({
            "content": [{"type": "text", "text": "request body example for step \"create-items\": cannot synthesize a JSON number for \"body\" satisfying minimum 1.7976931348623157e308 with exclusiveMinimum: true"}],
            "isError": true
        })
    );
}
