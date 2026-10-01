//! One test only: Pest's call limit is process-global.
#![allow(clippy::unwrap_used)]

use std::num::NonZeroUsize;

use arazzo_expr::{
    parse_runtime_expression, parse_simple_condition, ConditionErrorKind,
    RuntimeExpressionErrorKind,
};

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
        for (input, valid) in [
            (
                std::iter::repeat_n("true", length)
                    .collect::<Vec<_>>()
                    .join(" && "),
                true,
            ),
            (
                std::iter::repeat_n("false", length)
                    .collect::<Vec<_>>()
                    .join("||"),
                true,
            ),
            (
                std::iter::repeat_n("!true", length)
                    .collect::<Vec<_>>()
                    .join("&&"),
                true,
            ),
            (format!("$request.query.{}==null", "a".repeat(length)), true),
            (
                format!("$response.header.{}||true", "a".repeat(length)),
                true,
            ),
            (
                format!("$response.body#/{}==null", "a~0é".repeat(length)),
                true,
            ),
            (
                format!("$sourceDescriptions.s.{}!=null", "a./é".repeat(length)),
                true,
            ),
            (
                format!("'{}' == null", "a && b !(''é'') ".repeat(length)),
                true,
            ),
            (format!("'{}", "a''é".repeat(length)), false),
            (
                format!("$response.body#/{}~2 == null", "a".repeat(length)),
                false,
            ),
            (
                format!(
                    "{} trailing",
                    std::iter::repeat_n("true", length)
                        .collect::<Vec<_>>()
                        .join("||")
                ),
                false,
            ),
            (
                format!(
                    "{}&&",
                    std::iter::repeat_n("!true", length)
                        .collect::<Vec<_>>()
                        .join("&&")
                ),
                false,
            ),
            (format!("{} == false == true", "9".repeat(length)), false),
        ] {
            let budget = CALLS_PER_BYTE * input.len();
            let reset = ResetCallLimit;
            pest::set_call_limit(NonZeroUsize::new(budget));
            let result = parse_simple_condition(&input);
            drop(reset);
            if valid {
                assert!(
                    result.is_ok(),
                    "condition length {length}, budget {budget}: {result:?}"
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ConditionErrorKind::Syntax);
                assert!(
                    !error.message.contains("call limit"),
                    "condition exhausted budget {budget}: {error}"
                );
            }
        }
    }
    // A limit too low to enter Pest proves depth rejection precedes the parse.
    for (levels, kind) in [
        (32, ConditionErrorKind::Syntax),
        (33, ConditionErrorKind::NestingLimit),
    ] {
        let input = format!("{}true{}", "(".repeat(levels), ")".repeat(levels));
        let reset = ResetCallLimit;
        pest::set_call_limit(NonZeroUsize::new(1));
        let error = parse_simple_condition(&input).unwrap_err();
        drop(reset);
        assert_eq!(error.kind, kind);
        if levels == 33 {
            assert_eq!(error.byte_offset, 32);
        }
    }
    let input = format!("{}true{}", "!(".repeat(17), ")".repeat(17));
    let reset = ResetCallLimit;
    pest::set_call_limit(NonZeroUsize::new(1));
    let error = parse_simple_condition(&input).unwrap_err();
    drop(reset);
    assert_eq!(error.kind, ConditionErrorKind::NestingLimit);
    assert_eq!(error.byte_offset, 32);
}
