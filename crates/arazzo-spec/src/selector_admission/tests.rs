use super::borrowed_selector_context;
use crate::ValueSource;
use serde_yaml_ng::Value;

fn value(yaml: &str) -> Value {
    serde_yaml_ng::from_str(yaml).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn assert_context(value: &Value, admitted: bool) {
    let actual = borrowed_selector_context(value);
    if admitted {
        let original = value["context"]
            .as_str()
            .unwrap_or_else(|| panic!("admitted fixture context must be a string"));
        assert_eq!(actual, Some(original));
        assert!(actual.is_some_and(|context| std::ptr::eq(context, original)));
    } else {
        assert!(actual.is_none(), "must reject {value:?}");
    }
}

#[test]
fn borrowed_shape_defaults_and_field_types() {
    for (fields, admitted) in [
        ("type: jsonpath", true),
        ("type: future", true),
        ("type: {}", true),
        ("type: {version: rfc}", true),
        ("type: {unexpected: [keep]}", true),
        ("type: {type: jsonpath, version: rfc}", true),
        ("type: {type: 23}", false),
        ("type: {version: true}", false),
        ("type: {type: null}", false),
        ("type: {type: [jsonpath]}", false),
        ("type: null", false),
        ("type: 23", false),
        ("type: [jsonpath]", false),
        ("type: true", false),
        ("", false),
    ] {
        let fixture = value(&format!(
            "context: $response.body\nselector: $.item\n{fields}\n"
        ));
        assert_context(&fixture, admitted);
    }
    for yaml in [
        "context: 23\nselector: $.item\ntype: jsonpath",
        "context: $response.body\nselector: true\ntype: jsonpath",
        "selector: $.item\ntype: jsonpath",
        "context: $response.body\ntype: jsonpath",
        "ordinary: [literal]",
        "null",
        "[a, b]",
    ] {
        assert_context(&value(yaml), false);
    }
}

#[test]
fn borrowed_unknown_fields_and_key_shapes() {
    for (extension, admitted) in [
        ("x-note: {23: keep}", true),
        ("unknown: {nested: [keep, 2]}", true),
        ("23: keep", false),
        ("true: keep", false),
        ("? [a, b]\n: keep", false),
    ] {
        assert_context(
            &value(&format!(
                "context: $response.body\nselector: $.item\ntype: jsonpath\n{extension}\n"
            )),
            admitted,
        );
    }
    assert_context(
        &value("context: $response.body\nselector: $.item\ntype: {23: keep}"),
        false,
    );
}

#[test]
fn format_scan_rejects_tags_at_every_depth() {
    for yaml in [
        "!root {context: $response.body, selector: $.item, type: jsonpath}",
        "context: !context $response.body\nselector: $.item\ntype: jsonpath",
        "!key context: $response.body\nselector: $.item\ntype: jsonpath",
        "context: $response.body\nselector: $.item\ntype: !kind jsonpath",
        "context: $response.body\nselector: $.item\ntype: {type: jsonpath, x: [!leaf keep]}",
        "context: $response.body\nselector: $.item\ntype: jsonpath\nx: {nested: [!leaf keep]}",
        "context: $response.body\nselector: $.item\ntype: jsonpath\nx: {!key keep: value}",
        "ordinary: {nested: [!leaf keep]}",
    ] {
        let original = value(yaml);
        assert_context(&original, false);
        assert_eq!(
            ValueSource::from(original.clone()),
            ValueSource::Literal(original)
        );
    }
}

#[test]
fn nested_selectors_return_their_own_context_references() {
    let tree = value("children:\n - context: $response.body\n   selector: $.item\n   type: jsonpath\n - nested:\n     context: $inputs.data\n     selector: /item\n     type: jsonpointer\n");
    assert_context(&tree["children"][0], true);
    assert_context(&tree["children"][1]["nested"], true);
    assert_context(&tree, false);
    assert_context(&tree["children"], false);
}

#[test]
fn large_deep_extensions_are_discarded_without_losing_context_identity() {
    let mut tree =
        value("context: $response.body\nselector: $.item\ntype: {type: jsonpath, version: rfc}");
    let mut extension = Value::String("large".repeat(1_000_000));
    for _ in 0..128 {
        extension = Value::Sequence(vec![extension]);
    }
    tree.as_mapping_mut()
        .unwrap_or_else(|| panic!("selector mapping"))
        .insert(Value::String("x-large".into()), extension);
    assert_context(&tree, true);
}

#[test]
fn from_preserves_complete_literal_values() {
    for yaml in [
        "context: [not, string]\nselector: $.item\ntype: jsonpath\nx-note: {nested: [keep, 2]}",
        "context: $response.body\nselector: $.item\ntype: {type: 23}\nunknown: {nested: [keep, 2]}",
        "ordinary: [true, null, 23, {nested: keep}]",
    ] {
        let original = value(yaml);
        assert_context(&original, false);
        assert_eq!(
            ValueSource::from(original.clone()),
            ValueSource::Literal(original)
        );
    }
}
