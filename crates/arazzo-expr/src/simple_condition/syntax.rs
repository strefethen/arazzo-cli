//! Shared-grammar syntax only; evaluation policy belongs to the later evaluator.

use std::{fmt, ops::Range};

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
pub(super) struct Syntax<'a> {
    pub(super) kind: SyntaxKind<'a>,
    pub(super) span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SyntaxKind<'a> {
    Or(Vec<Syntax<'a>>),
    And(Vec<Syntax<'a>>),
    Not(Box<Syntax<'a>>),
    Comparison {
        left: Box<Syntax<'a>>,
        operator: &'a str,
        operator_span: Range<usize>,
        right: Box<Syntax<'a>>,
    },
    Postfix {
        primary: Box<Syntax<'a>>,
        accesses: Vec<Access<'a>>,
    },
    Group(Box<Syntax<'a>>),
    RuntimeExpression(usize),
    Boolean(&'a str),
    Null,
    Number(&'a str),
    String(&'a str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Access<'a> {
    pub(super) kind: AccessKind<'a>,
    pub(super) span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AccessKind<'a> {
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
    let span = pair.as_span().start()..pair.as_span().end();
    let kind = match rule {
        Rule::or_expr | Rule::and_expr => {
            let children: Vec<_> = pair
                .into_inner()
                .filter(|child| !matches!(child.as_rule(), Rule::or_operator | Rule::and_operator))
                .map(|child| map_syntax(child, expressions))
                .collect();
            if children.len() == 1 {
                return children.into_iter().next().expect("one child");
            } else if rule == Rule::or_expr {
                SyntaxKind::Or(children)
            } else {
                SyntaxKind::And(children)
            }
        }
        Rule::unary => {
            let mut children = pair.into_inner();
            let first = children.next().expect("unary operand");
            if first.as_rule() == Rule::not_operator {
                SyntaxKind::Not(Box::new(map_syntax(
                    children.next().expect("postfix operand"),
                    expressions,
                )))
            } else {
                return map_syntax(first, expressions);
            }
        }
        Rule::comparison => {
            let mut children = pair.into_inner();
            let left = map_syntax(children.next().expect("comparison lhs"), expressions);
            if let Some(operator) = children.next() {
                SyntaxKind::Comparison {
                    left: Box::new(left),
                    operator: operator.as_str(),
                    operator_span: operator.as_span().start()..operator.as_span().end(),
                    right: Box::new(map_syntax(
                        children.next().expect("comparison rhs"),
                        expressions,
                    )),
                }
            } else {
                return left;
            }
        }
        Rule::postfix => {
            let mut children = pair.into_inner();
            let primary = map_syntax(children.next().expect("primary"), expressions);
            let accesses: Vec<_> = children
                .map(|access| {
                    let rule = access.as_rule();
                    let span = access.as_span().start()..access.as_span().end();
                    let field = access.into_inner().next().expect("postfix field").as_str();
                    let kind = if rule == Rule::property {
                        AccessKind::Property(field)
                    } else {
                        AccessKind::Index(field)
                    };
                    Access { kind, span }
                })
                .collect();
            if accesses.is_empty() {
                return primary;
            } else {
                SyntaxKind::Postfix {
                    primary: Box::new(primary),
                    accesses,
                }
            }
        }
        Rule::group => SyntaxKind::Group(Box::new(map_syntax(
            pair.into_inner().next().expect("group expression"),
            expressions,
        ))),
        Rule::c_expression => {
            let index = expressions.len();
            expressions.push(map_runtime_expression(pair));
            SyntaxKind::RuntimeExpression(index)
        }
        Rule::boolean => SyntaxKind::Boolean(raw),
        Rule::null => SyntaxKind::Null,
        Rule::number => SyntaxKind::Number(raw),
        Rule::string => SyntaxKind::String(raw),
        _ => unreachable!("condition syntax rule from the shared grammar"),
    };
    Syntax { kind, span }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_flat_ordered_chains() {
        let parsed = parse_simple_condition("true && false || null").unwrap();
        let SyntaxKind::Or(children) = &parsed.syntax.kind else {
            panic!("or root")
        };
        assert!(matches!(children[0].kind, SyntaxKind::And(_)));
        assert!(matches!(children[1].kind, SyntaxKind::Null));
        let parsed = parse_simple_condition("1 == 2 && 3 < 4").unwrap();
        let SyntaxKind::And(children) = &parsed.syntax.kind else {
            panic!("and root")
        };
        assert!(children
            .iter()
            .all(|child| matches!(child.kind, SyntaxKind::Comparison { .. })));
        let parsed = parse_simple_condition("!true && false").unwrap();
        let SyntaxKind::And(children) = &parsed.syntax.kind else {
            panic!("and root")
        };
        assert!(matches!(children[0].kind, SyntaxKind::Not(_)));
        let parsed = parse_simple_condition("$inputs.foo.bar == $steps.s.outputs.x.y").unwrap();
        assert_eq!(parsed.expressions[0].raw(), "$inputs.foo.bar");
        assert_eq!(parsed.expressions[1].raw(), "$steps.s.outputs.x.y");
        let SyntaxKind::Comparison { operator_span, .. } = parsed.syntax.kind else {
            panic!("comparison")
        };
        assert_eq!(&parsed.raw[operator_span], "==");
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
