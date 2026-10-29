//! Current public-model admission compatibility, not normative Arazzo validity.
//! Incomplete/unknown type objects and entrypoint quirks below remain subject to
//! validation and future authorized behavior changes. Shared admission ownership
//! (ac-8d855) and expression-site inventory (ac-89176) remain separate work.

use arazzo_spec::{ExpressionType, OutputValue, SelectorObject, SelectorType, ValueSource};
use serde::Deserialize;
use serde_yaml_ng::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Admission {
    Selector,
    Literal,
    Error,
    Unavailable,
}

use Admission::{Error as E, Literal as L, Selector as S, Unavailable as U};

fn selector(
    context: &str,
    selection: &str,
    type_: SelectorType,
    extensions: &str,
) -> SelectorObject {
    SelectorObject {
        context: context.into(),
        selector: selection.into(),
        type_,
        extensions: serde_yaml_ng::from_str(extensions)
            .unwrap_or_else(|error| panic!("expected extension values: {error}")),
    }
}

fn named(name: &str, extensions: &str) -> Option<SelectorObject> {
    Some(selector(
        "$response.body",
        "$.item",
        SelectorType::Name(name.into()),
        extensions,
    ))
}

fn typed(
    name: &str,
    version: &str,
    extensions: &str,
    type_extensions: &str,
) -> Option<SelectorObject> {
    Some(selector(
        "$response.body",
        "$.item",
        SelectorType::ExpressionType(ExpressionType {
            type_: name.into(),
            version: version.into(),
            extensions: serde_yaml_ng::from_str(type_extensions)
                .unwrap_or_else(|error| panic!("expected type extension values: {error}")),
        }),
        extensions,
    ))
}

fn assert_source(
    entrypoint: &str,
    actual: Result<ValueSource, serde_yaml_ng::Error>,
    expected: Admission,
    selector: Option<&SelectorObject>,
    original: Option<&Value>,
) {
    match expected {
        S => assert_eq!(
            actual.unwrap_or_else(|error| panic!("{entrypoint}: {error}")),
            ValueSource::Selector(
                selector
                    .unwrap_or_else(|| panic!("explicit expected Selector for {entrypoint}"))
                    .clone()
            ),
            "{entrypoint}"
        ),
        L => assert_eq!(
            actual.unwrap_or_else(|error| panic!("{entrypoint}: {error}")),
            ValueSource::Literal(
                original
                    .unwrap_or_else(|| panic!("original Value for Literal equality: {entrypoint}"))
                    .clone()
            ),
            "{entrypoint}"
        ),
        E => assert!(actual.is_err(), "{entrypoint} must reject this input"),
        U => panic!("{entrypoint} must not run without a parsed Value"),
    }
}

fn assert_selector(
    entrypoint: &str,
    actual: Result<SelectorObject, serde_yaml_ng::Error>,
    expected: Admission,
    selector: Option<&SelectorObject>,
) {
    match expected {
        S => assert_eq!(
            &actual.unwrap_or_else(|error| panic!("{entrypoint}: {error}")),
            selector.unwrap_or_else(|| panic!("explicit expected Selector for {entrypoint}")),
            "{entrypoint}"
        ),
        E => assert!(actual.is_err(), "{entrypoint} must reject this input"),
        L | U => panic!("invalid direct Selector expectation for {entrypoint}"),
    }
}

fn check_case(yaml: &str, selector: Option<SelectorObject>, expected: [Admission; 5]) {
    // Each row explicitly lists: wire ValueSource, Value -> From, from_value
    // ValueSource, wire SelectorObject, borrowed SelectorObject. These helpers
    // only assert observations; they do not classify the YAML themselves.
    let [wire_source, value_from, value_serde, wire_selector, borrowed_selector] = expected;
    // Exercise the original wire input before any Value normalization. A Value
    // cannot represent the duplicate-key fixtures admitted by wire deserialization.
    let source = serde_yaml_ng::from_str::<ValueSource>(yaml);
    assert_selector(
        "wire SelectorObject",
        serde_yaml_ng::from_str(yaml),
        wire_selector,
        selector.as_ref(),
    );
    match serde_yaml_ng::from_str::<Value>(yaml) {
        Ok(value) => {
            assert_source(
                "wire ValueSource",
                source,
                wire_source,
                selector.as_ref(),
                Some(&value),
            );
            assert_source(
                "ValueSource::from",
                Ok(ValueSource::from(value.clone())),
                value_from,
                selector.as_ref(),
                Some(&value),
            );
            assert_source(
                "from_value ValueSource",
                serde_yaml_ng::from_value(value.clone()),
                value_serde,
                selector.as_ref(),
                Some(&value),
            );
            assert_selector(
                "borrowed SelectorObject",
                SelectorObject::deserialize(&value),
                borrowed_selector,
                selector.as_ref(),
            );
        }
        Err(_) => {
            assert_eq!(
                [value_from, value_serde, borrowed_selector],
                [U; 3],
                "Value preprocessing must reject this duplicate-key fixture"
            );
            assert_source(
                "wire ValueSource without normalization",
                source,
                wire_source,
                selector.as_ref(),
                None,
            );
        }
    }
}

macro_rules! admission_case {
    ($name:ident, $yaml:literal, $selector:expr, $expected:expr) => {
        #[test]
        fn $name() {
            check_case($yaml, $selector, $expected);
        }
    };
}

// These 24 fixtures retain the exact wire inputs of the executed admission
// probe. The expected typed fields are independent of the deserializer under test.
admission_case!(
    valid_name,
    "context: $response.body\nselector: $.item\ntype: jsonpath\nx-note: keep\n",
    named("jsonpath", "x-note: keep"),
    [S; 5]
);
admission_case!(
    valid_object,
    "context: $response.body\nselector: $.item\ntype: {type: jsonpath, version: rfc9535, x-note: keep, unknown: visible}\nx-note: {nested: [keep, 2]}\n",
    typed(
        "jsonpath",
        "rfc9535",
        "x-note: {nested: [keep, 2]}",
        "x-note: keep\nunknown: visible"
    ),
    [S; 5]
);
admission_case!(
    missing_selector,
    "context: $response.body\ntype: jsonpath\nx-note: keep\n",
    None,
    [L, L, L, E, E]
);
admission_case!(
    mistyped_context_sequence,
    "context: [not, string]\nselector: $.item\ntype: jsonpath\n",
    None,
    [L, L, L, E, E]
);
admission_case!(
    mistyped_context_number,
    "context: 23\nselector: $.item\ntype: jsonpath\n",
    Some(selector(
        "23",
        "$.item",
        SelectorType::Name("jsonpath".into()),
        "{}"
    )),
    [L, L, L, S, E]
);
admission_case!(
    mistyped_selector_bool,
    "context: $response.body\nselector: true\ntype: jsonpath\n",
    Some(selector(
        "$response.body",
        "true",
        SelectorType::Name("jsonpath".into()),
        "{}"
    )),
    [L, L, L, S, E]
);
admission_case!(
    mistyped_type_number,
    "context: $response.body\nselector: $.item\ntype: 23\n",
    None,
    [L, L, L, E, E]
);
admission_case!(
    incomplete_type_empty,
    "context: $response.body\nselector: $.item\ntype: {}\n",
    typed("", "", "{}", "{}"),
    [S; 5]
);
admission_case!(
    incomplete_type_version,
    "context: $response.body\nselector: $.item\ntype: {version: rfc9535}\n",
    typed("", "rfc9535", "{}", "{}"),
    [S; 5]
);
admission_case!(
    unknown_type_name,
    "context: $response.body\nselector: $.item\ntype: future\n",
    named("future", "{}"),
    [S; 5]
);
admission_case!(
    unknown_type_object,
    "context: $response.body\nselector: $.item\ntype: {unexpected: keep}\n",
    typed("", "", "{}", "unexpected: keep"),
    [S; 5]
);
admission_case!(
    root_tag_selector,
    "!root\ncontext: $response.body\nselector: $.item\ntype: jsonpath\n",
    named("jsonpath", "{}"),
    [E, L, E, S, S]
);
admission_case!(
    root_tag_literal,
    "!root {ordinary: keep}\n",
    None,
    [E, L, E, E, E]
);
admission_case!(
    tagged_context,
    "context: !context $response.body\nselector: $.item\ntype: jsonpath\n",
    named("jsonpath", "{}"),
    [E, L, E, S, S]
);
admission_case!(
    tagged_type,
    "context: $response.body\nselector: $.item\ntype: !kind jsonpath\n",
    None,
    [E, L, E, E, E]
);
admission_case!(
    tagged_key,
    "!field context: $response.body\nselector: $.item\ntype: jsonpath\n",
    named("jsonpath", "{}"),
    [E, L, E, S, S]
);
admission_case!(
    tagged_extension_descendant,
    "context: $response.body\nselector: $.item\ntype: jsonpath\nx-note: {nested: [!leaf keep]}\n",
    None,
    [E, L, E, E, E]
);
admission_case!(
    tagged_literal_descendant,
    "ordinary: {nested: [!leaf keep]}\n",
    None,
    [E, L, E, E, E]
);
admission_case!(
    duplicate_known,
    "context: first\ncontext: second\nselector: $.item\ntype: jsonpath\n",
    None,
    [E, U, U, E, U]
);
admission_case!(
    duplicate_extension,
    "context: $response.body\nselector: $.item\ntype: jsonpath\nx-note: first\nx-note: second\n",
    named("jsonpath", "x-note: second"),
    [S, U, U, S, U]
);
admission_case!(
    duplicate_type_known,
    "context: $response.body\nselector: $.item\ntype: {type: jsonpath, type: xpath}\n",
    None,
    [E, U, U, E, U]
);
admission_case!(
    duplicate_type_extension,
    "context: $response.body\nselector: $.item\ntype: {type: jsonpath, x-note: first, x-note: second}\n",
    typed("jsonpath", "", "{}", "x-note: second"),
    [S, U, U, S, U]
);
admission_case!(
    nonstring_root_key,
    "context: $response.body\nselector: $.item\ntype: jsonpath\n23: keep\n",
    named("jsonpath", "\"23\": keep"),
    [L, L, L, S, E]
);
admission_case!(
    nonstring_nested_extension_key,
    "context: $response.body\nselector: $.item\ntype: jsonpath\nx-note: {23: keep}\n",
    named("jsonpath", "x-note: {23: keep}"),
    [S; 5]
);

// Missing required-field siblings complete the original missing-selector case.
admission_case!(
    missing_context,
    "selector: $.item\ntype: jsonpath\nx-note: keep\n",
    None,
    [L, L, L, E, E]
);
admission_case!(
    missing_type,
    "context: $response.body\nselector: $.item\nx-note: keep\n",
    None,
    [L, L, L, E, E]
);

#[test]
fn output_runtime_expression_string() {
    assert_eq!(
        serde_yaml_ng::from_str::<OutputValue>("$response.body#/item")
            .unwrap_or_else(|error| panic!("output expression: {error}")),
        OutputValue::RuntimeExpression("$response.body#/item".into())
    );
}

#[test]
fn output_selector_preserves_fields_and_extensions() {
    assert_eq!(
        serde_yaml_ng::from_str::<OutputValue>(
            "context: $response.body\nselector: $.item\ntype: jsonpath\nx-note: {nested: [keep, 2]}\n"
        ).unwrap_or_else(|error| panic!("output Selector: {error}")),
        OutputValue::Selector(selector(
            "$response.body",
            "$.item",
            SelectorType::Name("jsonpath".into()),
            "x-note: {nested: [keep, 2]}"
        ))
    );
}

#[test]
fn output_rejects_malformed_selectors() {
    for yaml in [
        "context: $response.body\ntype: jsonpath\n",
        "selector: $.item\ntype: jsonpath\n",
        "context: $response.body\nselector: $.item\n",
    ] {
        assert!(
            serde_yaml_ng::from_str::<OutputValue>(yaml).is_err(),
            "{yaml}"
        );
    }
}
