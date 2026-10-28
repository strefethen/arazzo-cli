#![allow(clippy::unwrap_used, clippy::expect_used)] // Known syntax fixtures and error assertions.

use std::error::Error;

use arazzo_expr::{
    classify_value_string, local_step_output_references, parse_runtime_expression,
    parse_simple_condition, ExpressionStringError, ExpressionStringErrorKind,
    LocalStepOutputReference, StepReferenceError, StepReferenceSyntax,
};

use StepReferenceSyntax::{
    EmbeddedCondition, LiteralOrExpression, RequiredExpression, SimpleCondition,
};

fn assert_references(input: &str, syntax: StepReferenceSyntax, expected: &[(&str, &str)]) {
    let actual = local_step_output_references(input, syntax)
        .unwrap_or_else(|error| panic!("{syntax:?}: {input}: {error}"));
    let expected: Vec<_> = expected
        .iter()
        .map(|&(step_id, output_name)| LocalStepOutputReference {
            step_id,
            output_name,
        })
        .collect();
    assert_eq!(actual, expected, "{syntax:?}: {input}");
    // Each returned name is a slice of the original UTF-8 input, not a copy.
    for reference in actual {
        for name in [reference.step_id, reference.output_name] {
            let offset = name.as_ptr() as usize - input.as_ptr() as usize;
            assert_eq!(input.get(offset..offset + name.len()), Some(name));
        }
    }
}

#[test]
fn public_four_mode_zero_one_many_matrix() {
    for (syntax, input, expected) in [
        (
            LiteralOrExpression,
            "ordinary $steps.s.outputs.o text",
            &[][..],
        ),
        (
            LiteralOrExpression,
            "$steps.S-1.outputs.Output.name",
            &[("S-1", "Output.name")][..],
        ),
        (
            LiteralOrExpression,
            "{$steps.s.outputs.o}",
            &[("s", "o")][..],
        ),
        (
            LiteralOrExpression,
            "é{$steps.s.outputs.o}{$steps.T.outputs.p}🙂{$steps.s.outputs.o}",
            &[("s", "o"), ("T", "p"), ("s", "o")][..],
        ),
        (RequiredExpression, "$statusCode", &[][..]),
        (
            RequiredExpression,
            "$steps.S-1.outputs.Output.name",
            &[("S-1", "Output.name")][..],
        ),
        (SimpleCondition, "'$steps.s.outputs.o' == '雪'", &[][..]),
        (
            SimpleCondition,
            "$steps.s.outputs.o == '雪'",
            &[("s", "o")][..],
        ),
        (
            SimpleCondition,
            "$steps.s.outputs.o == $steps.T.outputs.p || $steps.s.outputs.o",
            &[("s", "o"), ("T", "p"), ("s", "o")][..],
        ),
        (EmbeddedCondition, "$steps.s.outputs.o", &[][..]),
        (EmbeddedCondition, "{$steps.s.outputs.o}", &[("s", "o")][..]),
        (
            EmbeddedCondition,
            "雪{$steps.s.outputs.o}{$steps.T.outputs.p}🙂{$steps.s.outputs.o}",
            &[("s", "o"), ("T", "p"), ("s", "o")][..],
        ),
    ] {
        assert_references(input, syntax, expected);
    }

    for input in [
        "{$steps.s.outputs.o}",
        "$steps.s.outputs.o $steps.t.outputs.p",
    ] {
        assert!(matches!(
            local_step_output_references(input, RequiredExpression),
            Err(StepReferenceError::RuntimeExpression(_))
        ));
    }
    assert!(matches!(
        local_step_output_references("{$steps.s.outputs.o}", SimpleCondition),
        Err(StepReferenceError::SimpleCondition(_))
    ));
}

#[test]
fn every_non_step_namespace_and_source_form_is_filtered_in_all_modes() {
    for expression in [
        "$url",
        "$method",
        "$statusCode",
        "$self",
        "$request.header.H",
        "$request.query.q",
        "$request.path.p",
        "$request.body#/p",
        "$response.header.H",
        "$response.query.q",
        "$response.path.p",
        "$response.body#/p",
        "$message.header.H",
        "$message.payload#/p",
        "$inputs.x",
        "$outputs.x",
        "$workflows.w.inputs.x",
        "$workflows.w.outputs.x",
        "$sourceDescriptions.s.ref.with.dots",
        "$components.parameters.x",
        "$components.successActions.x",
        "$components.failureActions.x",
    ] {
        assert_references(expression, RequiredExpression, &[]);
        assert_references(expression, LiteralOrExpression, &[]);
        assert_references(expression, SimpleCondition, &[]);
        let embedded = format!("{{{expression}}}");
        assert_references(&embedded, EmbeddedCondition, &[]);
        assert_references(&embedded, LiteralOrExpression, &[]);
        let mixed = format!("{{{expression}}}{{$steps.real.outputs.unresolved}}");
        assert_references(&mixed, EmbeddedCondition, &[("real", "unresolved")]);
        assert_references(&mixed, LiteralOrExpression, &[("real", "unresolved")]);
    }
}

#[test]
fn exact_names_case_insensitive_keywords_and_valid_pointers() {
    for expression in [
        "$steps.A_9-b.outputs.O.name-1",
        "$StEpS.A_9-b.OuTpUtS.O.name-1#",
        "$STEPS.A_9-b.OUTPUTS.O.name-1#/a~0b/~1/é/雪",
    ] {
        assert_references(expression, RequiredExpression, &[("A_9-b", "O.name-1")]);
        assert_references(expression, LiteralOrExpression, &[("A_9-b", "O.name-1")]);
        assert_references(expression, SimpleCondition, &[("A_9-b", "O.name-1")]);
        let embedded = format!("前{{{expression}}}後");
        assert_references(&embedded, EmbeddedCondition, &[("A_9-b", "O.name-1")]);
        assert_references(&embedded, LiteralOrExpression, &[("A_9-b", "O.name-1")]);
    }
}

#[test]
fn simple_parser_owns_quoted_literals_and_postfix_operand_recognition() {
    for input in [
        "'$steps.fake.outputs.o'",
        "'escaped ''$steps.fake.outputs.o'' text' == '$steps.other.outputs.o'",
        "'{$steps.fake.outputs.o}' == '雪'",
    ] {
        assert_references(input, SimpleCondition, &[]);
    }
    assert_references(
        "'$steps.fake.outputs.o' == $steps.Real.outputs.O.name || ($steps.Other.outputs.Values)[0]",
        SimpleCondition,
        &[("Real", "O.name"), ("Other", "Values")],
    );
}

#[test]
fn embedded_mode_keeps_native_conditions_braces_and_unbraced_dollars_opaque() {
    for input in [
        "",
        "^a{2,4}$",
        "{name}",
        "a{b",
        "a}b",
        "{}",
        "}",
        "$[?(@.text == '{name}') ]",
        "//*[text() = '{name}']",
        "$steps.s.outputs.o",
        "'$steps.s.outputs.o'",
        "{$stepsx.s.outputs.o text",
    ] {
        // A literal '{' followed by '$' is always reserved, even in native text.
        if input == "{$stepsx.s.outputs.o text" {
            assert!(local_step_output_references(input, EmbeddedCondition).is_err());
        } else {
            assert_references(input, EmbeddedCondition, &[]);
        }
    }
    assert_references(
        "^a{2,4}$ $[?(@.x == '{x}')] //x{native} $steps.ignored.outputs.o {$steps.Real.outputs.Value}",
        EmbeddedCondition,
        &[("Real", "Value")],
    );
}

#[test]
fn first_raw_closing_brace_matches_expression_string_boundaries() {
    for input in [
        "{{$steps.s.outputs.o}}}tail",
        "{$steps.s.outputs.o#/a}unbraced $steps.ignored.outputs.o}",
        r"{$steps.s.outputs.o#/a\u007Dtail}tail",
    ] {
        assert_references(input, LiteralOrExpression, &[("s", "o")]);
        assert_references(input, EmbeddedCondition, &[("s", "o")]);
    }
    for input in [
        "{$steps.s.outputs.o#/a}tail{$steps.t.outputs.p}",
        "{$sourceDescriptions.s.ref}tail{$steps.t.outputs.p}",
    ] {
        let expected = if input.starts_with("{$sourceDescriptions") {
            vec![("t", "p")]
        } else {
            vec![("s", "o"), ("t", "p")]
        };
        assert_references(input, LiteralOrExpression, &expected);
        assert_references(input, EmbeddedCondition, &expected);
    }
}

#[test]
fn malformed_required_syntax_preserves_complete_canonical_errors() {
    for input in [
        "",
        "é",
        "{$steps.s.outputs.o}",
        "$steps.",
        "$steps.a.b.outputs.x",
        "$steps.s.inputs.o",
        "$steps.s.outputs.",
        "$steps.é.outputs.o",
        "$steps.s.outputs.é",
        "$steps.s.outputs.o#/é~2",
        "$steps.s.outputs.o#/a~",
        "$steps.s.outputs.o tail",
        "$steps.s.outputs.o}tail",
        "$stepsx.s.outputs.o",
        "$steps.s.outputs.o[0]",
    ] {
        let canonical = parse_runtime_expression(input).unwrap_err();
        let error = local_step_output_references(input, RequiredExpression).unwrap_err();
        let StepReferenceError::RuntimeExpression(actual) = &error else {
            panic!("wrong error variant: {error:?}");
        };
        assert_eq!(actual, &canonical, "{input}");
        assert_eq!(error.to_string(), canonical.to_string());
        assert_eq!(error.source().unwrap().to_string(), canonical.to_string());
    }
    // Malformed unbraced lookalikes remain literal in the literal-capable mode.
    for input in [
        "$steps.a.b.outputs.x",
        "$steps.s.outputs.",
        "$steps.s.outputs.o tail",
    ] {
        assert_references(input, LiteralOrExpression, &[]);
    }
}

#[test]
fn malformed_reserved_candidates_return_first_error_and_no_partial_occurrences() {
    for input in [
        "{$steps.s.outputs.o",
        "é{$steps.s.outputs.o}🙂{$",
        "{$}",
        "{$steps.a.b.outputs.x}",
        "{$steps.s.inputs.o}",
        "{$steps.s.outputs.}",
        "{$steps.s.outputs.o#/é~2}",
        "{$steps.s.outputs.o}🙂{$steps.t.outputs.p tail}",
        "{$steps.s.outputs.o{tail}after}",
        "{$steps.s.outputs.o}🙂{$name}{$steps.t.outputs.p}",
    ] {
        let canonical = classify_value_string(input).unwrap_err();
        for syntax in [LiteralOrExpression, EmbeddedCondition] {
            let error = local_step_output_references(input, syntax).unwrap_err();
            let StepReferenceError::ExpressionString(actual) = &error else {
                panic!("wrong error variant: {error:?}");
            };
            assert_eq!(actual, &canonical, "{syntax:?}: {input}");
            assert_eq!(error.to_string(), canonical.to_string());
            assert_eq!(error.source().unwrap().to_string(), canonical.to_string());
        }
    }
}

#[test]
fn embedded_error_kind_range_and_message_are_unchanged() {
    let fixtures = [
        (
            "é{$",
            ExpressionStringErrorKind::Syntax,
            2..4,
            "embedded Runtime Expression is missing closing '}'",
        ),
        (
            "é{$name}",
            ExpressionStringErrorKind::EmbeddedRuntimeExpression,
            4..8,
            "invalid Runtime Expression at bytes 1..5: unknown namespace",
        ),
    ];
    for (input, kind, byte_range, message) in fixtures {
        for syntax in [LiteralOrExpression, EmbeddedCondition] {
            let StepReferenceError::ExpressionString(error) =
                local_step_output_references(input, syntax).unwrap_err()
            else {
                panic!("expected expression-string error");
            };
            assert_eq!(
                error,
                ExpressionStringError {
                    kind,
                    byte_range: byte_range.clone(),
                    message: message.to_owned(),
                }
            );
        }
    }
}

#[test]
fn malformed_simple_conditions_preserve_canonical_errors_without_partial_results() {
    let nested = format!("{}true{}", "(".repeat(33), ")".repeat(33));
    for input in [
        "",
        "$steps.s.outputs.o ==",
        "$steps.s.outputs.o && $steps.t.outputs.",
        &nested,
    ] {
        let canonical = parse_simple_condition(input).unwrap_err();
        let error = local_step_output_references(input, SimpleCondition).unwrap_err();
        let StepReferenceError::SimpleCondition(actual) = &error else {
            panic!("expected simple-condition error");
        };
        assert_eq!(actual, &canonical, "{input}");
        assert_eq!(error.to_string(), canonical.to_string());
        assert_eq!(error.source().unwrap().to_string(), canonical.to_string());
    }
}
