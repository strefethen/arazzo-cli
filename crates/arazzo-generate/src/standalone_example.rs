//! Generate example values from raw JSON Schema objects (no OpenAPI file needed).

use serde_json::Value;

use crate::examples;

/// Generate a realistic example value from a JSON Schema object.
///
/// Accepts a raw `serde_json::Value` representing a JSON Schema with fields like
/// `type`, `format`, `enum`, `minimum`, `maximum`, `properties`, `items`, etc.
/// Delegates to the typed example pipeline in `examples.rs`.
pub fn generate_from_json_schema(schema: &Value, field_name: &str) -> Value {
    generate_recursive(schema, field_name, 0)
}

fn generate_recursive(schema: &Value, field_name: &str, depth: usize) -> Value {
    if depth > 5 {
        return Value::Null;
    }

    // Priority 1: explicit example
    if let Some(example) = schema.get("example") {
        return example.clone();
    }

    // Priority 2: default
    if let Some(default) = schema.get("default") {
        return default.clone();
    }

    let type_str = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    match type_str {
        "string" => generate_string(schema, field_name),
        "integer" => generate_integer(schema),
        "number" => generate_number(schema),
        "boolean" => generate_boolean(schema),
        "array" => generate_array(schema, field_name, depth),
        "object" => generate_object(schema, field_name, depth),
        _ => Value::Null,
    }
}

fn generate_string(schema: &Value, field_name: &str) -> Value {
    // Check enum first.
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if let Some(first) = values.iter().find(|v| v.is_string()) {
            return first.clone();
        }
    }

    // Build an openapiv3::StringType from the JSON fields.
    let format = schema.get("format").and_then(Value::as_str).unwrap_or("");

    let format_field = if format.is_empty() {
        openapiv3::VariantOrUnknownOrEmpty::Empty
    } else {
        match format {
            "date-time" => {
                openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::DateTime)
            }
            "date" => openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::Date),
            other => openapiv3::VariantOrUnknownOrEmpty::Unknown(other.to_string()),
        }
    };

    let string_type = openapiv3::StringType {
        format: format_field,
        min_length: schema
            .get("minLength")
            .and_then(Value::as_u64)
            .map(|n| n as usize),
        max_length: schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .map(|n| n as usize),
        ..openapiv3::StringType::default()
    };

    examples::generate_string_example(field_name, &string_type)
}

fn generate_integer(schema: &Value) -> Value {
    // Check enum first.
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if let Some(first) = values.iter().find_map(Value::as_i64) {
            return Value::Number(first.into());
        }
    }

    let int_type = openapiv3::IntegerType {
        minimum: schema.get("minimum").and_then(Value::as_i64),
        maximum: schema.get("maximum").and_then(Value::as_i64),
        exclusive_minimum: schema
            .get("exclusiveMinimum")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        exclusive_maximum: schema
            .get("exclusiveMaximum")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..openapiv3::IntegerType::default()
    };

    examples::generate_integer_example(&int_type)
}

fn generate_number(schema: &Value) -> Value {
    // Check enum first.
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if let Some(first) = values.iter().find_map(Value::as_f64) {
            return serde_json::json!(first);
        }
    }

    let num_type = openapiv3::NumberType {
        minimum: schema.get("minimum").and_then(Value::as_f64),
        maximum: schema.get("maximum").and_then(Value::as_f64),
        exclusive_minimum: schema
            .get("exclusiveMinimum")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        exclusive_maximum: schema
            .get("exclusiveMaximum")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..openapiv3::NumberType::default()
    };

    examples::generate_number_example(&num_type)
}

fn generate_boolean(schema: &Value) -> Value {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if let Some(first) = values.iter().find_map(Value::as_bool) {
            return Value::Bool(first);
        }
    }
    Value::Bool(true)
}

fn generate_array(schema: &Value, _field_name: &str, depth: usize) -> Value {
    let item_schema = schema.get("items").unwrap_or(&Value::Null);
    if item_schema.is_null() {
        return Value::Array(vec![]);
    }

    let item_val = generate_recursive(item_schema, "item", depth + 1);
    let min_items = schema.get("minItems").and_then(Value::as_u64).unwrap_or(1) as usize;
    let count = if min_items > 1 { min_items.min(5) } else { 1 };

    Value::Array(vec![item_val; count])
}

fn generate_object(schema: &Value, _field_name: &str, depth: usize) -> Value {
    let properties = match schema.get("properties").and_then(Value::as_object) {
        Some(props) => props,
        None => return Value::Object(serde_json::Map::new()),
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut obj = serde_json::Map::new();
    let mut optional_count = 0;
    let max_optional = 5;

    // Required first.
    for name in &required {
        if let Some(prop_schema) = properties.get(*name) {
            obj.insert(
                name.to_string(),
                generate_recursive(prop_schema, name, depth + 1),
            );
        }
    }

    // Then optional (up to limit).
    for (name, prop_schema) in properties {
        if required.contains(&name.as_str()) {
            continue;
        }
        if optional_count >= max_optional {
            break;
        }
        obj.insert(
            name.clone(),
            generate_recursive(prop_schema, name, depth + 1),
        );
        optional_count += 1;
    }

    Value::Object(obj)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_string_with_format() {
        let schema = json!({"type": "string", "format": "email"});
        let result = generate_from_json_schema(&schema, "contact");
        assert_eq!(result, json!("user@example.com"));
    }

    #[test]
    fn test_string_field_name_heuristic() {
        let schema = json!({"type": "string"});
        let result = generate_from_json_schema(&schema, "phone");
        assert_eq!(result, json!("+1-555-555-0100"));
    }

    #[test]
    fn test_string_enum() {
        let schema = json!({"type": "string", "enum": ["active", "inactive"]});
        let result = generate_from_json_schema(&schema, "status");
        assert_eq!(result, json!("active"));
    }

    #[test]
    fn test_string_min_length() {
        let schema = json!({"type": "string", "minLength": 15});
        let result = generate_from_json_schema(&schema, "code");
        let s = result.as_str().unwrap();
        assert!(s.len() >= 15);
    }

    #[test]
    fn test_integer_with_minimum() {
        let schema = json!({"type": "integer", "minimum": 100});
        let result = generate_from_json_schema(&schema, "count");
        assert_eq!(result, json!(100));
    }

    #[test]
    fn test_integer_enum() {
        let schema = json!({"type": "integer", "enum": [10, 20, 30]});
        let result = generate_from_json_schema(&schema, "code");
        assert_eq!(result, json!(10));
    }

    #[test]
    fn test_number_with_minimum() {
        let schema = json!({"type": "number", "minimum": 99.5});
        let result = generate_from_json_schema(&schema, "price");
        assert_eq!(result, json!(99.5));
    }

    #[test]
    fn test_boolean_default() {
        let schema = json!({"type": "boolean"});
        let result = generate_from_json_schema(&schema, "active");
        assert_eq!(result, json!(true));
    }

    #[test]
    fn test_array_min_items() {
        let schema = json!({"type": "array", "items": {"type": "integer"}, "minItems": 3});
        let result = generate_from_json_schema(&schema, "tags");
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn test_object_with_properties() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            }
        });
        let result = generate_from_json_schema(&schema, "user");
        let obj = result.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("age"));
    }

    #[test]
    fn test_explicit_example_wins() {
        let schema = json!({"type": "string", "example": "custom-value"});
        let result = generate_from_json_schema(&schema, "anything");
        assert_eq!(result, json!("custom-value"));
    }

    #[test]
    fn test_default_wins_over_generation() {
        let schema = json!({"type": "integer", "default": 42});
        let result = generate_from_json_schema(&schema, "count");
        assert_eq!(result, json!(42));
    }
}
