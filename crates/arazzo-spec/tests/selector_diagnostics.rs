//! Public compatibility observations captured before the shared-owner extraction.
//! These are current Serde contracts, not normative Arazzo validity rules.

use arazzo_spec::{ExpressionType, SelectorObject, SelectorType, ValueSource};
use serde::de::value::{
    BorrowedBytesDeserializer, BytesDeserializer, Error, MapDeserializer, StrDeserializer,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;

fn yaml_error<T: DeserializeOwned>(yaml: &str) -> String {
    match serde_yaml_ng::from_str::<T>(yaml) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("expected rejection for {yaml}"),
    }
}

#[test]
fn extension_value_errors_keep_their_original_diagnostic() {
    for extension in ["{a: 1, a: 2}", "[{a: 1, a: 2}]"] {
        let yaml = format!(
            "context: $response.body\nselector: $.item\ntype: jsonpath\nx-z: {extension}\n"
        );
        assert_eq!(
            yaml_error::<SelectorObject>(&yaml),
            "duplicate entry with key \"a\""
        );
        assert_eq!(
            yaml_error::<ValueSource>(&yaml),
            "data did not match any variant of untagged enum ValueSource"
        );
    }
    assert_eq!(
        yaml_error::<ExpressionType>("{x: {a: 1, a: 2}}"),
        "duplicate entry with key \"a\""
    );
    assert_eq!(
        yaml_error::<SelectorType>("{x: {a: 1, a: 2}}"),
        "data did not match any variant of untagged enum SelectorType"
    );
}

#[test]
fn required_and_duplicate_fields_precede_extension_conversion_errors() {
    assert_eq!(
        yaml_error::<SelectorObject>("selector: $.item\ntype: jsonpath\nx-z: {a: 1, a: 2}\n"),
        "missing field `context`"
    );
    assert_eq!(yaml_error::<SelectorObject>("x-z: {a: 1, a: 2}\ncontext: first\ncontext: second\nselector: $.item\ntype: jsonpath\n"), "duplicate field `context`");
    assert_eq!(
        yaml_error::<ExpressionType>("{x: {a: 1, a: 2}, type: first, type: second}"),
        "duplicate field `type`"
    );
}

#[test]
fn tags_retain_their_envelope_diagnostics() {
    let yaml = "context: $response.body\nselector: $.item\ntype: jsonpath\nx-z: [!leaf keep]\n";
    let expected =
        "x-z[0]: untagged and internally tagged enums do not support enum input at line 4 column 7";
    assert_eq!(yaml_error::<SelectorObject>(yaml), expected);
    assert_eq!(yaml_error::<ValueSource>(yaml), expected);
    assert_eq!(
        yaml_error::<SelectorType>("{type: !tag jsonpath}"),
        "type: untagged and internally tagged enums do not support enum input at line 1 column 8"
    );
    assert_eq!(
        yaml_error::<ExpressionType>("{x: !tag keep}"),
        "x: untagged and internally tagged enums do not support enum input at line 1 column 5"
    );
}

#[test]
fn standalone_types_keep_distinct_wire_coercion() {
    let actual: ExpressionType = serde_yaml_ng::from_str("{type: 23}")
        .unwrap_or_else(|error| panic!("direct expression type: {error}"));
    assert_eq!(actual.type_, "23");
    assert_eq!(actual.version, "");
    assert_eq!(
        yaml_error::<SelectorType>("{type: 23}"),
        "data did not match any variant of untagged enum SelectorType"
    );
    assert_eq!(
        yaml_error::<ExpressionType>("{type: a, type: b}"),
        "duplicate field `type`"
    );
}

fn byte_keys<'a>(pairs: &'a [(&'a [u8], &'a str)]) -> impl serde::Deserializer<'a, Error = Error> {
    MapDeserializer::new(pairs.iter().map(|(key, value)| {
        (
            BytesDeserializer::<Error>::new(key),
            StrDeserializer::<Error>::new(value),
        )
    }))
}

#[test]
fn utf8_bytes_preserve_known_fields_and_unknown_keys() {
    let pairs: &[(&[u8], &str)] = &[
        (b"context", "$response.body"),
        (b"selector", "$.item"),
        (b"type", "jsonpath"),
        ("x-café".as_bytes(), "visible"),
    ];
    let selector = SelectorObject::deserialize(byte_keys(pairs))
        .unwrap_or_else(|error| panic!("byte keys: {error}"));
    assert_eq!(selector.context, "$response.body");
    assert_eq!(selector.selector, "$.item");
    assert_eq!(selector.type_, SelectorType::Name("jsonpath".into()));
    assert_eq!(
        selector.extensions.get("x-café"),
        Some(&serde_yaml_ng::Value::String("visible".into()))
    );
    let expression_fields = [
        (b"type".as_slice(), b"jsonpath".as_slice()),
        (b"version".as_slice(), b"rfc".as_slice()),
    ];
    let expression = ExpressionType::deserialize(MapDeserializer::<_, Error>::new(
        expression_fields.iter().map(|(key, value)| {
            (
                BytesDeserializer::<Error>::new(key),
                BytesDeserializer::<Error>::new(value),
            )
        }),
    ))
    .unwrap_or_else(|error| panic!("byte expression fields: {error}"));
    assert_eq!(expression.type_, "jsonpath");
    assert_eq!(expression.version, "rfc");
    for actual in [
        SelectorType::deserialize(BytesDeserializer::<Error>::new(b"jsonpath")),
        SelectorType::deserialize(BorrowedBytesDeserializer::<Error>::new(b"jsonpath")),
    ] {
        assert_eq!(
            actual.unwrap_or_else(|error| panic!("byte type: {error}")),
            SelectorType::Name("jsonpath".into())
        );
    }
}

#[test]
fn invalid_utf8_key_errors_follow_required_field_checks() {
    let complete: &[(&[u8], &str)] = &[
        (b"context", "$response.body"),
        (b"selector", "$.item"),
        (b"type", "jsonpath"),
        (&[255], "visible"),
    ];
    let missing: &[(&[u8], &str)] = &[
        (&[255], "visible"),
        (b"selector", "$.item"),
        (b"type", "jsonpath"),
    ];
    let duplicate: &[(&[u8], &str)] = &[
        (&[255], "visible"),
        (b"context", "first"),
        (b"context", "second"),
        (b"selector", "$.item"),
        (b"type", "jsonpath"),
    ];
    for (pairs, expected) in [
        (complete, "invalid value: byte array, expected a string"),
        (missing, "missing field `context`"),
        (duplicate, "duplicate field `context`"),
    ] {
        let actual = SelectorObject::deserialize(byte_keys(pairs));
        assert_eq!(
            actual.map_err(|error| error.to_string()),
            Err(expected.into())
        );
    }
}

#[test]
fn json_values_and_diagnostics_are_unchanged() {
    let selector: SelectorObject = serde_json::from_str(r#"{"context":"$response.body","selector":"$.item","type":{"version":"rfc"},"x-extra":1,"x-extra":2}"#).unwrap_or_else(|error| panic!("JSON Selector: {error}"));
    assert_eq!(
        selector.type_,
        SelectorType::ExpressionType(ExpressionType {
            type_: "".into(),
            version: "rfc".into(),
            extensions: Default::default()
        })
    );
    assert_eq!(
        selector.extensions.get("x-extra"),
        Some(&serde_yaml_ng::Value::Number(2.into()))
    );
    let missing = r#"{"context":"$response.body","type":"jsonpath"}"#;
    assert_eq!(
        serde_json::from_str::<SelectorObject>(missing).map_err(|error| error.to_string()),
        Err("missing field `selector` at line 1 column 46".into())
    );
    let nested = r#"{"context":"$response.body","selector":"$.item","type":"jsonpath","x-extra":{"a":1,"a":2}}"#;
    assert_eq!(
        serde_json::from_str::<SelectorObject>(nested).map_err(|error| error.to_string()),
        Err("duplicate entry with key \"a\" at line 1 column 90".into())
    );
}
