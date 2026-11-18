use super::url_crate::form_urlencoded;
use serde_json::{Map, Value};

pub(super) fn encode(fields: &Map<String, Value>) -> Vec<u8> {
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
    serializer.finish().into_bytes()
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
        ] {
            let Value::Object(fields) = value else {
                panic!("fixture must be an object");
            };
            assert_eq!(encode(&fields), expected.as_bytes());
        }
    }
}
