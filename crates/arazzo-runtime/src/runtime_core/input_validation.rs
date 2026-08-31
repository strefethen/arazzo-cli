use arazzo_spec::{JsonSchemaType, SchemaObject};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputIssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct InputIssue {
    pub severity: InputIssueSeverity,
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for InputIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "input \"{}\": {}", self.field, self.message)
    }
}

/// Validates workflow inputs against the declared schema.
///
/// Phases (in order):
/// 1. Default injection — insert missing inputs from property defaults
/// 2. Required check — error if a required field is missing or null
/// 3. Type check — error if a provided value doesn't match the declared type
/// 4. Enum check — error if a provided value is not a declared property enum member
/// 5. Extra input warnings — warn about inputs not declared in properties
pub fn validate_inputs(
    schema: &SchemaObject,
    inputs: &mut BTreeMap<String, Value>,
) -> Vec<InputIssue> {
    let mut issues = Vec::new();

    // Phase 1: Default injection
    for (name, prop) in &schema.properties {
        if !inputs.contains_key(name) {
            if let Some(default_val) = &prop.default {
                match serde_json::to_value(default_val) {
                    Ok(json_val) => {
                        inputs.insert(name.clone(), json_val);
                    }
                    Err(err) => {
                        issues.push(InputIssue {
                            severity: InputIssueSeverity::Warning,
                            field: name.clone(),
                            message: format!("failed to convert default value: {err}"),
                        });
                    }
                }
            }
        }
    }

    // Phase 2: Required check
    for required_name in &schema.required {
        match inputs.get(required_name) {
            None | Some(Value::Null) => {
                issues.push(InputIssue {
                    severity: InputIssueSeverity::Error,
                    field: required_name.clone(),
                    message: "required input is missing".to_string(),
                });
            }
            Some(_) => {}
        }
    }

    // Phase 3: Type check
    for (name, prop) in &schema.properties {
        if let Some(declared_type) = &prop.type_ {
            if let Some(value) = inputs.get(name) {
                if !value.is_null() && !json_type_matches(declared_type, value) {
                    issues.push(InputIssue {
                        severity: InputIssueSeverity::Error,
                        field: name.clone(),
                        message: format!(
                            "expected type {declared_type}, got {}",
                            json_type_name(value)
                        ),
                    });
                }
            }
        }
    }

    // Phase 4: Enum check
    for (name, prop) in &schema.properties {
        let Some(value) = inputs.get(name) else {
            continue;
        };
        let Some(serde_yaml_ng::Value::Sequence(values)) = prop.extensions.get("enum") else {
            continue;
        };
        let Ok(enum_values) = values
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<Value>, _>>()
        else {
            continue;
        };

        if !enum_values
            .iter()
            .any(|enum_value| json_schema_instance_equal(value, enum_value))
        {
            issues.push(InputIssue {
                severity: InputIssueSeverity::Error,
                field: name.clone(),
                message: "value is not one of the declared enum values".to_string(),
            });
        }
    }

    // Phase 5: Extra input warnings
    if !schema.properties.is_empty() {
        for key in inputs.keys() {
            if !schema.properties.contains_key(key) {
                issues.push(InputIssue {
                    severity: InputIssueSeverity::Warning,
                    field: key.clone(),
                    message: "input not declared in schema properties".to_string(),
                });
            }
        }
    }

    issues
}

/// JSON Schema Core 2020-12 instance equality. In particular, JSON numbers
/// compare by mathematical value without reducing every representation to f64.
fn json_schema_instance_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => json_numbers_equal(left, right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_schema_instance_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_schema_instance_equal(left, right))
                })
        }
        _ => false,
    }
}

fn json_numbers_equal(left: &serde_json::Number, right: &serde_json::Number) -> bool {
    match json_number_kind(left) {
        JsonNumberKind::I64(left) => match json_number_kind(right) {
            JsonNumberKind::I64(right) => left == right,
            JsonNumberKind::U64(right) => left >= 0 && left as u64 == right,
            JsonNumberKind::F64(right) => float_equals_i64(right, left),
        },
        JsonNumberKind::U64(left) => match json_number_kind(right) {
            JsonNumberKind::I64(right) => right >= 0 && left == right as u64,
            JsonNumberKind::U64(right) => left == right,
            JsonNumberKind::F64(right) => float_equals_u64(right, left),
        },
        JsonNumberKind::F64(left) => match json_number_kind(right) {
            JsonNumberKind::I64(right) => float_equals_i64(left, right),
            JsonNumberKind::U64(right) => float_equals_u64(left, right),
            JsonNumberKind::F64(right) => left == right,
        },
    }
}

enum JsonNumberKind {
    I64(i64),
    U64(u64),
    F64(f64),
}

fn json_number_kind(value: &serde_json::Number) -> JsonNumberKind {
    if let Some(value) = value.as_i64() {
        JsonNumberKind::I64(value)
    } else if let Some(value) = value.as_u64() {
        JsonNumberKind::U64(value)
    } else {
        // serde_json::Number stores exactly one of i64, u64, or f64.
        let Some(value) = value.as_f64() else {
            unreachable!("serde_json::Number must have an i64, u64, or f64 representation");
        };
        JsonNumberKind::F64(value)
    }
}

fn float_equals_i64(float: f64, integer: i64) -> bool {
    // `i64::MAX` rounds up to 2^63 as f64, so the upper comparison must be
    // strict to avoid the saturating float-to-int conversion at that endpoint.
    float.is_finite()
        && float.fract() == 0.0
        && float >= i64::MIN as f64
        && float < -(i64::MIN as f64)
        && float as i64 == integer
}

fn float_equals_u64(float: f64, integer: u64) -> bool {
    // 2^64 is the first out-of-range u64 value and is exactly representable as
    // f64, so exclude it before casting rather than relying on saturation.
    float.is_finite()
        && float.fract() == 0.0
        && (0.0..18_446_744_073_709_551_616.0).contains(&float)
        && float as u64 == integer
}

fn json_type_matches(declared: &JsonSchemaType, value: &Value) -> bool {
    match declared {
        JsonSchemaType::String => value.is_string(),
        JsonSchemaType::Integer => value.is_i64() || value.is_u64(),
        JsonSchemaType::Number => value.is_number(),
        JsonSchemaType::Boolean => value.is_boolean(),
        JsonSchemaType::Array => value.is_array(),
        JsonSchemaType::Object => value.is_object(),
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arazzo_spec::PropertyDef;
    use serde_json::json;

    fn property_with_enum(values: serde_yaml_ng::Value) -> PropertyDef {
        let mut extensions = BTreeMap::new();
        extensions.insert("enum".to_string(), values);
        PropertyDef {
            extensions,
            ..PropertyDef::default()
        }
    }

    fn schema_with_property_enum(name: &str, values: serde_yaml_ng::Value) -> SchemaObject {
        SchemaObject {
            properties: BTreeMap::from([(name.to_string(), property_with_enum(values))]),
            ..SchemaObject::default()
        }
    }

    fn yaml_value(input: &str) -> serde_yaml_ng::Value {
        match serde_yaml_ng::from_str(input) {
            Ok(value) => value,
            Err(err) => panic!("parsing test YAML: {err}"),
        }
    }

    fn json_value(input: &str) -> Value {
        match serde_json::from_str(input) {
            Ok(value) => value,
            Err(err) => panic!("parsing test JSON: {err}"),
        }
    }

    fn schema_with_required_string(name: &str) -> SchemaObject {
        let mut properties = BTreeMap::new();
        properties.insert(
            name.to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                ..PropertyDef::default()
            },
        );
        SchemaObject {
            properties,
            required: vec![name.to_string()],
            ..SchemaObject::default()
        }
    }

    #[test]
    fn required_input_missing() {
        let schema = schema_with_required_string("name");
        let mut inputs = BTreeMap::new();
        let issues = validate_inputs(&schema, &mut inputs);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert!(issues[0].message.contains("required"));
    }

    #[test]
    fn required_input_present() {
        let schema = schema_with_required_string("name");
        let mut inputs = BTreeMap::from([("name".to_string(), json!("Alice"))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
    }

    #[test]
    fn default_injection() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "color".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                default: Some(serde_yaml_ng::Value::String("blue".to_string())),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::new();
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
        assert_eq!(inputs.get("color"), Some(&json!("blue")));
    }

    #[test]
    fn default_satisfies_required() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "name".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                default: Some(serde_yaml_ng::Value::String("default-name".to_string())),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            required: vec!["name".to_string()],
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::new();
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
        assert_eq!(inputs.get("name"), Some(&json!("default-name")));
    }

    #[test]
    fn type_mismatch_string_vs_integer() {
        let schema = schema_with_required_string("name");
        let mut inputs = BTreeMap::from([("name".to_string(), json!(42))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert!(issues[0].message.contains("expected type string"));
    }

    #[test]
    fn type_match_integer() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "count".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::Integer),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            required: vec!["count".to_string()],
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::from([("count".to_string(), json!(5))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
    }

    #[test]
    fn number_accepts_integer() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "amount".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::Number),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            required: vec!["amount".to_string()],
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::from([("amount".to_string(), json!(10))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
    }

    #[test]
    fn type_mismatch_boolean_vs_string() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "flag".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::Boolean),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::from([("flag".to_string(), json!("yes"))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert!(issues[0].message.contains("expected type boolean"));
    }

    #[test]
    fn extra_input_warning() {
        let schema = schema_with_required_string("name");
        let mut inputs = BTreeMap::from([
            ("name".to_string(), json!("Alice")),
            ("unknown".to_string(), json!("extra")),
        ]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Warning);
        assert!(issues[0].message.contains("not declared"));
    }

    #[test]
    fn no_schema_properties_skips_extra_check() {
        let schema = SchemaObject::default();
        let mut inputs = BTreeMap::from([("anything".to_string(), json!("value"))]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.is_empty());
    }

    #[test]
    fn null_value_for_required_field() {
        let schema = schema_with_required_string("name");
        let mut inputs = BTreeMap::from([("name".to_string(), Value::Null)]);
        let issues = validate_inputs(&schema, &mut inputs);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert!(issues[0].message.contains("required"));
    }

    #[test]
    fn multiple_issues_collected() {
        let mut properties = BTreeMap::new();
        properties.insert(
            "name".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                ..PropertyDef::default()
            },
        );
        properties.insert(
            "age".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::Integer),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            required: vec!["name".to_string(), "age".to_string()],
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::new(); // both missing
        let issues = validate_inputs(&schema, &mut inputs);
        assert!(issues.len() >= 2);
    }

    #[test]
    fn property_enum_accepts_members_and_injects_matching_default() {
        let enum_values = yaml_value("- sunrise\n- sunset\n- both\n");
        let mut property = property_with_enum(enum_values);
        property.default = Some(serde_yaml_ng::Value::String("both".to_string()));
        let schema = SchemaObject {
            properties: BTreeMap::from([("preference".to_string(), property)]),
            ..SchemaObject::default()
        };

        for input in [Some("sunrise"), Some("sunset"), Some("both"), None] {
            let mut inputs = input.map_or_else(BTreeMap::new, |input| {
                BTreeMap::from([("preference".to_string(), json!(input))])
            });
            let issues = validate_inputs(&schema, &mut inputs);
            assert!(issues.is_empty(), "input {input:?}: {issues:?}");
            if input.is_none() {
                assert_eq!(inputs.get("preference"), Some(&json!("both")));
            }
        }
    }

    #[test]
    fn property_enum_rejects_non_member_with_stable_redacted_issue() {
        let schema =
            schema_with_property_enum("preference", yaml_value("- sunrise\n- sunset\n- both\n"));
        let mut inputs = BTreeMap::from([("preference".to_string(), json!("invalid"))]);

        let issues = validate_inputs(&schema, &mut inputs);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert_eq!(issues[0].field, "preference");
        assert_eq!(
            issues[0].message,
            "value is not one of the declared enum values"
        );
        assert!(!issues[0].message.contains("invalid"));
        assert!(!issues[0].message.contains("sunrise"));
    }

    #[test]
    fn injected_property_enum_default_uses_the_same_error_path() {
        let enum_values = yaml_value("- sunrise\n- sunset\n");
        let mut property = property_with_enum(enum_values);
        property.default = Some(serde_yaml_ng::Value::String("invalid-default".to_string()));
        let schema = SchemaObject {
            properties: BTreeMap::from([("preference".to_string(), property)]),
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::new();

        let issues = validate_inputs(&schema, &mut inputs);

        assert_eq!(inputs.get("preference"), Some(&json!("invalid-default")));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
        assert_eq!(issues[0].field, "preference");
        assert_eq!(
            issues[0].message,
            "value is not one of the declared enum values"
        );
    }

    #[test]
    fn property_enum_allows_optional_null() {
        let schema = schema_with_property_enum("preference", yaml_value("- null\n- sunrise\n"));
        let mut inputs = BTreeMap::from([("preference".to_string(), Value::Null)]);

        assert!(validate_inputs(&schema, &mut inputs).is_empty());
    }

    #[test]
    fn property_enum_uses_recursive_json_schema_instance_equality() {
        let schema = schema_with_property_enum(
            "value",
            yaml_value(
                "- 1.0\n- [one, {nested: [true, null]}]\n- {name: value, nested: {count: 2}}\n",
            ),
        );

        for value in [
            json!(1),
            json!(["one", {"nested": [true, null]}]),
            json!({"nested": {"count": 2}, "name": "value"}),
        ] {
            let mut inputs = BTreeMap::from([("value".to_string(), value)]);
            assert!(validate_inputs(&schema, &mut inputs).is_empty());
        }
    }

    #[test]
    fn property_enum_rejects_recursive_non_members() {
        let schema =
            schema_with_property_enum("value", yaml_value("- [one, {nested: [true, null]}]\n"));
        let mut inputs = BTreeMap::from([(
            "value".to_string(),
            json!(["one", {"nested": [false, null]}]),
        )]);

        let issues = validate_inputs(&schema, &mut inputs);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
    }

    #[test]
    fn property_enum_does_not_lose_integer_precision_against_float() {
        let schema = schema_with_property_enum("value", yaml_value("- 9007199254740992.0\n"));
        let mut inputs = BTreeMap::from([("value".to_string(), json!(9_007_199_254_740_993_u64))]);

        let issues = validate_inputs(&schema, &mut inputs);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, InputIssueSeverity::Error);
    }

    #[test]
    fn json_number_equality_handles_signed_and_unsigned_range_endpoints() {
        let signed_min_float = json_value("-9223372036854775808.0");
        let signed_overflow_float = json_value("9223372036854775808.0");
        let unsigned_overflow_float = json_value("18446744073709551616.0");

        assert!(json_schema_instance_equal(
            &json!(i64::MIN),
            &signed_min_float
        ));
        assert!(json_schema_instance_equal(
            &json!(9_223_372_036_854_775_808_u64),
            &signed_overflow_float
        ));
        assert!(!json_schema_instance_equal(
            &json!(i64::MAX),
            &signed_overflow_float
        ));
        assert!(!json_schema_instance_equal(
            &json!(u64::MAX),
            &unsigned_overflow_float
        ));
    }

    #[test]
    fn malformed_property_enums_are_skipped_without_partial_validation() {
        let non_sequence = schema_with_property_enum(
            "preference",
            serde_yaml_ng::Value::String("sunrise".to_string()),
        );
        let invalid_member = serde_yaml_ng::Value::Mapping({
            let mut map = serde_yaml_ng::Mapping::new();
            map.insert(
                serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::String(
                    "not-a-json-object-key".to_string(),
                )]),
                serde_yaml_ng::Value::String("value".to_string()),
            );
            map
        });
        let partially_convertible = schema_with_property_enum(
            "preference",
            serde_yaml_ng::Value::Sequence(vec![
                serde_yaml_ng::Value::String("sunrise".to_string()),
                invalid_member,
            ]),
        );

        for schema in [non_sequence, partially_convertible] {
            let mut inputs = BTreeMap::from([("preference".to_string(), json!("invalid"))]);
            assert!(validate_inputs(&schema, &mut inputs).is_empty());
        }
    }

    #[test]
    fn property_enum_preserves_existing_phase_order_and_issue_cardinality() {
        let mut properties = BTreeMap::new();
        let mut enum_property = property_with_enum(yaml_value("- sunrise\n"));
        enum_property.type_ = Some(JsonSchemaType::String);
        properties.insert("enum-value".to_string(), enum_property);
        properties.insert(
            "required-value".to_string(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                ..PropertyDef::default()
            },
        );
        let schema = SchemaObject {
            properties,
            required: vec!["required-value".to_string()],
            ..SchemaObject::default()
        };
        let mut inputs = BTreeMap::from([
            ("enum-value".to_string(), json!(1)),
            ("undeclared".to_string(), json!(true)),
        ]);

        let issues = validate_inputs(&schema, &mut inputs);

        assert_eq!(issues.len(), 4);
        assert_eq!(issues[0].field, "required-value");
        assert_eq!(issues[0].message, "required input is missing");
        assert_eq!(issues[1].field, "enum-value");
        assert_eq!(issues[1].message, "expected type string, got integer");
        assert_eq!(issues[2].field, "enum-value");
        assert_eq!(
            issues[2].message,
            "value is not one of the declared enum values"
        );
        assert_eq!(issues[3].field, "undeclared");
        assert_eq!(issues[3].severity, InputIssueSeverity::Warning);
    }
}
