//! One test only: Pest's call limit is process-global.
#![allow(clippy::unwrap_used)]

use std::num::NonZeroUsize;

use arazzo_expr::{parse_runtime_expression, RuntimeExpressionErrorKind};

struct ResetCallLimit;

impl Drop for ResetCallLimit {
    fn drop(&mut self) {
        pest::set_call_limit(None);
    }
}

#[test]
fn adversarial_runtime_expressions_stay_within_linear_rule_call_budget() {
    // The multiplier covers both the anchored parse and the diagnostic prefix
    // parse. Several lengths catch superlinear growth without wall-clock timing.
    const CALLS_PER_BYTE: usize = 64;
    for length in [64, 512, 4096, 32768] {
        for (input, expected_error) in [
            (format!("$inputs.{}", "a".repeat(length)), None),
            (format!("$steps.{}.outputs.x", "a".repeat(length)), None),
            (
                format!("$steps.{}.invalid", "a".repeat(length)),
                Some(RuntimeExpressionErrorKind::InvalidIdentifier),
            ),
            (
                format!("$inputs.{}{{", "a".repeat(length)),
                Some(RuntimeExpressionErrorKind::TrailingInput),
            ),
            (format!("$response.body#/{}", "a".repeat(length)), None),
            (
                format!("$response.body#/{}~2", "a".repeat(length)),
                Some(RuntimeExpressionErrorKind::InvalidJsonPointerEscape),
            ),
            (format!("$inputs.x#/{}", "~0".repeat(length)), None),
            (
                format!("$inputs.x#/{}~2", "~0".repeat(length)),
                Some(RuntimeExpressionErrorKind::InvalidJsonPointerEscape),
            ),
            (
                format!("$sourceDescriptions.s.{}", "a.b/~2é".repeat(length)),
                None,
            ),
            (
                format!("$sourceDescriptions.s.{}}}", "a.b/~2é".repeat(length)),
                Some(RuntimeExpressionErrorKind::TrailingInput),
            ),
        ] {
            let budget = CALLS_PER_BYTE * input.len();
            let reset = ResetCallLimit;
            pest::set_call_limit(NonZeroUsize::new(budget));
            let result = parse_runtime_expression(&input);
            drop(reset);
            match expected_error {
                None => assert!(
                    result.is_ok(),
                    "length {length}, budget {budget}: {result:?}"
                ),
                Some(kind) => assert_eq!(
                    result.unwrap_err().kind,
                    kind,
                    "length {length}, budget {budget}"
                ),
            }
        }
    }
}
