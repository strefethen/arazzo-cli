#![forbid(unsafe_code)]

//! Expression parser and evaluator for Arazzo runtime expressions.

use std::collections::BTreeMap;
use std::env;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Number, Value};

/// Error produced when evaluating an Arazzo dot-notation path against a JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The path string could not be tokenized (e.g. unclosed bracket, empty filter).
    InvalidSyntax { path: String, detail: String },
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSyntax { path, detail } => {
                write!(f, "invalid path syntax \"{path}\": {detail}")
            }
        }
    }
}

impl std::error::Error for PathError {}

static INTERPOLATE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\{(\$[^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)")
        .unwrap_or_else(|err| panic!("failed to compile interpolate regex: {err}"))
});

/// Evaluation context for expression resolution.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub inputs: BTreeMap<String, Value>,
    pub steps: BTreeMap<String, BTreeMap<String, Value>>,
    pub outputs: BTreeMap<String, Value>,
    pub status_code: Option<i64>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub request_headers: BTreeMap<String, String>,
    pub request_query: BTreeMap<String, String>,
    pub request_path: BTreeMap<String, String>,
    pub request_body: Option<Value>,
    pub source_descriptions: BTreeMap<String, String>,
    pub response_headers: BTreeMap<String, String>,
    pub response_body: Option<Value>,
}

/// Evaluates expressions and conditions using an [`EvalContext`].
#[derive(Debug, Clone, Default)]
pub struct ExpressionEvaluator {
    ctx: EvalContext,
}

impl ExpressionEvaluator {
    pub fn new(ctx: EvalContext) -> Self {
        Self { ctx }
    }

    pub fn context(&self) -> &EvalContext {
        &self.ctx
    }

    pub fn context_mut(&mut self) -> &mut EvalContext {
        &mut self.ctx
    }

    /// Evaluate an expression and return a dynamic JSON value.
    pub fn evaluate(&self, expr: &str) -> Value {
        let Some(rest) = expr.strip_prefix('$') else {
            return Value::String(expr.to_string());
        };

        if let Some(name) = rest.strip_prefix("env.") {
            return Value::String(env::var(name).unwrap_or_default());
        }

        if let Some(name) = rest.strip_prefix("inputs.") {
            return self.ctx.inputs.get(name).cloned().unwrap_or(Value::Null);
        }

        if let Some(after) = rest.strip_prefix("steps.") {
            if let Some((step_id, output_name)) = after.split_once(".outputs.") {
                return self
                    .ctx
                    .steps
                    .get(step_id)
                    .and_then(|outputs| outputs.get(output_name))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            return Value::Null;
        }

        if rest == "statusCode" {
            return self
                .ctx
                .status_code
                .map(|code| json!(code))
                .unwrap_or(Value::Null);
        }

        if rest == "method" {
            return self
                .ctx
                .method
                .as_ref()
                .map(|m| Value::String(m.clone()))
                .unwrap_or(Value::Null);
        }

        if rest == "url" {
            return self
                .ctx
                .url
                .as_ref()
                .map(|u| Value::String(u.clone()))
                .unwrap_or(Value::Null);
        }

        if let Some(after) = rest.strip_prefix("outputs.") {
            if let Some((name, pointer)) = after.split_once('#') {
                return self
                    .ctx
                    .outputs
                    .get(name)
                    .and_then(|v| v.pointer(pointer))
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            return self.ctx.outputs.get(after).cloned().unwrap_or(Value::Null);
        }

        if let Some(name) = rest.strip_prefix("request.header.") {
            return get_header_case_insensitive(&self.ctx.request_headers, name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
        }

        if let Some(name) = rest.strip_prefix("request.query.") {
            return self
                .ctx
                .request_query
                .get(name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
        }

        if let Some(name) = rest.strip_prefix("request.path.") {
            return self
                .ctx
                .request_path
                .get(name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
        }

        if let Some(suffix) = rest.strip_prefix("request.body") {
            return resolve_body_access(&self.ctx.request_body, suffix);
        }

        if let Some(after) = rest.strip_prefix("sourceDescriptions.") {
            if let Some(name) = after.strip_suffix(".url") {
                return self
                    .ctx
                    .source_descriptions
                    .get(name)
                    .map(|u| Value::String(u.clone()))
                    .unwrap_or(Value::Null);
            }
            return Value::Null;
        }

        if let Some(name) = rest.strip_prefix("response.header.") {
            return get_header_case_insensitive(&self.ctx.response_headers, name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null);
        }

        if let Some(suffix) = rest.strip_prefix("response.body") {
            return resolve_body_access(&self.ctx.response_body, suffix);
        }

        Value::Null
    }

    /// Evaluate an expression and convert to string with Go-compatible coercions.
    pub fn evaluate_string(&self, expr: &str) -> String {
        to_string_value(&self.evaluate(expr))
    }

    /// Evaluate a condition expression with `||` and `&&` precedence.
    pub fn evaluate_condition(&self, condition: &str) -> bool {
        let condition = condition.trim();
        if condition.is_empty() {
            return false;
        }

        if let Some(parts) = split_outside_quotes(condition, "||") {
            for part in parts {
                if self.evaluate_condition(part) {
                    return true;
                }
            }
            return false;
        }

        if let Some(parts) = split_outside_quotes(condition, "&&") {
            for part in parts {
                if !self.evaluate_condition(part) {
                    return false;
                }
            }
            return true;
        }

        self.evaluate_comparison(condition)
    }

    /// Interpolate `{$expr}` and `$inputs.foo` style segments in a string.
    pub fn interpolate_string(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut cursor = 0usize;

        for captures in INTERPOLATE_RE.captures_iter(input) {
            let Some(full) = captures.get(0) else {
                continue;
            };

            out.push_str(&input[cursor..full.start()]);

            let expr = if let Some(inner) = captures.get(1) {
                inner.as_str().to_string()
            } else {
                full.as_str().to_string()
            };
            out.push_str(&self.evaluate_string(&expr));
            cursor = full.end();
        }

        out.push_str(&input[cursor..]);
        out
    }

    fn evaluate_comparison(&self, condition: &str) -> bool {
        let (op, idx) = find_operator(condition);
        if op.is_empty() {
            return is_truthy(&resolve_operand(self, condition));
        }

        let left = resolve_operand(self, &condition[..idx]);
        let right = condition[idx + op.len()..].trim();

        match op.as_str() {
            "==" => compare_values(&left, &resolve_operand(self, right)),
            "!=" => !compare_values(&left, &resolve_operand(self, right)),
            ">" => compare_ordered(&left, &resolve_operand(self, right)) > 0,
            "<" => compare_ordered(&left, &resolve_operand(self, right)) < 0,
            ">=" => compare_ordered(&left, &resolve_operand(self, right)) >= 0,
            "<=" => compare_ordered(&left, &resolve_operand(self, right)) <= 0,
            " contains " => {
                let right_val = resolve_operand(self, right);
                to_string_value(&left).contains(&to_string_value(&right_val))
            }
            " matches " => {
                let right_val = resolve_operand(self, right);
                let pattern = to_string_value(&right_val);
                match Regex::new(&pattern) {
                    Ok(re) => re.is_match(&to_string_value(&left)),
                    Err(_) => false,
                }
            }
            " in " => eval_in(self, &left, right),
            _ => false,
        }
    }
}

fn get_header_case_insensitive<'a>(
    headers: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    if let Some(value) = headers.get(name) {
        return Some(value);
    }
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

fn resolve_operand(eval: &ExpressionEvaluator, raw: &str) -> Value {
    let token = raw.trim();
    if token.starts_with('$') {
        eval.evaluate(token)
    } else {
        parse_value(token)
    }
}

fn split_outside_quotes<'a>(input: &'a str, delim: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut found = false;

    for (idx, ch) in input.char_indices() {
        if idx < start {
            continue;
        }
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }

        if input[idx..].starts_with(delim) {
            parts.push(input[start..idx].trim());
            start = idx + delim.len();
            found = true;
        }
    }

    if !found {
        return None;
    }
    parts.push(input[start..].trim());
    Some(parts)
}

fn find_operator(input: &str) -> (String, usize) {
    for word_op in [" contains ", " matches ", " in "] {
        if let Some(idx) = index_outside_quotes(input, word_op) {
            return (word_op.to_string(), idx);
        }
    }

    let mut in_quote: Option<char> = None;
    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }

        if input[idx..].starts_with("!=") {
            return ("!=".to_string(), idx);
        }
        if input[idx..].starts_with(">=") {
            return (">=".to_string(), idx);
        }
        if input[idx..].starts_with("<=") {
            return ("<=".to_string(), idx);
        }
        if input[idx..].starts_with("==") {
            return ("==".to_string(), idx);
        }

        if ch == '>' || ch == '<' {
            return (ch.to_string(), idx);
        }
    }

    (String::new(), usize::MAX)
}

fn index_outside_quotes(input: &str, needle: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }
        if input[idx..].starts_with(needle) {
            return Some(idx);
        }
    }
    None
}

fn eval_in(eval: &ExpressionEvaluator, left: &Value, list_expr: &str) -> bool {
    let list_expr = list_expr.trim();
    if !(list_expr.starts_with('[') && list_expr.ends_with(']')) {
        return false;
    }
    let inner = &list_expr[1..list_expr.len() - 1];
    if inner.trim().is_empty() {
        return false;
    }

    for token in split_list_elements(inner) {
        if compare_values(left, &resolve_operand(eval, token)) {
            return true;
        }
    }
    false
}

fn split_list_elements(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;

    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            continue;
        }
        if ch == ',' {
            parts.push(input[start..idx].trim());
            start = idx + 1;
        }
    }
    parts.push(input[start..].trim());
    parts
}

fn parse_value(token: &str) -> Value {
    let token = token.trim();

    if let Ok(v) = token.parse::<i64>() {
        return Value::Number(Number::from(v));
    }
    if let Ok(v) = token.parse::<f64>() {
        if let Some(number) = Number::from_f64(v) {
            return Value::Number(number);
        }
    }
    match token {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => {
            if token.len() >= 2 {
                let bytes = token.as_bytes();
                if (bytes[0] == b'"' && bytes[token.len() - 1] == b'"')
                    || (bytes[0] == b'\'' && bytes[token.len() - 1] == b'\'')
                {
                    return Value::String(token[1..token.len() - 1].to_string());
                }
            }
            Value::String(token.to_string())
        }
    }
}

fn compare_values(a: &Value, b: &Value) -> bool {
    if a.is_null() && b.is_null() {
        return true;
    }
    if a.is_null() || b.is_null() {
        return false;
    }

    if let (Some(lhs), Some(rhs)) = (to_f64(a), to_f64(b)) {
        return lhs == rhs;
    }

    to_string_value(a) == to_string_value(b)
}

fn compare_ordered(a: &Value, b: &Value) -> i8 {
    if let (Some(lhs), Some(rhs)) = (to_f64(a), to_f64(b)) {
        if lhs < rhs {
            return -1;
        }
        if lhs > rhs {
            return 1;
        }
        return 0;
    }

    let lhs = to_string_value(a);
    let rhs = to_string_value(b);
    if lhs < rhs {
        -1
    } else if lhs > rhs {
        1
    } else {
        0
    }
}

fn to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn to_string_value(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => String::new(),
    }
}

pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}

fn resolve_body_access(body: &Option<Value>, suffix: &str) -> Value {
    if suffix.is_empty() {
        return body.clone().unwrap_or(Value::Null);
    }
    if let Some(pointer) = suffix.strip_prefix('#') {
        if let Some(b) = body {
            return b.pointer(pointer).cloned().unwrap_or(Value::Null);
        }
        return Value::Null;
    }
    if let Some(path) = suffix.strip_prefix('.') {
        if let Some(b) = body {
            return resolve_dot_path(b, path).unwrap_or(Value::Null);
        }
        return Value::Null;
    }
    Value::Null
}

fn resolve_dot_path(root: &Value, path: &str) -> Result<Value, PathError> {
    if path.is_empty() {
        return Ok(root.clone());
    }
    let tokens = tokenize_path(path)?;
    if tokens.is_empty() {
        return Ok(Value::Null);
    }

    let mut current = vec![root];
    for (idx, token) in tokens.iter().copied().enumerate() {
        let is_last = idx + 1 == tokens.len();
        if matches!(token, PathToken::Hash) && is_last {
            return Ok(terminal_hash_value(&current));
        }
        current = apply_path_token(&current, token);
        if current.is_empty() {
            return Ok(Value::Null);
        }
    }

    if current.len() == 1 {
        Ok(current[0].clone())
    } else {
        Ok(Value::Array(current.into_iter().cloned().collect()))
    }
}

#[derive(Debug, Clone, Copy)]
enum PathToken<'a> {
    Field(&'a str),
    Index(usize),
    Wildcard,
    Hash,
    Filter {
        expr: FilterExpr<'a>,
        all_matches: bool,
    },
}

#[derive(Debug, Clone, Copy)]
struct FilterExpr<'a> {
    path: &'a str,
    op: Option<FilterOp>,
    value_raw: &'a str,
}

#[derive(Debug, Clone, Copy)]
enum FilterOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

fn apply_path_token<'a>(nodes: &[&'a Value], token: PathToken<'a>) -> Vec<&'a Value> {
    let mut out = Vec::new();

    match token {
        PathToken::Field(name) => {
            for node in nodes {
                if let Some(obj) = node.as_object() {
                    if let Some(value) = obj.get(name) {
                        out.push(value);
                        continue;
                    }
                }

                if let Ok(idx) = name.parse::<usize>() {
                    if let Some(arr) = node.as_array() {
                        if let Some(value) = arr.get(idx) {
                            out.push(value);
                        }
                    }
                }
            }
        }
        PathToken::Index(idx) => {
            for node in nodes {
                if let Some(arr) = node.as_array() {
                    if let Some(value) = arr.get(idx) {
                        out.push(value);
                    }
                }
            }
        }
        PathToken::Wildcard => {
            for node in nodes {
                if let Some(arr) = node.as_array() {
                    out.extend(arr.iter());
                } else if let Some(obj) = node.as_object() {
                    out.extend(obj.values());
                }
            }
        }
        PathToken::Hash => {
            for node in nodes {
                if let Some(arr) = node.as_array() {
                    out.extend(arr.iter());
                }
            }
        }
        PathToken::Filter { expr, all_matches } => {
            for node in nodes {
                if let Some(arr) = node.as_array() {
                    for item in arr {
                        if filter_matches(item, expr) {
                            out.push(item);
                            if !all_matches {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

fn terminal_hash_value(nodes: &[&Value]) -> Value {
    if nodes.len() == 1 {
        return node_len(nodes[0]).map_or(Value::Null, |len| json!(len));
    }

    let values = nodes
        .iter()
        .map(|node| node_len(node).map_or(Value::Null, |len| json!(len)))
        .collect::<Vec<_>>();
    Value::Array(values)
}

fn node_len(node: &Value) -> Option<usize> {
    match node {
        Value::Array(items) => Some(items.len()),
        Value::Object(items) => Some(items.len()),
        _ => None,
    }
}

fn filter_matches(item: &Value, expr: FilterExpr<'_>) -> bool {
    let path = expr.path.strip_prefix("@.").unwrap_or(expr.path);
    let left = if path.is_empty() || path == "@" || path == "$" {
        item.clone()
    } else {
        resolve_dot_path(item, path).unwrap_or(Value::Null)
    };

    match expr.op {
        None => is_truthy(&left),
        Some(op) => {
            let right = parse_value(expr.value_raw);
            match op {
                FilterOp::Eq => compare_values(&left, &right),
                FilterOp::Ne => !compare_values(&left, &right),
                FilterOp::Gt => compare_ordered(&left, &right) > 0,
                FilterOp::Lt => compare_ordered(&left, &right) < 0,
                FilterOp::Ge => compare_ordered(&left, &right) >= 0,
                FilterOp::Le => compare_ordered(&left, &right) <= 0,
            }
        }
    }
}

fn tokenize_path(path: &str) -> Result<Vec<PathToken<'_>>, PathError> {
    let mut tokens = Vec::new();
    for segment in split_path_segments(path) {
        push_segment_tokens(segment, &mut tokens, path)?;
    }
    Ok(tokens)
}

fn split_path_segments(path: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (idx, ch) in path.char_indices() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == q {
                in_quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' => in_quote = Some(ch),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '.' if paren_depth == 0 && bracket_depth == 0 => {
                if start < idx {
                    out.push(&path[start..idx]);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }

    if start < path.len() {
        out.push(&path[start..]);
    }

    out.into_iter()
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn push_segment_tokens<'a>(
    segment: &'a str,
    out: &mut Vec<PathToken<'a>>,
    full_path: &str,
) -> Result<(), PathError> {
    let segment = segment.trim();
    if segment.is_empty() {
        return Ok(());
    }

    if segment == "*" {
        out.push(PathToken::Wildcard);
        return Ok(());
    }
    if segment == "#" {
        out.push(PathToken::Hash);
        return Ok(());
    }

    if segment.starts_with("#(") {
        if let Some((inner, all_matches)) = parse_filter_segment(segment) {
            if let Some(expr) = parse_filter_expr(inner) {
                out.push(PathToken::Filter { expr, all_matches });
                return Ok(());
            }
        }
        return Err(PathError::InvalidSyntax {
            path: full_path.to_string(),
            detail: format!("unbalanced filter expression: {segment}"),
        });
    }

    if segment.contains('[') {
        push_bracket_tokens(segment, out, full_path)?;
        return Ok(());
    }

    out.push(PathToken::Field(segment));
    Ok(())
}

fn parse_filter_segment(segment: &str) -> Option<(&str, bool)> {
    if !segment.starts_with("#(") {
        return None;
    }
    if segment.ends_with(")#") {
        return Some((&segment[2..segment.len() - 2], true));
    }
    if segment.ends_with(')') {
        return Some((&segment[2..segment.len() - 1], false));
    }
    None
}

fn parse_filter_expr(inner: &str) -> Option<FilterExpr<'_>> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }

    for (symbol, op) in [
        (">=", FilterOp::Ge),
        ("<=", FilterOp::Le),
        ("==", FilterOp::Eq),
        ("!=", FilterOp::Ne),
        (">", FilterOp::Gt),
        ("<", FilterOp::Lt),
    ] {
        if let Some(idx) = index_outside_quotes(inner, symbol) {
            let path = inner[..idx].trim();
            let value_raw = inner[idx + symbol.len()..].trim();
            if path.is_empty() || value_raw.is_empty() {
                return None;
            }
            return Some(FilterExpr {
                path,
                op: Some(op),
                value_raw,
            });
        }
    }

    Some(FilterExpr {
        path: inner,
        op: None,
        value_raw: "",
    })
}

fn push_bracket_tokens<'a>(
    segment: &'a str,
    out: &mut Vec<PathToken<'a>>,
    full_path: &str,
) -> Result<(), PathError> {
    let mut cursor = 0usize;

    while cursor < segment.len() {
        let Some(open_rel) = segment[cursor..].find('[') else {
            break;
        };
        let open = cursor + open_rel;

        if cursor < open {
            out.push(PathToken::Field(&segment[cursor..open]));
        }

        let Some(close_rel) = segment[open + 1..].find(']') else {
            return Err(PathError::InvalidSyntax {
                path: full_path.to_string(),
                detail: format!("unclosed bracket in: {segment}"),
            });
        };
        let close = open + 1 + close_rel;
        let index_expr = segment[open + 1..close].trim();

        if index_expr == "*" {
            out.push(PathToken::Wildcard);
        } else if let Ok(idx) = index_expr.parse::<usize>() {
            out.push(PathToken::Index(idx));
        } else if !index_expr.is_empty() {
            out.push(PathToken::Field(index_expr));
        }

        cursor = close + 1;
    }

    if cursor < segment.len() {
        out.push(PathToken::Field(&segment[cursor..]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{compare_ordered, compare_values, parse_value, EvalContext, ExpressionEvaluator};
    use proptest::prelude::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    #[test]
    fn evaluate_literal_and_unknown_expression() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("hello"), json!("hello"));
        assert_eq!(eval.evaluate("$unknown.thing"), Value::Null);
    }

    #[test]
    fn evaluate_inputs_and_step_outputs() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        ctx.steps.insert(
            "s1".to_string(),
            BTreeMap::from([("token".to_string(), json!("abc"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$inputs.name"), json!("Alice"));
        assert_eq!(eval.evaluate("$inputs.missing"), Value::Null);
        assert_eq!(eval.evaluate("$steps.s1.outputs.token"), json!("abc"));
        assert_eq!(eval.evaluate("$steps.nope.outputs.token"), Value::Null);
        assert_eq!(eval.evaluate("$steps.s1.token"), Value::Null);
    }

    #[test]
    fn evaluate_response_fields() {
        let mut ctx = EvalContext {
            status_code: Some(404),
            response_body: Some(json!({
                "user": {"name": "Bob"},
                "arr": [{"id": 7}],
                "users": [
                    {"id": 1, "name": "Alice", "group": "a"},
                    {"id": 2, "name": "Bob", "group": "b"},
                    {"id": 3, "name": "Cara", "group": "a"}
                ]
            })),
            ..EvalContext::default()
        };
        ctx.response_headers
            .insert("X-Request-Id".to_string(), "req-1".to_string());
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$statusCode"), json!(404));
        assert_eq!(
            eval.evaluate("$response.header.X-Request-Id"),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.header.x-request-id"),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.body"),
            json!({
                "user": {"name": "Bob"},
                "arr": [{"id": 7}],
                "users": [
                    {"id": 1, "name": "Alice", "group": "a"},
                    {"id": 2, "name": "Bob", "group": "b"},
                    {"id": 3, "name": "Cara", "group": "a"}
                ]
            })
        );
        assert_eq!(eval.evaluate("$response.body.user.name"), json!("Bob"));
        assert_eq!(eval.evaluate("$response.body.arr[0].id"), json!(7));
        assert_eq!(eval.evaluate("$response.body.arr.0.id"), json!(7));
        assert_eq!(eval.evaluate("$response.body.arr.#"), json!(1));
        assert_eq!(
            eval.evaluate("$response.body.users.#.name"),
            json!(["Alice", "Bob", "Cara"])
        );
        assert_eq!(
            eval.evaluate(r#"$response.body.users.#(id==2).name"#),
            json!("Bob")
        );
        assert_eq!(
            eval.evaluate(r#"$response.body.users.#(group=="a")#.id"#),
            json!([1, 3])
        );
        assert_eq!(
            eval.evaluate("$response.body.users[*].id"),
            json!([1, 2, 3])
        );
        assert_eq!(eval.evaluate("$response.body.missing"), Value::Null);
    }

    #[test]
    fn evaluate_response_fields_without_response() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$statusCode"), Value::Null);
        assert_eq!(eval.evaluate("$response.header.X-Foo"), Value::Null);
        assert_eq!(eval.evaluate("$response.body.user.name"), Value::Null);
    }

    #[test]
    fn evaluate_env_var() {
        std::env::set_var("ARAZZO_EXPR_TEST_ENV", "secret");
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$env.ARAZZO_EXPR_TEST_ENV"), json!("secret"));
    }

    #[test]
    fn evaluate_string_coercions() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("s".to_string(), json!("hello"));
        ctx.inputs.insert("f".to_string(), json!(2.5));
        ctx.inputs.insert("i".to_string(), json!(42));
        ctx.inputs.insert("t".to_string(), json!(true));
        ctx.inputs.insert("f2".to_string(), json!(false));
        ctx.inputs.insert("arr".to_string(), json!([1, 2]));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate_string("$inputs.missing"), "");
        assert_eq!(eval.evaluate_string("$inputs.s"), "hello");
        assert_eq!(eval.evaluate_string("$inputs.f"), "2.5");
        assert_eq!(eval.evaluate_string("$inputs.i"), "42");
        assert_eq!(eval.evaluate_string("$inputs.t"), "true");
        assert_eq!(eval.evaluate_string("$inputs.f2"), "false");
        assert_eq!(eval.evaluate_string("$inputs.arr"), "");
    }

    #[test]
    fn evaluate_condition_core_ops() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("$statusCode == 200"));
        assert!(eval.evaluate_condition("$statusCode != 404"));
        assert!(eval.evaluate_condition("$statusCode > 199"));
        assert!(eval.evaluate_condition("$statusCode < 300"));
        assert!(eval.evaluate_condition("$statusCode >= 200"));
        assert!(eval.evaluate_condition("$statusCode <= 200"));
        assert!(eval.evaluate_condition("$statusCode >= 200 && $statusCode < 300"));
        assert!(eval.evaluate_condition("$statusCode == 200 || $statusCode == 201"));
        assert!(!eval.evaluate_condition("$statusCode == 500"));
    }

    #[test]
    fn evaluate_condition_and_or_precedence() {
        let eval200 = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval200
            .evaluate_condition("$statusCode == 200 || $statusCode == 404 && $statusCode == 500"));

        let eval404 = ExpressionEvaluator::new(EvalContext {
            status_code: Some(404),
            ..EvalContext::default()
        });
        assert!(eval404
            .evaluate_condition("$statusCode == 200 || $statusCode == 404 && $statusCode == 404"));
    }

    #[test]
    fn evaluate_condition_contains_matches_and_in() {
        let mut ctx = EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        };
        ctx.steps.insert(
            "s1".to_string(),
            BTreeMap::from([
                ("msg".to_string(), json!("hello world")),
                ("email".to_string(), json!("alice@example.com")),
                ("val".to_string(), json!("hello, world")),
                ("role".to_string(), json!("admin")),
            ]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "world""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.msg contains "xyz""#));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.email matches "^[a-z]+@""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.email matches "^[0-9]+""#));
        assert!(!eval.evaluate_condition(r#"$steps.s1.outputs.email matches "[invalid""#));
        assert!(eval.evaluate_condition("$statusCode in [200, 201, 204]"));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.role in ["admin", "superadmin"]"#));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.val in ["hello, world", "foo"]"#));
        assert!(!eval.evaluate_condition("$statusCode in []"));
    }

    #[test]
    fn evaluate_condition_expression_both_sides() {
        let mut ctx = EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        };
        ctx.inputs.insert("expected".to_string(), json!(200));
        let eval = ExpressionEvaluator::new(ctx);
        assert!(eval.evaluate_condition("$statusCode == $inputs.expected"));
    }

    #[test]
    fn evaluate_condition_truthiness_and_quoted_operators() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("flag".to_string(), json!(true));
        ctx.inputs.insert("zero".to_string(), json!(0));
        ctx.inputs.insert("empty".to_string(), json!(""));
        ctx.steps.insert(
            "s1".to_string(),
            BTreeMap::from([("msg".to_string(), json!("status >= ok"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert!(eval.evaluate_condition("$inputs.flag"));
        assert!(!eval.evaluate_condition("$inputs.zero"));
        assert!(!eval.evaluate_condition("$inputs.empty"));
        assert!(!eval.evaluate_condition("$inputs.missing"));
        assert!(eval.evaluate_condition("just a string"));
        assert!(!eval.evaluate_condition(""));
        assert!(eval.evaluate_condition(r#"$steps.s1.outputs.msg == "status >= ok""#));
    }

    #[test]
    fn compare_ordered_matches_go_rules() {
        assert_eq!(compare_ordered(&json!(100), &json!(200)), -1);
        assert_eq!(compare_ordered(&json!(200), &json!(200)), 0);
        assert_eq!(compare_ordered(&json!(300), &json!(200)), 1);
        assert_eq!(compare_ordered(&json!("apple"), &json!("banana")), -1);
        assert_eq!(compare_ordered(&json!(10), &json!(10.0)), 0);
    }

    #[test]
    fn parse_value_variants() {
        assert_eq!(parse_value("42"), json!(42));
        assert_eq!(parse_value("2.5"), json!(2.5));
        assert_eq!(parse_value("true"), json!(true));
        assert_eq!(parse_value("false"), json!(false));
        assert_eq!(parse_value(r#""hello""#), json!("hello"));
        assert_eq!(parse_value("'world'"), json!("world"));
        assert_eq!(parse_value("abc"), json!("abc"));
        assert_eq!(parse_value("  200  "), json!(200));
        assert_eq!(parse_value("'"), json!("'"));
    }

    #[test]
    fn compare_values_variants() {
        assert!(compare_values(&Value::Null, &Value::Null));
        assert!(!compare_values(&Value::Null, &json!(1)));
        assert!(!compare_values(&json!("a"), &Value::Null));
        assert!(compare_values(&json!(200), &json!(200.0)));
        assert!(compare_values(&json!(42), &json!(42)));
        assert!(compare_values(&json!("hello"), &json!("hello")));
        assert!(!compare_values(&json!("hello"), &json!("world")));
    }

    #[test]
    fn method_expression() {
        let ctx = EvalContext {
            method: Some("GET".to_string()),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$method"), json!("GET"));

        let ctx_no_method = EvalContext::default();
        let eval_no_method = ExpressionEvaluator::new(ctx_no_method);
        assert_eq!(eval_no_method.evaluate("$method"), Value::Null);
    }

    #[test]
    fn interpolate_string_modes() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        ctx.inputs.insert("age".to_string(), json!(30));
        ctx.inputs.insert("a".to_string(), json!("X"));
        ctx.steps.insert(
            "s1".to_string(),
            BTreeMap::from([("b".to_string(), json!("Y"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.interpolate_string("Hello {$inputs.name}!"),
            "Hello Alice!"
        );
        assert_eq!(eval.interpolate_string("Age: $inputs.age"), "Age: 30");
        assert_eq!(
            eval.interpolate_string("{$inputs.a}-$steps.s1.outputs.b"),
            "X-Y"
        );
        assert_eq!(eval.interpolate_string("plain text"), "plain text");
        assert_eq!(
            eval.interpolate_string("Bearer {$inputs.name}"),
            "Bearer Alice"
        );
    }

    proptest! {
        #[test]
        fn interpolate_string_preserves_prefix_and_suffix(
            prefix in "[^$]{0,24}",
            value in "[a-zA-Z0-9 _\\-]{0,24}",
            suffix in "[^$]{0,24}",
        ) {
            let mut ctx = EvalContext::default();
            ctx.inputs.insert("token".to_string(), json!(value.clone()));
            let eval = ExpressionEvaluator::new(ctx);

            let expr = format!("{prefix}{{$inputs.token}}{suffix}");
            let rendered = eval.interpolate_string(&expr);
            prop_assert_eq!(rendered, format!("{prefix}{value}{suffix}"));
        }

        #[test]
        fn response_array_len_and_index_extraction_are_consistent(
            values in proptest::collection::vec(any::<i64>(), 0..20),
            idx in 0usize..25usize,
        ) {
            let eval = ExpressionEvaluator::new(EvalContext {
                response_body: Some(json!({"arr": values.clone()})),
                ..EvalContext::default()
            });

            let len_value = eval.evaluate("$response.body.arr.#");
            prop_assert_eq!(len_value, json!(values.len()));

            let at_value = eval.evaluate(&format!("$response.body.arr[{idx}]"));
            if idx < values.len() {
                prop_assert_eq!(at_value, json!(values[idx]));
            } else {
                prop_assert_eq!(at_value, Value::Null);
            }
        }

        #[test]
        fn evaluate_condition_fuzz_input_does_not_panic(condition in ".{0,96}") {
            let eval = ExpressionEvaluator::new(EvalContext::default());
            let _ = eval.evaluate_condition(&condition);
        }

        #[test]
        fn resolve_dot_path_fuzz_does_not_panic(path in ".{0,128}") {
            let root = json!({"a": [1, {"b": "c"}, [2, 3]], "d": null});
            let _ = super::resolve_dot_path(&root, &path);
        }

        #[test]
        fn resolve_dot_path_valid_field_chain_is_ok(
            keys in proptest::collection::vec("[a-z]{1,8}", 1..5),
        ) {
            let mut value = json!("leaf");
            for key in keys.iter().rev() {
                value = json!({ key.as_str(): value });
            }
            let path = keys.join(".");
            match super::resolve_dot_path(&value, &path) {
                Ok(v) => prop_assert_eq!(v, json!("leaf")),
                Err(e) => prop_assert!(false, "valid path should be Ok, got {e:?}"),
            }
        }

        #[test]
        fn resolve_dot_path_bracket_index_consistency(
            values in proptest::collection::vec(any::<i64>(), 0..20),
            idx in 0usize..30usize,
        ) {
            let root = json!(values);
            match super::resolve_dot_path(&root, &format!("[{idx}]")) {
                Ok(value) => {
                    if idx < values.len() {
                        prop_assert_eq!(value, json!(values[idx]));
                    } else {
                        prop_assert_eq!(value, Value::Null);
                    }
                }
                Err(e) => prop_assert!(false, "bracket index should be Ok, got {e:?}"),
            }
        }
    }

    #[test]
    fn resolve_dot_path_unclosed_bracket_is_error() {
        let root = json!({"foo": [1, 2]});
        let result = super::resolve_dot_path(&root, "foo[0");
        assert!(
            matches!(result, Err(super::PathError::InvalidSyntax { .. })),
            "expected InvalidSyntax, got {result:?}"
        );
    }

    #[test]
    fn resolve_dot_path_unbalanced_filter_is_error() {
        let root = json!({"arr": [{"id": 1}]});
        let result = super::resolve_dot_path(&root, "#(id==1");
        assert!(
            matches!(result, Err(super::PathError::InvalidSyntax { .. })),
            "expected InvalidSyntax, got {result:?}"
        );
    }

    #[test]
    fn resolve_dot_path_negative_index_on_array_returns_null() {
        let root = json!([1, 2, 3]);
        assert_eq!(
            super::resolve_dot_path(&root, "[-1]"),
            Ok(Value::Null),
            "negative index should not be a syntax error"
        );
    }

    #[test]
    fn resolve_dot_path_negative_index_on_object_returns_value() {
        let root = json!({"-1": "found"});
        assert_eq!(super::resolve_dot_path(&root, "[-1]"), Ok(json!("found")));
    }

    #[test]
    fn resolve_dot_path_null_field_returns_ok_null() {
        let root = json!({"a": null});
        assert_eq!(super::resolve_dot_path(&root, "a"), Ok(Value::Null));
    }

    #[test]
    fn resolve_dot_path_consecutive_dots_is_lenient() {
        let root = json!({"a": {"b": 42}});
        assert_eq!(super::resolve_dot_path(&root, "a..b"), Ok(json!(42)));
    }

    #[test]
    fn evaluate_outputs_expression() {
        let mut ctx = EvalContext::default();
        ctx.outputs.insert("total".to_string(), json!(42));
        ctx.outputs
            .insert("nested".to_string(), json!({"a": {"b": "deep"}}));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$outputs.total"), json!(42));
        assert_eq!(eval.evaluate("$outputs.missing"), Value::Null);
        assert_eq!(
            eval.evaluate("$outputs.nested"),
            json!({"a": {"b": "deep"}})
        );
    }

    #[test]
    fn evaluate_outputs_json_pointer() {
        let mut ctx = EvalContext::default();
        ctx.outputs
            .insert("data".to_string(), json!({"items": [{"id": 1}, {"id": 2}]}));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$outputs.data#/items/0/id"), json!(1));
        assert_eq!(eval.evaluate("$outputs.data#/items/1/id"), json!(2));
        assert_eq!(eval.evaluate("$outputs.data#/missing"), Value::Null);
    }

    #[test]
    fn evaluate_response_body_json_pointer() {
        let ctx = EvalContext {
            response_body: Some(json!({
                "data": [{"name": "Alice"}, {"name": "Bob"}],
                "meta": {"total": 2}
            })),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$response.body#/data/0/name"), json!("Alice"));
        assert_eq!(eval.evaluate("$response.body#/data/1/name"), json!("Bob"));
        assert_eq!(eval.evaluate("$response.body#/meta/total"), json!(2));
        assert_eq!(eval.evaluate("$response.body#/nonexistent"), Value::Null);
    }

    #[test]
    fn evaluate_response_body_json_pointer_without_body() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$response.body#/data/0"), Value::Null);
    }
}
