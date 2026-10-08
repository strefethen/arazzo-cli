//! Literal-capable value rendering from canonical expression-string syntax.

use serde_json::Value;

use crate::{
    classify_value_string, resolution::resolve_parsed, EvalContext, ExpressionWarning,
    ValueStringSyntax,
};

pub(crate) fn resolve_value(value: &str, context: &EvalContext) -> (Value, Vec<ExpressionWarning>) {
    match classify_value_string(value) {
        Ok(ValueStringSyntax::Expression(expression)) => {
            resolve_parsed(&expression, context).into_public()
        }
        Ok(ValueStringSyntax::Template(expressions)) => {
            let mut rendered = String::with_capacity(value.len());
            let mut warnings = Vec::new();
            let mut cursor = 0;
            for embedded in expressions {
                let range = embedded.full_range();
                rendered.push_str(&value[cursor..range.start]);
                let (resolved, mut expression_warnings) =
                    resolve_parsed(embedded.expression(), context).into_public();
                match resolved {
                    Value::String(text) => rendered.push_str(&text),
                    other => rendered.push_str(&other.to_string()),
                }
                warnings.append(&mut expression_warnings);
                cursor = range.end;
            }
            rendered.push_str(&value[cursor..]);
            (Value::String(rendered), warnings)
        }
        Ok(ValueStringSyntax::Literal(literal)) => (Value::String(literal.to_owned()), Vec::new()),
        Err(error) => (
            Value::String(value.to_owned()),
            vec![ExpressionWarning {
                expression: value.to_owned(),
                message: error.to_string(),
            }],
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{ExpressionEvaluator, ExpressionStringErrorKind};

    fn evaluator() -> ExpressionEvaluator {
        let mut context = EvalContext {
            response_body: Some(json!({"items": [{"id": 7}], "status": "ready"})),
            ..EvalContext::default()
        };
        for (name, value) in [
            ("text", json!("é🙂 \"hello\"\n")),
            ("number", json!(12.5)),
            ("integer", json!(9007199254740993_u64)),
            ("boolean", json!(true)),
            ("null", Value::Null),
            ("array", json!([1, "two", null, {"ok": true}])),
            ("object", json!({"nested": [false, "é"], "id": 7})),
            ("pet_id", json!(7)),
        ] {
            context.inputs.insert(name.to_owned(), value);
        }
        context
            .request_headers
            .insert("X-Custom".to_owned(), "header".to_owned());
        ExpressionEvaluator::new(context)
    }

    #[test]
    fn exact_expressions_preserve_every_json_type_through_parsed_resolution() {
        let evaluator = evaluator();
        for (name, expected) in &evaluator.context().inputs {
            let input = format!("$inputs.{name}");
            assert_eq!(
                evaluator.resolve_value_with_diagnostics(&input),
                (expected.clone(), Vec::new()),
                "{input}"
            );
            assert_eq!(evaluator.resolve_value(&input), *expected, "{input}");
        }
        assert_eq!(
            evaluator.resolve_value_with_diagnostics("$REQUEST.HEADER.x-custom"),
            (json!("header"), Vec::new())
        );
        assert_eq!(
            evaluator.resolve_value_with_diagnostics("$inputs.absent"),
            (
                Value::Null,
                vec![ExpressionWarning {
                    expression: "$inputs.absent".to_owned(),
                    message: "input \"absent\" not found in context".to_owned(),
                }]
            )
        );
    }

    #[test]
    fn dollar_text_and_compatible_literal_braces_preserve_all_bytes() {
        let evaluator = evaluator();
        for input in [
            "",
            "literal text",
            "$",
            "$USD",
            "$env.X",
            "pay $inputs.pet_id",
            "$response.body.status",
            "$response.body.items[0].id",
            "{name}",
            "a{b",
            "a}b",
            "{}",
            "{ $inputs.pet_id}",
            "前🙂 {name} $USD 後",
        ] {
            assert_eq!(
                evaluator.resolve_value_with_diagnostics(input),
                (Value::String(input.to_owned()), Vec::new()),
                "{input}"
            );
        }
    }

    #[test]
    fn one_span_templates_always_return_strings_with_spec_conversions() {
        let evaluator = evaluator();
        for (name, expected) in [
            ("text", "é🙂 \"hello\"\n"),
            ("number", "12.5"),
            ("integer", "9007199254740993"),
            ("boolean", "true"),
            ("null", "null"),
            ("array", r#"[1,"two",null,{"ok":true}]"#),
            ("object", r#"{"id":7,"nested":[false,"é"]}"#),
        ] {
            let input = format!("{{$inputs.{name}}}");
            assert_eq!(
                evaluator.resolve_value_with_diagnostics(&input),
                (Value::String(expected.to_owned()), Vec::new()),
                "{input}"
            );
        }
    }

    #[test]
    fn templates_preserve_literal_braces_unicode_and_unbraced_dollars() {
        let evaluator = evaluator();
        for (input, expected) in [
            ("a{$inputs.pet_id}b{$inputs.boolean}c", "a7btruec"),
            ("{$inputs.pet_id}{$inputs.boolean}", "7true"),
            (r#"{"petId": "{$inputs.pet_id}"}"#, r#"{"petId": "7"}"#),
            (
                "前{{$inputs.pet_id}}}🙂{$inputs.boolean}終",
                "前{7}}🙂true終",
            ),
            (
                "$USD {$inputs.pet_id} pay $inputs.pet_id {name}",
                "$USD 7 pay $inputs.pet_id {name}",
            ),
        ] {
            assert_eq!(
                evaluator.resolve_value_with_diagnostics(input),
                (json!(expected), Vec::new()),
                "{input}"
            );
        }
    }

    #[test]
    fn missing_values_project_to_null_with_each_warning_once_in_source_order() {
        let evaluator = evaluator();
        let input =
            "{$inputs.first}|{$inputs.null}|{$outputs.second}|{$inputs.first}|{$inputs.pet_id}";
        let expected_warnings = [
            ("$inputs.first", "input \"first\" not found in context"),
            ("$outputs.second", "output \"second\" not found in context"),
            ("$inputs.first", "input \"first\" not found in context"),
        ]
        .map(|(expression, message)| ExpressionWarning {
            expression: expression.to_owned(),
            message: message.to_owned(),
        });
        assert_eq!(
            evaluator.resolve_value_with_diagnostics(input),
            (json!("null|null|null|null|7"), expected_warnings.to_vec())
        );
    }

    #[test]
    fn each_classifier_error_preserves_the_whole_value_without_partial_resolution() {
        let evaluator = evaluator();
        for (input, expected_kind) in [
            (
                "{$inputs.absent} then {$",
                ExpressionStringErrorKind::Syntax,
            ),
            (
                "{$inputs.absent} then {$env.X} after {$inputs.other}",
                ExpressionStringErrorKind::EmbeddedRuntimeExpression,
            ),
            (
                "{$response.body.status}",
                ExpressionStringErrorKind::EmbeddedRuntimeExpression,
            ),
        ] {
            let error = match classify_value_string(input) {
                Err(error) => error,
                Ok(_) => panic!("expected classifier error for {input:?}"),
            };
            assert_eq!(error.kind, expected_kind);
            // A missing prefix would emit its own lookup warning if evaluated.
            assert_eq!(
                evaluator.resolve_value_with_diagnostics(input),
                (
                    Value::String(input.to_owned()),
                    vec![ExpressionWarning {
                        expression: input.to_owned(),
                        message: error.to_string(),
                    }]
                ),
                "{input}"
            );
            assert_eq!(evaluator.resolve_value(input), json!(input));
        }
    }

    #[test]
    fn long_unicode_literals_and_templates_keep_content_and_order() {
        let evaluator = evaluator();
        let literal = "前🙂 {name} $USD 後".repeat(4096);
        assert_eq!(
            evaluator.resolve_value_with_diagnostics(&literal),
            (json!(literal), Vec::new())
        );
        let template = "前🙂{{$inputs.pet_id}}$USD終".repeat(2048);
        let expected = "前🙂{7}$USD終".repeat(2048);
        assert_eq!(
            evaluator.resolve_value_with_diagnostics(&template),
            (json!(expected), Vec::new())
        );
    }

    #[test]
    fn intermediate_candidate_keeps_standalone_and_condition_routes_legacy() {
        let evaluator = evaluator();
        for condition in [
            "$response.body.items[0].id == 7",
            "$response.body#/items/0/id == 7",
        ] {
            assert_eq!(
                evaluator.evaluate_condition_with_diagnostics(condition),
                (true, Vec::new()),
                "{condition}"
            );
        }
        assert_eq!(
            evaluator.evaluate_with_diagnostics("$response.body.items[0].id"),
            (json!(7), Vec::new())
        );
        assert_eq!(
            evaluator.evaluate_with_diagnostics("$response.body#/items/0/id"),
            (json!(7), Vec::new())
        );
        assert_eq!(evaluator.evaluate("$USD"), Value::Null);
        assert_eq!(evaluator.resolve_value("$USD"), json!("$USD"));
    }
}
