#![forbid(unsafe_code)]

//! Expression parser and evaluator for Arazzo runtime expressions.

use std::collections::BTreeMap;
use std::env;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Number, Value};

static INTERPOLATE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{([^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)")
        .unwrap_or_else(|err| panic!("failed to compile interpolate regex: {err}"))
});

/// Evaluation context for expression resolution.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub inputs: BTreeMap<String, Value>,
    pub steps: BTreeMap<String, BTreeMap<String, Value>>,
    pub status_code: Option<i64>,
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

        if let Some(name) = rest.strip_prefix("response.header.") {
            if let Some(value) = self.ctx.response_headers.get(name) {
                return Value::String(value.clone());
            }
            if let Some((_, value)) = self
                .ctx
                .response_headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
            {
                return Value::String(value.clone());
            }
            return Value::Null;
        }

        if let Some(path) = rest.strip_prefix("response.body.") {
            if let Some(body) = &self.ctx.response_body {
                return extract_json_path(body, path)
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            return Value::Null;
        }

        Value::Null
    }

    /// Evaluate an expression and convert to string with Go-compatible coercions.
    pub fn evaluate_string(&self, expr: &str) -> String {
        to_string_coerce(&self.evaluate(expr))
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

    /// Interpolate `${expr}` and `$inputs.foo` style segments in a string.
    pub fn interpolate_string(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut cursor = 0usize;

        for captures in INTERPOLATE_RE.captures_iter(input) {
            let Some(full) = captures.get(0) else {
                continue;
            };

            out.push_str(&input[cursor..full.start()]);

            let expr = if let Some(inner) = captures.get(1) {
                format!("${}", inner.as_str())
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
    let bytes = input.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
            idx += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
            idx += 1;
            continue;
        }

        if idx + 1 < bytes.len() {
            match &input[idx..idx + 2] {
                "!=" | ">=" | "<=" | "==" => return (input[idx..idx + 2].to_string(), idx),
                _ => {}
            }
        }

        if ch == '>' || ch == '<' {
            return (input[idx..idx + 1].to_string(), idx);
        }
        idx += 1;
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

fn to_string_coerce(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(v) => v.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => String::new(),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}

fn extract_json_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    let tokens = tokenize_path(path);
    let mut current = root;

    for token in tokens {
        match token {
            PathToken::Field(name) => {
                current = current.get(name)?;
            }
            PathToken::Index(idx) => {
                let arr = current.as_array()?;
                current = arr.get(idx)?;
            }
        }
    }
    Some(current)
}

#[derive(Debug)]
enum PathToken<'a> {
    Field(&'a str),
    Index(usize),
}

fn tokenize_path(path: &str) -> Vec<PathToken<'_>> {
    let mut tokens = Vec::new();
    let chars = path.as_bytes();
    let mut start = 0usize;
    let mut idx = 0usize;

    while idx < chars.len() {
        match chars[idx] {
            b'.' => {
                if start < idx {
                    tokens.push(PathToken::Field(&path[start..idx]));
                }
                start = idx + 1;
                idx += 1;
            }
            b'[' => {
                if start < idx {
                    tokens.push(PathToken::Field(&path[start..idx]));
                }
                let mut end = idx + 1;
                while end < chars.len() && chars[end] != b']' {
                    end += 1;
                }
                if end < chars.len() {
                    if let Ok(n) = path[idx + 1..end].parse::<usize>() {
                        tokens.push(PathToken::Index(n));
                    }
                    idx = end + 1;
                    if idx < chars.len() && chars[idx] == b'.' {
                        idx += 1;
                    }
                    start = idx;
                } else {
                    break;
                }
            }
            _ => idx += 1,
        }
    }

    if start < path.len() {
        tokens.push(PathToken::Field(&path[start..]));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::{compare_ordered, compare_values, parse_value, EvalContext, ExpressionEvaluator};
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
            response_body: Some(json!({"user": {"name": "Bob"}, "arr": [{"id": 7}]})),
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
        assert_eq!(eval.evaluate("$response.body.user.name"), json!("Bob"));
        assert_eq!(eval.evaluate("$response.body.arr[0].id"), json!(7));
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
            eval.interpolate_string("Hello ${inputs.name}!"),
            "Hello Alice!"
        );
        assert_eq!(eval.interpolate_string("Age: $inputs.age"), "Age: 30");
        assert_eq!(
            eval.interpolate_string("${inputs.a}-$steps.s1.outputs.b"),
            "X-Y"
        );
        assert_eq!(eval.interpolate_string("plain text"), "plain text");
    }
}
