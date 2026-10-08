//! Borrowed expression-string syntax under the accepted literal-brace compatibility policy.

use std::{fmt, ops::Range};

use crate::{parse_runtime_expression, ParsedRuntimeExpression};

/// Syntax of a literal-or-expression value, without evaluation or substitution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueStringSyntax<'a> {
    Literal(&'a str),
    Expression(ParsedRuntimeExpression<'a>),
    /// Ordered embedded expressions; other input bytes are literal text.
    Template(Vec<EmbeddedExpression<'a>>),
}

/// An embedded Runtime Expression and its byte ranges in the original input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedExpression<'a> {
    full_range: Range<usize>,
    inner_range: Range<usize>,
    expression: ParsedRuntimeExpression<'a>,
}

impl<'a> EmbeddedExpression<'a> {
    /// Half-open byte range including the opening and closing braces.
    pub fn full_range(&self) -> Range<usize> {
        self.full_range.clone()
    }

    /// Half-open byte range containing the complete Runtime Expression.
    pub fn inner_range(&self) -> Range<usize> {
        self.inner_range.clone()
    }

    pub fn expression(&self) -> &ParsedRuntimeExpression<'a> {
        &self.expression
    }
}

/// Whether the reserved candidate is unterminated or its inner expression is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExpressionStringErrorKind {
    Syntax,
    EmbeddedRuntimeExpression,
}

/// The first syntax violation, with a half-open UTF-8 byte range in the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionStringError {
    pub kind: ExpressionStringErrorKind,
    pub byte_range: Range<usize>,
    pub message: String,
}

impl fmt::Display for ExpressionStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid expression string at bytes {}..{}: {}",
            self.byte_range.start, self.byte_range.end, self.message
        )
    }
}

impl std::error::Error for ExpressionStringError {}

/// Classify complete UTF-8 text using the canonical Runtime Expression parser.
///
/// A complete Runtime Expression takes precedence. Otherwise only `{$` opens
/// an embedded candidate, closed by the first raw `}`. All other braces and
/// unbraced dollar signs are literal text under the accepted compatibility
/// policy; this policy intentionally differs from the ABNF `literal-char` rule.
/// An invalid candidate returns one error and never a partial template.
pub fn classify_value_string(input: &str) -> Result<ValueStringSyntax<'_>, ExpressionStringError> {
    if let Ok(expression) = parse_runtime_expression(input) {
        return Ok(ValueStringSyntax::Expression(expression));
    }

    let mut expressions = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = input[cursor..].find("{$") {
        let start = cursor + offset;
        let inner_start = start + 1;
        let Some(close_offset) = input[inner_start..].find('}') else {
            return Err(ExpressionStringError {
                kind: ExpressionStringErrorKind::Syntax,
                byte_range: start..input.len(),
                message: "embedded Runtime Expression is missing closing '}'".to_owned(),
            });
        };
        let inner_end = inner_start + close_offset;
        let expression =
            parse_runtime_expression(&input[inner_start..inner_end]).map_err(|error| {
                ExpressionStringError {
                    kind: ExpressionStringErrorKind::EmbeddedRuntimeExpression,
                    byte_range: (inner_start + error.byte_range.start)
                        ..(inner_start + error.byte_range.end),
                    message: error.to_string(),
                }
            })?;
        cursor = inner_end + 1;
        expressions.push(EmbeddedExpression {
            full_range: start..cursor,
            inner_range: inner_start..inner_end,
            expression,
        });
    }

    if expressions.is_empty() {
        Ok(ValueStringSyntax::Literal(input))
    } else {
        Ok(ValueStringSyntax::Template(expressions))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn arbitrary_utf8_ranges_always_slice_original_input(input in any::<String>()) {
            match classify_value_string(&input) {
                Ok(ValueStringSyntax::Literal(literal)) => prop_assert_eq!(literal, input.as_str()),
                Ok(ValueStringSyntax::Expression(expression)) => {
                    prop_assert_eq!(expression.raw(), input.as_str());
                }
                Ok(ValueStringSyntax::Template(expressions)) => {
                    let mut previous_end = 0;
                    for embedded in expressions {
                        let full = embedded.full_range();
                        let inner = embedded.inner_range();
                        prop_assert!(full.start >= previous_end);
                        prop_assert_eq!(full.start + 1, inner.start);
                        prop_assert_eq!(full.end - 1, inner.end);
                        prop_assert_eq!(input.get(inner), Some(embedded.expression().raw()));
                        prop_assert!(input.get(full.clone()).is_some());
                        previous_end = full.end;
                    }
                }
                Err(error) => prop_assert!(input.get(error.byte_range).is_some()),
            }
        }

        #[test]
        fn valid_embedded_spans_borrow_unicode_surroundings(
            prefix in "[^$]*", suffix in "[^$]*", name in "[a-zA-Z_][a-zA-Z0-9_]{0,64}"
        ) {
            let inner = format!("$inputs.{name}");
            let input = format!("{prefix}{{{inner}}}{suffix}");
            let result = classify_value_string(&input)?;
            let ValueStringSyntax::Template(expressions) = result else {
                prop_assert!(false, "expected template");
                return Ok(());
            };
            prop_assert_eq!(expressions.len(), 1);
            prop_assert_eq!(expressions[0].full_range(), prefix.len()..prefix.len() + inner.len() + 2);
            prop_assert_eq!(expressions[0].expression().raw(), inner);
        }
    }
}
