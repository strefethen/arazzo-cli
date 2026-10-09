#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use arazzo_expr::{ConditionErrorKind, EvalContext, ExpressionEvaluator};
use serde_json::{json, Value};

fn evaluator() -> ExpressionEvaluator {
    ExpressionEvaluator::new(EvalContext {
        response_body: Some(json!({"items":[{"id":7}], "customer":{}, "nil":null, "number":42})),
        inputs: BTreeMap::from([
            ("yes".to_owned(), json!(true)),
            ("dot.name".to_owned(), json!(7)),
        ]),
        ..EvalContext::default()
    })
}

fn decision(evaluator: &ExpressionEvaluator, condition: &str, expected: bool) {
    let result = evaluator.evaluate_condition_detailed(condition);
    assert_eq!(result.error, None, "{condition}: {result:?}");
    assert_eq!(result.result, expected, "{condition}");
    assert_eq!(
        evaluator.evaluate_condition(condition),
        expected,
        "{condition}"
    );
    assert_eq!(
        evaluator.evaluate_condition_with_diagnostics(condition),
        (expected, result.warnings)
    );
}

#[test]
fn conformance_simple_condition_evaluation_positive_evidence() {
    let evaluator = evaluator();
    for (condition, expected) in [
        ("true", true),
        ("false", false),
        ("null", false),
        ("!null", true),
        ("$inputs.yes", true),
        ("$inputs.absent", false),
        ("$response.body.items[0].id == 7", true),
        ("$response.body#/items/0/id == 7", true),
        ("$inputs.dot.name == 7", true),
        ("$response.body.customer.address.city == null", true),
        ("$response.body.items[99].id == null", true),
        (
            "$response.body.items[18446744073709551616000000].id == null",
            true,
        ),
        ("'It''s' == 'IT''S'", true),
        ("'a\\' == 'A\\'", true),
        ("'Ä' == 'ä'", true),
        ("'10' < '2'", true),
        ("'01' == '1'", false),
        ("null == null", true),
        ("null != null", false),
        ("7 != null", true),
        ("!(7 == null)", true),
        ("null < 7", false),
        ("7 >= null", false),
        ("true || false && false", true),
        ("(true || false) && false", false),
        ("!(true && false)", true),
        ("false && 18446744073709551616 == 0", false),
        ("true || $response.body.nil.x == 1", true),
    ] {
        decision(&evaluator, condition, expected);
    }
}

#[test]
fn inequality_is_negated_equality_including_null_and_missing() {
    let evaluator = ExpressionEvaluator::new(EvalContext {
        response_body: Some(json!({"nil": null})),
        inputs: BTreeMap::from([
            ("array".to_owned(), json!([null, 7])),
            ("object".to_owned(), json!({"nil": null})),
        ]),
        ..EvalContext::default()
    });
    let operands = [
        "null",
        "$inputs.absent",
        "false",
        "true",
        "0",
        "7",
        "'7'",
        "'x'",
        "$inputs.array",
        "$inputs.object",
    ];
    for left in operands {
        for right in operands {
            let equal = evaluator.evaluate_condition_detailed(&format!("{left} == {right}"));
            let unequal = evaluator.evaluate_condition_detailed(&format!("{left} != {right}"));
            let negated = evaluator.evaluate_condition_detailed(&format!("!({left} == {right})"));
            assert_eq!(unequal.result, negated.result, "{left}, {right}");
            assert_eq!(unequal.error.is_some(), equal.error.is_some());
            assert_eq!(negated.error.is_some(), equal.error.is_some());
            assert_eq!(unequal.warnings, equal.warnings);
            assert_eq!(negated.warnings, equal.warnings);
            if equal.error.is_none() {
                assert_eq!(unequal.result, !equal.result, "{left}, {right}");
            } else {
                assert!(!equal.result && !unequal.result && !negated.result);
            }
        }
    }
    for condition in [
        "$response.body.nil.x != null",
        "!($response.body.nil.x == null)",
    ] {
        let result = evaluator.evaluate_condition_detailed(condition);
        assert!(!result.result);
        assert_eq!(
            result.error.unwrap().kind,
            ConditionErrorKind::InvalidEvaluation
        );
    }
}

#[test]
fn spec_non_null_data_guard_accepts_only_present_non_null_data() {
    for data in [
        Some(json!({})),
        Some(json!([])),
        Some(json!(false)),
        Some(json!(0)),
        Some(json!("")),
        Some(Value::Null),
        None,
    ] {
        let expected = data.as_ref().is_some_and(|value| !value.is_null());
        let body = data.map_or_else(|| json!({}), |value| json!({"data": value}));
        let evaluator = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            response_body: Some(body),
            ..EvalContext::default()
        });
        decision(
            &evaluator,
            "$statusCode == 200 && $response.body.data != null",
            expected,
        );
        decision(
            &evaluator,
            "$statusCode == 200 && !($response.body.data == null)",
            expected,
        );
    }
}

#[test]
fn conformance_simple_condition_evaluation_negative_evidence() {
    let evaluator = evaluator();
    for condition in [
        "0",
        "'false'",
        "$response.body",
        "$response.body.items",
        "!0",
        "true && 1",
        "false || 'x'",
        "null < null",
        "true >= false",
        "$response.body.items < $response.body.items",
        "$response.body < $response.body",
        "$response.body.nil.x == null",
        "$response.body.number.x == null",
        "$response.body.customer[0] == null",
        "$response.body.number[0] == null",
        "!($response.body.nil.x == null)",
        "$response.body.nil.x == null || true",
        "!(1 < true)",
        "(1 < true) || true",
    ] {
        let result = evaluator.evaluate_condition_detailed(condition);
        assert!(!result.result, "{condition}");
        assert_eq!(
            result.error.as_ref().map(|error| error.kind),
            Some(ConditionErrorKind::InvalidEvaluation),
            "{condition}"
        );
        assert!(!evaluator.evaluate_condition(condition));
        assert_eq!(
            evaluator.evaluate_condition_with_diagnostics(condition),
            (false, result.warnings)
        );
    }
    for condition in [
        "true || (",
        "false && (",
        "'x' contains 'x'",
        "'x' matches '['",
        "1 in [1]",
        "\"x\" == 'x'",
    ] {
        let result = evaluator.evaluate_condition_detailed(condition);
        assert!(!result.result);
        assert_eq!(
            result.error.unwrap().kind,
            ConditionErrorKind::Syntax,
            "{condition}"
        );
        assert!(!evaluator.evaluate_condition(condition));
        assert_eq!(
            evaluator.evaluate_condition_with_diagnostics(condition),
            (false, Vec::new())
        );
    }
    let excessive = format!("{}true{}", "(".repeat(33), ")".repeat(33));
    assert_eq!(
        evaluator
            .evaluate_condition_detailed(&excessive)
            .error
            .unwrap()
            .kind,
        ConditionErrorKind::NestingLimit
    );
}

#[test]
fn every_ordered_type_pair_and_operator_obeys_the_matrix() {
    // Number/string is deliberately valid numeric text; invalid conversion has its own table.
    let values = [
        json!(2),
        json!("2"),
        json!(true),
        Value::Null,
        json!([2]),
        json!({"key":2}),
    ];
    for (left_type, left) in values.iter().enumerate() {
        for (right_type, right) in values.iter().enumerate() {
            let evaluator = ExpressionEvaluator::new(EvalContext {
                inputs: BTreeMap::from([
                    ("left".to_owned(), left.clone()),
                    ("right".to_owned(), right.clone()),
                ]),
                ..EvalContext::default()
            });
            for operator in ["==", "!=", "<", "<=", ">", ">="] {
                let condition = format!("$inputs.left {operator} $inputs.right");
                let result = evaluator.evaluate_condition_detailed(&condition);
                let one_null = (left_type == 3) != (right_type == 3);
                let equality = left_type == right_type || (left_type < 2 && right_type < 2);
                if one_null && !matches!(operator, "==" | "!=") {
                    assert!(!result.result, "{condition} [{left_type},{right_type}]");
                    assert!(result.error.is_none());
                } else if matches!(operator, "==" | "!=") {
                    assert_eq!(
                        result.result,
                        if operator == "==" {
                            equality
                        } else {
                            !equality
                        },
                        "{condition} [{left_type},{right_type}]"
                    );
                    assert!(result.error.is_none());
                } else if left_type < 2 && right_type < 2 {
                    assert_eq!(result.result, matches!(operator, "<=" | ">="));
                    assert!(result.error.is_none());
                } else {
                    assert!(!result.result);
                    assert_eq!(
                        result.error.unwrap().kind,
                        ConditionErrorKind::InvalidEvaluation
                    );
                }
            }
        }
    }
}

#[test]
fn bounded_numeric_conversion_and_exact_represented_comparison() {
    let evaluator = evaluator();
    for (condition, expected) in [
        ("-9223372036854775808 < 18446744073709551615", true),
        ("18446744073709551615 == 18446744073709551615", true),
        ("9007199254740992 == 9007199254740993", false),
        ("9007199254740993 > 9007199254740992.0", true),
        ("9007199254740992.0 < 9007199254740993", true),
        ("18446744073709551615 < 18446744073709551616.0", true),
        ("-9223372036854775808 == -9223372036854775808.0", true),
        ("9223372036854775808 > 9223372036854775807", true),
        ("-1 < 0", true),
        ("1 == 1.0", true),
        ("1e3 == 1000", true),
        ("-0 == 0", true),
        ("-0.0 == 0.0", true),
        ("0e999 == -0.0", true),
        ("1 < 1.5", true),
        ("-1 > -1.5", true),
        ("1.0 == 1.0000000000000002", false),
        ("0.10000000000000001 == 0.1", true),
        ("'200' == 200", true),
        ("200 == '200'", true),
        ("'199' < 200", true),
        ("200 > '199'", true),
        ("'200' <= 200", true),
        ("200 >= '200'", true),
        ("'201' != 200", true),
        ("200 != '201'", true),
        ("'18446744073709551615' == 18446744073709551615", true),
        ("'1e3' == 1000", true),
        ("'0e-400' == 0", true),
    ] {
        decision(&evaluator, condition, expected);
    }
    for condition in [
        "18446744073709551616 == 0",
        "-9223372036854775809 == 0",
        "1e400 == 0",
        "1e-400 == 0",
        "' 200 ' == 200",
        "'01' == 1",
        "'+1' == 1",
        "'NaN' < 1",
        "'Infinity' == 1",
        "'1.' == 1",
        "'1x' == 1",
        "'18446744073709551616' == 0",
        "'-9223372036854775809' == 0",
        "'1e400' == 1",
        "'1e-400' == 0",
    ] {
        let result = evaluator.evaluate_condition_detailed(condition);
        assert!(!result.result, "{condition}");
        assert_eq!(
            result.error.unwrap().kind,
            ConditionErrorKind::InvalidEvaluation,
            "{condition}"
        );
    }
}

#[test]
fn warning_order_short_circuit_first_error_and_utf8_offsets() {
    let evaluator = evaluator();
    for condition in [
        "false && $inputs.absent",
        "true || $inputs.absent",
        "true || $response.body.nil.x",
    ] {
        assert!(evaluator
            .evaluate_condition_detailed(condition)
            .warnings
            .is_empty());
    }
    let condition =
        "$inputs.first == null && $inputs.second == null && $response.body.nil.x == null || true";
    let result = evaluator.evaluate_condition_detailed(condition);
    assert!(!result.result);
    assert_eq!(
        result
            .warnings
            .iter()
            .map(|warning| warning.expression.as_str())
            .collect::<Vec<_>>(),
        ["$inputs.first", "$inputs.second"]
    );
    assert_eq!(
        result.error.unwrap().byte_offset,
        condition.find(".x").unwrap()
    );
    for condition in [
        "'é' == 'É' && $response.body.nil.x == null",
        "'é' == 'É' && 1 < true",
        "'é' == 'É' && 'NaN' == 1",
        "'é' == 'É' && 1e400 == 1",
    ] {
        let error = evaluator
            .evaluate_condition_detailed(condition)
            .error
            .unwrap();
        let needle = if condition.contains(".x") {
            ".x"
        } else if condition.contains("NaN") {
            "'NaN'"
        } else if condition.contains("1e400") {
            "1e400"
        } else {
            "<"
        };
        assert_eq!(
            error.byte_offset,
            condition.find(needle).unwrap(),
            "{condition}"
        );
        assert!(condition.is_char_boundary(error.byte_offset));
    }
    let result = evaluator.evaluate_condition_detailed("$inputs.never == null || (");
    assert_eq!(result.error.unwrap().kind, ConditionErrorKind::Syntax);
    assert!(
        result.warnings.is_empty(),
        "parse the complete condition before any lookup"
    );
}

#[test]
fn structural_comparison_visits_only_required_values_in_fixed_order() {
    for (left, right, expected, error) in [
        (json!([false, "NaN"]), json!([true, 1]), true, false),
        (json!([true, "NaN"]), json!([true, 1]), false, true),
        (json!(["NaN"]), json!([1, 2]), true, false),
        (json!({"x":"NaN"}), json!({"x":1,"y":2}), true, false),
        (
            json!({"z":"NaN","a":false}),
            json!({"a":true,"z":1}),
            true,
            false,
        ),
        (
            json!({"z":"NaN","a":true}),
            json!({"a":true,"z":1}),
            false,
            true,
        ),
        (json!({"Name":1}), json!({"name":1}), true, false),
        (
            json!([{"name":"Ä","id":"7"}]),
            json!([{"id":7,"name":"ä"}]),
            false,
            false,
        ),
    ] {
        let evaluator = ExpressionEvaluator::new(EvalContext {
            inputs: BTreeMap::from([("left".to_owned(), left), ("right".to_owned(), right)]),
            ..EvalContext::default()
        });
        let result = evaluator.evaluate_condition_detailed("!($inputs.left == $inputs.right)");
        assert_eq!(result.result, expected, "{result:?}");
        assert_eq!(result.error.is_some(), error);
    }
}

#[test]
fn deeply_nested_runtime_values_compare_without_recursive_equality() {
    let mut left = json!("Ä");
    let mut right = json!("ä");
    for _ in 0..256 {
        left = Value::Array(vec![left]);
        right = Value::Array(vec![right]);
    }
    let evaluator = ExpressionEvaluator::new(EvalContext {
        inputs: BTreeMap::from([("left".to_owned(), left), ("right".to_owned(), right)]),
        ..EvalContext::default()
    });
    decision(&evaluator, "$inputs.left == $inputs.right", true);
}
