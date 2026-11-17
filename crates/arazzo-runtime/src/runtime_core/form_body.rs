use super::{url_crate::form_urlencoded, RuntimeError, RuntimeErrorKind};
use serde_json::Value;

pub(super) fn is_form(content_type: &str) -> bool {
    arazzo_spec::media_type::is_form_urlencoded(content_type)
}

pub(super) fn encode(value: &Value) -> Result<Vec<u8>, RuntimeError> {
    match value {
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        Value::Object(fields) => {
            let mut serializer = form_urlencoded::Serializer::new(String::new());
            for (name, value) in fields {
                match value {
                    Value::Array(items) => {
                        for item in items {
                            append(&mut serializer, name, item);
                        }
                    }
                    value => append(&mut serializer, name, value),
                }
            }
            Ok(serializer.finish().into_bytes())
        }
        value => {
            let kind = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::Array(_) => "array",
                Value::Object(_) | Value::String(_) => unreachable!(),
            };
            Err(RuntimeError::new(
                RuntimeErrorKind::InvalidRequestBody,
                format!("application/x-www-form-urlencoded request body must be an object or string; got {kind}"),
            ))
        }
    }
}

fn append(serializer: &mut form_urlencoded::Serializer<'_, String>, name: &str, value: &Value) {
    match value {
        Value::Null => {}
        Value::String(text) => {
            serializer.append_pair(name, text);
        }
        value => {
            serializer.append_pair(name, &value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn form_essence_is_exact() {
        for ct in [
            "application/x-www-form-urlencoded",
            "Application/X-WWW-Form-Urlencoded; charset=UTF-8",
        ] {
            assert!(is_form(ct));
        }
        for ct in [
            "",
            "application/json",
            "application/x-www-form-urlencoded-extra",
            "text/application/x-www-form-urlencoded",
        ] {
            assert!(!is_form(ct));
        }
    }

    #[test]
    fn invalid_shapes_have_only_type_diagnostics() {
        for (value, kind) in [
            (json!([]), "array"),
            (json!(["SECRET"]), "array"),
            (json!(42), "number"),
            (json!(true), "boolean"),
            (json!(false), "boolean"),
            (Value::Null, "null"),
        ] {
            let error = encode(&value)
                .err()
                .unwrap_or_else(|| panic!("must refuse {kind}"));
            assert_eq!(error.kind, RuntimeErrorKind::InvalidRequestBody);
            assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY");
            assert_eq!(error.message, format!("application/x-www-form-urlencoded request body must be an object or string; got {kind}"));
        }
    }

    #[test]
    fn field_types_and_encoding() {
        for (value, expected) in [
            (
                json!({"a b":"c~d", "empty":"", "null":null}),
                "a+b=c%7Ed&empty=",
            ),
            (json!({"n":42,"ok":true,"pi":1.5}), "n=42&ok=true&pi=1.5"),
            (
                json!({"tag":["a",null,"c d",{"x":null},[null]]}),
                "tag=a&tag=c+d&tag=%7B%22x%22%3Anull%7D&tag=%5Bnull%5D",
            ),
            (json!({"empty":[]}), ""),
            (json!("x=a+b%20c"), "x=a+b%20c"),
        ] {
            assert_eq!(encode(&value).ok().as_deref(), Some(expected.as_bytes()));
        }
    }
}
