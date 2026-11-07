use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_expr::{
    EvalContext, ExpressionEvaluator, ExpressionResolution, ExpressionWarning,
    RuntimeExpressionNamespace as Namespace, StoredValueReference as Reference, ValueResolution,
    WorkflowEvalState,
};
use serde_json::{json, Value};

fn evaluator() -> ExpressionEvaluator {
    let values = BTreeMap::from([
        ("nil".to_owned(), Value::Null),
        ("empty".to_owned(), json!("")),
        ("alias".to_owned(), Value::Null),
        (
            "data.items".to_owned(),
            json!({"a/b": {"~key": null}, "é": "🙂"}),
        ),
        ("array".to_owned(), json!([1, null, "é"])),
        ("object".to_owned(), json!({"b": true, "a": 7})),
        ("number".to_owned(), json!(12.5)),
        ("boolean".to_owned(), json!(true)),
    ]);
    ExpressionEvaluator::new(EvalContext {
        inputs: values.clone().into(),
        outputs: values.clone(),
        steps: Arc::new(BTreeMap::from([("Step".to_owned(), values.clone())])),
        workflows: BTreeMap::from([(
            "Work".to_owned(),
            WorkflowEvalState {
                inputs: values.clone(),
                outputs: values,
            },
        )])
        .into(),
        request_headers: BTreeMap::from([("X-Empty".to_owned(), String::new())]),
        response_headers: BTreeMap::from([("X-Empty".to_owned(), String::new())]),
        request_body: Some(json!({"nil": null, "empty": ""})),
        response_body: Some(json!({"nil": null, "empty": ""})),
        status_code: Some(200),
        ..EvalContext::default()
    })
}

fn assert_value_projection(evaluator: &ExpressionEvaluator, source: &str) -> ValueResolution {
    let result = evaluator.resolve_value_with_provenance(source);
    assert_eq!(
        (result.value.clone(), result.warnings.clone()),
        evaluator.resolve_value_with_diagnostics(source),
        "{source}"
    );
    assert_eq!(result.value, evaluator.resolve_value(source), "{source}");
    result
}

fn assert_expression_projection(evaluator: &ExpressionEvaluator, source: &str) -> ValueResolution {
    let result = evaluator.evaluate_with_provenance(source);
    assert_eq!(
        (result.value.clone(), result.warnings.clone()),
        evaluator.evaluate_with_diagnostics(source),
        "{source}"
    );
    assert_eq!(result.value, evaluator.evaluate(source), "{source}");
    assert_eq!(
        result,
        assert_value_projection(evaluator, source),
        "{source}"
    );
    result
}

#[test]
fn stored_values_retain_missing_null_empty_and_all_coordinates() {
    let evaluator = evaluator();
    for (prefix, namespace) in [
        ("$inputs", Namespace::Inputs),
        ("$outputs", Namespace::Outputs),
        ("$steps.Step.outputs", Namespace::Steps),
        ("$workflows.Work.inputs", Namespace::Workflows),
        ("$workflows.Work.outputs", Namespace::Workflows),
    ] {
        for (name, expected, missing, warning_count) in [
            ("absent", Value::Null, true, 1),
            ("nil", Value::Null, false, 0),
            ("empty", json!(""), false, 0),
            ("data.items#/a~1b/~0key", Value::Null, false, 0),
            ("data.items#/absent", Value::Null, true, 1),
        ] {
            let source = format!("{prefix}.{name}");
            let result = assert_expression_projection(&evaluator, &source);
            assert_eq!(result.value, expected, "{source}");
            assert_eq!(result.warnings.len(), warning_count, "{source}");
            assert!(!result.syntax_error, "{source}");
            assert_eq!(result.expressions.len(), 1, "{source}");
            let occurrence = &result.expressions[0];
            assert_eq!(occurrence.expression, source);
            assert_eq!(occurrence.namespace, namespace);
            assert_eq!(occurrence.missing, missing, "{source}");
            let (name, pointer) = name.split_once('#').map_or((name, None), |(name, suffix)| {
                (name, Some(format!("#{suffix}")))
            });
            let reference = match prefix {
                "$inputs" => Reference::Input {
                    name: name.to_owned(),
                    pointer,
                },
                "$outputs" => Reference::Output {
                    name: name.to_owned(),
                    pointer,
                },
                "$steps.Step.outputs" => Reference::StepOutput {
                    step_id: "Step".to_owned(),
                    name: name.to_owned(),
                    pointer,
                },
                "$workflows.Work.inputs" => Reference::WorkflowInput {
                    workflow_id: "Work".to_owned(),
                    name: name.to_owned(),
                    pointer,
                },
                _ => Reference::WorkflowOutput {
                    workflow_id: "Work".to_owned(),
                    name: name.to_owned(),
                    pointer,
                },
            };
            assert_eq!(occurrence.reference, Some(reference), "{source}");
            assert_eq!(
                evaluator.evaluate_resolved(&source),
                if missing { None } else { Some(expected) },
                "{source}"
            );
        }
    }
}

#[test]
fn request_and_response_misses_are_retained_without_warnings() {
    for context_present in [false, true] {
        let evaluator = if context_present {
            evaluator()
        } else {
            ExpressionEvaluator::default()
        };
        for (prefix, namespace) in [
            ("$request", Namespace::Request),
            ("$response", Namespace::Response),
        ] {
            for (suffix, present_value) in [
                ("header.X-Missing", None),
                ("header.x-empty", Some(json!(""))),
                ("body#/absent", None),
                ("body#/nil", Some(Value::Null)),
                ("body#/empty", Some(json!(""))),
            ] {
                let source = format!("{prefix}.{suffix}");
                let expected = if context_present { present_value } else { None };
                let result = assert_expression_projection(&evaluator, &source);
                assert_eq!(
                    result.value,
                    expected.clone().unwrap_or(Value::Null),
                    "{source}"
                );
                assert!(result.warnings.is_empty(), "{source}");
                assert!(!result.syntax_error);
                assert_eq!(
                    result.expressions,
                    vec![ExpressionResolution {
                        expression: source.clone(),
                        namespace,
                        reference: None,
                        missing: expected.is_none(),
                    }]
                );
                assert_eq!(evaluator.evaluate_resolved(&source), expected);
            }
        }
    }
}

#[test]
fn templates_retain_every_occurrence_and_warning_in_source_order() {
    let evaluator = evaluator();
    let source = "前{literal}🙂{$inputs.absent}|{$inputs.alias}|{$statusCode}|{$response.body#/absent}|{$outputs.absent}|{$inputs.absent}|{$inputs.alias}終";
    let result = assert_value_projection(&evaluator, source);
    assert_eq!(
        result.value,
        json!("前{literal}🙂null|null|200|null|null|null|null終")
    );
    assert!(!result.syntax_error);
    let expected = [
        ("$inputs.absent", Namespace::Inputs, true),
        ("$inputs.alias", Namespace::Inputs, false),
        ("$statusCode", Namespace::StatusCode, false),
        ("$response.body#/absent", Namespace::Response, true),
        ("$outputs.absent", Namespace::Outputs, true),
        ("$inputs.absent", Namespace::Inputs, true),
        ("$inputs.alias", Namespace::Inputs, false),
    ];
    assert_eq!(result.expressions.len(), expected.len());
    for (actual, (expression, namespace, missing)) in result.expressions.iter().zip(expected) {
        assert_eq!(actual.expression, expression);
        assert_eq!(actual.namespace, namespace);
        assert_eq!(actual.missing, missing);
        assert_eq!(
            actual,
            &evaluator.evaluate_with_provenance(expression).expressions[0]
        );
    }
    assert_eq!(
        result.warnings,
        vec![
            ExpressionWarning {
                expression: "$inputs.absent".to_owned(),
                message: "input \"absent\" not found in context".to_owned()
            },
            ExpressionWarning {
                expression: "$outputs.absent".to_owned(),
                message: "output \"absent\" not found in context".to_owned()
            },
            ExpressionWarning {
                expression: "$inputs.absent".to_owned(),
                message: "input \"absent\" not found in context".to_owned()
            },
        ]
    );
}

#[test]
fn exact_json_types_and_single_fragment_template_conversions_are_unchanged() {
    let evaluator = evaluator();
    for (name, expected, rendered) in [
        ("nil", Value::Null, "null"),
        ("empty", json!(""), ""),
        ("array", json!([1, null, "é"]), "[1,null,\"é\"]"),
        ("object", json!({"a": 7, "b": true}), "{\"a\":7,\"b\":true}"),
        ("number", json!(12.5), "12.5"),
        ("boolean", json!(true), "true"),
    ] {
        let expression = format!("$inputs.{name}");
        let exact = assert_expression_projection(&evaluator, &expression);
        let template = assert_value_projection(&evaluator, &format!("{{{expression}}}"));
        assert_eq!(exact.value, expected);
        assert_eq!(template.value, json!(rendered));
        assert_eq!(template.expressions, exact.expressions);
        assert!(!template.syntax_error);
    }
}

#[test]
fn identities_keep_dotted_names_case_unicode_and_pointer_bytes() {
    let evaluator = evaluator();
    for (source, reference, expected) in [
        (
            "$INPUTS.data.items#",
            Reference::Input {
                name: "data.items".to_owned(),
                pointer: Some("#".to_owned()),
            },
            json!({"a/b": {"~key": null}, "é": "🙂"}),
        ),
        (
            "$OUTPUTS.data.items#/é",
            Reference::Output {
                name: "data.items".to_owned(),
                pointer: Some("#/é".to_owned()),
            },
            json!("🙂"),
        ),
        (
            "$STEPS.Step.OUTPUTS.data.items#/a~1b/~0key",
            Reference::StepOutput {
                step_id: "Step".to_owned(),
                name: "data.items".to_owned(),
                pointer: Some("#/a~1b/~0key".to_owned()),
            },
            Value::Null,
        ),
        (
            "$WORKFLOWS.Work.INPUTS.data.items#/a~1b/~0key",
            Reference::WorkflowInput {
                workflow_id: "Work".to_owned(),
                name: "data.items".to_owned(),
                pointer: Some("#/a~1b/~0key".to_owned()),
            },
            Value::Null,
        ),
        (
            "$WORKFLOWS.Work.OUTPUTS.data.items#/a~1b/~0key",
            Reference::WorkflowOutput {
                workflow_id: "Work".to_owned(),
                name: "data.items".to_owned(),
                pointer: Some("#/a~1b/~0key".to_owned()),
            },
            Value::Null,
        ),
    ] {
        let result = assert_expression_projection(&evaluator, source);
        assert_eq!(result.value, expected);
        assert!(!result.expressions[0].missing);
        assert_eq!(result.expressions[0].reference, Some(reference));
    }
}

#[test]
fn invalid_templates_are_atomic_and_literals_have_no_provenance() {
    let evaluator = evaluator();
    for source in [
        "{$inputs.absent} then {$",
        "{$inputs.alias} then {$env.X}",
        "{$response.body.status}",
        "{$}",
    ] {
        let result = assert_value_projection(&evaluator, source);
        assert_eq!(result.value, json!(source));
        assert!(result.syntax_error, "{source}");
        assert!(result.expressions.is_empty(), "{source}");
        assert_eq!(result.warnings.len(), 1, "{source}");
        assert_eq!(result.warnings[0].expression, source);
    }
    for source in [
        "",
        "$",
        "$USD",
        "$env.X",
        "$response.body.status",
        "pay $inputs.nil",
        "前🙂 {name} $USD 後",
        "a{b",
        "a}b",
        "{}",
        "{ $inputs.nil}",
    ] {
        let result = assert_value_projection(&evaluator, source);
        assert_eq!(result.value, json!(source));
        assert!(!result.syntax_error, "{source}");
        assert!(result.expressions.is_empty());
        assert!(result.warnings.is_empty());
        let standalone = evaluator.evaluate_with_provenance(source);
        assert!(standalone.syntax_error, "{source}");
        assert_eq!(standalone.value, Value::Null);
        assert!(standalone.expressions.is_empty());
        assert_eq!(standalone.warnings.len(), 1);
        assert_eq!(
            (standalone.value, standalone.warnings),
            evaluator.evaluate_with_diagnostics(source)
        );
        assert_eq!(evaluator.evaluate_resolved(source), None);
    }
    assert!(evaluator
        .evaluate_with_provenance("$response.body.status")
        .warnings[0]
        .message
        .contains("use '$response.body#/status'"));
}

#[test]
fn nonstored_namespaces_keep_canonical_identity_even_when_missing() {
    let evaluator = evaluator();
    for (source, namespace, missing) in [
        ("$URL", Namespace::Url, true),
        ("$method", Namespace::Method, true),
        ("$statusCode", Namespace::StatusCode, false),
        ("$self", Namespace::SelfUri, true),
        ("$message.payload", Namespace::Message, true),
        (
            "$sourceDescriptions.api.url",
            Namespace::SourceDescriptions,
            true,
        ),
        ("$components.parameters.param", Namespace::Components, true),
        ("$response.query.q", Namespace::Response, true),
        ("$request.path.id", Namespace::Request, true),
    ] {
        let result = assert_expression_projection(&evaluator, source);
        assert!(!result.syntax_error);
        assert_eq!(
            result.expressions,
            vec![ExpressionResolution {
                expression: source.to_owned(),
                namespace,
                reference: None,
                missing,
            }]
        );
    }
}
