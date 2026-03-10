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
/// 4. Extra input warnings — warn about inputs not declared in properties
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

    // Phase 4: Extra input warnings
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
                default: Some(serde_yml::Value::String("blue".to_string())),
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
                default: Some(serde_yml::Value::String("default-name".to_string())),
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
}
