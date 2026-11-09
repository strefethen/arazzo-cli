use arazzo_expr::{ExpressionResolution, StoredValueReference};
use arazzo_spec::{ExpressionType, Replacement, SelectorType};

use super::payload::{
    resolve_selector_with_provenance, resolve_value_source_with_provenance, ResolvedValueSource,
    ValueResolutionFailureReason,
};
use super::*;

fn evaluator() -> ExpressionEvaluator {
    let mut context = EvalContext {
        request_body: Some(json!({"present": null})),
        response_body: Some(json!({"present": null})),
        inputs: BTreeMap::from([
            (
                "document".into(),
                json!({"items": [{"id": 1}, {"id": 2}], "nil": null}),
            ),
            ("nil".into(), Value::Null),
            ("empty".into(), json!("")),
            ("array".into(), json!([])),
            ("object".into(), json!({})),
            ("xml".into(), json!("<root><id>7</id></root>")),
            ("badXml".into(), json!("<root>")),
        ])
        .into(),
        outputs: BTreeMap::from([("nil".into(), Value::Null)]),
        workflows: BTreeMap::from([(
            "prior".into(),
            arazzo_expr::WorkflowEvalState {
                inputs: BTreeMap::from([("nil".into(), Value::Null)]),
                outputs: BTreeMap::from([("nil".into(), Value::Null)]),
            },
        )])
        .into(),
        ..EvalContext::default()
    };
    context.steps = Arc::new(BTreeMap::from([(
        "prior".into(),
        BTreeMap::from([("nil".into(), Value::Null)]),
    )]));
    ExpressionEvaluator::new(context)
}

fn source(value: Value) -> ValueSource {
    serde_yaml_ng::to_value(value)
        .unwrap_or_else(|error| panic!("converting test value to YAML: {error}"))
        .into()
}

fn selector(context: &str, query: &str, kind: &str) -> SelectorObject {
    SelectorObject {
        context: context.into(),
        selector: query.into(),
        type_: SelectorType::Name(kind.into()),
        extensions: BTreeMap::new(),
    }
}

fn versioned_selector(context: &str, query: &str, kind: &str, version: &str) -> SelectorObject {
    SelectorObject {
        type_: SelectorType::ExpressionType(ExpressionType {
            type_: kind.into(),
            version: version.into(),
            ..ExpressionType::default()
        }),
        ..selector(context, query, kind)
    }
}

fn occurrences(result: &ResolvedValueSource) -> Vec<(&str, &ExpressionResolution)> {
    result
        .expressions
        .iter()
        .map(|item| (item.pointer.as_str(), &item.resolution))
        .collect()
}

fn failures(result: &ResolvedValueSource) -> Vec<(&str, &str, ValueResolutionFailureReason)> {
    result
        .failures
        .iter()
        .map(|item| (item.pointer.as_str(), item.source.as_str(), item.reason))
        .collect()
}

#[test]
fn direct_values_distinguish_missing_from_present_null_empty_and_literals() {
    let eval = evaluator();
    for (text, value, count) in [
        ("$inputs.nil", Value::Null, 1),
        ("$inputs.empty", json!(""), 1),
        ("$inputs.array", json!([]), 1),
        ("$inputs.object", json!({}), 1),
        ("$USD", json!("$USD"), 0),
        ("$env.X", json!("$env.X"), 0),
        ("$response.body.id", json!("$response.body.id"), 0),
        ("plain", json!("plain"), 0),
    ] {
        let result = resolve_value_source_with_provenance(&source(json!(text)), &eval);
        assert_eq!(result.value, value, "{text}");
        assert!(result.warnings.is_empty(), "{text}: {:?}", result.warnings);
        assert_eq!(result.expressions.len(), count, "{text}");
        assert!(result
            .expressions
            .iter()
            .all(|item| item.pointer.is_empty() && !item.resolution.missing));
        assert!(result.failures.is_empty(), "{text}: {:?}", result.failures);
    }
    for text in [
        "$inputs.absent",
        "$request.body#/absent",
        "$response.body#/absent",
    ] {
        let result = resolve_value_source_with_provenance(&source(json!(text)), &eval);
        assert_eq!(result.value, Value::Null);
        assert_eq!(result.expressions.len(), 1);
        assert!(result.expressions[0].resolution.missing);
        assert_eq!(
            failures(&result),
            [("", text, ValueResolutionFailureReason::ExpressionMissing)]
        );
        assert_eq!(
            result
                .warnings
                .iter()
                .map(|warning| warning.expression.as_str())
                .collect::<Vec<_>>(),
            [text]
        );
    }
}

#[test]
fn recursive_occurrences_and_failures_preserve_order_and_escaped_locations() {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        "'a/b~c':\n  - '{$inputs.nil}:{$inputs.absent}:{$inputs.absent}'\n  - '$response.body#/absent'\n'':\n  '$outputs.nil'\nlast: '{$inputs.nil'\n",
    ).unwrap_or_else(|error| panic!("parsing test value: {error}"));
    let eval = evaluator();
    let result = resolve_value_source_with_provenance(&value.into(), &eval);
    assert_eq!(
        result.value,
        json!({"a/b~c": ["null:null:null", null], "": null, "last": "{$inputs.nil"})
    );
    let actual = occurrences(&result)
        .into_iter()
        .map(|(pointer, resolution)| (pointer, resolution.expression.as_str(), resolution.missing))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            ("/a~1b~0c/0", "$inputs.nil", false),
            ("/a~1b~0c/0", "$inputs.absent", true),
            ("/a~1b~0c/0", "$inputs.absent", true),
            ("/a~1b~0c/1", "$response.body#/absent", true),
            ("/", "$outputs.nil", false),
        ]
    );
    assert_eq!(
        failures(&result),
        [
            (
                "/a~1b~0c/0",
                "$inputs.absent",
                ValueResolutionFailureReason::ExpressionMissing
            ),
            (
                "/a~1b~0c/0",
                "$inputs.absent",
                ValueResolutionFailureReason::ExpressionMissing
            ),
            (
                "/a~1b~0c/1",
                "$response.body#/absent",
                ValueResolutionFailureReason::ExpressionMissing
            ),
            (
                "/last",
                "{$inputs.nil",
                ValueResolutionFailureReason::ExpressionSyntax
            ),
        ]
    );
    assert_eq!(
        result
            .warnings
            .iter()
            .map(|warning| warning.expression.as_str())
            .collect::<Vec<_>>(),
        [
            "$inputs.absent",
            "$inputs.absent",
            "$response.body#/absent",
            "{$inputs.nil"
        ]
    );
}

#[test]
fn selectors_retain_successful_aliases_at_the_selected_value_location() {
    let eval = evaluator();
    let result = resolve_value_source_with_provenance(
        &source(json!([
            {"context": "$inputs.document", "selector": "$.items[0].id", "type": "jsonpath"},
            {"context": "$steps.prior.outputs.nil", "selector": "$", "type": "jsonpath"},
            {"context": "$outputs.nil", "selector": "$", "type": "jsonpath"},
            {"context": "$workflows.prior.inputs.nil", "selector": "$", "type": "jsonpath"},
            {"context": "$workflows.prior.outputs.nil", "selector": "$", "type": "jsonpath"}
        ])),
        &eval,
    );
    assert_eq!(result.value, json!([1, null, null, null, null]));
    assert!(result.warnings.is_empty());
    assert!(result.failures.is_empty());
    assert_eq!(result.expressions.len(), 5);
    assert_eq!(result.expressions[0].pointer, "/0");
    assert_eq!(
        result.expressions[0].resolution.reference,
        Some(StoredValueReference::Input {
            name: "document".into(),
            pointer: None
        })
    );
    assert_eq!(result.expressions[1].pointer, "/1");
    assert_eq!(
        result.expressions[1].resolution.reference,
        Some(StoredValueReference::StepOutput {
            step_id: "prior".into(),
            name: "nil".into(),
            pointer: None
        })
    );
    assert_eq!(result.expressions[2].pointer, "/2");
    assert_eq!(
        result.expressions[2].resolution.reference,
        Some(StoredValueReference::Output {
            name: "nil".into(),
            pointer: None
        })
    );
    assert_eq!(result.expressions[3].pointer, "/3");
    assert_eq!(
        result.expressions[3].resolution.reference,
        Some(StoredValueReference::WorkflowInput {
            workflow_id: "prior".into(),
            name: "nil".into(),
            pointer: None
        })
    );
    assert_eq!(result.expressions[4].pointer, "/4");
    assert_eq!(
        result.expressions[4].resolution.reference,
        Some(StoredValueReference::WorkflowOutput {
            workflow_id: "prior".into(),
            name: "nil".into(),
            pointer: None
        })
    );
}

#[test]
fn nested_selector_failures_belong_to_the_selected_value_location() {
    let eval = evaluator();
    let result = resolve_value_source_with_provenance(
        &source(json!({"nested": [
            {"context": "$response.body#/absent", "selector": "$", "type": "jsonpath"},
            {"context": "$inputs.document", "selector": "invalid", "type": "jsonpointer"}
        ]})),
        &eval,
    );
    assert_eq!(result.value, json!({"nested": [null, null]}));
    assert_eq!(
        occurrences(&result)
            .into_iter()
            .map(|(pointer, _)| pointer)
            .collect::<Vec<_>>(),
        ["/nested/0", "/nested/1"]
    );
    assert_eq!(
        failures(&result),
        [
            (
                "/nested/0",
                "$response.body#/absent",
                ValueResolutionFailureReason::SelectorContextUnresolved
            ),
            (
                "/nested/1",
                "invalid",
                ValueResolutionFailureReason::SelectorHardFailure
            ),
        ]
    );
    assert_eq!(
        result
            .warnings
            .iter()
            .map(|warning| warning.expression.as_str())
            .collect::<Vec<_>>(),
        ["$response.body#/absent", "invalid"]
    );
}

#[test]
fn selector_request_and_response_context_misses_warn_and_are_unresolved() {
    let eval = evaluator();
    for (context, query, count, unresolved) in [
        ("$response.body#/absent", "$", 1, true),
        ("$response.body#/absent", "$.id", 1, true),
        ("$request.body#/absent", "$", 1, true),
        ("$inputs.nil", "$", 0, false),
        ("$response.body#/present", "$", 0, false),
    ] {
        let object = selector(context, query, "jsonpath");
        let result = resolve_selector_with_provenance(&object, &eval);
        assert_eq!(result.value, Value::Null);
        assert_eq!(result.warnings.len(), count, "{context} {query}");
        assert_eq!(result.expressions.len(), 1);
        assert_eq!(result.expressions[0].resolution.missing, unresolved);
        let expected = if unresolved {
            vec![(
                "",
                context,
                ValueResolutionFailureReason::SelectorContextUnresolved,
            )]
        } else {
            vec![]
        };
        assert_eq!(failures(&result), expected);
        assert_eq!(
            resolve_selector(&object, &eval),
            (result.value, result.warnings)
        );
    }
}

#[test]
fn selector_context_requires_complete_expression_and_retains_noisy_misses() {
    let eval = evaluator();
    for context in [
        "$inputs.absent",
        "$USD",
        "$response.body.id",
        "{$inputs.nil}",
    ] {
        let result = resolve_selector_with_provenance(&selector(context, "$", "jsonpath"), &eval);
        assert_eq!(result.value, Value::Null);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(
            failures(&result),
            [(
                "",
                context,
                ValueResolutionFailureReason::SelectorContextUnresolved
            )]
        );
        assert_eq!(
            result.expressions.len(),
            usize::from(context == "$inputs.absent")
        );
    }
}

#[test]
fn selector_zero_one_many_and_selected_null_are_sound() {
    let eval = evaluator();
    for (query, expected, warnings) in [
        ("$.absent", Value::Null, 1),
        ("$.items[0].id", json!(1), 0),
        ("$.items[*].id", json!([1, 2]), 0),
        ("$.nil", Value::Null, 0),
    ] {
        let result = resolve_selector_with_provenance(
            &selector("$inputs.document", query, "jsonpath"),
            &eval,
        );
        assert_eq!(result.value, expected);
        assert_eq!(result.warnings.len(), warnings);
        assert!(result.failures.is_empty(), "{query}: {:?}", result.failures);
        assert_eq!(result.expressions.len(), 1);
    }
    for (object, expected, warnings) in [
        (
            selector("$inputs.document", "/absent", "jsonpointer"),
            Value::Null,
            1,
        ),
        (
            selector("$inputs.document", "/nil", "jsonpointer"),
            Value::Null,
            0,
        ),
        (selector("$inputs.nil", "", "jsonpointer"), Value::Null, 0),
        (
            versioned_selector("$inputs.xml", "/root/absent", "xpath", "xpath-10"),
            Value::Null,
            1,
        ),
        (
            versioned_selector("$inputs.xml", "/root/id", "xpath", "xpath-10"),
            json!("7"),
            0,
        ),
    ] {
        let result = resolve_selector_with_provenance(&object, &eval);
        assert_eq!(result.value, expected);
        assert_eq!(result.warnings.len(), warnings);
        assert!(
            result.failures.is_empty(),
            "{object:?}: {:?}",
            result.failures
        );
    }
}

#[test]
fn jsonpath_admission_failures_precede_context_and_retain_one_hard_failure() {
    let eval = evaluator();
    for (object, diagnostic) in [
        (
            versioned_selector("$inputs.absent", "$", "jsonpath", "goessner"),
            "unsupported JSONPath version",
        ),
        (
            selector("$inputs.absent", "$[", "jsonpath"),
            "invalid JSONPath syntax",
        ),
        (
            selector(
                "$inputs.absent",
                &format!("${}", ".a".repeat(129)),
                "jsonpath",
            ),
            "structural characters limit",
        ),
        (
            selector(
                "$inputs.absent",
                &format!("$['{}']", "x".repeat(16_384)),
                "jsonpath",
            ),
            "query bytes limit",
        ),
    ] {
        let result = resolve_selector_with_provenance(&object, &eval);
        assert_eq!(result.value, Value::Null);
        assert!(result.expressions.is_empty());
        assert_eq!(result.warnings.len(), 1);
        assert!(
            result.warnings[0].message.contains(diagnostic),
            "{:?}",
            result.warnings
        );
        assert_eq!(
            failures(&result),
            [(
                "",
                object.selector.as_str(),
                ValueResolutionFailureReason::SelectorHardFailure
            )]
        );
    }
}

#[test]
fn selector_evaluation_and_non_jsonpath_failures_are_hard() {
    let mut deep = json!({});
    for _ in 0..129 {
        deep = json!([deep]);
    }
    let eval = ExpressionEvaluator::new(EvalContext {
        inputs: BTreeMap::from([
            ("deep".into(), deep),
            ("document".into(), json!({"items": [{"tag": "abc"}]})),
            ("xml".into(), json!("<root/>")),
            ("badXml".into(), json!("<root>")),
        ])
        .into(),
        ..EvalContext::default()
    });
    for (object, diagnostic) in [
        (
            selector("$inputs.deep", "$", "jsonpath"),
            "context nesting depth limit",
        ),
        (
            selector(
                "$inputs.document",
                "$.items[?!match(@.tag, 'a{1000000000}')].tag",
                "jsonpath",
            ),
            "JSONPath evaluation failed",
        ),
        (
            selector("$inputs.document", "not/slash", "jsonpointer"),
            "must be empty or start",
        ),
        (
            versioned_selector("$inputs.document", "", "jsonpointer", "other"),
            "unsupported JSON Pointer version",
        ),
        (
            versioned_selector("$inputs.document", "*", "xpath", "other"),
            "unsupported XPath version",
        ),
        (
            versioned_selector("$inputs.document", "*", "xpath", "xpath-10"),
            "must resolve to an XML string",
        ),
        (
            versioned_selector("$inputs.badXml", "*", "xpath", "xpath-10"),
            "XML",
        ),
        (
            versioned_selector("$inputs.xml", "[", "xpath", "xpath-10"),
            "XPath",
        ),
        (
            selector("$inputs.document", "$", "unknown"),
            "unsupported selector type",
        ),
    ] {
        let result = resolve_selector_with_provenance(&object, &eval);
        assert_eq!(result.value, Value::Null);
        assert_eq!(result.warnings.len(), 1, "{object:?}");
        assert!(
            result.warnings[0].message.contains(diagnostic),
            "{object:?}: {:?}",
            result.warnings
        );
        assert_eq!(result.expressions.len(), 1);
        assert!(!result.expressions[0].resolution.missing);
        assert_eq!(
            failures(&result),
            [(
                "",
                object.selector.as_str(),
                ValueResolutionFailureReason::SelectorHardFailure
            )]
        );
    }
}

#[test]
fn mappings_missing_selector_discriminators_remain_literal() {
    let eval = evaluator();
    for value in [
        json!({"selector": "$", "type": "jsonpath"}),
        json!({"context": "$inputs.nil", "type": "jsonpath"}),
        json!({"context": "$inputs.nil", "selector": "$"}),
    ] {
        let result = resolve_value_source_with_provenance(&source(value.clone()), &eval);
        assert!(result.value.is_object());
        assert!(result.failures.is_empty());
        assert!(result.warnings.is_empty());
        assert_eq!(
            result.expressions.len(),
            usize::from(value.get("context").is_some())
        );
        if let Some(item) = result.expressions.first() {
            assert_eq!(item.pointer, "/context");
        }
    }
}

#[test]
fn replacements_refuse_failed_values_and_preserve_admitted_warning_order() {
    let eval = evaluator();
    let vars = VarStore::default();
    let admission = request_admission::RequestAdmission::new("test", &vars);
    for (value, descriptor) in [
        (
            json!({"context":"$inputs.absent","selector":"$","type":"jsonpath"}),
            "$inputs.absent",
        ),
        (
            json!({"context":"$response.body#/absent","selector":"$","type":"jsonpath"}),
            "$response.body#/absent",
        ),
        (
            json!({"context":"$inputs.document","selector":"invalid","type":"jsonpointer"}),
            "Selector value",
        ),
        (json!("$inputs.absent"), "$inputs.absent"),
        (json!("{$inputs.nil"), "invalid expression syntax"),
    ] {
        let replacements = vec![
            Replacement {
                target: "/first".into(),
                value: source(json!("new")),
                ..Replacement::default()
            },
            Replacement {
                target: "/value".into(),
                value: source(value),
                ..Replacement::default()
            },
        ];
        let error = apply_replacements(
            json!({"first":"old","value":"old"}),
            "application/json",
            &replacements,
            &eval,
            &admission,
        )
        .err()
        .unwrap_or_else(|| panic!("unresolved value"));
        assert_eq!(error.code(), "RUNTIME_UNRESOLVED_REQUEST_VALUE");
        assert_eq!(error.message, format!("step \"test\": requestBody.replacements[1].value: {descriptor} did not resolve; request not sent"));
    }
    let replacements = [
        ("/first", json!("new")),
        ("/null", json!(null)),
        (
            "/zero",
            json!({"context":"$inputs.document","selector":"$.absent","type":"jsonpath"}),
        ),
        ("/last", json!("new")),
    ]
    .into_iter()
    .map(|(target, value)| Replacement {
        target: target.into(),
        value: source(value),
        ..Replacement::default()
    })
    .collect::<Vec<_>>();
    let (body, warnings) = apply_replacements(
        json!({"first":"old","null":"old","zero":"old","last":"old"}),
        "application/json",
        &replacements,
        &eval,
        &admission,
    )
    .unwrap_or_else(|error| panic!("admitted values: {error}"));
    assert_eq!(
        body,
        json!({"first":"new","null":null,"zero":null,"last":"new"})
    );
    assert_eq!(
        warnings,
        vec!["requestBody.replacements[2]: $.absent: selector matched no values"]
    );
}
