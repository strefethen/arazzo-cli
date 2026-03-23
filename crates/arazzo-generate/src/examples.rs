//! Example value generation from OpenAPI schemas.

use std::collections::HashSet;

use indexmap::IndexMap;
use openapiv3::ReferenceOr;
use serde_json::Value;

use crate::refs::{ref_box_to_ref, resolve_schema_ref};

pub fn generate_example(
    schema_ref: &ReferenceOr<openapiv3::Schema>,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    if depth > 5 {
        return Value::Null;
    }

    let mut visited = HashSet::new();
    let schema = match resolve_schema_ref(schema_ref, components, &mut visited) {
        Some(s) => s,
        None => return Value::Null,
    };

    generate_example_from_schema(schema, field_name, components, depth)
}

pub fn generate_example_from_schema(
    schema: &openapiv3::Schema,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    if depth > 5 {
        return Value::Null;
    }

    // Check for explicit example.
    if let Some(example) = &schema.schema_data.example {
        return example.clone();
    }

    // Check for default.
    if let Some(default) = &schema.schema_data.default {
        return default.clone();
    }

    match &schema.schema_kind {
        openapiv3::SchemaKind::Type(type_info) => {
            generate_from_type(type_info, field_name, components, depth)
        }
        openapiv3::SchemaKind::Any(any) => generate_from_any(any, field_name, components, depth),
        _ => Value::Null,
    }
}

pub fn generate_from_type(
    type_info: &openapiv3::Type,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    match type_info {
        openapiv3::Type::String(s) => {
            // Enum takes priority.
            if let Some(val) = first_enum_value(&s.enumeration) {
                return Value::String(val);
            }
            generate_string_example(field_name, s)
        }
        openapiv3::Type::Integer(i) => {
            if let Some(val) = first_enum_value(&i.enumeration) {
                return Value::Number(val.into());
            }
            generate_integer_example(i)
        }
        openapiv3::Type::Number(n) => {
            if let Some(val) = first_enum_value(&n.enumeration) {
                return serde_json::json!(val);
            }
            generate_number_example(n)
        }
        openapiv3::Type::Boolean(b) => {
            if let Some(val) = first_enum_value(&b.enumeration) {
                return Value::Bool(val);
            }
            Value::Bool(true)
        }
        openapiv3::Type::Array(arr) => {
            if let Some(items) = &arr.items {
                let item_ref = match items {
                    ReferenceOr::Item(schema) => ReferenceOr::Item(*schema.clone()),
                    ReferenceOr::Reference { reference } => ReferenceOr::Reference {
                        reference: reference.clone(),
                    },
                };
                let item_val = generate_example(&item_ref, "item", components, depth + 1);
                let count = arr
                    .min_items
                    .filter(|&n| n > 1)
                    .map(|n| n.min(5))
                    .unwrap_or(1);
                Value::Array(vec![item_val; count])
            } else {
                Value::Array(vec![])
            }
        }
        openapiv3::Type::Object(obj) => {
            generate_object_example(&obj.properties, &obj.required, components, depth)
        }
    }
}

/// Returns the first `Some` value from an enumeration vec.
pub fn first_enum_value<T: Clone>(enumeration: &[Option<T>]) -> Option<T> {
    enumeration.iter().find_map(|v| v.clone())
}

pub fn generate_integer_example(i: &openapiv3::IntegerType) -> Value {
    let val = if let Some(min) = i.minimum {
        if i.exclusive_minimum {
            min + 1
        } else {
            min
        }
    } else if let Some(max) = i.maximum {
        if max < 1 {
            if i.exclusive_maximum {
                max - 1
            } else {
                max
            }
        } else {
            1
        }
    } else {
        1
    };
    Value::Number(val.into())
}

pub fn generate_number_example(n: &openapiv3::NumberType) -> Value {
    let val = if let Some(min) = n.minimum {
        if n.exclusive_minimum {
            min + 1.0
        } else {
            min
        }
    } else if let Some(max) = n.maximum {
        if max < 1.0 {
            if n.exclusive_maximum {
                max - 1.0
            } else {
                max
            }
        } else {
            1.0
        }
    } else {
        1.0
    };
    serde_json::json!(val)
}

pub fn generate_from_any(
    any: &openapiv3::AnySchema,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    // If it has properties, treat as object.
    if !any.properties.is_empty() {
        return generate_object_example(&any.properties, &any.required, components, depth);
    }

    // Fall back to type hint.
    if let Some(ref ty) = any.typ {
        match ty.as_str() {
            "string" => {
                let s = openapiv3::StringType::default();
                return generate_string_example(field_name, &s);
            }
            "integer" => return Value::Number(1.into()),
            "number" => return serde_json::json!(1.0),
            "boolean" => return Value::Bool(true),
            _ => {}
        }
    }

    Value::Null
}

pub fn generate_object_example(
    properties: &IndexMap<String, ReferenceOr<Box<openapiv3::Schema>>>,
    required: &[String],
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    let mut obj = serde_json::Map::new();
    let mut count = 0;
    let max_optional = 5;

    // Required first.
    for name in required {
        if let Some(prop_ref) = properties.get(name) {
            let prop_ref = ref_box_to_ref(prop_ref);
            obj.insert(
                name.clone(),
                generate_example(&prop_ref, name, components, depth + 1),
            );
        }
    }

    // Then optional (up to limit).
    for (name, prop_ref) in properties {
        if required.contains(name) {
            continue;
        }
        if count >= max_optional {
            break;
        }
        let prop_ref = ref_box_to_ref(prop_ref);
        obj.insert(
            name.clone(),
            generate_example(&prop_ref, name, components, depth + 1),
        );
        count += 1;
    }

    Value::Object(obj)
}

pub fn generate_string_example(field_name: &str, string_type: &openapiv3::StringType) -> Value {
    // 1. Format-based values take highest priority.
    let base = match &string_type.format {
        openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::DateTime) => {
            "2024-01-01T00:00:00Z".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::Date) => {
            "2024-01-01".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "email" => {
            "user@example.com".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "uuid" => {
            "550e8400-e29b-41d4-a716-446655440000".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "uri" || s == "url" => {
            "https://example.com".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "password" => {
            "P@ssw0rd123".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "byte" => {
            "SGVsbG8gV29ybGQ=".to_string()
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "ipv4" => "192.0.2.1".to_string(),
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "ipv6" => "2001:db8::1".to_string(),
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "hostname" => {
            "api.example.com".to_string()
        }
        _ => {
            // 2. Field-name heuristics (no format matched).
            let lower = field_name.to_lowercase();
            field_name_heuristic(&lower).unwrap_or_else(|| format!("example-{field_name}"))
        }
    };

    // 3. Apply minLength/maxLength constraints.
    apply_length_constraints(base, string_type.min_length, string_type.max_length)
}

/// Match common field name patterns to realistic example values.
pub fn field_name_heuristic(lower: &str) -> Option<String> {
    // More specific patterns first.
    if lower.ends_with("first_name") || lower.ends_with("firstname") {
        return Some("Jane".to_string());
    }
    if lower.ends_with("last_name") || lower.ends_with("lastname") {
        return Some("Doe".to_string());
    }
    if lower.ends_with("phone_number") || lower.ends_with("phone") {
        return Some("+1-555-555-0100".to_string());
    }
    if lower.ends_with("postal_code") || lower.ends_with("zipcode") || lower.ends_with("zip") {
        return Some("90210".to_string());
    }
    if lower.ends_with("country_code") || lower.ends_with("country") {
        return Some("US".to_string());
    }
    if lower.ends_with("address") || lower.ends_with("street") {
        return Some("123 Main St".to_string());
    }
    if lower.ends_with("city") {
        return Some("San Francisco".to_string());
    }
    if lower.ends_with("state") || lower.ends_with("province") {
        return Some("CA".to_string());
    }
    // General _name / name after more specific *_name patterns.
    if lower.ends_with("_name") || lower == "name" {
        return Some("Jane Doe".to_string());
    }
    if lower == "username" || lower == "user_name" {
        return Some("jdoe".to_string());
    }
    if lower == "description" {
        return Some("A sample description".to_string());
    }
    if lower == "title" {
        return Some("Sample Title".to_string());
    }
    None
}

/// Pad or truncate a string to satisfy minLength/maxLength.
/// JSON Schema defines these in Unicode code points, so we use `chars()`.
pub fn apply_length_constraints(
    mut s: String,
    min_length: Option<usize>,
    max_length: Option<usize>,
) -> Value {
    if let Some(min) = min_length {
        while s.chars().count() < min {
            s.push('a');
        }
    }
    if let Some(max) = max_length {
        if s.chars().count() > max {
            s = s.chars().take(max).collect();
        }
    }
    Value::String(s)
}

/// Convert serde_json::Value to serde_yaml_ng::Value.
pub fn json_to_yml(v: Value) -> serde_yaml_ng::Value {
    match v {
        Value::Null => serde_yaml_ng::Value::Null,
        Value::Bool(b) => serde_yaml_ng::Value::Bool(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml_ng::Value::Number(serde_yaml_ng::Number::from(i))
            } else if let Some(u) = n.as_u64() {
                serde_yaml_ng::Value::Number(serde_yaml_ng::Number::from(u))
            } else if let Some(f) = n.as_f64() {
                serde_yaml_ng::Value::Number(serde_yaml_ng::Number::from(f))
            } else {
                serde_yaml_ng::Value::Null
            }
        }
        Value::String(s) => serde_yaml_ng::Value::String(s),
        Value::Array(arr) => {
            serde_yaml_ng::Value::Sequence(arr.into_iter().map(json_to_yml).collect())
        }
        Value::Object(map) => {
            let mut m = serde_yaml_ng::Mapping::new();
            for (k, v) in map {
                m.insert(serde_yaml_ng::Value::String(k), json_to_yml(v));
            }
            serde_yaml_ng::Value::Mapping(m)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_example_generation_string() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType::default(),
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "myField", &None, 0);
        assert_eq!(result, Value::String("example-myField".to_string()));
    }

    #[test]
    fn test_example_generation_integer() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType::default(),
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "count", &None, 0);
        assert_eq!(result, serde_json::json!(1));
    }

    #[test]
    fn test_enum_string() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType {
                    enumeration: vec![Some("active".into()), Some("inactive".into())],
                    ..openapiv3::StringType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "status", &None, 0);
        assert_eq!(result, Value::String("active".to_string()));
    }

    #[test]
    fn test_enum_integer() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType {
                    enumeration: vec![Some(10), Some(20), Some(30)],
                    ..openapiv3::IntegerType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "code", &None, 0);
        assert_eq!(result, serde_json::json!(10));
    }

    #[test]
    fn test_integer_minimum() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType {
                    minimum: Some(100),
                    ..openapiv3::IntegerType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "count", &None, 0);
        assert_eq!(result, serde_json::json!(100));
    }

    #[test]
    fn test_integer_maximum_below_default() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType {
                    maximum: Some(-5),
                    ..openapiv3::IntegerType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "offset", &None, 0);
        assert_eq!(result, serde_json::json!(-5));
    }

    #[test]
    fn test_integer_exclusive_minimum() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType {
                    minimum: Some(0),
                    exclusive_minimum: true,
                    ..openapiv3::IntegerType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "positive", &None, 0);
        assert_eq!(result, serde_json::json!(1));
    }

    #[test]
    fn test_number_minimum() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Number(
                openapiv3::NumberType {
                    minimum: Some(99.5),
                    ..openapiv3::NumberType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "price", &None, 0);
        assert_eq!(result, serde_json::json!(99.5));
    }

    #[test]
    fn test_string_min_length() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType {
                    min_length: Some(10),
                    ..openapiv3::StringType::default()
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "code", &None, 0);
        let s = result.as_str().unwrap();
        assert!(s.len() >= 10, "got length {}: {s}", s.len());
    }

    #[test]
    fn test_string_field_name_phone() {
        let s = openapiv3::StringType::default();
        let result = generate_string_example("phone", &s);
        assert_eq!(result, Value::String("+1-555-555-0100".to_string()));
    }

    #[test]
    fn test_string_field_name_email_format_wins() {
        let s = openapiv3::StringType {
            format: openapiv3::VariantOrUnknownOrEmpty::Unknown("email".to_string()),
            ..openapiv3::StringType::default()
        };
        let result = generate_string_example("phone", &s);
        assert_eq!(result, Value::String("user@example.com".to_string()));
    }

    #[test]
    fn test_string_format_ipv4() {
        let s = openapiv3::StringType {
            format: openapiv3::VariantOrUnknownOrEmpty::Unknown("ipv4".to_string()),
            ..openapiv3::StringType::default()
        };
        let result = generate_string_example("addr", &s);
        assert_eq!(result, Value::String("192.0.2.1".to_string()));
    }

    #[test]
    fn test_array_min_items() {
        let item_schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType::default(),
            )),
        };
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Array(
                openapiv3::ArrayType {
                    items: Some(ReferenceOr::Item(Box::new(item_schema))),
                    min_items: Some(3),
                    max_items: None,
                    unique_items: false,
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "tags", &None, 0);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn test_array_min_items_capped_at_5() {
        let item_schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType::default(),
            )),
        };
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Array(
                openapiv3::ArrayType {
                    items: Some(ReferenceOr::Item(Box::new(item_schema))),
                    min_items: Some(100),
                    max_items: None,
                    unique_items: false,
                },
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "big", &None, 0);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 5, "min_items should be capped at 5");
    }
}
