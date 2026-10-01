//! Shared-grammar syntax only; evaluation policy belongs to the later evaluator.

use std::fmt;

use pest::{iterators::Pair, Parser};

use crate::runtime_expression::{
    map_runtime_expression, ParsedRuntimeExpression, Rule, RuntimeExpressionParser,
};

/// Stable simple-condition error categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionErrorKind {
    Syntax,
    NestingLimit,
    /// Reserved for evaluation; never returned by the syntax parser.
    InvalidEvaluation,
}

/// An owned error at a UTF-8 byte boundary in the original condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionError {
    pub kind: ConditionErrorKind,
    pub byte_offset: usize,
    pub message: String,
}

impl fmt::Display for ConditionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "condition at byte {}: {}",
            self.byte_offset, self.message
        )
    }
}

impl std::error::Error for ConditionError {}

/// Borrowed, fully consumed condition syntax and its ordered canonical operands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSimpleCondition<'a> {
    raw: &'a str,
    expressions: Vec<ParsedRuntimeExpression<'a>>,
    // The downstream evaluator consumes this tree without reinterpreting syntax.
    #[allow(dead_code)]
    pub(super) syntax: Syntax<'a>,
}

impl<'a> ParsedSimpleCondition<'a> {
    pub fn raw(&self) -> &'a str {
        self.raw
    }

    pub fn runtime_expressions(&self) -> &[ParsedRuntimeExpression<'a>] {
        &self.expressions
    }
}

// Keep literals and postfix fields as source text: converting numbers/indices,
// unescaping strings, and resolving properties are evaluation decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Syntax<'a> {
    Or(Vec<Self>),
    And(Vec<Self>),
    Not(Box<Self>),
    Comparison {
        left: Box<Self>,
        operator: &'a str,
        right: Box<Self>,
    },
    Postfix {
        primary: Box<Self>,
        accesses: Vec<Access<'a>>,
    },
    Group(Box<Self>),
    RuntimeExpression(usize),
    Boolean(&'a str),
    Null,
    Number(&'a str),
    String(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Access<'a> {
    Property(&'a str),
    Index(&'a str),
}

/// Parse the accepted simple Criterion grammar without evaluating any operand.
pub fn parse_simple_condition(input: &str) -> Result<ParsedSimpleCondition<'_>, ConditionError> {
    check_nesting(input)?;
    let mut pairs = RuntimeExpressionParser::parse(Rule::condition, input).map_err(|error| {
        let byte_offset = match error.location {
            pest::error::InputLocation::Pos(offset)
            | pest::error::InputLocation::Span((offset, _)) => offset,
        };
        ConditionError {
            kind: ConditionErrorKind::Syntax,
            byte_offset,
            message: error.to_string(),
        }
    })?;
    // A successful entry always contains the one or_expr and EOI.
    #[allow(clippy::expect_used)]
    let expression = pairs
        .next()
        .expect("condition entry")
        .into_inner()
        .next()
        .expect("or_expr");
    let mut expressions = Vec::new();
    let syntax = map_syntax(expression, &mut expressions);
    Ok(ParsedSimpleCondition {
        raw: input,
        expressions,
        syntax,
    })
}

/// Linear punctuation/quote pre-scan, independent of Runtime Expression syntax.
/// A frame holds the active unary operands at one group level. Unary lifetime
/// extends over its complete postfix (including grouped primaries and indices)
/// and ends at the next operator or enclosing group's close.
fn check_nesting(input: &str) -> Result<(), ConditionError> {
    const LIMIT: usize = 32;
    let mut unary_frames = vec![0usize];
    let mut depth = 0usize;
    let mut characters = input.char_indices().peekable();
    let mut quoted = false;
    while let Some((offset, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
            } else {
                quoted = !quoted;
            }
            continue;
        }
        if quoted {
            continue;
        }
        match character {
            '(' => {
                depth += 1;
                if depth > LIMIT {
                    return Err(nesting_error(offset));
                }
                unary_frames.push(0);
            }
            '!' if !characters.peek().is_some_and(|(_, next)| *next == '=') => {
                depth += 1;
                if depth > LIMIT {
                    return Err(nesting_error(offset));
                }
                if let Some(unaries) = unary_frames.last_mut() {
                    *unaries += 1;
                }
            }
            ')' => {
                if unary_frames.len() > 1 {
                    depth -= unary_frames.pop().unwrap_or(0) + 1;
                } else {
                    // Unbalanced syntax is Pest's responsibility.
                    end_unary_operand(&mut unary_frames, &mut depth);
                }
            }
            '=' | '!' | '<' | '>' | '&' | '|' => {
                end_unary_operand(&mut unary_frames, &mut depth);
            }
            _ => {}
        }
    }
    Ok(())
}

fn end_unary_operand(frames: &mut [usize], depth: &mut usize) {
    if let Some(unaries) = frames.last_mut() {
        *depth -= *unaries;
        *unaries = 0;
    }
}

fn nesting_error(byte_offset: usize) -> ConditionError {
    ConditionError {
        kind: ConditionErrorKind::NestingLimit,
        byte_offset,
        message: "simple condition nesting exceeds 32 levels".to_owned(),
    }
}

#[allow(clippy::expect_used)] // The shared grammar fixes every successful pair's children.
fn map_syntax<'a>(
    pair: Pair<'a, Rule>,
    expressions: &mut Vec<ParsedRuntimeExpression<'a>>,
) -> Syntax<'a> {
    let rule = pair.as_rule();
    let raw = pair.as_str();
    match rule {
        Rule::or_expr | Rule::and_expr => {
            let children: Vec<_> = pair
                .into_inner()
                .filter(|child| !matches!(child.as_rule(), Rule::or_operator | Rule::and_operator))
                .map(|child| map_syntax(child, expressions))
                .collect();
            if children.len() == 1 {
                children.into_iter().next().expect("one child")
            } else if rule == Rule::or_expr {
                Syntax::Or(children)
            } else {
                Syntax::And(children)
            }
        }
        Rule::unary => {
            let mut children = pair.into_inner();
            let first = children.next().expect("unary operand");
            if first.as_rule() == Rule::not_operator {
                Syntax::Not(Box::new(map_syntax(
                    children.next().expect("postfix operand"),
                    expressions,
                )))
            } else {
                map_syntax(first, expressions)
            }
        }
        Rule::comparison => {
            let mut children = pair.into_inner();
            let left = map_syntax(children.next().expect("comparison lhs"), expressions);
            if let Some(operator) = children.next() {
                Syntax::Comparison {
                    left: Box::new(left),
                    operator: operator.as_str(),
                    right: Box::new(map_syntax(
                        children.next().expect("comparison rhs"),
                        expressions,
                    )),
                }
            } else {
                left
            }
        }
        Rule::postfix => {
            let mut children = pair.into_inner();
            let primary = map_syntax(children.next().expect("primary"), expressions);
            let accesses: Vec<_> = children
                .map(|access| {
                    let rule = access.as_rule();
                    let field = access.into_inner().next().expect("postfix field").as_str();
                    if rule == Rule::property {
                        Access::Property(field)
                    } else {
                        Access::Index(field)
                    }
                })
                .collect();
            if accesses.is_empty() {
                primary
            } else {
                Syntax::Postfix {
                    primary: Box::new(primary),
                    accesses,
                }
            }
        }
        Rule::group => Syntax::Group(Box::new(map_syntax(
            pair.into_inner().next().expect("group expression"),
            expressions,
        ))),
        Rule::c_expression => {
            let index = expressions.len();
            expressions.push(map_runtime_expression(pair));
            Syntax::RuntimeExpression(index)
        }
        Rule::boolean => Syntax::Boolean(raw),
        Rule::null => Syntax::Null,
        Rule::number => Syntax::Number(raw),
        Rule::string => Syntax::String(raw),
        _ => unreachable!("condition syntax rule from the shared grammar"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tree(input: &str) -> Syntax<'_> {
        parse_simple_condition(input).unwrap().syntax
    }

    #[test]
    fn precedence_and_flat_ordered_chains() {
        use Syntax::*;
        assert_eq!(
            tree("true && false || null"),
            Or(vec![And(vec![Boolean("true"), Boolean("false")]), Null])
        );
        assert_eq!(
            tree("true || false && null"),
            Or(vec![Boolean("true"), And(vec![Boolean("false"), Null])])
        );
        assert_eq!(
            tree("1 == 2 && 3 < 4"),
            And(vec![
                Comparison {
                    left: Box::new(Number("1")),
                    operator: "==",
                    right: Box::new(Number("2"))
                },
                Comparison {
                    left: Box::new(Number("3")),
                    operator: "<",
                    right: Box::new(Number("4"))
                },
            ])
        );
        assert_eq!(
            tree("!true && false"),
            And(vec![Not(Box::new(Boolean("true"))), Boolean("false")])
        );
        assert_eq!(
            tree("!(true == false)"),
            Not(Box::new(Group(Box::new(Comparison {
                left: Box::new(Boolean("true")),
                operator: "==",
                right: Box::new(Boolean("false")),
            }))))
        );
        assert_eq!(
            tree("$response.body.items[0].id == 2e999"),
            Comparison {
                left: Box::new(Postfix {
                    primary: Box::new(RuntimeExpression(0)),
                    accesses: vec![
                        Access::Property("items"),
                        Access::Index("0"),
                        Access::Property("id")
                    ]
                }),
                operator: "==",
                right: Box::new(Number("2e999")),
            }
        );
        assert_eq!(
            tree("true || false || null"),
            Or(vec![Boolean("true"), Boolean("false"), Null])
        );
        assert_eq!(
            tree("true && false && null"),
            And(vec![Boolean("true"), Boolean("false"), Null])
        );
        let parsed = parse_simple_condition("$inputs.foo.bar == $steps.s.outputs.x.y").unwrap();
        assert_eq!(parsed.expressions[0].raw(), "$inputs.foo.bar");
        assert_eq!(parsed.expressions[1].raw(), "$steps.s.outputs.x.y");
        assert_eq!(tree("'It''s'"), String("'It''s'"));
        assert_eq!(tree("-0.50E+999"), Number("-0.50E+999"));
    }

    #[test]
    fn simultaneous_group_and_unary_boundaries_and_offsets() {
        let groups = |count| format!("{}true{}", "(".repeat(count), ")".repeat(count));
        assert!(parse_simple_condition(&groups(32)).is_ok());
        assert_eq!(
            parse_simple_condition(&groups(33)).unwrap_err(),
            nesting_error(32)
        );
        let mixed = |count| format!("{}true{}", "!(".repeat(count), ")".repeat(count));
        assert!(parse_simple_condition(&mixed(16)).is_ok());
        assert_eq!(
            parse_simple_condition(&mixed(17)).unwrap_err(),
            nesting_error(32)
        );
        let unary_offender = format!("{}!true{}", "(".repeat(32), ")".repeat(32));
        assert_eq!(
            parse_simple_condition(&unary_offender).unwrap_err(),
            nesting_error(32)
        );
        let utf8 = format!("'é' == 'é' && {}", groups(33));
        assert_eq!(
            parse_simple_condition(&utf8).unwrap_err(),
            nesting_error("'é' == 'é' && ".len() + 32)
        );
        let independent = std::iter::repeat_n("!true", 80)
            .collect::<Vec<_>>()
            .join(" && ");
        assert!(parse_simple_condition(&independent).is_ok());
        let control = format!("{}true != false{}", "(".repeat(32), ")".repeat(32));
        assert!(parse_simple_condition(&control).is_ok());
        assert!(parse_simple_condition("'!((It''s))' == '!'").is_ok());
        let quoted_control = format!(
            "{}'{}It''s{}'{}",
            "(".repeat(32),
            "!(".repeat(40),
            ")".repeat(40),
            ")".repeat(32)
        );
        assert!(parse_simple_condition(&quoted_control).is_ok());
        assert_eq!(
            parse_simple_condition("!!true").unwrap_err().kind,
            ConditionErrorKind::Syntax
        );
        // A completed unary grouped postfix must not accumulate with the next.
        assert!(parse_simple_condition(&format!("{} && {}", mixed(16), mixed(16))).is_ok());
    }
}
