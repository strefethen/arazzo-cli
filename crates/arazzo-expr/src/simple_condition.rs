//! Simple-condition syntax, evaluation and compatibility entry points.

mod evaluation;
mod numeric;
mod syntax;

pub use evaluation::ConditionEvaluation;
pub use syntax::{
    parse_simple_condition, ConditionError, ConditionErrorKind, ParsedSimpleCondition,
};

use crate::{ExpressionEvaluator, ExpressionWarning};

impl ExpressionEvaluator {
    /// Evaluate a condition, failing closed for syntax and evaluation errors.
    pub fn evaluate_condition(&self, condition: &str) -> bool {
        self.evaluate_condition_detailed(condition).result
    }

    /// Evaluate a condition and retain lookup warnings without inventing an error warning.
    pub fn evaluate_condition_with_diagnostics(
        &self,
        condition: &str,
    ) -> (bool, Vec<ExpressionWarning>) {
        let evaluation = self.evaluate_condition_detailed(condition);
        (evaluation.result, evaluation.warnings)
    }

    /// Parse the complete condition once, then evaluate its canonical AST.
    pub fn evaluate_condition_detailed(&self, condition: &str) -> ConditionEvaluation {
        match parse_simple_condition(condition) {
            Ok(parsed) => evaluation::evaluate(&parsed, self.context()),
            Err(error) => ConditionEvaluation {
                result: false,
                warnings: Vec::new(),
                error: Some(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{EvalContext, ExpressionEvaluator};
    use serde_json::json;

    /// Arazzo 1.1.0 §5.8.11.2 and §5.8.11.4.1: the six normative Simple
    /// comparison operators normalize string operands case-insensitively.
    #[test]
    fn simple_string_comparisons_are_case_insensitive_for_all_normative_operators() {
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

    /// Keep non-string types and independent JSONPath semantics outside string normalization.
    #[test]
    fn simple_string_comparison_negative_evidence() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        for (condition, expected) in [
            ("2 < 10", true),
            ("'10' < '2'", true),
            ("'10' == '010'", false),
            ("'10' == 10", true),
            ("true == true", true),
            ("null == null", true),
            ("null == false", false),
        ] {
            let result = eval.evaluate_condition_detailed(condition);
            assert_eq!(result.result, expected, "{condition}");
            assert!(result.error.is_none(), "{condition}: {:?}", result.error);
        }
        for condition in ["'a' contains 'a'", "'a' matches 'a'", "'a' in 'a'"] {
            let result = eval.evaluate_condition_detailed(condition);
            assert!(!result.result);
            assert_eq!(
                result.error.as_ref().map(|error| error.kind),
                Some(super::ConditionErrorKind::Syntax)
            );
            assert!(result.warnings.is_empty());
        }
        crate::tests::json_path_filters_remain_case_sensitive_for_equality_and_ordering();
    }
}
