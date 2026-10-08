#![allow(clippy::unwrap_used, clippy::expect_used)] // Assertions on canonical parser rejection.

use arazzo_expr::{
    body_pointer_migration_hint, parse_runtime_expression, RuntimeExpressionError,
    RuntimeExpressionErrorKind,
};

const AMBIGUOUS_GUIDANCE: &str = "use an explicit JSON Pointer after '#'; choose the property tokens explicitly and escape '~' as '~0' and '/' as '~1'; this path cannot be converted automatically";

fn rejected_hint(input: &str) -> Option<String> {
    let error = parse_runtime_expression(input).expect_err(input);
    let before = error.clone();
    let display = error.to_string();
    let hint = body_pointer_migration_hint(input, &error);
    assert_eq!(error, before, "canonical error changed for {input}");
    assert_eq!(error.to_string(), display, "Display changed for {input}");
    assert_eq!(parse_runtime_expression(input).unwrap_err(), before);
    hint
}

#[test]
fn ordinary_members_offer_explicit_intent_guidance() {
    for prefix in ["$request.body", "$response.body", "$message.payload"] {
        for member in ["status", "a_1-B", "0"] {
            assert_eq!(
                rejected_hint(&format!("{prefix}.{member}")),
                Some(format!("use '{prefix}#/{member}'"))
            );
        }
        assert_eq!(
            rejected_hint(&format!("{prefix}.a.b")),
            Some(format!("for nested member access, use '{prefix}#/a/b'; for a literal property named 'a.b', use '{prefix}#/a.b'"))
        );
    }
}

#[test]
fn mixed_case_keywords_and_member_case_are_preserved() {
    for prefix in ["$ReQuEsT.BoDy", "$RESPONSE.body", "$message.PAYLOAD"] {
        assert_eq!(
            rejected_hint(&format!("{prefix}.Status")),
            Some(format!("use '{prefix}#/Status'"))
        );
        assert_eq!(
            rejected_hint(&format!("{prefix}.Parent.Child")),
            Some(format!("for nested member access, use '{prefix}#/Parent/Child'; for a literal property named 'Parent.Child', use '{prefix}#/Parent.Child'"))
        );
    }
}

#[test]
fn ambiguous_paths_never_invent_a_pointer() {
    for prefix in ["$request.body", "$response.body", "$message.payload"] {
        for suffix in [
            "[*]",
            ".items[*]",
            ".#",
            ".items.#(sku==\"A\")",
            ".items.#(sku==\"A\")#",
            "[?(@.active == true)]",
            "[0]",
            "['key']",
            "[\"key\"]",
            ".",
            "..a",
            ".a..b",
            ".a.",
            ".a~b",
            ".a/b",
            ".é",
            ".a.雪",
            ".a b",
        ] {
            assert_eq!(
                rejected_hint(&format!("{prefix}{suffix}")),
                Some(AMBIGUOUS_GUIDANCE.to_owned()),
                "{prefix}{suffix}"
            );
        }
    }
}

#[test]
fn valid_references_and_pointers_receive_no_hint() {
    // A stale error must not turn a valid expression into a migration candidate.
    let stale_error = parse_runtime_expression("$response.body.status").unwrap_err();
    for prefix in ["$request.body", "$response.body", "$message.payload"] {
        for suffix in ["", "#", "#/a.b", "#/a/b", "#/a~0b/a~1b", "#/é"] {
            let input = format!("{prefix}{suffix}");
            assert!(parse_runtime_expression(&input).is_ok(), "{input}");
            assert_eq!(body_pointer_migration_hint(&input, &stale_error), None);
        }
    }
}

#[test]
fn unrelated_errors_and_invalid_pointer_escapes_receive_no_hint() {
    for input in [
        "",
        "body.status",
        "{$response.body.status}",
        "$url.extra",
        "$inputs.é",
        "$inputs.x{tail",
        "$request.query.x{tail",
        "$response.header.X{tail",
        "$message.body.status",
        "$request.payload.status",
        "$response.bodyish.status",
        "$response.body#/a~2",
        "$request.body#/a~",
        "$message.payload#/é~2",
        "$response.body#bad",
        "$response.body garbage",
    ] {
        assert_eq!(rejected_hint(input), None, "{input}");
    }
}

#[test]
fn canonical_error_identity_and_display_remain_exact() {
    let input = "$response.body.status";
    let error = parse_runtime_expression(input).unwrap_err();
    assert_eq!(error.kind, RuntimeExpressionErrorKind::TrailingInput);
    assert_eq!(error.byte_range, 14..21);
    assert_eq!(
        error.to_string(),
        "invalid Runtime Expression at bytes 14..21: trailing input"
    );
    assert_eq!(
        rejected_hint(input),
        Some("use '$response.body#/status'".to_owned())
    );
}

#[test]
fn malformed_error_ranges_do_not_panic_or_suggest_a_pointer() {
    let input = "$response.body.é";
    for byte_range in [15..16, 16..17, 17..17, 14..100, 100..101] {
        let error = RuntimeExpressionError {
            kind: RuntimeExpressionErrorKind::TrailingInput,
            byte_range,
        };
        assert_eq!(body_pointer_migration_hint(input, &error), None);
    }
}
