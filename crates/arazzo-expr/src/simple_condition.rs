use std::cmp::Ordering;
use std::fmt;

use serde_json::Value;

use crate::{is_truthy, ExpressionEvaluator, ExpressionWarning};

const MAX_CONDITION_DEPTH: usize = 128;

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

    match evaluate_expression(eval, condition, &expression, 1) {
        Ok((value, warnings)) => ConditionEvaluation {
            result: value.is_truthy(),
            warnings,
            error: None,
        },
        Err(error) => ConditionEvaluation {
            result: false,
            warnings: Vec::new(),
            error: Some(error),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JsonNumber {
    negative: bool,
    digits: String,
    exponent: DecimalInteger,
}

/// A signed, arbitrary-size decimal integer used only for JSON-number scales.
///
/// Keeping the magnitude as normalized base-10 digits makes work and storage
/// proportional to the condition input. In particular, an exponent such as
/// `1e999999999999999999999` never turns into an allocation of that size and
/// is never narrowed or saturated to a machine integer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecimalInteger {
    negative: bool,
    digits: Vec<u8>,
}

impl DecimalInteger {
    fn zero() -> Self {
        Self {
            negative: false,
            digits: vec![0],
        }
    }

    fn from_ascii_digits(input: &[u8], negative: bool) -> Self {
        let first_nonzero = input
            .iter()
            .position(|digit| *digit != b'0')
            .unwrap_or(input.len());
        if first_nonzero == input.len() {
            return Self::zero();
        }
        Self {
            negative,
            digits: input[first_nonzero..]
                .iter()
                .map(|digit| digit - b'0')
                .collect(),
        }
    }

    fn from_usize(value: usize) -> Self {
        let text = value.to_string();
        Self::from_ascii_digits(text.as_bytes(), false)
    }

    fn is_zero(&self) -> bool {
        self.digits == [0]
    }

    fn add_unsigned(&self, value: usize) -> Self {
        self.add(&Self::from_usize(value))
    }

    fn subtract_unsigned(&self, value: usize) -> Self {
        let mut right = Self::from_usize(value);
        if !right.is_zero() {
            right.negative = true;
        }
        self.add(&right)
    }

    fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            return Self::from_digits(
                self.negative,
                add_decimal_magnitudes(&self.digits, &other.digits),
            );
        }

        match compare_decimal_magnitudes(&self.digits, &other.digits) {
            Ordering::Equal => Self::zero(),
            Ordering::Greater => Self::from_digits(
                self.negative,
                subtract_decimal_magnitudes(&self.digits, &other.digits),
            ),
            Ordering::Less => Self::from_digits(
                other.negative,
                subtract_decimal_magnitudes(&other.digits, &self.digits),
            ),
        }
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => compare_decimal_magnitudes(&self.digits, &other.digits),
            (true, true) => compare_decimal_magnitudes(&self.digits, &other.digits).reverse(),
        }
    }

    fn from_digits(negative: bool, digits: Vec<u8>) -> Self {
        let first_nonzero = digits
            .iter()
            .position(|digit| *digit != 0)
            .unwrap_or(digits.len());
        if first_nonzero == digits.len() {
            return Self::zero();
        }
        Self {
            negative,
            digits: digits[first_nonzero..].to_vec(),
        }
    }
}

impl fmt::Display for DecimalInteger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        for digit in &self.digits {
            write!(f, "{digit}")?;
        }
        Ok(())
    }
}

fn compare_decimal_magnitudes(left: &[u8], right: &[u8]) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn add_decimal_magnitudes(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(left.len().max(right.len()) + 1);
    let mut left_cursor = left.len();
    let mut right_cursor = right.len();
    let mut carry = 0u8;
    while left_cursor > 0 || right_cursor > 0 || carry != 0 {
        let left_digit = if left_cursor > 0 {
            left_cursor -= 1;
            left[left_cursor]
        } else {
            0
        };
        let right_digit = if right_cursor > 0 {
            right_cursor -= 1;
            right[right_cursor]
        } else {
            0
        };
        let sum = left_digit + right_digit + carry;
        output.push(sum % 10);
        carry = sum / 10;
    }
    output.reverse();
    output
}

/// Subtract `right` from `left`; callers prove `left >= right` first.
fn subtract_decimal_magnitudes(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(left.len());
    let mut left_cursor = left.len();
    let mut right_cursor = right.len();
    let mut borrow = 0i8;
    while left_cursor > 0 {
        left_cursor -= 1;
        let mut digit = left[left_cursor] as i8 - borrow;
        let right_digit = if right_cursor > 0 {
            right_cursor -= 1;
            right[right_cursor] as i8
        } else {
            0
        };
        if digit < right_digit {
            digit += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        output.push((digit - right_digit) as u8);
    }
    debug_assert_eq!(borrow, 0);
    output.reverse();
    output
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

        let mut explicit_exponent = DecimalInteger::zero();
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
            let exponent_start = cursor;
            while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            explicit_exponent = DecimalInteger::from_ascii_digits(
                &bytes[exponent_start..cursor],
                exponent_negative,
            );
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
                exponent: DecimalInteger::zero(),
            });
        }
        digits.drain(..first_nonzero);

        let fraction_len = fraction_end.saturating_sub(fraction_start);
        Ok(Self {
            negative,
            digits,
            exponent: explicit_exponent.subtract_unsigned(fraction_len),
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

        let self_scale = self.exponent.add_unsigned(self.digits.len());
        let other_scale = other.exponent.add_unsigned(other.digits.len());
        match self_scale.compare(&other_scale) {
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
        let expression = &self.condition[start..];

        for exact in ["$statusCode", "$method", "$url", "$self"] {
            if expression.starts_with(exact) {
                let end = start + exact.len();
                if self.is_condition_boundary(end) {
                    self.cursor = end;
                    return self.runtime_expression_token(start);
                }
            }
        }

        for prefix in ["$request.header.", "$response.header.", "$message.header."] {
            if expression.starts_with(prefix) {
                self.cursor = start + prefix.len();
                self.scan_header_name();
                return self.runtime_expression_token(start);
            }
        }

        for prefix in [
            "$request.query.",
            "$request.path.",
            "$response.query.",
            "$response.path.",
            "$env.",
        ] {
            if expression.starts_with(prefix) {
                self.cursor = start + prefix.len();
                self.scan_unrestricted_runtime_tail();
                return self.runtime_expression_token(start);
            }
        }

        for prefix in ["$request.body", "$response.body", "$message.payload"] {
            if expression.starts_with(prefix) {
                let tail = start + prefix.len();
                if self.is_condition_boundary(tail) {
                    self.cursor = tail;
                    return self.runtime_expression_token(start);
                }
                match self.condition.as_bytes().get(tail) {
                    Some(b'#') => {
                        self.cursor = tail + 1;
                        self.scan_unrestricted_runtime_tail();
                        return self.runtime_expression_token(start);
                    }
                    Some(b'.' | b'[') => {
                        self.cursor = tail;
                        self.scan_body_traversal()?;
                        return self.runtime_expression_token(start);
                    }
                    _ => {}
                }
            }
        }

        if expression.starts_with("$sourceDescriptions.") {
            self.cursor = start + "$sourceDescriptions.".len();
            while self
                .condition
                .as_bytes()
                .get(self.cursor)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                self.cursor += 1;
            }
            if self.condition.as_bytes().get(self.cursor) == Some(&b'.') {
                self.cursor += 1;
                self.scan_unrestricted_runtime_tail();
            }
            return self.runtime_expression_token(start);
        }

        for prefix in [
            "$inputs.",
            "$outputs.",
            "$steps.",
            "$workflows.",
            "$components.",
        ] {
            if expression.starts_with(prefix) {
                self.cursor = start + prefix.len();
                self.scan_restricted_runtime_tail()?;
                return self.runtime_expression_token(start);
            }
        }

        // Preserve the legacy behavior for unknown namespaces. Recognized
        // namespaces above use their own grammar so operator punctuation that
        // belongs to a header, unrestricted name, pointer, or body traversal
        // cannot be mistaken for a simple-condition operator.
        self.cursor += 1;
        self.scan_generic_runtime_tail()?;
        self.runtime_expression_token(start)
    }

    fn scan_header_name(&mut self) {
        while let Some(byte) = self.condition.as_bytes().get(self.cursor).copied() {
            // `!` is a valid RFC header-name tchar, but the complete `!=`
            // sequence cannot be part of a header name because `=` is not.
            if self.condition[self.cursor..].starts_with("!=") || !is_header_tchar(byte) {
                break;
            }
            self.cursor += 1;
        }
    }

    fn scan_unrestricted_runtime_tail(&mut self) {
        while let Some(character) = self.current_char() {
            if character.is_whitespace() || character == ')' {
                break;
            }
            self.cursor += character.len_utf8();
        }
    }

    fn scan_restricted_runtime_tail(&mut self) -> Result<(), ConditionError> {
        let mut bracket_stack = Vec::new();
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

            if !bracket_stack.is_empty() {
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
                    _ => {}
                }
                self.cursor += character.len_utf8();
                continue;
            }

            if character.is_whitespace() || character == ')' {
                break;
            }
            match character {
                '[' => bracket_stack.push(self.cursor),
                ']' => return Err(self.error(self.cursor, "unbalanced runtime-expression index")),
                '#' => {
                    self.cursor += 1;
                    self.scan_unrestricted_runtime_tail();
                    break;
                }
                _ if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') => {}
                _ => break,
            }
            self.cursor += character.len_utf8();
        }

        if let Some((_, quote_offset)) = quote {
            return Err(self.error(quote_offset, "unterminated quote in runtime expression"));
        }
        if let Some(open) = bracket_stack.last().copied() {
            return Err(self.error(open, "unclosed runtime-expression index"));
        }
        Ok(())
    }

    fn scan_body_traversal(&mut self) -> Result<(), ConditionError> {
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

            if character.is_whitespace() || character == ')' {
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
        Ok(())
    }

    fn scan_generic_runtime_tail(&mut self) -> Result<(), ConditionError> {
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
        Ok(())
    }

    fn is_condition_boundary(&self, offset: usize) -> bool {
        let Some(remainder) = self.condition.get(offset..) else {
            return false;
        };
        if remainder.is_empty() {
            return true;
        }
        if remainder
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace() || matches!(character, '(' | ')'))
        {
            return true;
        }
        ["&&", "||", "==", "!=", "<=", ">="]
            .iter()
            .any(|operator| remainder.starts_with(operator))
            || remainder
                .chars()
                .next()
                .is_some_and(|character| matches!(character, '<' | '>' | '!'))
    }

    fn runtime_expression_token(&self, start: usize) -> Result<TokenKind, ConditionError> {
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

fn is_header_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
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
struct Expression {
    kind: ExpressionKind,
    offset: usize,
    depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpressionKind {
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

impl Expression {
    fn leaf(kind: ExpressionKind, offset: usize) -> Self {
        Self {
            kind,
            offset,
            depth: 1,
        }
    }
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
        let expression = self.parse_or(0)?;
        if !matches!(self.current().kind, TokenKind::End) {
            return Err(self.error(self.current().offset, "unexpected trailing token"));
        }
        Ok(expression)
    }

    fn parse_or(&mut self, nesting: usize) -> Result<Expression, ConditionError> {
        let mut expression = self.parse_and(nesting)?;
        while matches!(self.current().kind, TokenKind::Or) {
            let offset = self.current().offset;
            self.advance();
            let right = self.parse_and(nesting)?;
            expression = self.binary(offset, expression, right, |left, right| {
                ExpressionKind::Or(left, right)
            })?;
        }
        Ok(expression)
    }

    fn parse_and(&mut self, nesting: usize) -> Result<Expression, ConditionError> {
        let mut expression = self.parse_comparison(nesting)?;
        while matches!(self.current().kind, TokenKind::And) {
            let offset = self.current().offset;
            self.advance();
            let right = self.parse_comparison(nesting)?;
            expression = self.binary(offset, expression, right, |left, right| {
                ExpressionKind::And(left, right)
            })?;
        }
        Ok(expression)
    }

    fn parse_comparison(&mut self, nesting: usize) -> Result<Expression, ConditionError> {
        let left = self.parse_unary(nesting)?;
        let Some(operator) = self.current_comparison() else {
            return Ok(left);
        };
        let offset = self.current().offset;
        self.advance();
        let right = self.parse_unary(nesting)?;
        if self.current_comparison().is_some() {
            return Err(self.error(self.current().offset, "chained comparisons are not allowed"));
        }
        self.binary(offset, left, right, |left, right| {
            ExpressionKind::Comparison {
                operator,
                left,
                right,
            }
        })
    }

    fn parse_unary(&mut self, nesting: usize) -> Result<Expression, ConditionError> {
        if matches!(self.current().kind, TokenKind::Not) {
            let offset = self.current().offset;
            let nested = self.enter_nested(nesting, offset)?;
            self.advance();
            let inner = self.parse_unary(nested)?;
            let depth = inner.depth + 1;
            self.ensure_ast_depth(depth, offset)?;
            return Ok(Expression {
                kind: ExpressionKind::Not(Box::new(inner)),
                offset,
                depth,
            });
        }
        self.parse_primary(nesting)
    }

    fn parse_primary(&mut self, nesting: usize) -> Result<Expression, ConditionError> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::True => {
                self.advance();
                Ok(Expression::leaf(ExpressionKind::Bool(true), token.offset))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expression::leaf(ExpressionKind::Bool(false), token.offset))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expression::leaf(ExpressionKind::Null, token.offset))
            }
            TokenKind::Number(number) => {
                self.advance();
                Ok(Expression::leaf(
                    ExpressionKind::Number(number),
                    token.offset,
                ))
            }
            TokenKind::String(value) => {
                self.advance();
                Ok(Expression::leaf(
                    ExpressionKind::String(value),
                    token.offset,
                ))
            }
            TokenKind::RuntimeExpression(expression) => {
                self.advance();
                Ok(Expression::leaf(
                    ExpressionKind::RuntimeValue(expression),
                    token.offset,
                ))
            }
            TokenKind::LeftParen => {
                let nested = self.enter_nested(nesting, token.offset)?;
                self.advance();
                let expression = self.parse_or(nested)?;
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

    fn binary(
        &self,
        offset: usize,
        left: Expression,
        right: Expression,
        kind: impl FnOnce(Box<Expression>, Box<Expression>) -> ExpressionKind,
    ) -> Result<Expression, ConditionError> {
        let depth = left.depth.max(right.depth) + 1;
        self.ensure_ast_depth(depth, offset)?;
        Ok(Expression {
            kind: kind(Box::new(left), Box::new(right)),
            offset,
            depth,
        })
    }

    fn enter_nested(&self, nesting: usize, offset: usize) -> Result<usize, ConditionError> {
        let next = nesting + 1;
        if next > MAX_CONDITION_DEPTH {
            return Err(self.error(
                offset,
                format!("condition nesting exceeds {MAX_CONDITION_DEPTH} levels"),
            ));
        }
        Ok(next)
    }

    fn ensure_ast_depth(&self, depth: usize, offset: usize) -> Result<(), ConditionError> {
        if depth > MAX_CONDITION_DEPTH {
            return Err(self.error(
                offset,
                format!("condition expression exceeds {MAX_CONDITION_DEPTH} levels"),
            ));
        }
        Ok(())
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
    condition: &str,
    expression: &Expression,
    depth: usize,
) -> Result<(EvaluatedValue, Vec<ExpressionWarning>), ConditionError> {
    if depth > MAX_CONDITION_DEPTH {
        return Err(ConditionError {
            condition: condition.to_string(),
            offset: expression.offset,
            message: format!("condition evaluation exceeds {MAX_CONDITION_DEPTH} levels"),
        });
    }

    let evaluated = match &expression.kind {
        ExpressionKind::Null => (EvaluatedValue::Null, Vec::new()),
        ExpressionKind::Bool(value) => (EvaluatedValue::Bool(*value), Vec::new()),
        ExpressionKind::Number(value) => (EvaluatedValue::Number(value.clone()), Vec::new()),
        ExpressionKind::String(value) => (EvaluatedValue::String(value.clone()), Vec::new()),
        ExpressionKind::RuntimeValue(expression) => {
            let (value, warnings) = eval.evaluate_with_diagnostics(expression);
            (EvaluatedValue::Runtime(value), warnings)
        }
        ExpressionKind::Not(inner) => {
            let (value, warnings) = evaluate_expression(eval, condition, inner, depth + 1)?;
            (EvaluatedValue::Bool(!value.is_truthy()), warnings)
        }
        ExpressionKind::And(left, right) => {
            let (left, mut warnings) = evaluate_expression(eval, condition, left, depth + 1)?;
            if !left.is_truthy() {
                return Ok((EvaluatedValue::Bool(false), warnings));
            }
            let (right, right_warnings) = evaluate_expression(eval, condition, right, depth + 1)?;
            warnings.extend(right_warnings);
            (EvaluatedValue::Bool(right.is_truthy()), warnings)
        }
        ExpressionKind::Or(left, right) => {
            let (left, mut warnings) = evaluate_expression(eval, condition, left, depth + 1)?;
            if left.is_truthy() {
                return Ok((EvaluatedValue::Bool(true), warnings));
            }
            let (right, right_warnings) = evaluate_expression(eval, condition, right, depth + 1)?;
            warnings.extend(right_warnings);
            (EvaluatedValue::Bool(right.is_truthy()), warnings)
        }
        ExpressionKind::Comparison {
            operator,
            left,
            right,
        } => {
            let (left, mut warnings) = evaluate_expression(eval, condition, left, depth + 1)?;
            let (right, right_warnings) = evaluate_expression(eval, condition, right, depth + 1)?;
            warnings.extend(right_warnings);
            (
                EvaluatedValue::Bool(compare(*operator, &left, &right)),
                warnings,
            )
        }
    };
    Ok(evaluated)
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
            request_query: BTreeMap::from([("a==b".to_string(), "query-value".to_string())]),
            request_path: BTreeMap::from([("x&&y".to_string(), "path-value".to_string())]),
            response_headers: BTreeMap::from([("X||Y".to_string(), "header-value".to_string())]),
            response_body: Some(json!({
                "a==b": "pointer-value",
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
            ("true||false&&false", true),
            ("(false || true) && true", true),
            ("$statusCode == 200", true),
            ("$statusCode==200", true),
            ("$statusCode>=200&&$statusCode<300", true),
            ("$inputs.number==10", true),
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
    pub(crate) fn precedence_grouping_and_unary_are_deterministic() {
        assert_condition("true || false && false", true);
        assert_condition("(true || false) && false", false);
        assert_condition("!($statusCode == 404)", true);
        assert_condition("!!true", true);
        assert_condition("!2 < 1", false);
        assert_condition("!(2 < 1)", true);
    }

    #[test]
    pub(crate) fn parser_consumes_invalid_short_circuited_branches() {
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
    pub(crate) fn arbitrary_size_exponents_compare_without_saturation() {
        for (condition, expected) in [
            ("1e9223372036854775807 == 1e9223372036854775808", false),
            ("1e9223372036854775807 < 1e9223372036854775808", true),
            ("10e9223372036854775807 == 1e9223372036854775808", true),
            ("-1e9223372036854775807 > -1e9223372036854775808", true),
            ("1e-9223372036854775808 > 1e-9223372036854775809", true),
            ("0.1e9223372036854775808 == 1e9223372036854775807", true),
            ("0.1e-9223372036854775808 == 1e-9223372036854775809", true),
            (
                "'1e999999999999999999999' < '1e1000000000000000000000'",
                true,
            ),
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
    pub(crate) fn runtime_expression_legacy_traversal_remains_an_operand() {
        for condition in [
            "$response.body['name||title'] == 'Something'",
            "$response.body.users[?(@.role=='admin')].name == 'Alice'",
            "$response.body.users.#(role=='admin').name == 'Alice'",
        ] {
            assert_condition(condition, true);
        }
    }

    #[test]
    pub(crate) fn runtime_expression_names_own_operator_punctuation() {
        for (condition, expected) in [
            ("$response.header.X||Y", true),
            ("$request.query.a==b", true),
            ("$request.path.x&&y", true),
            ("$response.body#/a==b", true),
            ("$response.header.X||Y == 'HEADER-VALUE'", true),
            ("$request.query.a==b == 'query-value'", true),
            ("$request.path.x&&y == 'path-value'", true),
            ("$response.body#/a==b == 'pointer-value'", true),
        ] {
            assert_condition(condition, expected);
        }
    }

    #[test]
    pub(crate) fn excessive_parser_and_evaluator_depth_fails_closed() {
        let deep_unary = format!("{}true", "!".repeat(10_000));
        let deep_groups = format!("{}true{}", "(".repeat(10_000), ")".repeat(10_000));
        let left_deep = format!("{}true", "true && ".repeat(10_000));

        for condition in [deep_unary, deep_groups, left_deep] {
            let error = assert_syntax_error(&condition);
            assert_eq!(error.condition, condition);
            assert!(error.offset <= error.condition.len());
            assert!(error.condition.is_char_boundary(error.offset));
            assert!(error.message.contains("exceeds"), "{error:?}");
        }

        let leaf = Expression::leaf(ExpressionKind::Bool(true), 0);
        let error = match evaluate_expression(&evaluator(), "true", &leaf, MAX_CONDITION_DEPTH + 1)
        {
            Err(error) => error,
            Ok(result) => panic!("evaluator depth guard unexpectedly returned {result:?}"),
        };
        assert_eq!(error.condition, "true");
        assert_eq!(error.offset, 0);
        assert!(error.message.contains("evaluation exceeds"));
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
