use std::cmp::Ordering;
use std::collections::HashSet;

use serde_json::Value;
use serde_yaml_ng::Value as Yaml;

mod budget;
pub(crate) mod typed_bridge;

use budget::{Budget, LIMIT};

#[derive(Debug, PartialEq)]
pub struct Example {
    pub value: Value,
    pub trimmed: bool,
}

pub fn generate_example(schema: &Yaml, root: &Yaml, field_name: &str) -> Result<Example, String> {
    let mut budget = Budget::default();
    let value = walk(schema, root, field_name, 0, &mut budget)?;
    Ok(Example {
        value,
        trimmed: budget.trimmed,
    })
}

fn resolve<'a>(mut schema: &'a Yaml, root: &'a Yaml) -> Option<&'a Yaml> {
    let mut visited = HashSet::new();
    while let Some(reference) = schema.get("$ref").and_then(Yaml::as_str) {
        if !visited.insert(reference) {
            return None;
        }
        let pointer = reference.strip_prefix("#/")?;
        let mut target = root;
        for token in pointer.split('/') {
            let mut decoded = String::new();
            let mut chars = token.chars();
            while let Some(ch) = chars.next() {
                if ch == '~' {
                    decoded.push(match chars.next()? {
                        '0' => '~',
                        '1' => '/',
                        _ => return None,
                    });
                } else {
                    decoded.push(ch);
                }
            }
            target = match target {
                Yaml::Mapping(map) => map.get(Yaml::String(decoded))?,
                Yaml::Sequence(values) => {
                    if decoded.is_empty()
                        || (decoded.starts_with('0') && decoded.len() > 1)
                        || !decoded.bytes().all(|b| b.is_ascii_digit())
                    {
                        return None;
                    }
                    values.get(decoded.parse::<usize>().ok()?)?
                }
                _ => return None,
            };
        }
        schema = target;
    }
    Some(schema)
}

fn schema_type(schema: &Yaml) -> Option<&str> {
    match schema.get("type") {
        Some(Yaml::String(typ)) => Some(typ),
        Some(Yaml::Sequence(types)) => types
            .iter()
            .filter_map(Yaml::as_str)
            .find(|typ| *typ != "null"),
        None if schema.get("properties").is_some() => Some("object"),
        _ => None,
    }
}

fn literal(schema: &Yaml) -> Option<&Yaml> {
    [
        schema.get("example"),
        schema
            .get("examples")
            .and_then(Yaml::as_sequence)
            .and_then(|values| values.first()),
        schema.get("const"),
        schema.get("default"),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.is_null())
}

fn walk(
    schema: &Yaml,
    root: &Yaml,
    field_name: &str,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    if !budget.charge(1) {
        return Ok(Value::Null);
    }
    if depth > 5 {
        return Ok(Value::Null);
    }
    let Some(schema) = resolve(schema, root) else {
        return Ok(Value::Null);
    };
    if let Some(value) = literal(schema) {
        return Ok(if budget.literal(value) {
            yaml_literal(value)
        } else {
            Value::Null
        });
    }
    let typ = schema_type(schema);
    if let Some(values) = schema.get("enum").and_then(Yaml::as_sequence) {
        let first = values.iter().find(|value| match typ {
            Some("string") => value.as_str().is_some(),
            Some("integer") => value.as_i64().is_some(),
            Some("number") => value.as_f64().is_some(),
            Some("boolean") => value.as_bool().is_some(),
            _ => false,
        });
        if let Some(value) = first {
            return Ok(if budget.literal(value) {
                if typ == Some("number") {
                    serde_json::json!(value.as_f64())
                } else {
                    yaml_literal(value)
                }
            } else {
                Value::Null
            });
        }
    }
    Ok(match typ {
        Some("string") => string_example(schema, field_name, budget),
        Some("integer") => numeric_example(schema, field_name, true)?,
        Some("number") => numeric_example(schema, field_name, false)?,
        Some("boolean") => Value::Bool(true),
        Some("array") => array_example(schema, root, depth, budget)?,
        Some("object") => object_example(schema, root, depth, budget)?,
        _ => Value::Null,
    })
}

fn yaml_literal(value: &Yaml) -> Value {
    match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(*value),
        Yaml::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::from(value)
            } else if let Some(value) = value.as_u64() {
                Value::from(value)
            } else {
                serde_json::json!(value.as_f64())
            }
        }
        Yaml::String(value) => Value::String(value.clone()),
        Yaml::Sequence(values) => Value::Array(values.iter().map(yaml_literal).collect()),
        Yaml::Mapping(values) => Value::Object(
            values
                .iter()
                .filter_map(|(key, value)| {
                    key.as_str()
                        .map(|key| (key.to_owned(), yaml_literal(value)))
                })
                .collect(),
        ),
        Yaml::Tagged(value) => yaml_literal(&value.value),
    }
}

#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    fn of(value: &Yaml) -> Option<Num> {
        let Yaml::Number(number) = value else {
            return None;
        };
        number
            .as_i64()
            .map(|value| Num::Int(value.into()))
            .or_else(|| number.as_u64().map(|value| Num::Int(value.into())))
            .or_else(|| number.as_f64().map(Num::Float))
    }

    fn is_nan(self) -> bool {
        matches!(self, Num::Float(value) if value.is_nan())
    }

    fn compare(self, other: Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(&b)),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(&b),
            (Num::Int(a), Num::Float(b)) => int_float_order(a, b),
            (Num::Float(a), Num::Int(b)) => int_float_order(b, a).map(Ordering::reverse),
        }
    }
}

impl std::fmt::Display for Num {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Num::Int(value) => write!(f, "{value}"),
            Num::Float(value) => write!(f, "{value:?}"),
        }
    }
}

fn int_float_order(int: i128, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        None
    } else if float >= i128::MAX as f64 {
        Some(Ordering::Less)
    } else if float < i128::MIN as f64 {
        Some(Ordering::Greater)
    } else {
        let floor = float.floor();
        Some(int.cmp(&(floor as i128)).then(if float > floor {
            Ordering::Less
        } else {
            Ordering::Equal
        }))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Strictness {
    Inclusive,
    BooleanExclusive,
    Exclusive,
}

#[derive(Clone, Copy)]
struct Bound {
    value: Num,
    strictness: Strictness,
    lower: bool,
}

impl Bound {
    fn read(schema: &Yaml, lower: bool) -> Option<Bound> {
        let (inclusive_key, exclusive_key) = if lower {
            ("minimum", "exclusiveMinimum")
        } else {
            ("maximum", "exclusiveMaximum")
        };
        let exclusive = schema.get(exclusive_key);
        let inclusive = schema
            .get(inclusive_key)
            .and_then(Num::of)
            .map(|value| Bound {
                value,
                strictness: if exclusive.and_then(Yaml::as_bool) == Some(true) {
                    Strictness::BooleanExclusive
                } else {
                    Strictness::Inclusive
                },
                lower,
            });
        let exclusive = exclusive.and_then(Num::of).map(|value| Bound {
            value,
            strictness: Strictness::Exclusive,
            lower,
        });
        let tighter = if lower {
            Ordering::Greater
        } else {
            Ordering::Less
        };
        match (inclusive, exclusive) {
            (Some(inclusive), Some(exclusive)) => {
                Some(match inclusive.value.compare(exclusive.value) {
                    Some(order) if order == tighter => inclusive,
                    Some(_) => exclusive,
                    None if exclusive.value.is_nan() => exclusive,
                    None => inclusive,
                })
            }
            (inclusive, exclusive) => inclusive.or(exclusive),
        }
    }

    fn strict(self) -> bool {
        self.strictness != Strictness::Inclusive
    }

    fn admits(self, candidate: Num) -> bool {
        let wanted = if self.lower {
            Ordering::Greater
        } else {
            Ordering::Less
        };
        match candidate.compare(self.value) {
            Some(order) if order == wanted => true,
            Some(Ordering::Equal) => !self.strict(),
            _ => false,
        }
    }

    fn describe(self) -> String {
        let (inclusive_key, exclusive_key) = if self.lower {
            ("minimum", "exclusiveMinimum")
        } else {
            ("maximum", "exclusiveMaximum")
        };
        match self.strictness {
            Strictness::Inclusive => format!("{inclusive_key} {}", self.value),
            Strictness::BooleanExclusive => {
                format!("{inclusive_key} {} with {exclusive_key}: true", self.value)
            }
            Strictness::Exclusive => format!("{exclusive_key} {}", self.value),
        }
    }
}

fn numeric_example(schema: &Yaml, field_name: &str, integer: bool) -> Result<Value, String> {
    let bound = Bound::read(schema, true).or_else(|| {
        Bound::read(schema, false).filter(|bound| match bound.value.compare(Num::Int(1)) {
            Some(Ordering::Less) | None => true,
            Some(Ordering::Equal) => bound.strictness == Strictness::Exclusive,
            Some(Ordering::Greater) => false,
        })
    });
    let Some(bound) = bound else {
        return Ok(if integer {
            Value::from(1)
        } else {
            serde_json::json!(1.0)
        });
    };
    satisfy(bound, integer).ok_or_else(|| {
        format!(
            "cannot synthesize a JSON {} for \"{field_name}\" satisfying {}",
            if integer { "integer" } else { "number" },
            bound.describe()
        )
    })
}

fn satisfy(bound: Bound, integer: bool) -> Option<Value> {
    let shift: i8 = match (bound.lower, bound.strict()) {
        (_, false) => 0,
        (true, true) => 1,
        (false, true) => -1,
    };
    let mut candidate = match (bound.value, integer) {
        (Num::Int(value), true) => {
            let value = value + i128::from(shift);
            if let Ok(value) = i64::try_from(value) {
                return Some(Value::from(value));
            }
            if let Ok(value) = u64::try_from(value) {
                return Some(Value::from(value));
            }
            value as f64
        }
        (Num::Float(value), true) => match (bound.lower, bound.strict()) {
            (true, true) => value.floor() + 1.0,
            (true, false) => value.ceil(),
            (false, true) => value.ceil() - 1.0,
            (false, false) => value.floor(),
        },
        (Num::Int(value), false) => value as f64 + f64::from(shift),
        (Num::Float(value), false) => value + f64::from(shift),
    };
    for _ in 0..2 {
        if candidate.is_finite() && bound.admits(Num::Float(candidate)) {
            break;
        }
        candidate = if bound.lower {
            candidate.next_up()
        } else {
            candidate.next_down()
        };
    }
    if !candidate.is_finite()
        || !bound.admits(Num::Float(candidate))
        || (integer && candidate.fract() != 0.0)
    {
        return None;
    }
    if integer && candidate >= i64::MIN as f64 && candidate < i64::MAX as f64 {
        return Some(Value::from(candidate as i64));
    }
    if integer && candidate >= 0.0 && candidate < u64::MAX as f64 {
        return Some(Value::from(candidate as u64));
    }
    serde_json::Number::from_f64(candidate).map(Value::Number)
}

fn array_example(
    schema: &Yaml,
    root: &Yaml,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    let Some(items) = schema.get("items").filter(|items| !items.is_null()) else {
        return Ok(Value::Array(Vec::new()));
    };
    if budget.remaining() == 0 {
        budget.charge(1);
        return Ok(Value::Array(Vec::new()));
    }
    let item = walk(items, root, "item", depth + 1, budget)?;
    let count = schema
        .get("minItems")
        .and_then(Yaml::as_u64)
        .unwrap_or(1)
        .clamp(1, 5);
    let mut values = vec![item];
    for _ in 1..count {
        if budget.trimmed || !budget.charge(budget::json_cost(&values[0])) {
            break;
        }
        values.push(values[0].clone());
    }
    Ok(Value::Array(values))
}

fn object_example(
    schema: &Yaml,
    root: &Yaml,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    let mut object = serde_json::Map::new();
    let Some(properties) = schema.get("properties").and_then(Yaml::as_mapping) else {
        return Ok(Value::Object(object));
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Yaml::as_sequence)
        .map(|values| values.iter().filter_map(Yaml::as_str).collect())
        .unwrap_or_default();
    let chosen = required
        .iter()
        .filter_map(|name| {
            properties
                .get(Yaml::String((*name).to_owned()))
                .map(|schema| (*name, schema))
        })
        .chain(
            properties
                .iter()
                .filter_map(|(key, schema)| key.as_str().map(|name| (name, schema)))
                .filter(|(name, _)| !required.contains(name))
                .take(5),
        );
    for (name, schema) in chosen {
        let value = if budget.trimmed || !budget.charge(name.chars().take(LIMIT + 1).count()) {
            Value::Null
        } else {
            walk(schema, root, name, depth + 1, budget)?
        };
        object.insert(name.to_owned(), value);
    }
    Ok(Value::Object(object))
}

fn string_example(schema: &Yaml, field_name: &str, budget: &mut Budget) -> Value {
    let format = schema.get("format").and_then(Yaml::as_str).unwrap_or("");
    let base = match format {
        "date-time" => "2024-01-01T00:00:00Z",
        "date" => "2024-01-01",
        "email" => "user@example.com",
        "uuid" => "550e8400-e29b-41d4-a716-446655440000",
        "uri" | "url" => "https://example.com",
        "password" => "P@ssw0rd123",
        "byte" => "SGVsbG8gV29ybGQ=",
        "ipv4" => "192.0.2.1",
        "ipv6" => "2001:db8::1",
        "hostname" => "api.example.com",
        _ => "",
    };
    let base = if base.is_empty() {
        field_name_heuristic(&field_name.to_lowercase())
            .unwrap_or_else(|| format!("example-{field_name}"))
    } else {
        base.to_owned()
    };
    let minimum = schema.get("minLength").and_then(Yaml::as_u64).unwrap_or(0);
    if minimum > LIMIT as u64 {
        budget.charge(LIMIT + 1);
        return Value::Null;
    }
    let base_len = base.chars().count();
    let padded_len = base_len.max(minimum as usize);
    let length = schema
        .get("maxLength")
        .and_then(Yaml::as_u64)
        .map(|max| padded_len.min(usize::try_from(max).unwrap_or(usize::MAX)))
        .unwrap_or(padded_len);
    if !budget.charge(length) {
        return Value::Null;
    }
    let mut value: String = base.chars().take(length).collect();
    value.extend(std::iter::repeat_n('a', length.saturating_sub(base_len)));
    Value::String(value)
}

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
    use super::{typed_bridge, Value};
    use openapiv3::ReferenceOr;

    fn generate_example(
        schema: &ReferenceOr<openapiv3::Schema>,
        field_name: &str,
        components: &Option<openapiv3::Components>,
        _depth: usize,
    ) -> Value {
        let openapi = openapiv3::OpenAPI {
            components: components.clone(),
            ..Default::default()
        };
        let root = typed_bridge::root(&openapi).unwrap();
        let schema = typed_bridge::schema(schema).unwrap();
        super::generate_example(&schema, &root, field_name)
            .unwrap()
            .value
    }

    fn generate_string_example(field_name: &str, string_type: &openapiv3::StringType) -> Value {
        let schema = openapiv3::Schema {
            schema_data: Default::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(string_type.clone())),
        };
        generate_example(&ReferenceOr::Item(schema), field_name, &None, 0)
    }

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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod standalone_tests {
    use super::*;

    fn generate_from_json_schema(schema: &Value, field_name: &str) -> Value {
        let schema = serde_yaml_ng::to_value(schema).unwrap();
        generate_example(&schema, &schema, field_name)
            .unwrap()
            .value
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod raw_tests {
    use super::*;
    use serde_json::json;

    fn example(schema: Value) -> Example {
        let schema = serde_yaml_ng::to_value(schema).unwrap();
        generate_example(&schema, &schema, "body").unwrap()
    }

    fn typed(value: Value) -> Value {
        let schema: openapiv3::Schema = serde_json::from_value(value).unwrap();
        let schema = typed_bridge::schema(&openapiv3::ReferenceOr::Item(schema)).unwrap();
        generate_example(&schema, &Yaml::Null, "body")
            .unwrap()
            .value
    }

    #[test]
    fn literals_and_type_arrays_follow_selected_precedence() {
        for typ in [
            json!("string"),
            json!(["string", "null"]),
            json!(["null", "string"]),
        ] {
            assert_eq!(example(json!({"type": typ})).value, json!("example-body"));
            assert_eq!(example(json!({"type": typ, "example": "a", "examples": ["b"], "const": "c", "default": "d"})).value, json!("a"));
        }
        for (schema, expected) in [
            (
                json!({"example": null, "examples": ["b"], "const": "c", "default": "d"}),
                json!("b"),
            ),
            (
                json!({"example": null, "examples": [null, "ignored"], "const": "c", "default": "d"}),
                json!("c"),
            ),
            (json!({"const": null, "default": "d"}), json!("d")),
            (json!({"type": ["null"]}), Value::Null),
            (
                json!({"properties": {"count": {"type": "integer"}}}),
                json!({"count": 1}),
            ),
            (json!({"format": "email"}), Value::Null),
        ] {
            assert_eq!(example(schema).value, expected);
        }
    }

    #[test]
    fn formats_and_numeric_bound_forms() {
        for (format, expected) in [("password", "P@ssw0rd123"), ("byte", "SGVsbG8gV29ybGQ=")] {
            assert_eq!(
                example(json!({"type": "string", "format": format})).value,
                json!(expected)
            );
            assert_eq!(
                typed(json!({"type": "string", "format": format})),
                json!(expected)
            );
        }
        for (schema, expected) in [
            (
                json!({"type": "integer", "minimum": 3, "exclusiveMinimum": true}),
                json!(4),
            ),
            (json!({"type": "integer", "exclusiveMinimum": 3}), json!(4)),
            (
                json!({"type": "integer", "maximum": -3, "exclusiveMaximum": true}),
                json!(-4),
            ),
            (
                json!({"type": "integer", "exclusiveMaximum": -3}),
                json!(-4),
            ),
            (
                json!({"type": "integer", "maximum": 1, "exclusiveMaximum": true}),
                json!(1),
            ),
            (
                json!({"type": "number", "minimum": 3.5, "exclusiveMinimum": true}),
                json!(4.5),
            ),
            (
                json!({"type": "number", "exclusiveMinimum": 3.5}),
                json!(4.5),
            ),
            (
                json!({"type": "number", "maximum": -3.5, "exclusiveMaximum": true}),
                json!(-4.5),
            ),
            (
                json!({"type": "number", "exclusiveMaximum": -3.5}),
                json!(-4.5),
            ),
            (
                json!({"type": "number", "exclusiveMaximum": 1.0}),
                json!(0.0),
            ),
        ] {
            assert_eq!(example(schema).value, expected);
        }
    }

    fn attempt(schema: &str) -> Result<Example, String> {
        let schema: Yaml = serde_yaml_ng::from_str(schema).unwrap();
        generate_example(&schema, &schema, "body")
    }

    #[test]
    fn integer_bounds_use_exact_integers_beyond_signed_range() {
        for (schema, expected) in [
            (
                json!({"type": "integer", "exclusiveMinimum": i64::MAX}),
                json!(9_223_372_036_854_775_808_u64),
            ),
            (
                json!({"type": "integer", "minimum": i64::MAX, "exclusiveMinimum": true}),
                json!(9_223_372_036_854_775_808_u64),
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": u64::MAX - 1}),
                json!(u64::MAX),
            ),
            (
                json!({"type": "integer", "minimum": u64::MAX}),
                json!(u64::MAX),
            ),
            (
                json!({"type": "integer", "exclusiveMaximum": i64::MIN + 1}),
                json!(i64::MIN),
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": u64::MAX}),
                json!(18_446_744_073_709_551_616.0),
            ),
            (
                json!({"type": "integer", "maximum": i64::MIN, "exclusiveMaximum": true}),
                json!(-9_223_372_036_854_777_856.0),
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": 1e300}),
                json!(1e300_f64.next_up()),
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": 3.5}),
                json!(4),
            ),
            (
                json!({"type": "integer", "exclusiveMaximum": -3.5}),
                json!(-4),
            ),
            (json!({"type": "integer", "minimum": 3.5}), json!(4)),
            (json!({"type": "integer", "maximum": 0.5}), json!(0)),
        ] {
            assert_eq!(example(schema.clone()).value, expected, "{schema}");
        }
    }

    #[test]
    fn number_bounds_stay_strict_after_float_rounding() {
        let rounded = 9_007_199_254_740_993_u64;
        for (schema, expected) in [
            (
                json!({"type": "number", "exclusiveMinimum": i64::MAX}),
                json!(9_223_372_036_854_775_808.0),
            ),
            (
                json!({"type": "number", "exclusiveMinimum": rounded}),
                json!(9_007_199_254_740_994.0),
            ),
            (
                json!({"type": "number", "minimum": rounded}),
                json!(9_007_199_254_740_994.0),
            ),
            (
                json!({"type": "number", "maximum": -(rounded as i64)}),
                json!(-9_007_199_254_740_994.0),
            ),
            (
                json!({"type": "number", "exclusiveMinimum": 1e300}),
                json!(1e300_f64.next_up()),
            ),
            (json!({"type": "number", "exclusiveMinimum": 3}), json!(4.0)),
        ] {
            assert_eq!(example(schema.clone()).value, expected, "{schema}");
        }
    }

    #[test]
    fn tighter_of_inclusive_and_exclusive_bounds_wins() {
        for (schema, expected) in [
            (
                json!({"type": "integer", "minimum": 10, "exclusiveMinimum": 5}),
                json!(10),
            ),
            (
                json!({"type": "integer", "minimum": 5, "exclusiveMinimum": 5}),
                json!(6),
            ),
            (
                json!({"type": "integer", "minimum": 5, "exclusiveMinimum": 10}),
                json!(11),
            ),
            (
                json!({"type": "integer", "maximum": -10, "exclusiveMaximum": -5}),
                json!(-10),
            ),
            (
                json!({"type": "number", "maximum": -5.0, "exclusiveMaximum": -5.0}),
                json!(-6.0),
            ),
        ] {
            assert_eq!(example(schema.clone()).value, expected, "{schema}");
        }
    }

    #[test]
    fn unrepresentable_strict_bounds_are_errors() {
        let max = f64::MAX;
        for (schema, message) in [
            (
                json!({"type": "number", "exclusiveMinimum": max}),
                "cannot synthesize a JSON number for \"body\" satisfying exclusiveMinimum 1.7976931348623157e308",
            ),
            (
                json!({"type": "number", "exclusiveMaximum": -max}),
                "cannot synthesize a JSON number for \"body\" satisfying exclusiveMaximum -1.7976931348623157e308",
            ),
            (
                json!({"type": "number", "minimum": max, "exclusiveMinimum": true}),
                "cannot synthesize a JSON number for \"body\" satisfying minimum 1.7976931348623157e308 with exclusiveMinimum: true",
            ),
            (
                json!({"type": "number", "maximum": -max, "exclusiveMaximum": true}),
                "cannot synthesize a JSON number for \"body\" satisfying maximum -1.7976931348623157e308 with exclusiveMaximum: true",
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": max}),
                "cannot synthesize a JSON integer for \"body\" satisfying exclusiveMinimum 1.7976931348623157e308",
            ),
            (
                json!({"type": ["integer", "null"], "exclusiveMaximum": -max}),
                "cannot synthesize a JSON integer for \"body\" satisfying exclusiveMaximum -1.7976931348623157e308",
            ),
        ] {
            let schema = serde_yaml_ng::to_value(schema).unwrap();
            assert_eq!(
                generate_example(&schema, &schema, "body"),
                Err(message.to_owned())
            );
        }
        for schema in [
            "{type: number, minimum: .inf}",
            "{type: integer, minimum: .nan}",
            "{type: number, maximum: -.inf}",
            "{type: integer, exclusiveMaximum: .nan}",
        ] {
            assert!(attempt(schema).is_err(), "{schema}");
        }
        let nested = json!({"type": "object", "properties": {
            "fine": {"type": "integer"},
            "price": {"type": "number", "exclusiveMinimum": max}
        }});
        let nested = serde_yaml_ng::to_value(nested).unwrap();
        assert_eq!(
            generate_example(&nested, &nested, "body"),
            Err("cannot synthesize a JSON number for \"price\" satisfying exclusiveMinimum 1.7976931348623157e308".to_owned())
        );
    }

    #[test]
    fn strict_bound_neighbors_remain_representable() {
        let max = f64::MAX;
        let below_max = max.next_down();
        for (schema, expected) in [
            (
                json!({"type": "number", "exclusiveMinimum": below_max}),
                json!(max),
            ),
            (
                json!({"type": "number", "exclusiveMaximum": -below_max}),
                json!(-max),
            ),
            (
                json!({"type": "integer", "exclusiveMinimum": below_max}),
                json!(max),
            ),
            (json!({"type": "number", "minimum": max}), json!(max)),
            (json!({"type": "integer", "minimum": max}), json!(max)),
            (json!({"type": "number", "maximum": -max}), json!(-max)),
            (
                json!({"type": "number", "exclusiveMaximum": max}),
                json!(1.0),
            ),
            (
                json!({"type": "number", "maximum": f64::MIN_POSITIVE, "exclusiveMaximum": true}),
                json!(f64::MIN_POSITIVE - 1.0),
            ),
        ] {
            let result = example(schema.clone());
            assert_eq!(result.value, expected, "{schema}");
            assert!(!result.trimmed);
        }
        assert_eq!(
            attempt("{type: number, maximum: .inf}").unwrap().value,
            json!(1.0)
        );
    }

    #[test]
    fn literals_and_trimming_keep_precedence_over_unrepresentable_bounds() {
        let max = f64::MAX;
        for literal in ["example", "default", "const"] {
            assert_eq!(
                example(json!({"type": "number", "exclusiveMinimum": max, literal: 5})).value,
                json!(5)
            );
        }
        let trimmed = example(json!({"type": "object", "properties": {
            "filler": {"example": "x".repeat(990)},
            "price": {"type": "number", "exclusiveMinimum": max}
        }}));
        assert!(trimmed.trimmed);
        assert!(trimmed.value["price"].is_null());
    }

    #[test]
    fn optional_properties_keep_declaration_order() {
        let schema: Yaml = serde_yaml_ng::from_str("type: object\nproperties:\n  z: {type: integer}\n  y: {type: integer}\n  x: {type: integer}\n  w: {type: integer}\n  v: {type: integer}\n  a: {type: integer}\n").unwrap();
        let result = generate_example(&schema, &schema, "body").unwrap();
        assert_eq!(
            result.value,
            json!({"z": 1, "y": 1, "x": 1, "w": 1, "v": 1})
        );
    }

    #[test]
    fn local_pointer_decodes_keys_and_array_indices() {
        for pointer in ["#/definitions/a~1b~0c", "#/list/0"] {
            let schema = json!({"$ref": pointer, "definitions": {"a/b~c": {"type": "integer", "minimum": 9}}, "list": [{"type": "integer", "minimum": 9}]});
            assert_eq!(example(schema).value, json!(9));
        }
        for pointer in [
            "#/missing",
            "#/definitions/a~2b",
            "#/list/01",
            "#/list/-",
            "#/list/+0",
            "#",
            "other.yaml#/A",
        ] {
            assert_eq!(
                example(json!({"$ref": pointer, "list": [{"type": "integer"}]})).value,
                Value::Null
            );
        }
    }

    #[test]
    fn references_have_separate_cycle_and_structural_bounds() {
        assert_eq!(
            example(json!({"$ref": "#/a", "a": {"$ref": "#/a"}})).value,
            Value::Null
        );
        assert_eq!(
            example(json!({"$ref": "#/a", "a": {"$ref": "#/b"}, "b": {"$ref": "#/a"}})).value,
            Value::Null
        );
        let recursive = example(
            json!({"$ref": "#/a", "a": {"type": "object", "properties": {"next": {"$ref": "#/a"}}}}),
        );
        let mut value = &recursive.value;
        for _ in 0..6 {
            value = &value["next"];
        }
        assert!(value.is_null());
        assert!(!recursive.trimmed);
        let mut schemas = serde_json::Map::new();
        for index in 0..40 {
            schemas.insert(
                index.to_string(),
                json!({"$ref": format!("#/aliases/{}", index + 1)}),
            );
        }
        schemas.insert("40".into(), json!({"type": "integer", "minimum": 17}));
        assert_eq!(
            example(json!({"$ref": "#/aliases/0", "aliases": schemas})).value,
            json!(17)
        );
    }

    #[test]
    fn literal_array_and_padding_charges_are_bounded() {
        assert_eq!(
            example(json!({"type": "string", "minLength": 4000000000u64})),
            Example {
                value: Value::Null,
                trimmed: true
            }
        );
        assert_eq!(
            example(json!({"example": "a".repeat(1000)})),
            Example {
                value: Value::Null,
                trimmed: true
            }
        );
        let reused = example(json!({"type": "object", "properties": {
            "a": {"example": "x".repeat(600)},
            "b": {"example": "x".repeat(600)},
            "c": {"type": "object", "properties": {"never": {"type": "integer"}}}
        }}));
        assert!(reused.trimmed);
        assert_eq!(reused.value["a"], json!("x".repeat(600)));
        assert!(reused.value["b"].is_null());
        assert!(reused.value["c"].is_null());
        let oversized_item = example(
            json!({"type": "array", "items": {"example": "x".repeat(2000)}, "minItems": 5}),
        );
        assert_eq!(
            oversized_item,
            Example {
                value: json!([null]),
                trimmed: true
            }
        );
        let copies =
            example(json!({"type": "array", "items": {"example": "x".repeat(400)}, "minItems": 5}));
        assert_eq!(copies.value.as_array().unwrap().len(), 2);
        assert!(copies.trimmed);
        assert!(budget::json_cost(&copies.value) <= LIMIT);
        let unicode = example(json!({"example": "é".repeat(999)}));
        assert!(!unicode.trimmed);
        assert_eq!(budget::json_cost(&unicode.value), LIMIT);
    }

    #[test]
    fn wide_reference_tree_trims_only_open_objects() {
        let names: Vec<String> = (0..16).map(|i| format!("field{i}")).collect();
        let mut definitions = serde_json::Map::new();
        for level in 0..6 {
            let properties: serde_json::Map<String, Value> = names
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        if level == 5 {
                            json!({"type": "string"})
                        } else {
                            json!({"$ref": format!("#/definitions/level{}", level + 1)})
                        },
                    )
                })
                .collect();
            definitions.insert(
                format!("level{level}"),
                json!({"type": "object", "required": names, "properties": properties}),
            );
        }
        let result = example(json!({"$ref": "#/definitions/level0", "definitions": definitions}));
        assert!(result.trimmed);
        assert!(result.value.is_object());
        fn null_allowance(value: &Value) -> usize {
            match value {
                Value::Object(values) => values
                    .iter()
                    .map(|(key, value)| {
                        if value.is_null() {
                            key.chars().count() + 1
                        } else {
                            null_allowance(value)
                        }
                    })
                    .sum(),
                Value::Array(values) => values.iter().map(null_allowance).sum(),
                _ => 0,
            }
        }
        assert!(budget::json_cost(&result.value) <= LIMIT + null_allowance(&result.value));
        assert_eq!(result.value.as_object().unwrap().len(), 16);
    }

    #[test]
    fn typed_normalization_preserves_variant_information_and_null_literals() {
        let normal = json!({"type": "string", "format": "email"});
        let any = json!({"type": "string", "format": "email", "properties": {}});
        let normal_schema: openapiv3::Schema = serde_json::from_value(normal.clone()).unwrap();
        let any_schema: openapiv3::Schema = serde_json::from_value(any.clone()).unwrap();
        assert_eq!(
            serde_yaml_ng::to_value(normal_schema).unwrap(),
            serde_yaml_ng::to_value(any_schema).unwrap()
        );
        assert_eq!(typed(normal), json!("user@example.com"));
        assert_eq!(typed(any), json!("example-body"));
        assert_eq!(
            typed(json!({"type": "integer", "minimum": 50, "properties": {}})),
            json!(1)
        );
        assert_eq!(
            typed(json!({"type": "array", "properties": {"count": {"type": "integer"}}})),
            json!({"count": 1})
        );
        assert_eq!(
            typed(json!({"type": "array", "properties": {}})),
            Value::Null
        );
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData {
                example: Some(Value::Null),
                default: Some(json!("default")),
                ..Default::default()
            },
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(Default::default())),
        };
        let schema = typed_bridge::schema(&openapiv3::ReferenceOr::Item(schema)).unwrap();
        assert_eq!(
            generate_example(&schema, &Yaml::Null, "body")
                .unwrap()
                .value,
            Value::Null
        );
    }

    #[test]
    fn typed_component_references_keep_literal_suffix_semantics() {
        let openapi: openapiv3::OpenAPI = serde_json::from_value(json!({
            "openapi": "3.0.3", "info": {"title": "test", "version": "1"}, "paths": {},
            "components": {"schemas": {
                "A/B": {"type": "integer", "minimum": 7},
                "A~1B": {"type": "integer", "minimum": 9}
            }}
        }))
        .unwrap();
        let root = typed_bridge::root(&openapi).unwrap();
        for (reference, expected) in [
            ("#/components/schemas/A/B", json!(7)),
            ("#/components/schemas/A~1B", json!(9)),
            ("#/info", Value::Null),
            ("elsewhere.yaml#/A", Value::Null),
        ] {
            let schema = typed_bridge::schema(&openapiv3::ReferenceOr::Reference {
                reference: reference.into(),
            })
            .unwrap();
            assert_eq!(
                generate_example(&schema, &root, "body").unwrap().value,
                expected
            );
        }
    }
}
