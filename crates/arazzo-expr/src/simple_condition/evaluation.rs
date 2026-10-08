//! AST execution with absence, strict booleans and deterministic comparisons.

use std::cmp::Ordering;

use serde_json::Value;

use super::numeric;
use super::syntax::{AccessKind, Syntax, SyntaxKind};
use super::{ConditionError, ConditionErrorKind, ParsedSimpleCondition};
use crate::resolution::{resolve_parsed, ResolvedValue};
use crate::{EvalContext, ExpressionWarning};

/// A condition decision, ordered lookup warnings and an optional failure reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionEvaluation {
    pub result: bool,
    pub warnings: Vec<ExpressionWarning>,
    pub error: Option<ConditionError>,
}

pub(super) fn evaluate(
    parsed: &ParsedSimpleCondition<'_>,
    context: &EvalContext,
) -> ConditionEvaluation {
    let mut execution = Execution {
        parsed,
        context,
        warnings: Vec::new(),
    };
    let result = execution
        .node(&parsed.syntax)
        .and_then(|value| truth(&value, parsed.syntax.span.start));
    match result {
        Ok(result) => ConditionEvaluation {
            result,
            warnings: execution.warnings,
            error: None,
        },
        Err(error) => ConditionEvaluation {
            result: false,
            warnings: execution.warnings,
            error: Some(error),
        },
    }
}

struct Execution<'a, 'input> {
    parsed: &'a ParsedSimpleCondition<'input>,
    context: &'a EvalContext,
    warnings: Vec<ExpressionWarning>,
}

impl Execution<'_, '_> {
    fn node(&mut self, syntax: &Syntax<'_>) -> Result<ResolvedValue, ConditionError> {
        use SyntaxKind as Kind;
        let value = match &syntax.kind {
            Kind::Or(children) | Kind::And(children) => {
                let is_or = matches!(syntax.kind, Kind::Or(_));
                for child in children {
                    let value = self.node(child)?;
                    let boolean = truth(&value, child.span.start)?;
                    if boolean == is_or {
                        return Ok(ResolvedValue::Present(Value::Bool(boolean)));
                    }
                }
                Value::Bool(!is_or)
            }
            Kind::Not(child) => {
                let value = self.node(child)?;
                Value::Bool(!truth(&value, child.span.start)?)
            }
            Kind::Group(child) => return self.node(child),
            Kind::Comparison {
                left,
                operator,
                operator_span,
                right,
            } => {
                let left_value = self.node(left)?;
                let right_value = self.node(right)?;
                Value::Bool(
                    compare(&left_value, &right_value, operator).map_err(|message| {
                        let offset = match (public_value(&left_value), public_value(&right_value)) {
                            (Value::Number(_), Value::String(_)) => right.span.start,
                            (Value::String(_), Value::Number(_)) => left.span.start,
                            _ => operator_span.start,
                        };
                        invalid(offset, message)
                    })?,
                )
            }
            Kind::Postfix { primary, accesses } => {
                let mut value = self.node(primary)?;
                for access in accesses {
                    value = match (value, &access.kind) {
                        (ResolvedValue::Missing, _) => ResolvedValue::Missing,
                        (
                            ResolvedValue::Present(Value::Object(mut object)),
                            AccessKind::Property(name),
                        ) => object
                            .remove(*name)
                            .map_or(ResolvedValue::Missing, ResolvedValue::Present),
                        (
                            ResolvedValue::Present(Value::Array(mut array)),
                            AccessKind::Index(raw),
                        ) => raw
                            .parse::<usize>()
                            .ok()
                            .filter(|index| *index < array.len())
                            .map_or(ResolvedValue::Missing, |index| {
                                ResolvedValue::Present(array.swap_remove(index))
                            }),
                        (_, AccessKind::Property(_)) => {
                            return Err(invalid(
                                access.span.start,
                                "property access requires an object",
                            ))
                        }
                        (_, AccessKind::Index(_)) => {
                            return Err(invalid(
                                access.span.start,
                                "index access requires an array",
                            ))
                        }
                    };
                }
                return Ok(value);
            }
            Kind::RuntimeExpression(index) => {
                let resolution =
                    resolve_parsed(&self.parsed.runtime_expressions()[*index], self.context);
                self.warnings.extend(resolution.warnings);
                return Ok(resolution.value);
            }
            Kind::Boolean(raw) => Value::Bool(*raw == "true"),
            Kind::Null => Value::Null,
            Kind::Number(raw) => Value::Number(
                numeric::parse(raw).map_err(|message| invalid(syntax.span.start, message))?,
            ),
            Kind::String(raw) => Value::String(raw[1..raw.len() - 1].replace("''", "'")),
        };
        Ok(ResolvedValue::Present(value))
    }
}

fn invalid(offset: usize, message: impl Into<String>) -> ConditionError {
    ConditionError {
        kind: ConditionErrorKind::InvalidEvaluation,
        byte_offset: offset,
        message: message.into(),
    }
}

fn truth(value: &ResolvedValue, offset: usize) -> Result<bool, ConditionError> {
    match value {
        ResolvedValue::Missing | ResolvedValue::Present(Value::Null) => Ok(false),
        ResolvedValue::Present(Value::Bool(value)) => Ok(*value),
        _ => Err(invalid(offset, "boolean or null required")),
    }
}

fn public_value(value: &ResolvedValue) -> &Value {
    match value {
        ResolvedValue::Missing => &Value::Null,
        ResolvedValue::Present(value) => value,
    }
}

fn compare(left: &ResolvedValue, right: &ResolvedValue, operator: &str) -> Result<bool, String> {
    let (left, right) = (public_value(left), public_value(right));
    // The accepted null prose applies to all six operators, including !=.
    if left.is_null() != right.is_null() {
        return Ok(false);
    }
    if matches!(operator, "==" | "!=") {
        let equal = structural_equal(left, right)?;
        return Ok(if operator == "==" { equal } else { !equal });
    }
    let ordering = ordered(left, right)?;
    Ok(match operator {
        "<" => ordering.is_lt(),
        "<=" => ordering.is_le(),
        ">" => ordering.is_gt(),
        ">=" => ordering.is_ge(),
        _ => unreachable!("grammar-owned comparison operator"),
    })
}

fn ordered(left: &Value, right: &Value) -> Result<Ordering, String> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => Ok(numeric::compare(left, right)),
        (Value::Number(left), Value::String(right)) => {
            numeric::parse(right).map(|right| numeric::compare(left, &right))
        }
        (Value::String(left), Value::Number(right)) => {
            numeric::parse(left).map(|left| numeric::compare(&left, right))
        }
        (Value::String(left), Value::String(right)) => {
            Ok(left.to_lowercase().cmp(&right.to_lowercase()))
        }
        _ => Err("ordering requires numbers or strings".to_owned()),
    }
}

/// Heap frames retain declaration order without recursive runtime-value traversal.
fn structural_equal(left: &Value, right: &Value) -> Result<bool, String> {
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        let equal = match (left, right) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(left), Value::Bool(right)) => left == right,
            (Value::Number(_), Value::Number(_) | Value::String(_))
            | (Value::String(_), Value::Number(_) | Value::String(_)) => {
                ordered(left, right)?.is_eq()
            }
            (Value::Array(left), Value::Array(right)) => {
                if left.len() != right.len() {
                    return Ok(false);
                }
                pending.extend(left.iter().zip(right).rev());
                true
            }
            (Value::Object(left), Value::Object(right)) => {
                if left.len() != right.len() || left.keys().any(|key| !right.contains_key(key)) {
                    return Ok(false);
                }
                let mut keys: Vec<_> = left.keys().collect();
                keys.sort_unstable();
                for key in keys.into_iter().rev() {
                    pending.push((&left[key], &right[key]));
                }
                true
            }
            _ => false,
        };
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::structural_equal;
    use serde_json::Value;

    #[test]
    fn deep_structural_comparison_uses_heap_frames() {
        fn nested(leaf: bool) -> Value {
            let mut value = Value::Bool(leaf);
            for _ in 0..20_000 {
                value = Value::Array(vec![value]);
            }
            value
        }
        fn dispose(value: Value) {
            let mut pending = vec![value];
            while let Some(value) = pending.pop() {
                if let Value::Array(children) = value {
                    pending.extend(children);
                }
            }
        }
        let left = nested(true);
        let right = nested(true);
        let different = nested(false);
        assert_eq!(structural_equal(&left, &right), Ok(true));
        assert_eq!(structural_equal(&left, &different), Ok(false));
        dispose(left);
        dispose(right);
        dispose(different);
    }
}
