//! Field-mode adapters for syntax-only local Step output reference discovery.

use std::fmt;

use crate::{
    classify_value_string, expression_string::reserved_embedded_expressions,
    parse_runtime_expression, parse_simple_condition, ConditionError, ExpressionStringError,
    ParsedRuntimeExpression, RuntimeExpressionError, ValueStringSyntax,
};

/// The syntax already assigned to a field by its document-level owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepReferenceSyntax {
    /// A complete expression, a template, or ordinary literal text.
    LiteralOrExpression,
    /// Exactly one complete Runtime Expression.
    RequiredExpression,
    /// A complete simple condition, excluding quoted literal operands.
    SimpleCondition,
    /// Native condition text whose only reserved candidates begin with `{$`.
    EmbeddedCondition,
}

/// One occurrence, borrowing exact case-sensitive names from the field text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalStepOutputReference<'a> {
    pub step_id: &'a str,
    pub output_name: &'a str,
}

/// The canonical syntax error for the selected field mode.
#[derive(Debug)]
pub enum StepReferenceError {
    RuntimeExpression(RuntimeExpressionError),
    ExpressionString(ExpressionStringError),
    SimpleCondition(ConditionError),
}

impl fmt::Display for StepReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeExpression(error) => error.fmt(formatter),
            Self::ExpressionString(error) => error.fmt(formatter),
            Self::SimpleCondition(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StepReferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::RuntimeExpression(error) => error,
            Self::ExpressionString(error) => error,
            Self::SimpleCondition(error) => error,
        })
    }
}

/// Extract exact local Step-output occurrences without evaluating or resolving them.
///
/// Names, duplicates, and source order are retained. Pointer syntax is validated by
/// the canonical parser; output existence and dependency deduplication are caller
/// responsibilities. An invalid reserved candidate returns an error, never a
/// partial occurrence list. Native braces and unbraced dollars in embedded
/// conditions are left uninterpreted.
pub fn local_step_output_references(
    input: &str,
    syntax: StepReferenceSyntax,
) -> Result<Vec<LocalStepOutputReference<'_>>, StepReferenceError> {
    let mut references = Vec::new();
    match syntax {
        StepReferenceSyntax::LiteralOrExpression => {
            match classify_value_string(input).map_err(StepReferenceError::ExpressionString)? {
                ValueStringSyntax::Literal(_) => {}
                ValueStringSyntax::Expression(expression) => {
                    references.extend(local_reference(&expression));
                }
                ValueStringSyntax::Template(expressions) => {
                    references.extend(
                        expressions
                            .iter()
                            .filter_map(|embedded| local_reference(embedded.expression())),
                    );
                }
            }
        }
        StepReferenceSyntax::RequiredExpression => {
            let expression =
                parse_runtime_expression(input).map_err(StepReferenceError::RuntimeExpression)?;
            references.extend(local_reference(&expression));
        }
        StepReferenceSyntax::SimpleCondition => {
            let condition =
                parse_simple_condition(input).map_err(StepReferenceError::SimpleCondition)?;
            references.extend(
                condition
                    .runtime_expressions()
                    .iter()
                    .filter_map(local_reference),
            );
        }
        StepReferenceSyntax::EmbeddedCondition => {
            for embedded in reserved_embedded_expressions(input) {
                let embedded = embedded.map_err(StepReferenceError::ExpressionString)?;
                references.extend(local_reference(embedded.expression()));
            }
        }
    }
    Ok(references)
}

fn local_reference<'a>(
    expression: &ParsedRuntimeExpression<'a>,
) -> Option<LocalStepOutputReference<'a>> {
    expression
        .step_output_reference()
        .map(|reference| LocalStepOutputReference {
            step_id: reference.step_id(),
            output_name: reference.output_name(),
        })
}
