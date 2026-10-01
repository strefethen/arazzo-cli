//! Syntax evidence for the accepted DEC-2 interpretation of vendored
//! spec/arazzo/v1.1.0.html Sections 5.8.11.1, 5.8.11.2, and 5.4.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use arazzo_expr::{parse_runtime_expression, parse_simple_condition, ConditionErrorKind};
use proptest::prelude::*;

#[test]
fn conformance_simple_condition_syntax_positive_evidence() {
    // A/B/C placeholders from DEC-2 are actual lowercase literals here.
    for input in [
        "$statusCode==200",
        "( $statusCode == 200 )",
        "\t(\r\n$statusCode\t==\n200\r) ",
        "true && false || null",
        "!(true == false)",
        "!true && false",
        "$request.query.x==true",
        "$response.header.X||$inputs.y",
        "$response.body.items[0].id",
        "true[0]",
        "null[12]",
        "'x'[ 12 ] . id",
        "$response.body[0][12].x.y",
        "(true).value",
        "true.value",
        "200",
        "-1",
        "2.5",
        "2e2",
        "0",
        "-0",
        "-0.50E+999",
        "2e-2",
        "true",
        "false",
        "null",
        "'It''s'",
        "'a\\'",
        "'a && b'",
        "'é 雪'",
        "'$inputs.x' == '$response.body'",
        "''",
        "''''",
        "'abc' < 'def'",
        "1<=2",
        "2>1",
        "2>=1",
        "1==1",
        "1!=2",
        "!$response.body.items[0] && $statusCode == 200",
        "$url",
        "$method",
        "$self",
        "$STATUSCODE",
        "$REQUEST.HEADER.X == 1",
        "$request.query. == null",
        "$request.path.id!=null",
        "$response.query.x <= 2",
        "$response.path.x > 2",
        "$request.body#/a~1b/é == null",
        "$response.body# == null",
        "$message.header.X||false",
        "$message.payload#/a[0]",
        "$inputs.foo.bar#/a",
        "$outputs.foo.bar#/a",
        "$steps.s.outputs.foo.bar#/a",
        "$WORKFLOWS.W.OUTPUTS.Foo#/a",
        "$sourceDescriptions.s.ref.with.dots/~2é == null",
        "$components.parameters.foo.bar",
        "$components.successActions.foo",
        "$components.failureActions.foo",
        "$inputs.x && $outputs.y && $inputs.x",
    ] {
        let parsed =
            parse_simple_condition(input).unwrap_or_else(|error| panic!("{input}: {error}"));
        assert_eq!(parsed.raw(), input);
        for operand in parsed.runtime_expressions() {
            assert_eq!(
                &parse_runtime_expression(operand.raw()).unwrap(),
                operand,
                "{input}"
            );
        }
    }
    let input = String::from("'$inputs.fake' == $inputs.x || $STATUSCODE == 200");
    let parsed = parse_simple_condition(&input).unwrap();
    assert_eq!(parsed.raw().as_ptr(), input.as_ptr());
    assert_eq!(
        parsed
            .runtime_expressions()
            .iter()
            .map(|operand| operand.raw())
            .collect::<Vec<_>>(),
        vec!["$inputs.x", "$STATUSCODE"]
    );
    assert_eq!(
        parsed.runtime_expressions()[0].raw().as_ptr(),
        input[18..].as_ptr()
    );
}

#[test]
fn conformance_simple_condition_syntax_negative_evidence() {
    for input in [
        "",
        " ",
        "$statusCode = = 200",
        "!true == false",
        "true == false == null",
        "!!true",
        "!",
        "$response.header.X||Y",
        "$response.body.",
        "true[-1]",
        "true[01]",
        "true['a']",
        "true[]",
        "true[+1]",
        "true[1.0]",
        "+200",
        "0200",
        ".5",
        "5.",
        "1e",
        "1e+",
        "--1",
        "\"apple\"",
        "$inputs.x contains 'a'",
        "$inputs.x in [1]",
        "$inputs.x matches 'a'",
        "true trailing",
        "'é' trailing",
        "'unterminated",
        "'a' 'b'",
        "truefalse",
        "nullx",
        "True",
        "FALSE",
        "NULL",
        "(true",
        "true)",
        "true & & false",
        "true | | false",
        "$status Code==200",
        "$ inputs.x",
        "2 e2",
        "tru e",
        "true &&",
        "||true",
        "$message.query.x",
        "$request.payload",
        "$response.body#/a~2 == null",
        "$inputs.é",
        "$response.header.é",
        "$sourceDescriptions.s. == 1",
        "true.foo?",
        "$inputs.x{y",
        "$request.query.x{y == 1",
        "true[1 2]",
    ] {
        let error = parse_simple_condition(input).expect_err(input);
        assert_eq!(error.kind, ConditionErrorKind::Syntax, "{input}: {error}");
        assert!(error.byte_offset <= input.len(), "{input}");
        assert!(input.is_char_boundary(error.byte_offset), "{input}");
        assert!(!error.message.is_empty());
    }
}

fn operand_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec!["$url", "$METHOD", "$statusCode", "$self"])
            .prop_map(str::to_owned),
        (
            "[a-zA-Z0-9_-]{1,30}",
            prop::sample::select(vec![
                "$inputs.",
                "$OUTPUTS.",
                "$steps.s.outputs.",
                "$WORKFLOWS.w.INPUTS.",
                "$components.parameters."
            ])
        )
            .prop_map(|(name, prefix)| format!("{prefix}{name}.dotted")),
        (
            "[a-zA-Z0-9_.%/+~-]{0,30}",
            prop::sample::select(vec!["$request.query.", "$RESPONSE.PATH."])
        )
            .prop_map(|(name, prefix)| format!("{prefix}{name}")),
        (
            "[a-zA-Z0-9_.$%+~-]{1,30}",
            prop::sample::select(vec![
                "$response.header.",
                "$REQUEST.HEADER.",
                "$message.header."
            ])
        )
            .prop_map(|(name, prefix)| format!("{prefix}{name}")),
        (
            prop::sample::select(vec![
                "$response.body",
                "$REQUEST.BODY",
                "$message.payload",
                "$inputs.x",
                "$steps.s.outputs.x",
                "$workflows.w.outputs.x"
            ]),
            prop::collection::vec(
                prop::sample::select(vec!["a", "é", "~0", "~1", "", "x.y"]),
                0..8
            )
        )
            .prop_map(|(prefix, tokens)| format!("{prefix}#/{}", tokens.join("/"))),
        "[a-zA-Z0-9_./$%+~-]{1,30}"
            .prop_map(|reference| format!("$SOURCEdescriptions.s.{reference}é")),
    ]
}

proptest! {
    #[test]
    fn generated_conditions_preserve_canonical_operands(
        operands in prop::collection::vec(operand_strategy(), 1..20),
        separator in prop::sample::select(vec![" && ", "||"]),
        operator in prop::sample::select(vec!["==", "!=", "<", "<=", ">", ">="]),
    ) {
        let input = operands.iter().map(|operand| format!("({operand}{operator}null)")).collect::<Vec<_>>().join(separator);
        let parsed = parse_simple_condition(&input).unwrap();
        prop_assert_eq!(parsed.runtime_expressions().len(), operands.len());
        for (operand, original) in parsed.runtime_expressions().iter().zip(&operands) {
            prop_assert_eq!(operand.raw(), original);
            prop_assert_eq!(operand, &parse_runtime_expression(operand.raw()).unwrap());
        }
    }
}
