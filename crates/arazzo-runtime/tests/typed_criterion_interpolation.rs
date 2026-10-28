//! Production Criterion routing for §5.8.11.3 brace-only interpolation and
//! §5.8.11.4 null-context failure. XPath controls explicitly select 1.0.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{
    CriterionExpressionType, CriterionType, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::*;
use serde_json::{json, Value};

const BODY: &str = r#"{"text":"ready","empty":"","nil":null,"items":[{"id":7,"state":"ready","flag":true,"nil":null,"integer":9007199254740993,"objectText":"{\"id\":7}","arrayText":"[1,\"two\",null]","literal":"$inputs.text {name} 7"}],"xml":"<root><item id='7'>ready</item></root>"}"#;

fn typed(kind: &str, context: &str, condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        condition: condition.to_owned(),
        context: context.to_owned(),
        type_: Some(if kind == "xpath" {
            versioned(kind, "xpath-10")
        } else {
            CriterionType::Name(kind.to_owned())
        }),
        ..SuccessCriterion::default()
    }
}

fn versioned(kind: &str, version: &str) -> CriterionType {
    CriterionType::ExpressionType(CriterionExpressionType {
        type_: kind.to_owned(),
        version: version.to_owned(),
        ..CriterionExpressionType::default()
    })
}

fn inputs() -> BTreeMap<String, Value> {
    [
        ("text", json!("ready")),
        ("text-id", json!("ready7")),
        ("id", json!(7)),
        ("boolean", json!(true)),
        ("nil", Value::Null),
        ("integer", json!(9007199254740993_u64)),
        ("object", json!({"id":7})),
        ("array", json!([1, "two", null])),
        ("regex", json!("^ready$")),
        ("jsonpath", json!("$[?@.id == 7]")),
        ("xpath", json!("/root/item[@id = '7']")),
        ("literal", json!("$inputs.text {name} 7")),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

async fn execute(
    criteria: Vec<SuccessCriterion>,
    inputs: BTreeMap<String, Value>,
) -> ExecutionResult {
    let server = start_server(|_, _, _, _| MockHttpResponse::json(200, BODY));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_owned(),
            steps: vec![Step {
                step_id: "check".to_owned(),
                target: Some(StepTarget::OperationPath(
                    "{$sourceDescriptions.test.url}#/paths/~1data/get".to_owned(),
                )),
                success_criteria: criteria,
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = fixture_engine_builder(spec)
        .trace(true)
        .build()
        .expect("hermetic engine");
    engine.execute_collect("wf", inputs).await
}

fn assert_passed(result: &ExecutionResult, criteria: &[SuccessCriterion]) {
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let traces = result.trace_steps();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].criteria.len(), criteria.len());
    for (record, declared) in traces[0].criteria.iter().zip(criteria) {
        assert!(record.result, "{record:?}");
        assert_eq!(record.condition, declared.condition);
        assert!(record.warnings.is_empty(), "{record:?}");
    }
}

fn assert_failed(result: &ExecutionResult, declared: &SuccessCriterion) {
    assert_eq!(
        result.outputs.as_ref().unwrap_err().kind,
        RuntimeErrorKind::SuccessCriteriaFailed
    );
    let traces = result.trace_steps();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].criteria.len(), 1);
    assert!(!traces[0].criteria[0].result);
    assert_eq!(traces[0].criteria[0].condition, declared.condition);
}

#[tokio::test]
async fn typed_conditions_interpolate_scalars_multiple_spans_and_entire_templates() {
    let criteria = vec![
        typed("regex", "$response.body#/text", "^{$inputs.text}$"),
        typed("regex", "$inputs.text-id", "^{$inputs.text}{$inputs.id}$"),
        typed(
            "regex",
            "$inputs.literal",
            r"^\$inputs\.text \{name\} {$inputs.id}$",
        ),
        typed("regex", "$response.body#/text", "{$inputs.regex}"),
        typed(
            "jsonpath",
            "$response.body#/items",
            "$[?@.id == {$inputs.id} && @.state == '{$inputs.text}']",
        ),
        typed("jsonpath", "$response.body#/items", "{$inputs.jsonpath}"),
        typed(
            "xpath",
            "$response.body#/xml",
            "/root/item[@id = '{$inputs.id}' and text() = '{$inputs.text}']",
        ),
        typed("xpath", "$response.body#/xml", "{$inputs.xpath}"),
    ];
    let result = execute(criteria.clone(), inputs()).await;
    assert_passed(&result, &criteria);
}

#[tokio::test]
async fn typed_conditions_use_frozen_json_string_conversions() {
    let mut criteria = Vec::new();
    for (name, rendered) in [
        ("boolean", "true"),
        ("nil", "null"),
        ("integer", "9007199254740993"),
        ("object", r#"{"id":7}"#),
        ("array", r#"[1,"two",null]"#),
    ] {
        let expression = format!("{{$inputs.{name}}}");
        // Escape the leading JSON punctuation for regex, preserving every
        // byte emitted by the canonical conversion inside the expression.
        let escape = if matches!(name, "object" | "array") {
            "\\"
        } else {
            ""
        };
        criteria.push(typed(
            "regex",
            &format!("$inputs.rendered-{name}"),
            &format!("^{escape}{expression}$"),
        ));
        criteria.push(typed(
            "xpath",
            "$response.body#/xml",
            &format!("'{expression}' = '{rendered}'"),
        ));
    }
    for (field, name) in [("flag", "boolean"), ("nil", "nil"), ("integer", "integer")] {
        criteria.push(typed(
            "jsonpath",
            "$response.body#/items",
            &format!("$[?@.{field} == {{$inputs.{name}}}]"),
        ));
    }
    for (field, name) in [("objectText", "object"), ("arrayText", "array")] {
        criteria.push(typed(
            "jsonpath",
            "$response.body#/items",
            &format!("$[?@.{field} == '{{$inputs.{name}}}']"),
        ));
    }
    let mut values = inputs();
    for name in ["boolean", "nil", "integer", "object", "array"] {
        values.insert(format!("rendered-{name}"), json!(values[name].to_string()));
    }
    let result = execute(criteria.clone(), values).await;
    assert_passed(&result, &criteria);
}

#[tokio::test]
async fn native_dollars_literal_braces_and_omitted_context_remain_compatible() {
    let criteria = vec![
        typed("regex", "$inputs.literal", r"^\$inputs\.text \{name\} 7$"),
        typed("regex", "$statusCode", r"^2\d{2}$"),
        typed("regex", "", "ready"),
        typed(
            "jsonpath",
            "$response.body#/items",
            "$[?@.literal == '$inputs.text {name} {$inputs.id}']",
        ),
        typed(
            "jsonpath",
            "$response.body#/items",
            "$[?@.literal == '$inputs.text {name} 7']",
        ),
        typed(
            "xpath",
            "$response.body#/xml",
            "'$inputs.text {name} {$inputs.id}' = '$inputs.text {name} 7'",
        ),
        typed(
            "xpath",
            "$response.body#/xml",
            "'$inputs.text {name}' = '$inputs.text {name}'",
        ),
    ];
    let result = execute(criteria.clone(), inputs()).await;
    assert_passed(&result, &criteria);
    let unchanged = typed("regex", "$response.body#/text", "$inputs.regex");
    let result = execute(vec![unchanged.clone()], inputs()).await;
    assert_failed(&result, &unchanged);
    assert!(result.trace_steps()[0].criteria[0].warnings.is_empty());
}

#[tokio::test]
async fn typed_conditions_fail_on_null_and_undefined_context() {
    for (kind, condition) in [("regex", "^$"), ("jsonpath", "$"), ("xpath", "true()")] {
        for context in [
            "$response.body#/nil",
            "$response.body#/missing",
            "$inputs.absent",
        ] {
            let criterion = typed(kind, context, condition);
            let result = execute(vec![criterion.clone()], inputs()).await;
            assert_failed(&result, &criterion);
            if context == "$inputs.absent" {
                assert_eq!(result.trace_steps()[0].criteria[0].warnings.len(), 1);
            }
        }
    }
    let empty = typed("regex", "$response.body#/empty", "^$");
    let result = execute(vec![empty.clone()], inputs()).await;
    assert_passed(&result, &[empty]);
}

#[tokio::test]
async fn expanded_conditions_that_do_not_match_fail_without_interpolation_warnings() {
    for criterion in [
        typed("regex", "$response.body#/text", "^{$inputs.id}$"),
        typed(
            "jsonpath",
            "$response.body#/items",
            "$[?@.state == '{$inputs.id}']",
        ),
        typed(
            "xpath",
            "$response.body#/xml",
            "/root/item[@id = '{$inputs.text}']",
        ),
    ] {
        let result = execute(vec![criterion.clone()], inputs()).await;
        assert_failed(&result, &criterion);
        assert!(result.trace_steps()[0].criteria[0].warnings.is_empty());
    }
}

#[tokio::test]
async fn unresolved_substitutions_never_dispatch_partial_conditions() {
    for (kind, condition) in [
        ("regex", "ready|{$inputs.absent}|{$outputs.other}"),
        (
            "jsonpath",
            "$[?@.nil == {$inputs.absent} || @.id == {$inputs.id} || @.nil == {$outputs.other}]",
        ),
        ("xpath", "true() or '{$inputs.absent}' = '{$outputs.other}'"),
    ] {
        let context = match kind {
            "regex" => "$response.body#/text",
            "jsonpath" => "$response.body#/items",
            _ => "$response.body#/xml",
        };
        let criterion = typed(kind, context, condition);
        let result = execute(vec![criterion.clone()], inputs()).await;
        assert_failed(&result, &criterion);
        let traces = result.trace_steps();
        let warnings = &traces[0].criteria[0].warnings;
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("input \"absent\" not found"));
        assert!(warnings[1].contains("output \"other\" not found"));
        assert!(!warnings
            .iter()
            .any(|warning| warning.contains("invalid JSONPath")));
    }
}

#[tokio::test]
async fn malformed_templates_fail_direct_engine_calls_before_partial_resolution() {
    for kind in ["regex", "jsonpath", "xpath"] {
        for condition in ["{$inputs.absent} then {$", "{$inputs.absent} then {$env.X}"] {
            let criterion = typed(kind, "$response.body", condition);
            let result = execute(vec![criterion.clone()], inputs()).await;
            assert_failed(&result, &criterion);
            let traces = result.trace_steps();
            let warnings = &traces[0].criteria[0].warnings;
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(
                !warnings[0].contains("not found"),
                "no partial lookup: {warnings:?}"
            );
            assert!(
                !warnings[0].contains("invalid JSONPath"),
                "no typed dispatch: {warnings:?}"
            );
        }
    }
}

#[tokio::test]
async fn expanded_jsonpath_admission_errors_survive_null_context() {
    for (query, version, needle) in [
        ("$[?(".to_owned(), None, "invalid JSONPath syntax"),
        (
            format!("$[?{}@.a{}]", "@[?".repeat(8), "]".repeat(8)),
            None,
            "bracket nesting depth limit",
        ),
        (
            "$.items".to_owned(),
            Some("draft-goessner-dispatch-jsonpath-00"),
            "unsupported JSONPath version",
        ),
    ] {
        let mut criterion = typed("jsonpath", "$response.body#/nil", "{$inputs.query}");
        if let Some(version) = version {
            criterion.type_ = Some(versioned("jsonpath", version));
        }
        let mut values = inputs();
        values.insert("query".to_owned(), json!(query));
        let result = execute(vec![criterion.clone()], values).await;
        assert_failed(&result, &criterion);
        let traces = result.trace_steps();
        let warnings = &traces[0].criteria[0].warnings;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("{$inputs.query}: "));
        assert!(warnings[0].contains(needle), "{warnings:?}");
    }
}

#[tokio::test]
async fn unsupported_xpath_gjson_and_simple_condition_owners_remain_unchanged() {
    for version in [None, Some("xpath-20"), Some("xpath-30"), Some("xpath-31")] {
        let mut criterion = typed("xpath", "$response.body#/xml", "{$inputs.xpath}");
        criterion.type_ = Some(version.map_or_else(
            || CriterionType::Name("xpath".to_owned()),
            |version| versioned("xpath", version),
        ));
        let result = execute(vec![criterion.clone()], inputs()).await;
        assert_failed(&result, &criterion);
    }
    let gjson = typed("jsonpath", "$response.body", "$.items.#");
    let result = execute(vec![gjson.clone()], inputs()).await;
    assert_failed(&result, &gjson);
    assert!(result.trace_steps()[0].criteria[0].warnings[0].contains("invalid JSONPath syntax"));
    let simple = typed(
        "simple",
        "",
        "$statusCode == 200 && '$inputs.text' == '$inputs.text'",
    );
    let result = execute(vec![simple.clone()], inputs()).await;
    assert_passed(&result, &[simple]);
    let no_interpolation = typed("simple", "", "'{$inputs.text}' == 'ready'");
    let result = execute(vec![no_interpolation.clone()], inputs()).await;
    assert_failed(&result, &no_interpolation);
}
