#![allow(clippy::unwrap_used, clippy::expect_used)] // Assertions on known valid/rejected syntax fixtures.

use std::ops::Range;

use arazzo_expr::{
    classify_value_string, parse_runtime_expression, ExpressionStringError,
    ExpressionStringErrorKind, ValueStringSyntax,
};

fn assert_template(input: &str, spans: &[(Range<usize>, &str)]) {
    let ValueStringSyntax::Template(expressions) = classify_value_string(input).unwrap() else {
        panic!("expected template: {input}");
    };
    assert_eq!(expressions.len(), spans.len(), "{input}");
    for (embedded, (full, raw)) in expressions.iter().zip(spans) {
        assert_eq!(embedded.full_range(), *full, "{input}");
        assert_eq!(embedded.inner_range(), full.start + 1..full.end - 1);
        assert_eq!(&input[embedded.full_range()], format!("{{{raw}}}"));
        assert_eq!(&input[embedded.inner_range()], *raw);
        assert_eq!(embedded.expression().raw(), *raw);
        assert_eq!(
            embedded.expression(),
            &parse_runtime_expression(raw).unwrap()
        );
    }
    assert_eq!(expressions.clone(), expressions);
}

#[test]
fn complete_runtime_expressions_take_precedence() {
    for input in [
        "$url",
        "$method",
        "$statusCode",
        "$self",
        "$inputs.pet_id",
        "$outputs.result",
        "$steps.s.outputs.pet_id",
        "$workflows.w.inputs.x",
        "$sourceDescriptions.s.ref",
        "$components.parameters.x",
        "$REQUEST.HEADER.Accept",
        "$response.query.q",
        "$message.header.h",
    ] {
        assert_eq!(
            classify_value_string(input).unwrap(),
            ValueStringSyntax::Expression(parse_runtime_expression(input).unwrap()),
            "{input}",
        );
    }
}

#[test]
fn accepted_literal_braces_dollars_and_empty_text() {
    for input in [
        "",
        "plain text",
        "$",
        "$USD",
        "pay in $USD",
        "hi $inputs.name",
        "{name}",
        "a{b",
        "a}b",
        "}",
        "{}",
        "{{}",
        "{ $inputs.x}",
        "é🙂 {name} $USD",
        "$url tail",
        "$sourceDescriptions.s.ref}tail",
    ] {
        assert_eq!(
            classify_value_string(input).unwrap(),
            ValueStringSyntax::Literal(input)
        );
    }
}

#[test]
fn accepted_templates_are_ordered_exact_borrowed_spans() {
    assert_template("{$inputs.pet_id}", &[(0..16, "$inputs.pet_id")]);
    let json = r#"{"petOrder": {"petId": "{$inputs.pet_id}"}}"#;
    let start = json.find("{$").unwrap();
    assert_template(json, &[(start..start + 16, "$inputs.pet_id")]);
    assert_template("a{$url}b{$method}c", &[(1..7, "$url"), (8..17, "$method")]);
    assert_template("{$url}{$method}", &[(0..6, "$url"), (6..15, "$method")]);
    assert_template(
        "é{$url}🙂{$method}終",
        &[(2..8, "$url"), (12..21, "$method")],
    );
    assert_template("{{$url}}}", &[(1..7, "$url")]);
}

#[test]
fn first_raw_closing_brace_is_the_only_delimiter() {
    let input = "{$sourceDescriptions.s.ref}tail";
    assert_template(input, &[(0..27, "$sourceDescriptions.s.ref")]);
    assert_eq!(&input[27..], "tail");
    for input in [
        "$sourceDescriptions.s.ref}tail",
        "$sourceDescriptions.s.ref{tail",
        "$inputs.a{b",
        "$request.query.a}tail",
        "$request.path.a{tail",
        "$request.header.a}tail",
        "$response.body#/a}tail",
        "$message.payload#/a{tail",
    ] {
        assert!(parse_runtime_expression(input).is_err(), "{input}");
    }
    assert_template("{$url}}tail", &[(0..6, "$url")]);
}

#[test]
fn escaped_document_text_contains_no_raw_delimiter() {
    for raw in [
        r"$sourceDescriptions.s.ref\u007Dtail",
        r"$sourceDescriptions.s.ref\u007Btail",
        r"$request.query.x\u007Dtail",
    ] {
        let input = format!("前{{{raw}}}後");
        assert_template(&input, &[("前".len().."前".len() + raw.len() + 2, raw)]);
    }
}

fn assert_error(input: &str, kind: ExpressionStringErrorKind, range: Range<usize>, message: &str) {
    let error = classify_value_string(input).expect_err(input);
    assert_eq!(
        error,
        ExpressionStringError {
            kind,
            byte_range: range.clone(),
            message: message.into()
        }
    );
    assert!(input.is_char_boundary(range.start));
    assert!(input.is_char_boundary(range.end));
    assert_eq!(
        error.to_string(),
        format!(
            "invalid expression string at bytes {}..{}: {message}",
            range.start, range.end
        )
    );
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn missing_delimiters_report_syntax_without_partial_results() {
    for (input, range) in [
        ("{$inputs.pet_id", 0..15),
        ("é{$inputs.pet_id", 2..17),
        ("{$url}🙂{$", 10..12),
    ] {
        assert_error(
            input,
            ExpressionStringErrorKind::Syntax,
            range,
            "embedded Runtime Expression is missing closing '}'",
        );
    }
}

#[test]
fn embedded_errors_preserve_canonical_messages_and_shift_exact_ranges() {
    for (input, range, message) in [
        (
            "{$}",
            2..2,
            "invalid Runtime Expression at bytes 1..1: unknown namespace",
        ),
        (
            "{$name}",
            2..6,
            "invalid Runtime Expression at bytes 1..5: unknown namespace",
        ),
        (
            "{$inputs.a{b}",
            10..12,
            "invalid Runtime Expression at bytes 9..11: trailing input",
        ),
        (
            "é{$inputs.é}",
            11..13,
            "invalid Runtime Expression at bytes 8..10: invalid identifier",
        ),
        (
            "{$url}🙂{$response.body.status}",
            25..32,
            "invalid Runtime Expression at bytes 14..21: trailing input",
        ),
        (
            "{$name}{$inputs.x",
            2..6,
            "invalid Runtime Expression at bytes 1..5: unknown namespace",
        ),
        (
            "{$inputs.x#/é~2}",
            14..16,
            "invalid Runtime Expression at bytes 13..15: invalid JSON Pointer escape",
        ),
    ] {
        assert_error(
            input,
            ExpressionStringErrorKind::EmbeddedRuntimeExpression,
            range,
            message,
        );
    }
}

#[test]
fn dec5_body_and_payload_classification_preserves_literal_fields() {
    for prefix in [
        "$request.body",
        "$response.body",
        "$message.payload",
        "$REQUEST.BODY",
        "$Response.Body",
        "$MESSAGE.PAYLOAD",
    ] {
        for suffix in ["", "#", "#/a.b", "#/a/b", "#/a~0b", "#/a~1b", "#/é"] {
            let input = format!("{prefix}{suffix}");
            assert_eq!(
                classify_value_string(&input).unwrap(),
                ValueStringSyntax::Expression(parse_runtime_expression(&input).unwrap())
            );
            let template = format!("{{{input}}}");
            assert_template(&template, &[(0..template.len(), &input)]);
        }
        for suffix in [".status", ".nested.value", "[0]"] {
            let input = format!("{prefix}{suffix}");
            assert_eq!(
                classify_value_string(&input).unwrap(),
                ValueStringSyntax::Literal(&input)
            );
            let template = format!("é{{{input}}}後");
            let canonical = parse_runtime_expression(&input).unwrap_err();
            assert_error(
                &template,
                ExpressionStringErrorKind::EmbeddedRuntimeExpression,
                3 + canonical.byte_range.start..3 + canonical.byte_range.end,
                &canonical.to_string(),
            );
        }
    }
    assert_eq!(
        classify_value_string("$USD").unwrap(),
        ValueStringSyntax::Literal("$USD")
    );
}

#[test]
fn long_and_adversarial_inputs_keep_order_and_first_error() {
    let literal = "é{}$USD".repeat(20_000);
    assert_eq!(
        classify_value_string(&literal).unwrap(),
        ValueStringSyntax::Literal(&literal)
    );

    let input = format!("{literal}{}", "{$url}".repeat(10_000));
    let ValueStringSyntax::Template(expressions) = classify_value_string(&input).unwrap() else {
        panic!("expected template");
    };
    assert_eq!(expressions.len(), 10_000);
    for (index, embedded) in expressions.iter().enumerate() {
        let start = literal.len() + index * 6;
        assert_eq!(embedded.full_range(), start..start + 6);
        assert_eq!(embedded.expression().raw(), "$url");
    }

    let missing = format!("{literal}{{$inputs.x{}", "{".repeat(100_000));
    assert_error(
        &missing,
        ExpressionStringErrorKind::Syntax,
        literal.len()..missing.len(),
        "embedded Runtime Expression is missing closing '}'",
    );
    let invalid = format!("{literal}{{$name}}{}", "{$url}".repeat(10_000));
    assert_error(
        &invalid,
        ExpressionStringErrorKind::EmbeddedRuntimeExpression,
        literal.len() + 2..literal.len() + 6,
        "invalid Runtime Expression at bytes 1..5: unknown namespace",
    );
}
