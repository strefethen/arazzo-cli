//! Bounded conversion and exact comparison of the existing JSON number storage.

use std::cmp::Ordering;

use pest::Parser;
use serde_json::Number;

use crate::runtime_expression::{Rule, RuntimeExpressionParser};

/// Numeric strings use the same JSON-number production as condition literals.
pub(super) fn parse(raw: &str) -> Result<Number, String> {
    let parsed = RuntimeExpressionParser::parse(Rule::number, raw)
        .ok()
        .and_then(|mut pairs| pairs.next());
    if !parsed.is_some_and(|pair| pair.as_span().end() == raw.len()) {
        return Err("invalid numeric spelling".to_owned());
    }
    if !raw.contains(['.', 'e', 'E']) {
        return if raw.starts_with('-') {
            raw.parse::<i64>().map(Number::from)
        } else {
            raw.parse::<u64>().map(Number::from)
        }
        .map_err(|_| "integer is outside [-2^63, 2^64-1]".to_owned());
    }
    let value = raw
        .parse::<f64>()
        .map_err(|_| "invalid floating-point number".to_owned())?;
    if !value.is_finite() {
        return Err("numeric conversion overflow".to_owned());
    }
    // Exponent digits do not make a zero mantissa nonzero.
    let mantissa = raw.split(['e', 'E']).next().unwrap_or(raw);
    if value == 0.0 && mantissa.bytes().any(|byte| matches!(byte, b'1'..=b'9')) {
        return Err("nonzero numeric conversion underflowed to zero".to_owned());
    }
    Number::from_f64(value).ok_or_else(|| "non-finite number".to_owned())
}

#[allow(clippy::expect_used)] // serde_json::Number stores only i64, u64 or finite f64.
pub(super) fn compare(left: &Number, right: &Number) -> Ordering {
    match (integer(left), integer(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(left), None) => {
            compare_integer_float(left, right.as_f64().expect("finite JSON float"))
        }
        (None, Some(right)) => {
            compare_integer_float(right, left.as_f64().expect("finite JSON float")).reverse()
        }
        (None, None) => left
            .as_f64()
            .expect("finite JSON float")
            .partial_cmp(&right.as_f64().expect("finite JSON float"))
            .expect("finite JSON floats are ordered"),
    }
}

fn integer(number: &Number) -> Option<i128> {
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}

fn compare_integer_float(integer: i128, float: f64) -> Ordering {
    // Every represented integer fits these bounds. The upper bound is exclusive:
    // 2^64 is exactly representable, while u64::MAX rounds to it as f64.
    if float >= 18_446_744_073_709_551_616.0 {
        return Ordering::Less;
    }
    if float < -9_223_372_036_854_775_808.0 {
        return Ordering::Greater;
    }
    let truncated = float as i128;
    match integer.cmp(&truncated) {
        Ordering::Equal if float.fract() > 0.0 => Ordering::Less,
        Ordering::Equal if float.fract() < 0.0 => Ordering::Greater,
        ordering => ordering,
    }
}
