#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_expr::{
    body_pointer_migration_hint, parse_runtime_expression, EvalContext, ExpressionEvaluator,
    SourceDescriptionContext, WorkflowEvalState,
};
use serde_json::{json, Value};

fn evaluator() -> ExpressionEvaluator {
    let body = json!({"items":[{"id":7}],"a.b":1,"a":{"b":2},"a/b":{"~key":3}});
    ExpressionEvaluator::new(EvalContext {
        inputs: BTreeMap::from([
            ("dot.name".to_owned(), json!(9)),
            ("body".to_owned(), body.clone()),
            ("number".to_owned(), json!(7)),
        ]),
        outputs: BTreeMap::from([("dot.name".to_owned(), json!(10))]),
        steps: Arc::new(BTreeMap::from([(
            "s".to_owned(),
            BTreeMap::from([("dot.name".to_owned(), json!(11))]),
        )])),
        workflows: BTreeMap::from([(
            "wf".to_owned(),
            WorkflowEvalState {
                inputs: BTreeMap::from([("dot.name".to_owned(), json!(12))]),
                outputs: BTreeMap::from([("dot.name".to_owned(), json!(13))]),
            },
        )]),
        status_code: Some(201),
        method: Some("POST".to_owned()),
        url: Some("https://example.test/".to_owned()),
        self_uri: Some("workflows/test.yaml".to_owned()),
        source_descriptions: BTreeMap::from([(
            "api".to_owned(),
            SourceDescriptionContext {
                url: "openapi.yaml".to_owned(),
                type_: "openapi".to_owned(),
            },
        )]),
        request_headers: BTreeMap::from([("X-Id".to_owned(), "request".to_owned())]),
        response_headers: BTreeMap::from([("X-Id".to_owned(), "response".to_owned())]),
        message_headers: BTreeMap::from([("X-Id".to_owned(), "message".to_owned())]),
        request_query: BTreeMap::from([("q".to_owned(), "query".to_owned())]),
        request_path: BTreeMap::from([("id".to_owned(), "path".to_owned())]),
        request_body: Some(body.clone()),
        response_body: Some(body.clone()),
        message_payload: Some(body),
    })
}

#[test]
fn supported_namespaces_exact_names_pointers_and_caseless_keywords() {
    let evaluator = evaluator();
    for (expression, expected) in [
        ("$url", json!("https://example.test/")),
        ("$METHOD", json!("POST")),
        ("$statusCode", json!(201)),
        ("$SELF", json!("workflows/test.yaml")),
        ("$inputs.dot.name", json!(9)),
        ("$outputs.dot.name", json!(10)),
        ("$steps.s.outputs.dot.name", json!(11)),
        ("$workflows.wf.inputs.dot.name", json!(12)),
        ("$workflows.wf.outputs.dot.name", json!(13)),
        ("$sourceDescriptions.api.url", json!("openapi.yaml")),
        ("$sourceDescriptions.api.type", json!("openapi")),
        ("$request.HEADER.x-id", json!("request")),
        ("$response.header.x-id", json!("response")),
        ("$message.header.x-id", json!("message")),
        ("$request.query.q", json!("query")),
        ("$request.path.id", json!("path")),
        ("$inputs.body#/items/0/id", json!(7)),
    ] {
        assert_eq!(
            evaluator.evaluate_with_diagnostics(expression),
            (expected, Vec::new()),
            "{expression}"
        );
    }
    for base in ["$request.body", "$response.body", "$message.payload"] {
        assert!(evaluator.evaluate(base).is_object());
        assert_eq!(
            evaluator.evaluate(&format!("{base}#")),
            evaluator.evaluate(base)
        );
        for (pointer, expected) in [
            ("/a.b", json!(1)),
            ("/a/b", json!(2)),
            ("/a~1b/~0key", json!(3)),
        ] {
            assert_eq!(
                evaluator.evaluate_with_diagnostics(&format!("{base}#{pointer}")),
                (expected, Vec::new())
            );
        }
    }
    // Canonical syntax with no current context data remains absent.
    for expression in [
        "$response.query.q",
        "$response.path.id",
        "$components.parameters.x",
        "$components.successActions.x",
        "$components.failureActions.x",
    ] {
        assert_eq!(evaluator.evaluate(expression), Value::Null);
    }
}

#[test]
fn invalid_standalone_calls_project_one_structured_error_and_only_safe_hints() {
    let evaluator = evaluator();
    for expression in [
        "",
        "hello",
        "$env.X",
        "$USD",
        "$statusCode.extra",
        "$inputs.",
        "$inputs.body#/a~2b",
        "$response.body.items",
        "$request.body.items[0]",
        "$message.payload.items",
        "$response.body.items[*]",
        "$response.body.items.#",
        "$response.body.items[?(@.x==1)]",
    ] {
        let error = parse_runtime_expression(expression).unwrap_err();
        let (value, warnings) = evaluator.evaluate_with_diagnostics(expression);
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1, "{expression}");
        let expected = match body_pointer_migration_hint(expression, &error) {
            Some(hint) => format!("{error}; {hint}"),
            None => error.to_string(),
        };
        assert_eq!(warnings[0].expression, expression);
        assert_eq!(warnings[0].message, expected);
        if expression.contains('[') || expression.contains(".#") {
            assert!(
                !warnings[0].message.contains("use '$"),
                "ambiguous traversal gets no inferred pointer"
            );
        }
    }
}

#[test]
fn missing_values_keep_existing_warning_identity_and_order() {
    let evaluator = evaluator();
    for (expression, message) in [
        ("$inputs.missing", "input \"missing\" not found in context"),
        (
            "$outputs.missing",
            "output \"missing\" not found in context",
        ),
        (
            "$steps.absent.outputs.x",
            "step \"absent\" not found in context",
        ),
        (
            "$inputs.body#/absent",
            "JSON Pointer \"/absent\" did not resolve in value \"body\"",
        ),
    ] {
        let (value, warnings) = evaluator.evaluate_with_diagnostics(expression);
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].message, message);
    }
    assert_eq!(
        evaluator.evaluate_with_diagnostics("$request.body#/absent"),
        (Value::Null, Vec::new())
    );
}

#[test]
fn literal_renderer_and_conformant_legacy_interpolation_survive_cutover() {
    let evaluator = evaluator();
    for literal in ["$USD", "$env.X", "$response.body.items", "ordinary text"] {
        assert_eq!(
            evaluator.resolve_value_with_diagnostics(literal),
            (json!(literal), Vec::new())
        );
    }
    assert_eq!(evaluator.resolve_value("$inputs.number"), json!(7));
    assert_eq!(evaluator.resolve_value("{$inputs.number}"), json!("7"));
    assert_eq!(
        evaluator.resolve_value("id={$response.body#/items/0/id}"),
        json!("id=7")
    );
    let (_, warnings) =
        evaluator.resolve_value_with_diagnostics("{$inputs.first}/{$inputs.second}");
    assert_eq!(
        warnings
            .iter()
            .map(|warning| warning.expression.as_str())
            .collect::<Vec<_>>(),
        ["$inputs.first", "$inputs.second"]
    );
    assert_eq!(
        evaluator.interpolate_string("id={$response.body#/items/0/id}"),
        "id=7"
    );
    assert_eq!(
        evaluator.interpolate_string("id={$response.body.items[0].id}"),
        "id="
    );
    assert_eq!(evaluator.interpolate_string("n=$inputs.number"), "n=7");
    assert!(evaluator.evaluate_condition("$response.body.items[0].id == 7"));
}

#[test]
fn canonical_routes_have_one_parse_boundary() {
    let facade = include_str!("../src/lib.rs");
    let standalone = facade
        .split("pub fn evaluate_with_diagnostics")
        .nth(1)
        .unwrap()
        .split("/// Evaluate an expression and convert")
        .next()
        .unwrap();
    assert_eq!(standalone.matches("parse_runtime_expression(").count(), 1);
    assert_eq!(standalone.matches("resolution::resolve_parsed(").count(), 1);
    let resolver = include_str!("../src/resolution.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(!resolver.contains("parse_runtime_expression("));
    let condition = include_str!("../src/simple_condition.rs");
    assert_eq!(
        condition
            .matches("match parse_simple_condition(condition)")
            .count(),
        1
    );
    let execution = include_str!("../src/simple_condition/evaluation.rs");
    assert!(!execution.contains("parse_runtime_expression("));
    assert_eq!(execution.matches("resolve_parsed(").count(), 1);
}
