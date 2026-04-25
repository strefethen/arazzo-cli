use super::*;

pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

pub(super) fn resolve_payload(value: &serde_yaml_ng::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(v) => Value::Bool(*v),
        serde_yaml_ng::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else {
                Value::Null
            }
        }
        serde_yaml_ng::Value::String(v) => eval.resolve_value(v),
        serde_yaml_ng::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(resolve_payload(item, eval));
            }
            Value::Array(out)
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                out.insert(key, resolve_payload(v, eval));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

pub(super) fn to_json_path(expr: &str) -> String {
    if let Some(path) = expr.strip_prefix("$response.body.") {
        return path.to_string();
    }
    if let Some(path) = expr.strip_prefix("$response.body") {
        return path.trim_start_matches('.').to_string();
    }
    expr.to_string()
}
