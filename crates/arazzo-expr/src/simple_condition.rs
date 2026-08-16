use std::cmp::Ordering;
use std::fmt;

use serde_json::Value;

use crate::{is_truthy, ExpressionEvaluator, ExpressionWarning};

/// The complete result of evaluating an Arazzo `simple` Criterion condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionEvaluation {
    pub result: bool,
    pub warnings: Vec<ExpressionWarning>,
    pub error: Option<ConditionError>,
}

/// A syntax error in an Arazzo `simple` Criterion condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionError {
    pub condition: String,
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ConditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid simple condition at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl std::error::Error for ConditionError {}

pub(crate) fn evaluate(eval: &ExpressionEvaluator, condition: &str) -> ConditionEvaluation {
    let tokens = match Lexer::new(condition).tokenize() {
        Ok(tokens) => tokens,
        Err(error) => {
            return ConditionEvaluation {
                result: false,
                warnings: Vec::new(),
                error: Some(error),
            }
        }
    };
    let expression = match Parser::new(condition, tokens).parse() {
        Ok(expression) => expression,
        Err(error) => {
            return ConditionEvaluation {
                result: false,
                warnings: Vec::new(),
                error: Some(error),
            }
        }
    };

    let (value, warnings) = evaluate_expression(eval, &expression);
    ConditionEvaluation {
        result: value.is_truthy(),
        warnings,
        error: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonNumber {
    negative: bool,
    digits: String,
    exponent: i64,
}

impl JsonNumber {
    fn parse(input: &str) -> Result<Self, NumberSyntaxError> {
        let bytes = input.as_bytes();
        let mut cursor = 0usize;
        let negative = if bytes.first() == Some(&b'-') {
            cursor += 1;
            true
        } else {
            false
        };

        let integer_start = cursor;
        match bytes.get(cursor).copied() {
            Some(b'0') => {
                cursor += 1;
                if bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    return Err(NumberSyntaxError::new(
                        cursor,
                        "leading zeros are not allowed",
                    ));
                }
            }
            Some(b'1'..=b'9') => {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
            }
            _ => {
                return Err(NumberSyntaxError::new(
                    cursor,
                    "expected a digit in JSON number",
                ))
            }
        }
        let integer_end = cursor;

        let mut fraction_start = cursor;
        let mut fraction_end = cursor;
        if bytes.get(cursor) == Some(&b'.') {
            cursor += 1;
            fraction_start = cursor;
            if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                return Err(NumberSyntaxError::new(
                    cursor,
                    "fraction requires at least one digit",
                ));
            }
            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            fraction_end = cursor;
        }

        let mut explicit_exponent = 0i64;
        if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
            cursor += 1;
            let exponent_negative = match bytes.get(cursor) {
                Some(b'+') => {
                    cursor += 1;
                    false
                }
                Some(b'-') => {
                    cursor += 1;
                    true
                }
                _ => false,
            };
            if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                return Err(NumberSyntaxError::new(
                    cursor,
                    "exponent requires at least one digit",
                ));
            }
            while let Some(digit @ b'0'..=b'9') = bytes.get(cursor).copied() {
                explicit_exponent = explicit_exponent
                    .saturating_mul(10)
                    .saturating_add(i64::from(digit - b'0'));
                cursor += 1;
            }
            if exponent_negative {
                explicit_exponent = explicit_exponent.saturating_neg();
            }
        }

        if cursor != bytes.len() {
            return Err(NumberSyntaxError::new(
                cursor,
                "unexpected character in JSON number",
            ));
        }

        let mut digits = String::with_capacity(
            integer_end - integer_start + fraction_end.saturating_sub(fraction_start),
        );
        digits.push_str(&input[integer_start..integer_end]);
        if fraction_end > fraction_start {
            digits.push_str(&input[fraction_start..fraction_end]);
        }
        let first_nonzero = digits
            .as_bytes()
            .iter()
            .position(|digit| *digit != b'0')
            .unwrap_or(digits.len());
        if first_nonzero == digits.len() {
            return Ok(Self {
                negative: false,
                digits: "0".to_string(),
                exponent: 0,
            });
        }
        digits.drain(..first_nonzero);

        let fraction_len =
            i64::try_from(fraction_end.saturating_sub(fraction_start)).unwrap_or(i64::MAX);
        Ok(Self {
            negative,
            digits,
            exponent: explicit_exponent.saturating_sub(fraction_len),
        })
    }

    fn is_zero(&self) -> bool {
        self.digits == "0"
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => self.compare_magnitude(other),
            (true, true) => self.compare_magnitude(other).reverse(),
        }
    }

    fn compare_magnitude(&self, other: &Self) -> Ordering {
        if self.is_zero() || other.is_zero() {
            return self.is_zero().cmp(&other.is_zero()).reverse();
        }

        let self_scale = self
            .exponent
            .saturating_add(i64::try_from(self.digits.len()).unwrap_or(i64::MAX));
        let other_scale = other
            .exponent
            .saturating_add(i64::try_from(other.digits.len()).unwrap_or(i64::MAX));
        match self_scale.cmp(&other_scale) {
            Ordering::Equal => {
                let width = self.digits.len().max(other.digits.len());
                for index in 0..width {
                    let lhs = self.digits.as_bytes().get(index).copied().unwrap_or(b'0');
                    let rhs = other.digits.as_bytes().get(index).copied().unwrap_or(b'0');
                    match lhs.cmp(&rhs) {
                        Ordering::Equal => {}
                        ordering => return ordering,
                    }
                }
                Ordering::Equal
            }
            ordering => ordering,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NumberSyntaxError {
    offset: usize,
    message: &'static str,
}

impl NumberSyntaxError {
    fn new(offset: usize, message: &'static str) -> Self {
        Self { offset, message }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    True,
    False,
    Null,
    Number(JsonNumber),
    String(String),
    RuntimeExpression(String),
    LeftParen,
    RightParen,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Not,
    And,
    Or,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

struct Lexer<'a> {
    condition: &'a str,
    cursor: usize,
}

impl<'a> Lexer<'a> {
    fn new(condition: &'a str) -> Self {
        Self {
            condition,
            cursor: 0,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, ConditionError> {
        let mut tokens = Vec::new();
        while self.cursor < self.condition.len() {
            self.skip_whitespace();
            if self.cursor == self.condition.len() {
                break;
            }
            let offset = self.cursor;
            let remainder = &self.condition[offset..];
            let token = if remainder.starts_with("&&") {
                self.cursor += 2;
                TokenKind::And
            } else if remainder.starts_with("||") {
                self.cursor += 2;
                TokenKind::Or
            } else if remainder.starts_with("==") {
                self.cursor += 2;
                TokenKind::Equal
            } else if remainder.starts_with("!=") {
                self.cursor += 2;
                TokenKind::NotEqual
            } else if remainder.starts_with("<=") {
                self.cursor += 2;
                TokenKind::LessEqual
            } else if remainder.starts_with(">=") {
                self.cursor += 2;
                TokenKind::GreaterEqual
            } else {
                match self.current_char() {
                    Some('(') => {
                        self.cursor += 1;
                        TokenKind::LeftParen
                    }
                    Some(')') => {
                        self.cursor += 1;
                        TokenKind::RightParen
                    }
                    Some('<') => {
                        self.cursor += 1;
                        TokenKind::Less
                    }
                    Some('>') => {
                        self.cursor += 1;
                        TokenKind::Greater
                    }
                    Some('!') => {
                        self.cursor += 1;
                        TokenKind::Not
                    }
                    Some('\'') => self.scan_string()?,
                    Some('"') => {
                        return Err(self.error(
                            offset,
                            "double-quoted strings are not allowed; use single quotes",
                        ))
                    }
                    Some('$') => self.scan_runtime_expression()?,
                    Some('-' | '0'..='9') => self.scan_number()?,
                    Some(_) if self.scan_keyword("true") => TokenKind::True,
                    Some(_) if self.scan_keyword("false") => TokenKind::False,
                    Some(_) if self.scan_keyword("null") => TokenKind::Null,
                    Some(_) => {
                        return Err(self.error(
                            offset,
                            "bare strings and unsupported operators are not allowed",
                        ))
                    }
                    None => break,
                }
            };
            tokens.push(Token {
                kind: token,
                offset,
            });
        }
        tokens.push(Token {
            kind: TokenKind::End,
            offset: self.condition.len(),
        });
        Ok(tokens)
    }

    fn current_char(&self) -> Option<char> {
        self.condition[self.cursor..].chars().next()
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.current_char() {
            if !character.is_whitespace() {
                break;
            }
            self.cursor += character.len_utf8();
        }
    }

    fn scan_keyword(&mut self, keyword: &str) -> bool {
        let remainder = &self.condition[self.cursor..];
        if !remainder.starts_with(keyword) {
            return false;
        }
        let end = self.cursor + keyword.len();
        if self
            .condition
            .get(end..)
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            return false;
        }
        self.cursor = end;
        true
    }

    fn scan_string(&mut self) -> Result<TokenKind, ConditionError> {
        let start = self.cursor;
        self.cursor += 1;
        let mut value = String::new();
        while let Some(character) = self.current_char() {
            if character == '\'' {
                let next = self.cursor + 1;
                if self.condition.as_bytes().get(next) == Some(&b'\'') {
                    value.push('\'');
                    self.cursor += 2;
                    continue;
                }
                self.cursor += 1;
                return Ok(TokenKind::String(value));
            }
            value.push(character);
            self.cursor += character.len_utf8();
        }
        Err(self.error(start, "unterminated single-quoted string"))
    }

    fn scan_number(&mut self) -> Result<TokenKind, ConditionError> {
        let start = self.cursor;
        while let Some(byte) = self.condition.as_bytes().get(self.cursor).copied() {
            if !matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                break;
            }
            self.cursor += 1;
        }
        let raw = &self.condition[start..self.cursor];
        match JsonNumber::parse(raw) {
            Ok(number) => Ok(TokenKind::Number(number)),
            Err(error) => Err(self.error(start + error.offset, error.message)),
        }
    }

    fn scan_runtime_expression(&mut self) -> Result<TokenKind, ConditionError> {
        let start = self.cursor;
        self.cursor += 1;
        let mut bracket_stack = Vec::new();
        let mut runtime_paren_stack = Vec::new();
        let mut quote: Option<(char, usize)> = None;
        let mut escaped = false;

        while self.cursor < self.condition.len() {
            let character = match self.current_char() {
                Some(character) => character,
                None => break,
            };
            if let Some((delimiter, _)) = quote {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == delimiter {
                    quote = None;
                }
                self.cursor += character.len_utf8();
                continue;
            }

            if !bracket_stack.is_empty() || !runtime_paren_stack.is_empty() {
                match character {
                    '\'' | '"' => quote = Some((character, self.cursor)),
                    '[' => bracket_stack.push(self.cursor),
                    ']' => {
                        let Some(open) = bracket_stack.pop() else {
                            return Err(
                                self.error(self.cursor, "unbalanced runtime-expression index")
                            );
                        };
                        if self.condition[open + 1..self.cursor].trim().is_empty() {
                            return Err(
                                self.error(open, "runtime-expression index cannot be empty")
                            );
                        }
                    }
                    '(' => runtime_paren_stack.push(self.cursor),
                    ')' if runtime_paren_stack.pop().is_none() && bracket_stack.is_empty() => break,
                    ')' => {}
                    _ => {}
                }
                self.cursor += character.len_utf8();
                continue;
            }

            let remainder = &self.condition[self.cursor..];
            if character.is_whitespace()
                || character == ')'
                || remainder.starts_with("&&")
                || remainder.starts_with("||")
                || remainder.starts_with("==")
                || remainder.starts_with("!=")
                || remainder.starts_with("<=")
                || remainder.starts_with(">=")
                || matches!(character, '<' | '>' | '=')
            {
                break;
            }
            match character {
                '[' => bracket_stack.push(self.cursor),
                ']' => return Err(self.error(self.cursor, "unbalanced runtime-expression index")),
                '(' if self.condition.as_bytes().get(self.cursor.wrapping_sub(1))
                    == Some(&b'#') =>
                {
                    runtime_paren_stack.push(self.cursor)
                }
                '(' => break,
                _ => {}
            }
            self.cursor += character.len_utf8();
        }

        if let Some((_, quote_offset)) = quote {
            return Err(self.error(quote_offset, "unterminated quote in runtime expression"));
        }
        if let Some(open) = bracket_stack.last().copied() {
            return Err(self.error(open, "unclosed runtime-expression index"));
        }
        if let Some(open) = runtime_paren_stack.last().copied() {
            return Err(self.error(open, "unclosed runtime-expression traversal"));
        }
        if self.cursor == start + 1 {
            return Err(self.error(start, "runtime expression is incomplete"));
        }
        Ok(TokenKind::RuntimeExpression(
            self.condition[start..self.cursor].to_string(),
        ))
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> ConditionError {
        ConditionError {
            condition: self.condition.to_string(),
            offset,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComparisonOperator {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expression {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    RuntimeValue(String),
    Not(Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Comparison {
        operator: ComparisonOperator,
        left: Box<Expression>,
        right: Box<Expression>,
    },
}

struct Parser<'a> {
    condition: &'a str,
    tokens: Vec<Token>,
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn new(condition: &'a str, tokens: Vec<Token>) -> Self {
        Self {
            condition,
            tokens,
            cursor: 0,
        }
    }

    fn parse(mut self) -> Result<Expression, ConditionError> {
        if matches!(self.current().kind, TokenKind::End) {
            return Err(self.error(self.current().offset, "condition cannot be empty"));
        }
        let expression = self.parse_or()?;
        if !matches!(self.current().kind, TokenKind::End) {
            return Err(self.error(self.current().offset, "unexpected trailing token"));
        }
        Ok(expression)
    }

    fn parse_or(&mut self) -> Result<Expression, ConditionError> {
        let mut expression = self.parse_and()?;
        while matches!(self.current().kind, TokenKind::Or) {
            self.advance();
            let right = self.parse_and()?;
            expression = Expression::Or(Box::new(expression), Box::new(right));
        }
        Ok(expression)
    }

    fn parse_and(&mut self) -> Result<Expression, ConditionError> {
        let mut expression = self.parse_unary()?;
        while matches!(self.current().kind, TokenKind::And) {
            self.advance();
            let right = self.parse_unary()?;
            expression = Expression::And(Box::new(expression), Box::new(right));
        }
        Ok(expression)
    }

    fn parse_unary(&mut self) -> Result<Expression, ConditionError> {
        if matches!(self.current().kind, TokenKind::Not) {
            self.advance();
            return Ok(Expression::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expression, ConditionError> {
        let left = self.parse_primary()?;
        let Some(operator) = self.current_comparison() else {
            return Ok(left);
        };
        self.advance();
        let right = self.parse_primary()?;
        if self.current_comparison().is_some() {
            return Err(self.error(self.current().offset, "chained comparisons are not allowed"));
        }
        Ok(Expression::Comparison {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    fn parse_primary(&mut self) -> Result<Expression, ConditionError> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::True => {
                self.advance();
                Ok(Expression::Bool(true))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expression::Bool(false))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expression::Null)
            }
            TokenKind::Number(number) => {
                self.advance();
                Ok(Expression::Number(number))
            }
            TokenKind::String(value) => {
                self.advance();
                Ok(Expression::String(value))
            }
            TokenKind::RuntimeExpression(expression) => {
                self.advance();
                Ok(Expression::RuntimeValue(expression))
            }
            TokenKind::LeftParen => {
                self.advance();
                let expression = self.parse_or()?;
                if !matches!(self.current().kind, TokenKind::RightParen) {
                    return Err(self.error(self.current().offset, "expected closing parenthesis"));
                }
                self.advance();
                Ok(expression)
            }
            TokenKind::RightParen => {
                Err(self.error(token.offset, "unexpected closing parenthesis"))
            }
            TokenKind::End => Err(self.error(token.offset, "expected a condition operand")),
            _ => Err(self.error(token.offset, "expected a condition operand")),
        }
    }

    fn current_comparison(&self) -> Option<ComparisonOperator> {
        match self.current().kind {
            TokenKind::Less => Some(ComparisonOperator::Less),
            TokenKind::LessEqual => Some(ComparisonOperator::LessEqual),
            TokenKind::Greater => Some(ComparisonOperator::Greater),
            TokenKind::GreaterEqual => Some(ComparisonOperator::GreaterEqual),
            TokenKind::Equal => Some(ComparisonOperator::Equal),
            TokenKind::NotEqual => Some(ComparisonOperator::NotEqual),
            _ => None,
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    fn advance(&mut self) {
        if self.cursor + 1 < self.tokens.len() {
            self.cursor += 1;
        }
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> ConditionError {
        ConditionError {
            condition: self.condition.to_string(),
            offset,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum EvaluatedValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Runtime(Value),
}

impl EvaluatedValue {
    fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Number(value) => !value.is_zero(),
            Self::String(value) => !value.is_empty(),
            Self::Runtime(value) => is_truthy(value),
        }
    }

    fn is_null(&self) -> bool {
        matches!(self, Self::Null | Self::Runtime(Value::Null))
    }

    fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) | Self::Runtime(Value::String(value)) => Some(value),
            _ => None,
        }
    }

    fn as_number(&self) -> Option<JsonNumber> {
        match self {
            Self::Number(value) => Some(value.clone()),
            Self::String(value) | Self::Runtime(Value::String(value)) => {
                JsonNumber::parse(value).ok()
            }
            Self::Runtime(Value::Number(value)) => JsonNumber::parse(&value.to_string()).ok(),
            _ => None,
        }
    }

    fn legacy_text(&self) -> String {
        match self {
            Self::Null | Self::Runtime(Value::Null) => String::new(),
            Self::Bool(value) | Self::Runtime(Value::Bool(value)) => value.to_string(),
            Self::Number(value) => format_number(value),
            Self::String(value) | Self::Runtime(Value::String(value)) => value.clone(),
            Self::Runtime(Value::Number(value)) => value.to_string(),
            Self::Runtime(_) => String::new(),
        }
    }
}

fn evaluate_expression(
    eval: &ExpressionEvaluator,
    expression: &Expression,
) -> (EvaluatedValue, Vec<ExpressionWarning>) {
    match expression {
        Expression::Null => (EvaluatedValue::Null, Vec::new()),
        Expression::Bool(value) => (EvaluatedValue::Bool(*value), Vec::new()),
        Expression::Number(value) => (EvaluatedValue::Number(value.clone()), Vec::new()),
        Expression::String(value) => (EvaluatedValue::String(value.clone()), Vec::new()),
        Expression::RuntimeValue(expression) => {
            let (value, warnings) = eval.evaluate_with_diagnostics(expression);
            (EvaluatedValue::Runtime(value), warnings)
        }
        Expression::Not(inner) => {
            let (value, warnings) = evaluate_expression(eval, inner);
            (EvaluatedValue::Bool(!value.is_truthy()), warnings)
        }
        Expression::And(left, right) => {
            let (left, mut warnings) = evaluate_expression(eval, left);
            if !left.is_truthy() {
                return (EvaluatedValue::Bool(false), warnings);
            }
            let (right, right_warnings) = evaluate_expression(eval, right);
            warnings.extend(right_warnings);
            (EvaluatedValue::Bool(right.is_truthy()), warnings)
        }
        Expression::Or(left, right) => {
            let (left, mut warnings) = evaluate_expression(eval, left);
            if left.is_truthy() {
                return (EvaluatedValue::Bool(true), warnings);
            }
            let (right, right_warnings) = evaluate_expression(eval, right);
            warnings.extend(right_warnings);
            (EvaluatedValue::Bool(right.is_truthy()), warnings)
        }
        Expression::Comparison {
            operator,
            left,
            right,
        } => {
            let (left, mut warnings) = evaluate_expression(eval, left);
            let (right, right_warnings) = evaluate_expression(eval, right);
            warnings.extend(right_warnings);
            (
                EvaluatedValue::Bool(compare(*operator, &left, &right)),
                warnings,
            )
        }
    }
}

fn compare(operator: ComparisonOperator, left: &EvaluatedValue, right: &EvaluatedValue) -> bool {
    if left.is_null() || right.is_null() {
        return matches!(operator, ComparisonOperator::Equal) && left.is_null() && right.is_null();
    }

    if let (Some(left), Some(right)) = (left.as_number(), right.as_number()) {
        return ordering_matches(operator, left.compare(&right));
    }

    if let (Some(left), Some(right)) = (left.as_string(), right.as_string()) {
        return ordering_matches(operator, left.to_lowercase().cmp(&right.to_lowercase()));
    }

    ordering_matches(operator, left.legacy_text().cmp(&right.legacy_text()))
}

fn ordering_matches(operator: ComparisonOperator, ordering: Ordering) -> bool {
    match operator {
        ComparisonOperator::Less => ordering.is_lt(),
        ComparisonOperator::LessEqual => ordering.is_le(),
        ComparisonOperator::Greater => ordering.is_gt(),
        ComparisonOperator::GreaterEqual => ordering.is_ge(),
        ComparisonOperator::Equal => ordering.is_eq(),
        ComparisonOperator::NotEqual => !ordering.is_eq(),
    }
}

fn format_number(number: &JsonNumber) -> String {
    if number.is_zero() {
        return "0".to_string();
    }
    let sign = if number.negative { "-" } else { "" };
    format!("{sign}{}e{}", number.digits, number.exponent)
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use serde_json::{json, Value};

    use super::*;
    use crate::EvalContext;

    fn evaluator() -> ExpressionEvaluator {
        ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            inputs: BTreeMap::from([
                ("number".to_string(), json!(10)),
                ("numeric".to_string(), json!("10")),
                ("smallerNumeric".to_string(), json!("2")),
                ("text".to_string(), json!("Alpha")),
                ("none".to_string(), Value::Null),
                ("flag".to_string(), json!(true)),
            ]),
            response_body: Some(json!({
                "name||title": "Something",
                "nested": {"value": 7},
                "users": [
                    {"name": "Alice", "role": "admin"},
                    {"name": "Bob", "role": "reader"}
                ]
            })),
            ..EvalContext::default()
        })
    }

    fn assert_condition(condition: &str, expected: bool) {
        let evaluation = evaluator().evaluate_condition_detailed(condition);
        assert_eq!(evaluation.error, None, "condition={condition}");
        assert_eq!(evaluation.result, expected, "condition={condition}");
    }

    fn assert_syntax_error(condition: &str) -> ConditionError {
        let evaluation = evaluator().evaluate_condition_detailed(condition);
        assert!(!evaluation.result, "condition={condition}");
        match evaluation.error {
            Some(error) => error,
            None => panic!("condition must fail syntax validation: {condition}"),
        }
    }

    #[test]
    pub(crate) fn conformance_positive_matrix() {
        for (condition, expected) in [
            ("true", true),
            ("false", false),
            ("null", false),
            ("0", false),
            ("-1", true),
            ("1.25e2", true),
            ("''", false),
            ("'hello'", true),
            ("1 < 2", true),
            ("2 <= 2", true),
            ("3 > 2", true),
            ("3 >= 3", true),
            ("3 == 3.0", true),
            ("3 != 4", true),
            ("true && !false", true),
            ("false || true", true),
            ("(false || true) && true", true),
            ("$statusCode == 200", true),
            ("$inputs.text == 'aLpHa'", true),
            ("$response.body.users[0].name == 'alice'", true),
            ("$response.body#/nested/value == 7", true),
        ] {
            assert_condition(condition, expected);
        }
    }

    #[test]
    pub(crate) fn conformance_negative_matrix() {
        for condition in [
            "",
            "bare",
            "\"double quoted\"",
            "true contains false",
            "'abc' matches 'a'",
            "1 in [1]",
            "1 < 2 < 3",
            "1 <",
            "== 1",
            "()",
            "(true",
            "true)",
            "[]",
            "$response.body[]",
            "$response.body[0",
            "'unterminated",
            "true trailing",
            "01 == 1",
            "+1 == 1",
            ".5 == 0.5",
            "1. == 1",
            "1e == 1",
            "true & false",
            "true | false",
        ] {
            let error = assert_syntax_error(condition);
            assert_eq!(error.condition, condition);
            assert!(error.offset <= condition.len(), "condition={condition}");
            assert!(
                condition.is_char_boundary(error.offset),
                "condition={condition}"
            );
        }
    }

    #[test]
    fn doubled_single_quote_is_the_only_string_escape() {
        assert_condition("'it''s' == 'IT''S'", true);
        assert_condition("'''' == ''''", true);
        assert_syntax_error("'it\\'s'");
    }

    #[test]
    fn precedence_grouping_and_unary_are_deterministic() {
        assert_condition("true || false && false", true);
        assert_condition("(true || false) && false", false);
        assert_condition("!($statusCode == 404)", true);
        assert_condition("!!true", true);
    }

    #[test]
    fn parser_consumes_invalid_short_circuited_branches() {
        assert_syntax_error("true || contains");
        assert_syntax_error("false && (1 <)");
    }

    #[test]
    fn numeric_strings_use_strict_exact_numeric_comparisons() {
        for (condition, expected) in [
            ("$inputs.numeric == 10", true),
            ("$inputs.numeric != 10", false),
            ("$inputs.numeric < 11", true),
            ("$inputs.numeric <= 10", true),
            ("$inputs.numeric > $inputs.smallerNumeric", true),
            ("$inputs.numeric >= '1e1'", true),
            ("'010' == 10", false),
            ("' 10 ' == 10", false),
            ("9007199254740992 == 9007199254740993", false),
            ("9007199254740992 < '9007199254740993'", true),
            ("1e400 > 1e399", true),
        ] {
            assert_condition(condition, expected);
        }
    }

    #[test]
    fn null_only_equals_null_and_is_otherwise_falsy() {
        for (condition, expected) in [
            ("null == null", true),
            ("null != null", false),
            ("null == 0", false),
            ("null != 0", false),
            ("null < 0", false),
            ("null <= null", false),
            ("$inputs.none == null", true),
            ("$inputs.none != false", false),
            ("$inputs.missing", false),
        ] {
            assert_condition(condition, expected);
        }
    }

    #[test]
    fn runtime_expression_legacy_traversal_remains_an_operand() {
        for condition in [
            "$response.body['name||title'] == 'Something'",
            "$response.body.users[?(@.role=='admin')].name == 'Alice'",
            "$response.body.users.#(role=='admin').name == 'Alice'",
        ] {
            assert_condition(condition, true);
        }
    }

    #[test]
    fn syntax_errors_retain_original_utf8_byte_offsets() {
        let condition = "  true || 💥";
        let error = assert_syntax_error(condition);
        assert_eq!(error.condition, condition);
        assert_eq!(error.offset, "  true || ".len());
        assert!(condition.is_char_boundary(error.offset));
        assert!(error.to_string().contains("byte 10"));
    }

    #[test]
    fn evaluation_warnings_follow_only_evaluated_branches() {
        let eval = evaluator();
        let short = eval.evaluate_condition_detailed("true || $inputs.missing");
        assert!(short.result);
        assert!(short.warnings.is_empty());
        assert!(short.error.is_none());

        let evaluated = eval.evaluate_condition_detailed("false || $inputs.missing");
        assert!(!evaluated.result);
        assert_eq!(evaluated.warnings.len(), 1);
        assert!(evaluated.error.is_none());
    }

    proptest! {
        #[test]
        fn arbitrary_utf8_never_panics_and_error_offsets_are_boundaries(
            condition in proptest::collection::vec(any::<char>(), 0..160)
                .prop_map(|characters| characters.into_iter().collect::<String>()),
        ) {
            let evaluation = evaluator().evaluate_condition_detailed(&condition);
            if let Some(error) = evaluation.error {
                prop_assert_eq!(&error.condition, &condition);
                prop_assert!(error.offset <= condition.len());
                prop_assert!(condition.is_char_boundary(error.offset));
            }
        }
    }
}
