//! Simple Criterion condition evaluation behind
//! `ExpressionEvaluator::evaluate_condition` and
//! `ExpressionEvaluator::evaluate_condition_with_diagnostics`.
//!
//! This module owns the condition splitter, operator scan, operand
//! resolution, and the Simple string comparison helpers. It records the
//! evaluator's current behavior and makes no claim about what the
//! specification allows.

use std::cmp::Ordering;

use serde_json::Value;

use super::{
    compare_ordered, compare_values, index_outside_quotes, is_truthy, matches_operator,
    parse_value, to_string_value, ExpressionEvaluator, ExpressionWarning,
};

impl ExpressionEvaluator {
    /// Evaluate a condition expression with `||` and `&&` precedence.
    pub fn evaluate_condition(&self, condition: &str) -> bool {
        self.evaluate_condition_with_diagnostics(condition).0
    }

    /// Evaluate a condition expression, returning both the boolean result and
    /// any diagnostic warnings from expression resolution.
    pub fn evaluate_condition_with_diagnostics(
        &self,
        condition: &str,
    ) -> (bool, Vec<ExpressionWarning>) {
        let condition = condition.trim();
        if condition.is_empty() {
            return (false, Vec::new());
        }

        let mut warnings = Vec::new();

        // Parenthesis grouping: if the entire expression is wrapped in balanced
        // parens (depth reaches zero only at the last char), strip them and recurse.
        if condition.starts_with('(')
            && condition.ends_with(')')
            && is_balanced_outer_parens(condition)
        {
            let inner = &condition[1..condition.len() - 1];
            let (result, w) = self.evaluate_condition_with_diagnostics(inner);
            warnings.extend(w);
            return (result, warnings);
        }

        // Split on `||` first (lowest precedence).
        if let Some(parts) = split_outside_quotes(condition, "||") {
            for part in parts {
                let (result, w) = self.evaluate_condition_with_diagnostics(part);
                warnings.extend(w);
                if result {
                    return (true, warnings);
                }
            }
            return (false, warnings);
        }

        // Split on `&&` next.
        if let Some(parts) = split_outside_quotes(condition, "&&") {
            for part in parts {
                let (result, w) = self.evaluate_condition_with_diagnostics(part);
                warnings.extend(w);
                if !result {
                    return (false, warnings);
                }
            }
            return (true, warnings);
        }

        // NOT operator: strip leading `!` but only when the next char is not `=`
        // (to avoid consuming `!=` as NOT + `=`). Applied after `||`/`&&` splits
        // so that NOT binds tighter than logical connectives.
        if condition.starts_with('!') && !condition.starts_with("!=") {
            let inner = condition[1..].trim();
            let (result, w) = self.evaluate_condition_with_diagnostics(inner);
            warnings.extend(w);
            return (!result, warnings);
        }

        self.evaluate_comparison_with_diagnostics(condition)
    }

    fn evaluate_comparison_with_diagnostics(
        &self,
        condition: &str,
    ) -> (bool, Vec<ExpressionWarning>) {
        let mut warnings = Vec::new();
        let (op, idx) = find_operator(condition);
        if op.is_empty() {
            let (val, w) = resolve_operand_with_diagnostics(self, condition);
            warnings.extend(w);
            return (is_truthy(&val), warnings);
        }

        let (left, left_w) = resolve_operand_with_diagnostics(self, &condition[..idx]);
        warnings.extend(left_w);
        let right = condition[idx + op.len()..].trim();

        let result = match op {
            "==" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_simple_values(&left, &rv)
            }
            "!=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                !compare_simple_values(&left, &rv)
            }
            ">" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_simple_ordered(&left, &rv).is_gt()
            }
            "<" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_simple_ordered(&left, &rv).is_lt()
            }
            ">=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_simple_ordered(&left, &rv).is_ge()
            }
            "<=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_simple_ordered(&left, &rv).is_le()
            }
            " contains " => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                to_string_value(&left).contains(&*to_string_value(&rv))
            }
            " matches " => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                matches_operator::evaluate(condition, &left, &rv, &mut warnings)
            }
            " in " => {
                let (result, w) = eval_in_with_diagnostics(self, &left, right);
                warnings.extend(w);
                result
            }
            _ => false,
        };
        (result, warnings)
    }
}

fn resolve_operand_with_diagnostics(
    eval: &ExpressionEvaluator,
    raw: &str,
) -> (Value, Vec<ExpressionWarning>) {
    let token = raw.trim();
    if token.starts_with('$') {
        eval.evaluate_with_diagnostics(token)
    } else {
        (parse_value(token), Vec::new())
    }
}

/// Returns `true` when the first `(` and the last `)` in `s` form a balanced
/// pair that encloses the entire expression — i.e. the paren depth only reaches
/// zero at the very last character.
fn is_balanced_outer_parens(s: &str) -> bool {
    debug_assert!(s.starts_with('(') && s.ends_with(')'));
    let mut depth: usize = 0;
    let last = s.len() - 1;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx != last {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_outside_quotes<'a>(input: &'a str, delim: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut paren_depth: usize = 0;
    let mut bracket_depth: usize = 0;
    let mut found = false;
    let mut prev_backslash = false;

    for (idx, ch) in input.char_indices() {
        if idx < start {
            prev_backslash = false;
            continue;
        }
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '"' || ch == '\'') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        if ch == '(' {
            paren_depth += 1;
            prev_backslash = false;
            continue;
        }
        if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            prev_backslash = false;
            continue;
        }
        if ch == '[' {
            bracket_depth += 1;
            prev_backslash = false;
            continue;
        }
        if ch == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            prev_backslash = false;
            continue;
        }

        if paren_depth == 0 && bracket_depth == 0 && input[idx..].starts_with(delim) {
            parts.push(input[start..idx].trim());
            start = idx + delim.len();
            found = true;
        }
        prev_backslash = ch == '\\';
    }

    if !found {
        return None;
    }
    parts.push(input[start..].trim());
    Some(parts)
}

fn find_operator(input: &str) -> (&'static str, usize) {
    for word_op in [" contains ", " matches ", " in "] {
        if let Some(idx) = index_outside_quotes(input, word_op) {
            return (word_op, idx);
        }
    }

    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;
    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '"' || ch == '\'') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        prev_backslash = ch == '\\' && !prev_backslash;

        if input[idx..].starts_with("!=") {
            return ("!=", idx);
        }
        if input[idx..].starts_with(">=") {
            return (">=", idx);
        }
        if input[idx..].starts_with("<=") {
            return ("<=", idx);
        }
        if input[idx..].starts_with("==") {
            return ("==", idx);
        }

        if ch == '>' {
            return (">", idx);
        }
        if ch == '<' {
            return ("<", idx);
        }
    }

    ("", usize::MAX)
}

fn eval_in_with_diagnostics(
    eval: &ExpressionEvaluator,
    left: &Value,
    list_expr: &str,
) -> (bool, Vec<ExpressionWarning>) {
    let list_expr = list_expr.trim();
    let mut warnings = Vec::new();
    if !(list_expr.starts_with('[') && list_expr.ends_with(']')) {
        return (false, warnings);
    }
    let inner = &list_expr[1..list_expr.len() - 1];
    if inner.trim().is_empty() {
        return (false, warnings);
    }

    for token in split_list_elements(inner) {
        let (val, w) = resolve_operand_with_diagnostics(eval, token);
        warnings.extend(w);
        if compare_values(left, &val) {
            return (true, warnings);
        }
    }
    (false, warnings)
}

fn split_list_elements(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;

    for (idx, ch) in input.char_indices() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if ch == '\\' {
            prev_backslash = true;
            continue;
        }
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }
        if ch == ',' {
            parts.push(input[start..idx].trim());
            start = idx + 1;
        }
    }
    parts.push(input[start..].trim());
    parts
}

/// Simple Criterion string comparisons use Unicode lowercase normalization.
/// This is deliberately separate from the shared helpers: JSONPath filters
/// and the legacy `in` operator retain their existing byte-sensitive behavior.
pub(super) fn compare_simple_values(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(lhs), Value::String(rhs)) => lhs.to_lowercase() == rhs.to_lowercase(),
        _ => compare_values(a, b),
    }
}

/// Ordered Simple Criterion comparisons follow the same case-insensitive
/// string contract without changing JSONPath's shared ordering helper.
pub(super) fn compare_simple_ordered(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::String(lhs), Value::String(rhs)) => lhs.to_lowercase().cmp(&rhs.to_lowercase()),
        _ => compare_ordered(a, b),
    }
}

#[cfg(test)]
pub(super) fn run_simple_string_comparisons_evidence() {
    tests::simple_string_comparisons_are_case_insensitive_for_all_normative_operators();
}

#[cfg(test)]
pub(super) fn run_simple_non_string_comparisons_evidence() {
    tests::simple_non_string_comparisons_preserve_existing_semantics();
}

#[cfg(test)]
pub(super) fn run_contains_matches_and_in_evidence() {
    tests::evaluate_condition_contains_matches_and_in();
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::{EvalContext, ExpressionEvaluator};
    use proptest::prelude::*;
    use serde_json::{json, Value};

    #[test]
    fn evaluate_condition_core_ops() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("$statusCode == 200"));
        assert!(eval.evaluate_condition("$statusCode != 404"));
        assert!(eval.evaluate_condition("$statusCode > 199"));
        assert!(eval.evaluate_condition("$statusCode < 300"));
        assert!(eval.evaluate_condition("$statusCode >= 200"));
        assert!(eval.evaluate_condition("$statusCode <= 200"));
        assert!(eval.evaluate_condition("$statusCode >= 200 && $statusCode < 300"));
        assert!(eval.evaluate_condition("$statusCode == 200 || $statusCode == 201"));
        assert!(!eval.evaluate_condition("$statusCode == 500"));
    }

    /// Arazzo 1.1.0 §5.8.11.2 and §5.8.11.4.1: the six normative Simple
    /// comparison operators normalize string operands case-insensitively.
    #[test]
    pub(super) fn simple_string_comparisons_are_case_insensitive_for_all_normative_operators() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("ascii".to_string(), json!("Alpha"));
        ctx.inputs.insert("unicode".to_string(), json!("Ångström"));
        let eval = ExpressionEvaluator::new(ctx);

        for (condition, expected) in [
            (r#"$inputs.ascii == 'aLpHa'"#, true),
            (r#"$inputs.ascii != 'aLpHa'"#, false),
            (r#"$inputs.ascii < 'bravo'"#, true),
            (r#"$inputs.ascii <= 'ALPHA'"#, true),
            (r#"$inputs.ascii > 'aardvark'"#, true),
            (r#"$inputs.ascii >= 'alpha'"#, true),
            (r#"$inputs.unicode == 'ångström'"#, true),
            (r#"$inputs.unicode > 'zebra'"#, true),
        ] {
            assert_eq!(
                eval.evaluate_condition(condition),
                expected,
                "condition={condition}"
            );
        }
    }

    /// Numeric, boolean, and null pairs retain the existing helpers instead
    /// of being coerced through the Simple string normalization path.
    #[test]
    pub(super) fn simple_non_string_comparisons_preserve_existing_semantics() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("number".to_string(), json!(2));
        ctx.inputs.insert("numericString".to_string(), json!("10"));
        ctx.inputs
            .insert("smallerNumericString".to_string(), json!("2"));
        ctx.inputs.insert("boolean".to_string(), json!(true));
        ctx.inputs.insert("none".to_string(), Value::Null);
        let eval = ExpressionEvaluator::new(ctx);

        assert!(eval.evaluate_condition("$inputs.number < 10"));
        assert!(eval.evaluate_condition("$inputs.numericString < $inputs.smallerNumericString"));
        assert!(!eval.evaluate_condition("$inputs.numericString == '010'"));
        assert!(eval.evaluate_condition("$inputs.numericString == 10"));
        assert!(eval.evaluate_condition("$inputs.boolean == true"));
        assert!(!eval.evaluate_condition("$inputs.none == null"));
        assert!(!eval.evaluate_condition("$inputs.none == false"));
    }

    #[test]
    fn evaluate_condition_and_or_precedence() {
        let eval200 = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval200
            .evaluate_condition("$statusCode == 200 || $statusCode == 404 && $statusCode == 500"));

        let eval404 = ExpressionEvaluator::new(EvalContext {
            status_code: Some(404),
            ..EvalContext::default()
        });
        assert!(eval404
            .evaluate_condition("$statusCode == 200 || $statusCode == 404 && $statusCode == 404"));
    }

    #[test]
    pub(super) fn evaluate_condition_contains_matches_and_in() {
        let mut ctx = EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        };
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([
                ("msg".to_string(), json!("hello world")),
                ("email".to_string(), json!("alice@example.com")),
                ("val".to_string(), json!("hello, world")),
                ("role".to_string(), json!("admin")),
            ]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "world""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "xyz""#));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.email matches "^[a-z]+@""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.email matches "^[0-9]+""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.email matches "[invalid""#));
        assert!(eval.evaluate_condition("$statusCode in [200, 201, 204]"));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.role in ["admin", "superadmin"]"#));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.val in ["hello, world", "foo"]"#));
        assert!(!eval.evaluate_condition("$statusCode in []"));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "hello""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "HELLO""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.email matches "^[A-Z]+@""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.role in ["ADMIN"]"#));
    }

    #[test]
    fn evaluate_condition_expression_both_sides() {
        let mut ctx = EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        };
        ctx.inputs.insert("expected".to_string(), json!(200));
        let eval = ExpressionEvaluator::new(ctx);
        assert!(eval.evaluate_condition("$statusCode == $inputs.expected"));
    }

    #[test]
    fn evaluate_condition_with_diagnostics_surfaces_warnings() {
        let ctx = EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        // Known expression — no warnings
        let (result, warnings) = eval.evaluate_condition_with_diagnostics("$statusCode == 200");
        assert!(result);
        assert!(warnings.is_empty());

        // Unknown step reference — should produce a warning (resolves to null)
        let (result, warnings) =
            eval.evaluate_condition_with_diagnostics("$steps.missing.outputs.x == 42");
        assert!(!result);
        assert!(!warnings.is_empty());
        assert!(
            warnings.iter().any(|w| w.message.contains("missing")),
            "expected warning about missing step, got: {warnings:?}"
        );

        // Compound condition with one unknown — warnings from both branches collected
        let (_, warnings) = eval.evaluate_condition_with_diagnostics(
            "$statusCode == 200 && $inputs.nonexistent == true",
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("nonexistent")),
            "expected warning about nonexistent input, got: {warnings:?}"
        );
    }

    #[test]
    fn evaluate_condition_truthiness_and_quoted_operators() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("flag".to_string(), json!(true));
        ctx.inputs.insert("zero".to_string(), json!(0));
        ctx.inputs.insert("empty".to_string(), json!(""));
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("msg".to_string(), json!("status >= ok"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert!(eval.evaluate_condition("$inputs.flag"));
        assert!(!eval.evaluate_condition("$inputs.zero"));
        assert!(!eval.evaluate_condition("$inputs.empty"));
        assert!(!eval.evaluate_condition("$inputs.missing"));
        assert!(eval.evaluate_condition("just a string"));
        assert!(!eval.evaluate_condition(""));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.msg == "status >= ok""#));
    }

    #[test]
    fn logical_not_precedence_bug() {
        let ctx = EvalContext::default();
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "!true || true";
        assert!(
            eval.evaluate_condition(condition),
            "NOT precedence should be higher than OR"
        );
    }

    #[test]
    fn split_outside_quotes_handles_brackets_and_escapes() {
        let ctx = EvalContext {
            response_body: Some(json!({"name||title": "Something"})),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "$response.body['name||title'] == 'Something'";
        assert!(
            eval.evaluate_condition(condition),
            "Should not split || inside brackets"
        );
    }

    #[test]
    fn split_outside_quotes_fails_on_escaped_quotes() {
        let ctx = EvalContext {
            inputs: std::collections::BTreeMap::from([("name".to_string(), json!("Alice\"Bob"))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "$inputs.name == \"Alice\\\"Bob\"";
        assert!(
            eval.evaluate_condition(condition),
            "Should handle escaped quotes in strings"
        );
    }

    proptest! {
        #[test]
        fn evaluate_condition_fuzz_input_does_not_panic(condition in ".{0,96}") {
            let eval = ExpressionEvaluator::new(EvalContext::default());
            match eval.evaluate_condition(&condition) {
                true | false => {}
            }
        }
    }

    // ── Phase 1: NOT operator and parenthesis grouping ──────────────

    #[test]
    fn not_operator_negates_true() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert!(!eval.evaluate_condition("!true"));
    }

    #[test]
    fn not_operator_negates_false() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert!(eval.evaluate_condition("!false"));
    }

    #[test]
    fn not_operator_on_expression() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(404),
            ..EvalContext::default()
        });
        // !($statusCode == 200) should be true when status is 404
        assert!(eval.evaluate_condition("!($statusCode == 200)"));
        // !($statusCode == 404) should be false when status is 404
        assert!(!eval.evaluate_condition("!($statusCode == 404)"));
    }

    #[test]
    fn not_does_not_consume_ne_operator() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("a".to_string(), json!(1));
        ctx.inputs.insert("b".to_string(), json!(2));
        let eval = ExpressionEvaluator::new(ctx);
        // != must still work correctly — the `!` must NOT be consumed as NOT
        assert!(eval.evaluate_condition("$inputs.a != $inputs.b"));
    }

    #[test]
    fn paren_grouping_simple() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("($statusCode == 200)"));
    }

    #[test]
    fn paren_grouping_or() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("($statusCode == 200 || $statusCode == 201)"));
    }

    #[test]
    fn paren_grouping_with_and() {
        let mut ctx = EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        };
        ctx.inputs.insert("c".to_string(), json!(3));
        let eval = ExpressionEvaluator::new(ctx);
        // ($statusCode == 200 || $statusCode == 201) && $inputs.c == 3
        assert!(
            eval.evaluate_condition("($statusCode == 200 || $statusCode == 201) && $inputs.c == 3")
        );
        // Should be false when the && side fails
        assert!(!eval
            .evaluate_condition("($statusCode == 200 || $statusCode == 201) && $inputs.c == 999"));
    }

    #[test]
    fn nested_parens() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("a".to_string(), json!(1));
        ctx.inputs.insert("b".to_string(), json!(2));
        let eval = ExpressionEvaluator::new(ctx);
        assert!(eval.evaluate_condition("(($inputs.a == 1)) && $inputs.b == 2"));
    }

    #[test]
    fn not_on_paren_grouped_expression() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            response_body: Some(json!({"error": null})),
            ..EvalContext::default()
        });
        // !($response.body.error) should be true when error is null (falsy)
        assert!(eval.evaluate_condition("!($response.body.error)"));
        // !($statusCode == 500) should be true when status is 200
        assert!(eval.evaluate_condition("!($statusCode == 500)"));
    }
}
