#![forbid(unsafe_code)]

//! Expression parser and evaluator for Arazzo runtime expressions.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;

use std::sync::LazyLock;

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

/// Warning produced when an expression resolves to `Null` due to a missing key,
/// unknown step, or unrecognised namespace. Collected by
/// [`ExpressionEvaluator::evaluate_with_diagnostics`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionWarning {
    pub expression: String,
    pub message: String,
}

/// Value and cardinality produced by a supported JSONPath selection.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPathSelection {
    /// `Null` for zero matches, the selected value for one match, or an array
    /// preserving traversal order for multiple matches.
    pub value: Value,
    /// Number of nodes selected before cardinality collapse.
    pub match_count: usize,
}

impl std::fmt::Display for ExpressionWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.expression, self.message)
    }
}

static INTERPOLATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{(\$[^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)")
        .unwrap_or_else(|err| panic!("failed to compile interpolate regex: {err}"))
});

/// State snapshot for a completed workflow, used by `$workflows.<id>.*` expressions.
#[derive(Debug, Clone, Default)]
pub struct WorkflowEvalState {
    pub inputs: BTreeMap<String, Value>,
    pub outputs: BTreeMap<String, Value>,
}

/// Runtime-visible fields from a Source Description Object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceDescriptionContext {
    pub url: String,
    pub type_: String,
}

/// Evaluation context for expression resolution.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub inputs: BTreeMap<String, Value>,
    /// Step outputs, wrapped in `Arc` for cheap cloning during repeated evaluation.
    pub steps: Arc<BTreeMap<String, BTreeMap<String, Value>>>,
    pub outputs: BTreeMap<String, Value>,
    pub workflows: BTreeMap<String, WorkflowEvalState>,
    pub status_code: Option<i64>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub request_headers: BTreeMap<String, String>,
    pub request_query: BTreeMap<String, String>,
    pub request_path: BTreeMap<String, String>,
    pub request_body: Option<Value>,
    /// Headers from an asynchronous message when message execution is available.
    pub message_headers: BTreeMap<String, String>,
    /// Payload from an asynchronous message when message execution is available.
    pub message_payload: Option<Value>,
    pub self_uri: Option<String>,
    pub source_descriptions: BTreeMap<String, SourceDescriptionContext>,
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

    /// Resolve a value string using the canonical three-way dispatch:
    /// - `$...` full expression → [`evaluate`](Self::evaluate)
    /// - contains `{$...}` → [`interpolate_string`](Self::interpolate_string)
    /// - otherwise → literal string
    pub fn resolve_value(&self, value: &str) -> Value {
        self.resolve_value_with_diagnostics(value).0
    }

    /// Resolve a value string using the canonical dispatch while retaining
    /// diagnostics for full runtime expressions.
    pub fn resolve_value_with_diagnostics(&self, value: &str) -> (Value, Vec<ExpressionWarning>) {
        if value.starts_with('$') {
            self.evaluate_with_diagnostics(value)
        } else if value.contains("{$") {
            (Value::String(self.interpolate_string(value)), Vec::new())
        } else {
            (Value::String(value.to_string()), Vec::new())
        }
    }

    /// Evaluate an expression and return a dynamic JSON value.
    ///
    /// Missing keys and unknown namespaces silently return `Value::Null`.
    /// Use [`evaluate_with_diagnostics`](Self::evaluate_with_diagnostics) to
    /// collect warnings about unresolved expressions.
    pub fn evaluate(&self, expr: &str) -> Value {
        self.evaluate_with_diagnostics(expr).0
    }

    /// Evaluate an expression, returning both the value and any diagnostic
    /// warnings produced when resolution falls back to `Null`.
    pub fn evaluate_with_diagnostics(&self, expr: &str) -> (Value, Vec<ExpressionWarning>) {
        let mut warnings = Vec::new();

        let Some(rest) = expr.strip_prefix('$') else {
            return (Value::String(expr.to_string()), warnings);
        };

        // Split into top-level namespace and remainder after the first `.`.
        // Standalone keywords (statusCode, method, url) have no remainder.
        let (namespace, remainder) = match rest.split_once('.') {
            Some((ns, rem)) => (ns, Some(rem)),
            None => (rest, None),
        };

        let value = match namespace {
            "env" => {
                let name = remainder.unwrap_or("");
                Value::String(env::var(name).unwrap_or_default())
            }

            "inputs" => {
                let full = remainder.unwrap_or("");
                if full.contains('#') || !full.contains('.') {
                    resolve_named_value(&self.ctx.inputs, full, expr, &mut warnings, |name| {
                        format!("input \"{name}\" not found in context")
                    })
                } else {
                    let (key, sub_path) = match full.split_once('.') {
                        Some((k, rest)) => (k, rest),
                        None => (full, ""),
                    };
                    match self.ctx.inputs.get(key) {
                        Some(root) => {
                            if sub_path.is_empty() {
                                root.clone()
                            } else {
                                resolve_dot_path(root, sub_path).unwrap_or(Value::Null)
                            }
                        }
                        None => {
                            warnings.push(ExpressionWarning {
                                expression: expr.to_string(),
                                message: format!("input \"{key}\" not found in context"),
                            });
                            Value::Null
                        }
                    }
                }
            }

            "steps" => {
                let after = remainder.unwrap_or("");
                if let Some((step_id, output_name)) = after.split_once(".outputs.") {
                    match self.ctx.steps.get(step_id) {
                        Some(outputs) => {
                            resolve_named_value(outputs, output_name, expr, &mut warnings, |name| {
                                format!("output \"{name}\" not found in step \"{step_id}\"")
                            })
                        }
                        None => {
                            warnings.push(ExpressionWarning {
                                expression: expr.to_string(),
                                message: format!("step \"{step_id}\" not found in context"),
                            });
                            Value::Null
                        }
                    }
                } else {
                    warnings.push(ExpressionWarning {
                        expression: expr.to_string(),
                        message: "invalid $steps expression: expected $steps.<id>.outputs.<key>"
                            .to_string(),
                    });
                    Value::Null
                }
            }

            "statusCode" => self
                .ctx
                .status_code
                .map(|code| json!(code))
                .unwrap_or(Value::Null),

            "method" => self
                .ctx
                .method
                .as_ref()
                .map(|m| Value::String(m.clone()))
                .unwrap_or(Value::Null),

            "url" => self
                .ctx
                .url
                .as_ref()
                .map(|u| Value::String(u.clone()))
                .unwrap_or(Value::Null),

            "outputs" => {
                let after = remainder.unwrap_or("");
                resolve_named_value(&self.ctx.outputs, after, expr, &mut warnings, |name| {
                    format!("output \"{name}\" not found in context")
                })
            }

            "request" => self.resolve_request(remainder.unwrap_or("")),

            "message" => self.resolve_message(expr, remainder.unwrap_or(""), &mut warnings),

            "self" => {
                if remainder.is_some() {
                    warnings.push(ExpressionWarning {
                        expression: expr.to_string(),
                        message: "invalid $self expression: no sub-path is supported".to_string(),
                    });
                    Value::Null
                } else {
                    self.ctx.self_uri.as_ref().map_or_else(
                        || {
                            warnings.push(ExpressionWarning {
                                expression: expr.to_string(),
                                message: "self URI not found in context".to_string(),
                            });
                            Value::Null
                        },
                        |self_uri| Value::String(self_uri.clone()),
                    )
                }
            }

            "sourceDescriptions" => {
                let after = remainder.unwrap_or("");
                let Some((name, reference)) = after.split_once('.') else {
                    warnings.push(ExpressionWarning {
                        expression: expr.to_string(),
                        message: "invalid $sourceDescriptions expression: expected $sourceDescriptions.<name>.<reference>".to_string(),
                    });
                    return (Value::Null, warnings);
                };
                match self.ctx.source_descriptions.get(name) {
                    Some(source) => match reference {
                        "url" => Value::String(source.url.clone()),
                        "type" => Value::String(source.type_.clone()),
                        _ => {
                            warnings.push(ExpressionWarning {
                                expression: expr.to_string(),
                                message: format!(
                                    "source description reference \"{reference}\" for \"{name}\" cannot be resolved without loaded source document metadata"
                                ),
                            });
                            Value::Null
                        }
                    },
                    None => {
                        warnings.push(ExpressionWarning {
                            expression: expr.to_string(),
                            message: format!("source description \"{name}\" not found in context"),
                        });
                        Value::Null
                    }
                }
            }

            "response" => self.resolve_response(remainder.unwrap_or("")),

            "workflows" => {
                let after = remainder.unwrap_or("");
                let (wf_id, tail) = match after.split_once('.') {
                    Some(pair) => pair,
                    None => {
                        warnings.push(ExpressionWarning {
                            expression: expr.to_string(),
                            message: "invalid $workflows expression: expected $workflows.<id>.inputs.<name> or $workflows.<id>.outputs.<name>".to_string(),
                        });
                        return (Value::Null, warnings);
                    }
                };
                match self.ctx.workflows.get(wf_id) {
                    Some(state) => {
                        if let Some(rest) = tail.strip_prefix("inputs.") {
                            resolve_named_value(&state.inputs, rest, expr, &mut warnings, |name| {
                                format!("input \"{name}\" not found in workflow \"{wf_id}\"")
                            })
                        } else if let Some(rest) = tail.strip_prefix("outputs.") {
                            resolve_named_value(&state.outputs, rest, expr, &mut warnings, |name| {
                                format!("output \"{name}\" not found in workflow \"{wf_id}\"")
                            })
                        } else {
                            warnings.push(ExpressionWarning {
                                expression: expr.to_string(),
                                message: format!(
                                    "invalid $workflows.{wf_id} sub-path: expected \"inputs.<name>\" or \"outputs.<name>\""
                                ),
                            });
                            Value::Null
                        }
                    }
                    None => {
                        warnings.push(ExpressionWarning {
                            expression: expr.to_string(),
                            message: format!("workflow \"{wf_id}\" not found in workflows context"),
                        });
                        Value::Null
                    }
                }
            }

            _ => {
                warnings.push(ExpressionWarning {
                    expression: expr.to_string(),
                    message: format!("unknown expression namespace \"${rest}\""),
                });
                Value::Null
            }
        };

        (value, warnings)
    }

    /// Dispatch `$request.<sub>` expressions.
    fn resolve_request(&self, remainder: &str) -> Value {
        if let Some(name) = remainder.strip_prefix("header.") {
            get_header_case_insensitive(&self.ctx.request_headers, name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null)
        } else if let Some(name) = remainder.strip_prefix("query.") {
            self.ctx
                .request_query
                .get(name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null)
        } else if let Some(name) = remainder.strip_prefix("path.") {
            self.ctx
                .request_path
                .get(name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null)
        } else if let Some(suffix) = remainder.strip_prefix("body") {
            resolve_body_access(&self.ctx.request_body, suffix)
        } else {
            Value::Null
        }
    }

    /// Dispatch `$response.<sub>` expressions.
    fn resolve_response(&self, remainder: &str) -> Value {
        if let Some(name) = remainder.strip_prefix("header.") {
            get_header_case_insensitive(&self.ctx.response_headers, name)
                .map(|v| Value::String(v.clone()))
                .unwrap_or(Value::Null)
        } else if let Some(suffix) = remainder.strip_prefix("body") {
            resolve_body_access(&self.ctx.response_body, suffix)
        } else {
            Value::Null
        }
    }

    /// Dispatch `$message.<sub>` expressions without assuming a transport.
    fn resolve_message(
        &self,
        expr: &str,
        remainder: &str,
        warnings: &mut Vec<ExpressionWarning>,
    ) -> Value {
        if let Some(name) = remainder.strip_prefix("header.") {
            get_header_case_insensitive(&self.ctx.message_headers, name).map_or_else(
                || {
                    warnings.push(ExpressionWarning {
                        expression: expr.to_string(),
                        message: format!("message header \"{name}\" not found in context"),
                    });
                    Value::Null
                },
                |value| Value::String(value.clone()),
            )
        } else if let Some(suffix) = remainder.strip_prefix("payload") {
            let Some(payload) = self.ctx.message_payload.as_ref() else {
                warnings.push(ExpressionWarning {
                    expression: expr.to_string(),
                    message: "message payload not found in context".to_string(),
                });
                return Value::Null;
            };

            resolve_body_value(payload, suffix).unwrap_or_else(|| {
                warnings.push(ExpressionWarning {
                    expression: expr.to_string(),
                    message: format!("message payload suffix \"{suffix}\" did not resolve"),
                });
                Value::Null
            })
        } else {
            warnings.push(ExpressionWarning {
                expression: expr.to_string(),
                message: "invalid $message expression: expected $message.header.<name> or $message.payload[#/pointer]".to_string(),
            });
            Value::Null
        }
    }

    /// Evaluate an expression and convert to string with Go-compatible coercions.
    pub fn evaluate_string(&self, expr: &str) -> String {
        to_string_value(&self.evaluate(expr)).into_owned()
    }

    /// Evaluate a condition expression with `||` and `&&` precedence.
    pub fn evaluate_condition(&self, condition: &str) -> bool {
        self.evaluate_condition_with_diagnostics(condition).0
    }

    /// Evaluate a condition expression, returning both the boolean result and
    /// any diagnostic warnings from expression resolution.
    pub fn evaluate_condition_with_diagnostics(
        &self,
        condition: &str,
    ) -> (bool, Vec<ExpressionWarning>) {
        let condition = condition.trim();
        if condition.is_empty() {
            return (false, Vec::new());
        }

        let mut warnings = Vec::new();

        // Parenthesis grouping: if the entire expression is wrapped in balanced
        // parens (depth reaches zero only at the last char), strip them and recurse.
        if condition.starts_with('(')
            && condition.ends_with(')')
            && is_balanced_outer_parens(condition)
        {
            let inner = &condition[1..condition.len() - 1];
            let (result, w) = self.evaluate_condition_with_diagnostics(inner);
            warnings.extend(w);
            return (result, warnings);
        }

        // Split on `||` first (lowest precedence).
        if let Some(parts) = split_outside_quotes(condition, "||") {
            for part in parts {
                let (result, w) = self.evaluate_condition_with_diagnostics(part);
                warnings.extend(w);
                if result {
                    return (true, warnings);
                }
            }
            return (false, warnings);
        }

        // Split on `&&` next.
        if let Some(parts) = split_outside_quotes(condition, "&&") {
            for part in parts {
                let (result, w) = self.evaluate_condition_with_diagnostics(part);
                warnings.extend(w);
                if !result {
                    return (false, warnings);
                }
            }
            return (true, warnings);
        }

        // NOT operator: strip leading `!` but only when the next char is not `=`
        // (to avoid consuming `!=` as NOT + `=`). Applied after `||`/`&&` splits
        // so that NOT binds tighter than logical connectives.
        if condition.starts_with('!') && !condition.starts_with("!=") {
            let inner = condition[1..].trim();
            let (result, w) = self.evaluate_condition_with_diagnostics(inner);
            warnings.extend(w);
            return (!result, warnings);
        }

        self.evaluate_comparison_with_diagnostics(condition)
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
                inner.as_str()
            } else {
                full.as_str()
            };
            out.push_str(&self.evaluate_string(expr));
            cursor = full.end();
        }

        out.push_str(&input[cursor..]);
        out
    }

    fn evaluate_comparison_with_diagnostics(
        &self,
        condition: &str,
    ) -> (bool, Vec<ExpressionWarning>) {
        let mut warnings = Vec::new();
        let (op, idx) = find_operator(condition);
        if op.is_empty() {
            let (val, w) = resolve_operand_with_diagnostics(self, condition);
            warnings.extend(w);
            return (is_truthy(&val), warnings);
        }

        let (left, left_w) = resolve_operand_with_diagnostics(self, &condition[..idx]);
        warnings.extend(left_w);
        let right = condition[idx + op.len()..].trim();

        let result = match op {
            "==" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_values(&left, &rv)
            }
            "!=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                !compare_values(&left, &rv)
            }
            ">" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_ordered(&left, &rv).is_gt()
            }
            "<" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_ordered(&left, &rv).is_lt()
            }
            ">=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_ordered(&left, &rv).is_ge()
            }
            "<=" => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                compare_ordered(&left, &rv).is_le()
            }
            " contains " => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                to_string_value(&left).contains(&*to_string_value(&rv))
            }
            " matches " => {
                let (rv, w) = resolve_operand_with_diagnostics(self, right);
                warnings.extend(w);
                let pattern = to_string_value(&rv);
                match Regex::new(&pattern) {
                    Ok(re) => re.is_match(&to_string_value(&left)),
                    Err(_) => false,
                }
            }
            " in " => {
                let (result, w) = eval_in_with_diagnostics(self, &left, right);
                warnings.extend(w);
                result
            }
            _ => false,
        };
        (result, warnings)
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

fn resolve_named_value(
    values: &BTreeMap<String, Value>,
    reference: &str,
    expression: &str,
    warnings: &mut Vec<ExpressionWarning>,
    missing_value_message: impl FnOnce(&str) -> String,
) -> Value {
    let (name, pointer) = reference
        .split_once('#')
        .map_or((reference, None), |(name, pointer)| (name, Some(pointer)));
    let Some(value) = values.get(name) else {
        warnings.push(ExpressionWarning {
            expression: expression.to_string(),
            message: missing_value_message(name),
        });
        return Value::Null;
    };

    pointer.map_or_else(
        || value.clone(),
        |pointer| {
            value.pointer(pointer).cloned().unwrap_or_else(|| {
                warnings.push(ExpressionWarning {
                    expression: expression.to_string(),
                    message: format!(
                        "JSON Pointer \"{pointer}\" did not resolve in value \"{name}\""
                    ),
                });
                Value::Null
            })
        },
    )
}

fn resolve_operand_with_diagnostics(
    eval: &ExpressionEvaluator,
    raw: &str,
) -> (Value, Vec<ExpressionWarning>) {
    let token = raw.trim();
    if token.starts_with('$') {
        eval.evaluate_with_diagnostics(token)
    } else {
        (parse_value(token), Vec::new())
    }
}

/// Returns `true` when the first `(` and the last `)` in `s` form a balanced
/// pair that encloses the entire expression — i.e. the paren depth only reaches
/// zero at the very last character.
fn is_balanced_outer_parens(s: &str) -> bool {
    debug_assert!(s.starts_with('(') && s.ends_with(')'));
    let mut depth: usize = 0;
    let last = s.len() - 1;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx != last {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_outside_quotes<'a>(input: &'a str, delim: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut paren_depth: usize = 0;
    let mut bracket_depth: usize = 0;
    let mut found = false;
    let mut prev_backslash = false;

    for (idx, ch) in input.char_indices() {
        if idx < start {
            prev_backslash = false;
            continue;
        }
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '"' || ch == '\'') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        if ch == '(' {
            paren_depth += 1;
            prev_backslash = false;
            continue;
        }
        if ch == ')' {
            paren_depth = paren_depth.saturating_sub(1);
            prev_backslash = false;
            continue;
        }
        if ch == '[' {
            bracket_depth += 1;
            prev_backslash = false;
            continue;
        }
        if ch == ']' {
            bracket_depth = bracket_depth.saturating_sub(1);
            prev_backslash = false;
            continue;
        }

        if paren_depth == 0 && bracket_depth == 0 && input[idx..].starts_with(delim) {
            parts.push(input[start..idx].trim());
            start = idx + delim.len();
            found = true;
        }
        prev_backslash = ch == '\\';
    }

    if !found {
        return None;
    }
    parts.push(input[start..].trim());
    Some(parts)
}

fn find_operator(input: &str) -> (&'static str, usize) {
    for word_op in [" contains ", " matches ", " in "] {
        if let Some(idx) = index_outside_quotes(input, word_op) {
            return (word_op, idx);
        }
    }

    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;
    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '"' || ch == '\'') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        prev_backslash = ch == '\\' && !prev_backslash;

        if input[idx..].starts_with("!=") {
            return ("!=", idx);
        }
        if input[idx..].starts_with(">=") {
            return (">=", idx);
        }
        if input[idx..].starts_with("<=") {
            return ("<=", idx);
        }
        if input[idx..].starts_with("==") {
            return ("==", idx);
        }

        if ch == '>' {
            return (">", idx);
        }
        if ch == '<' {
            return ("<", idx);
        }
    }

    ("", usize::MAX)
}

fn index_outside_quotes(input: &str, needle: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;
    for (idx, ch) in input.char_indices() {
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '"' || ch == '\'') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        prev_backslash = ch == '\\' && !prev_backslash;
        if input[idx..].starts_with(needle) {
            return Some(idx);
        }
    }
    None
}

fn eval_in_with_diagnostics(
    eval: &ExpressionEvaluator,
    left: &Value,
    list_expr: &str,
) -> (bool, Vec<ExpressionWarning>) {
    let list_expr = list_expr.trim();
    let mut warnings = Vec::new();
    if !(list_expr.starts_with('[') && list_expr.ends_with(']')) {
        return (false, warnings);
    }
    let inner = &list_expr[1..list_expr.len() - 1];
    if inner.trim().is_empty() {
        return (false, warnings);
    }

    for token in split_list_elements(inner) {
        let (val, w) = resolve_operand_with_diagnostics(eval, token);
        warnings.extend(w);
        if compare_values(left, &val) {
            return (true, warnings);
        }
    }
    (false, warnings)
}

fn split_list_elements(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;

    for (idx, ch) in input.char_indices() {
        if prev_backslash {
            prev_backslash = false;
            continue;
        }
        if ch == '\\' {
            prev_backslash = true;
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
                    let inner = &token[1..token.len() - 1];
                    let unescaped = inner
                        .replace("\\\"", "\"")
                        .replace("\\'", "'")
                        .replace("\\\\", "\\");
                    return Value::String(unescaped);
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
        return f64_approx_eq(lhs, rhs);
    }

    to_string_value(a) == to_string_value(b)
}

/// Approximate f64 equality using a scaled epsilon. Handles the common case
/// where two JSON numbers representing the same value may differ slightly
/// due to serialization round-trips.
fn f64_approx_eq(a: f64, b: f64) -> bool {
    if a == b {
        return true; // exact match, ±0, infinities
    }
    let diff = (a - b).abs();
    // Scale epsilon by the magnitude of the larger operand (floor at 1.0
    // so that values near zero use an absolute epsilon).
    diff <= f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

fn compare_ordered(a: &Value, b: &Value) -> Ordering {
    if let (Some(lhs), Some(rhs)) = (to_f64(a), to_f64(b)) {
        if f64_approx_eq(lhs, rhs) {
            return Ordering::Equal;
        }
        if lhs < rhs {
            return Ordering::Less;
        }
        return Ordering::Greater;
    }

    let lhs = to_string_value(a);
    let rhs = to_string_value(b);
    lhs.cmp(&rhs)
}

fn to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn to_string_value(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(v) => Cow::Borrowed(v.as_str()),
        Value::Number(n) => Cow::Owned(n.to_string()),
        Value::Bool(v) => Cow::Borrowed(if *v { "true" } else { "false" }),
        _ => Cow::Borrowed(""),
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
    body.as_ref()
        .and_then(|body| resolve_body_value(body, suffix))
        .unwrap_or(Value::Null)
}

fn resolve_body_value(body: &Value, suffix: &str) -> Option<Value> {
    if suffix.is_empty() {
        return Some(body.clone());
    }
    if let Some(pointer) = suffix.strip_prefix('#') {
        return body.pointer(pointer).cloned();
    }
    if let Some(path) = suffix.strip_prefix('.') {
        return resolve_dot_path(body, path).ok();
    }
    // Handle bracket notation directly after body: $response.body['key']
    if suffix.starts_with('[') {
        return resolve_dot_path(body, suffix).ok();
    }
    None
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

/// Select JSON nodes with the runtime's supported JSONPath subset.
///
/// Supported selectors include root (`$`), dot/bracket fields, array indexes,
/// wildcards, and simple filter predicates. Syntax outside that subset returns
/// [`PathError`] instead of silently producing no match.
pub fn select_json_path(root: &Value, selector: &str) -> Result<JsonPathSelection, PathError> {
    let trimmed = selector.trim();
    validate_json_path_subset(trimmed)?;
    let normalized = normalize_json_path(trimmed);
    let value = resolve_dot_path(root, normalized)?;
    let match_count = count_resolved_path_nodes(root, normalized)?;
    Ok(JsonPathSelection { value, match_count })
}

fn normalize_json_path(path: &str) -> &str {
    if path == "$" || path == "@" {
        return "";
    }
    path.strip_prefix("$.")
        .or_else(|| path.strip_prefix("@."))
        .or_else(|| path.strip_prefix('$'))
        .or_else(|| path.strip_prefix('@'))
        .unwrap_or(path)
        .trim_start_matches('.')
}

fn validate_json_path_subset(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError::InvalidSyntax {
            path: path.to_string(),
            detail: "selector is empty".to_string(),
        });
    }

    let masked = mask_json_path_literals(path);
    let unsupported = if masked.contains("..") {
        Some("recursive descent '..' is not supported")
    } else if masked.contains("&&") || masked.contains("||") {
        Some("compound filter predicates are not supported")
    } else {
        bracket_subset_error(&masked)
    };

    if let Some(detail) = unsupported {
        return Err(PathError::InvalidSyntax {
            path: path.to_string(),
            detail: detail.to_string(),
        });
    }
    Ok(())
}

fn bracket_subset_error(masked: &str) -> Option<&'static str> {
    let mut rest = masked;
    while let Some(open) = rest.find('[') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find(']') else {
            break;
        };
        let inner = &after_open[..close];
        if inner.contains(':') {
            return Some("array slices are not supported");
        }
        if inner.contains(',') {
            return Some("union selectors are not supported");
        }
        rest = &after_open[close + 1..];
    }
    None
}

fn mask_json_path_literals(path: &str) -> String {
    let mut masked = String::with_capacity(path.len());
    let mut quote = None;
    let mut escaped = false;
    for character in path.chars() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                masked.push('_');
                continue;
            }
            if character == '\\' {
                escaped = true;
                masked.push('_');
                continue;
            }
            if character == active_quote {
                quote = None;
                masked.push(character);
            } else {
                masked.push('_');
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
        }
        masked.push(character);
    }
    masked
}

/// Count the JSON nodes selected by an Arazzo dot-notation path before the
/// public evaluator collapses the result into a JSON value.
pub fn count_resolved_path_nodes(root: &Value, path: &str) -> Result<usize, PathError> {
    if path.is_empty() {
        return Ok(1);
    }
    let tokens = tokenize_path(path)?;
    if tokens.is_empty() {
        return Ok(0);
    }

    let mut current = vec![root];
    for (idx, token) in tokens.iter().copied().enumerate() {
        let is_last = idx + 1 == tokens.len();
        if matches!(token, PathToken::Hash) && is_last {
            return Ok(usize::from(!current.is_empty()));
        }
        current = apply_path_token(&current, token);
        if current.is_empty() {
            return Ok(0);
        }
    }

    Ok(current.len())
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
                FilterOp::Gt => compare_ordered(&left, &right).is_gt(),
                FilterOp::Lt => compare_ordered(&left, &right).is_lt(),
                FilterOp::Ge => compare_ordered(&left, &right).is_ge(),
                FilterOp::Le => compare_ordered(&left, &right).is_le(),
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
        let tail = &path[start..];
        if !tail.is_empty() {
            out.push(tail);
        }
    }

    out
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

        let Some(close) = find_matching_bracket(segment, open) else {
            return Err(PathError::InvalidSyntax {
                path: full_path.to_string(),
                detail: format!("unclosed bracket in: {segment}"),
            });
        };
        let index_expr = segment[open + 1..close].trim();

        if index_expr == "*" {
            out.push(PathToken::Wildcard);
        } else if let Ok(idx) = index_expr.parse::<usize>() {
            out.push(PathToken::Index(idx));
        } else if let Some(inner) = parse_bracket_filter_expr(index_expr) {
            if let Some(expr) = parse_filter_expr(inner) {
                out.push(PathToken::Filter {
                    expr,
                    all_matches: true,
                });
            } else {
                return Err(PathError::InvalidSyntax {
                    path: full_path.to_string(),
                    detail: format!("invalid filter expression: {index_expr}"),
                });
            }
        } else if index_expr.starts_with("?(") {
            return Err(PathError::InvalidSyntax {
                path: full_path.to_string(),
                detail: format!("unbalanced filter expression: {index_expr}"),
            });
        } else if !index_expr.is_empty() {
            // Strip surrounding quotes for bracket key access: ['key'] or ["key"]
            let key = if (index_expr.starts_with('\'') && index_expr.ends_with('\''))
                || (index_expr.starts_with('"') && index_expr.ends_with('"'))
            {
                &index_expr[1..index_expr.len() - 1]
            } else {
                index_expr
            };
            out.push(PathToken::Field(key));
        }

        cursor = close + 1;
    }

    if cursor < segment.len() {
        out.push(PathToken::Field(&segment[cursor..]));
    }
    Ok(())
}

fn parse_bracket_filter_expr(segment: &str) -> Option<&str> {
    let trimmed = segment.trim();
    if trimmed.starts_with("?(") && trimmed.ends_with(')') {
        Some(trimmed[2..trimmed.len() - 1].trim())
    } else {
        None
    }
}

fn find_matching_bracket(input: &str, open: usize) -> Option<usize> {
    let mut bracket_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for (idx, ch) in input[open..].char_indices() {
        let idx = open + idx;
        if let Some(quote) = in_quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' => in_quote = Some(ch),
            '(' => paren_depth = paren_depth.saturating_add(1),
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                if bracket_depth == 0 && paren_depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::sync::Arc;

    use super::{
        compare_ordered, compare_values, parse_value, EvalContext, ExpressionEvaluator,
        SourceDescriptionContext,
    };
    use proptest::prelude::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn selected(root: &Value, path: &str) -> super::JsonPathSelection {
        match super::select_json_path(root, path) {
            Ok(selection) => selection,
            Err(error) => panic!("selecting {path:?}: {error}"),
        }
    }

    fn selection_error(root: &Value, path: &str) -> super::PathError {
        match super::select_json_path(root, path) {
            Ok(selection) => panic!("expected {path:?} to fail, got {selection:?}"),
            Err(error) => error,
        }
    }

    #[test]
    fn evaluate_literal_and_unknown_expression() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("hello"), json!("hello"));
        assert_eq!(eval.evaluate("$unknown.thing"), Value::Null);
    }

    #[test]
    fn resolve_value_dispatches_correctly() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        ctx.inputs.insert("token".to_string(), json!("xyz"));
        let eval = ExpressionEvaluator::new(ctx);

        // Full expression → evaluate
        assert_eq!(eval.resolve_value("$inputs.name"), json!("Alice"));

        // Interpolated → interpolate_string
        assert_eq!(
            eval.resolve_value("Bearer {$inputs.token}"),
            json!("Bearer xyz")
        );

        // Literal → as-is string
        assert_eq!(eval.resolve_value("literal"), json!("literal"));
    }

    #[test]
    fn evaluate_inputs_and_step_outputs() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        Arc::make_mut(&mut ctx.steps).insert(
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
        assert_eq!(
            eval.evaluate(r#"$response.body.users[?(@.group=="a")].id"#),
            json!([1, 3])
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
        Arc::make_mut(&mut ctx.steps).insert(
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
    fn evaluate_condition_with_diagnostics_surfaces_warnings() {
        let ctx = EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        // Known expression — no warnings
        let (result, warnings) = eval.evaluate_condition_with_diagnostics("$statusCode == 200");
        assert!(result);
        assert!(warnings.is_empty());

        // Unknown step reference — should produce a warning (resolves to null)
        let (result, warnings) =
            eval.evaluate_condition_with_diagnostics("$steps.missing.outputs.x == 42");
        assert!(!result);
        assert!(!warnings.is_empty());
        assert!(
            warnings.iter().any(|w| w.message.contains("missing")),
            "expected warning about missing step, got: {warnings:?}"
        );

        // Compound condition with one unknown — warnings from both branches collected
        let (_, warnings) = eval.evaluate_condition_with_diagnostics(
            "$statusCode == 200 && $inputs.nonexistent == true",
        );
        assert!(
            warnings.iter().any(|w| w.message.contains("nonexistent")),
            "expected warning about nonexistent input, got: {warnings:?}"
        );
    }

    #[test]
    fn evaluate_condition_truthiness_and_quoted_operators() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("flag".to_string(), json!(true));
        ctx.inputs.insert("zero".to_string(), json!(0));
        ctx.inputs.insert("empty".to_string(), json!(""));
        Arc::make_mut(&mut ctx.steps).insert(
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
        assert_eq!(compare_ordered(&json!(100), &json!(200)), Ordering::Less);
        assert_eq!(compare_ordered(&json!(200), &json!(200)), Ordering::Equal);
        assert_eq!(compare_ordered(&json!(300), &json!(200)), Ordering::Greater);
        assert_eq!(
            compare_ordered(&json!("apple"), &json!("banana")),
            Ordering::Less
        );
        assert_eq!(compare_ordered(&json!(10), &json!(10.0)), Ordering::Equal);
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
        Arc::make_mut(&mut ctx.steps).insert(
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

    #[test]
    fn logical_not_precedence_bug() {
        let ctx = EvalContext::default();
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "!true || true";
        assert!(
            eval.evaluate_condition(condition),
            "NOT precedence should be higher than OR"
        );
    }

    #[test]
    fn split_outside_quotes_handles_brackets_and_escapes() {
        let ctx = EvalContext {
            response_body: Some(json!({"name||title": "Something"})),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "$response.body['name||title'] == 'Something'";
        assert!(
            eval.evaluate_condition(condition),
            "Should not split || inside brackets"
        );
    }

    #[test]
    fn split_outside_quotes_fails_on_escaped_quotes() {
        let ctx = EvalContext {
            inputs: std::collections::BTreeMap::from([("name".to_string(), json!("Alice\"Bob"))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        let condition = "$inputs.name == \"Alice\\\"Bob\"";
        assert!(
            eval.evaluate_condition(condition),
            "Should handle escaped quotes in strings"
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
            match eval.evaluate_condition(&condition) {
                true | false => {}
            }
        }

        #[test]
        fn resolve_dot_path_fuzz_does_not_panic(path in ".{0,128}") {
            let root = json!({"a": [1, {"b": "c"}, [2, 3]], "d": null});
            match super::resolve_dot_path(&root, &path) {
                Ok(_) | Err(_) => {}
            }
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
    fn resolve_dot_path_bracket_filter_handles_literal_delimiters() {
        let root = json!({
            "items": [
                {"id": 1, "type": "foo)bar", "group": "a"},
                {"id": 2, "type": "foo]bar", "group": "b"},
                {"id": 3, "type": "other", "group": "a"}
            ]
        });

        assert_eq!(
            super::resolve_dot_path(&root, "items[?(@.type == 'foo)bar')].id"),
            Ok(json!(1))
        );
        assert_eq!(
            super::resolve_dot_path(&root, "items[?(@.type == 'foo]bar')].id"),
            Ok(json!(2))
        );
        assert_eq!(
            super::resolve_dot_path(&root, "items[?(@.group == 'a')].id"),
            Ok(json!([1, 3]))
        );
    }

    #[test]
    fn count_resolved_path_nodes_preserves_nodelist_cardinality() {
        let root = json!({
            "items": [
                {"id": 1, "type": "match", "extra": true},
                {"id": 2, "type": "other"}
            ]
        });

        assert_eq!(super::count_resolved_path_nodes(&root, ""), Ok(1));
        assert_eq!(
            super::count_resolved_path_nodes(&root, "items[?(@.type == 'match')]"),
            Ok(1)
        );
        assert_eq!(
            super::count_resolved_path_nodes(&root, "items[?(@.type == 'missing')]"),
            Ok(0)
        );
        assert_eq!(super::count_resolved_path_nodes(&root, "items[0]"), Ok(1));
    }

    #[test]
    fn select_json_path_preserves_zero_one_and_many_cardinality() {
        let root = json!({
            "items": [
                {"id": 1, "enabled": true},
                {"id": 2, "enabled": false},
                {"id": 3, "enabled": true}
            ]
        });

        let one = selected(&root, "$.items[0].id");
        assert_eq!(one.value, json!(1));
        assert_eq!(one.match_count, 1);

        let many = selected(&root, "$.items[*].id");
        assert_eq!(many.value, json!([1, 2, 3]));
        assert_eq!(many.match_count, 3);

        let filtered = selected(&root, "$.items[?(@.enabled == true)].id");
        assert_eq!(filtered.value, json!([1, 3]));
        assert_eq!(filtered.match_count, 2);

        let zero = selected(&root, "$.missing");
        assert_eq!(zero.value, Value::Null);
        assert_eq!(zero.match_count, 0);
    }

    #[test]
    fn select_json_path_reports_unsupported_syntax() {
        let root = json!({"items": [1, 2, 3]});

        let recursive = selection_error(&root, "$..items");
        assert!(recursive.to_string().contains("recursive descent"));

        let slice = selection_error(&root, "$.items[0:2]");
        assert!(slice.to_string().contains("array slices"));
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
    fn evaluate_named_value_json_pointers() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert(
            "user".to_string(),
            json!({"profile": {"email": "alice@example.com"}}),
        );
        Arc::make_mut(&mut ctx.steps).insert(
            "lookup".to_string(),
            BTreeMap::from([("payload".to_string(), json!({"items": [{"id": "item-1"}]}))]),
        );
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::from([("config".to_string(), json!({"env": "production"}))]),
                outputs: BTreeMap::from([(
                    "tokenPayload".to_string(),
                    json!({"token": "abc-123"}),
                )]),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$inputs.user#/profile/email"),
            json!("alice@example.com")
        );
        assert_eq!(
            eval.evaluate("$steps.lookup.outputs.payload#/items/0/id"),
            json!("item-1")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.tokenPayload#/token"),
            json!("abc-123")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.config#/env"),
            json!("production")
        );
    }

    #[test]
    fn evaluate_message_header_and_payload() {
        let ctx = EvalContext {
            message_headers: BTreeMap::from([(
                "x-request-id".to_string(),
                "request-123".to_string(),
            )]),
            message_payload: Some(json!({"order": {"id": "order-42"}})),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$message.header.X-Request-Id"),
            json!("request-123")
        );
        assert_eq!(
            eval.evaluate("$message.payload"),
            json!({"order": {"id": "order-42"}})
        );
        assert_eq!(
            eval.evaluate("$message.payload#/order/id"),
            json!("order-42")
        );
    }

    #[test]
    fn missing_json_pointer_returns_null_with_diagnostic() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("user".to_string(), json!({"profile": {}}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        let (value, warnings) = eval.evaluate_with_diagnostics("$inputs.user#/profile/email");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("JSON Pointer"));
        assert!(warnings[0].message.contains("/profile/email"));
    }

    #[test]
    fn missing_message_context_returns_null_with_diagnostics() {
        let eval = ExpressionEvaluator::new(EvalContext::default());

        let (header, header_warnings) =
            eval.evaluate_with_diagnostics("$message.header.X-Request-Id");
        assert_eq!(header, Value::Null);
        assert_eq!(header_warnings.len(), 1);
        assert!(header_warnings[0].message.contains("message header"));

        let (payload, payload_warnings) =
            eval.evaluate_with_diagnostics("$message.payload#/order/id");
        assert_eq!(payload, Value::Null);
        assert_eq!(payload_warnings.len(), 1);
        assert!(payload_warnings[0].message.contains("message payload"));
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

    #[test]
    fn diagnostics_missing_step_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$steps.missing.outputs.x");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("step \"missing\" not found"));
    }

    #[test]
    fn diagnostics_missing_output_key_warns() {
        let mut ctx = EvalContext::default();
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("a".to_string(), json!(1))]),
        );
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$steps.s1.outputs.nope");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("output \"nope\" not found"));
    }

    #[test]
    fn diagnostics_valid_expression_no_warnings() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$inputs.name");
        assert_eq!(value, json!("Alice"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn diagnostics_unknown_namespace_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$foo.bar");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown expression namespace"));
    }

    #[test]
    fn diagnostics_missing_source_description_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$sourceDescriptions.missing.url");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .message
            .contains("source description \"missing\""));
    }

    #[test]
    fn self_expression_resolves_configured_uri() {
        let eval = ExpressionEvaluator::new(EvalContext {
            self_uri: Some("workflows/purchase.arazzo.yaml".to_string()),
            ..EvalContext::default()
        });
        let (value, warnings) = eval.evaluate_with_diagnostics("$self");
        assert_eq!(value, json!("workflows/purchase.arazzo.yaml"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn self_expression_without_uri_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$self");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("self URI not found"));
    }

    #[test]
    fn source_description_url_and_type_resolve() {
        let mut ctx = EvalContext::default();
        ctx.source_descriptions.insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.url"),
            json!("https://api.example.com/openapi.yaml")
        );
        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.type"),
            json!("openapi")
        );
    }

    #[test]
    fn unsupported_source_description_reference_warns() {
        let mut ctx = EvalContext::default();
        ctx.source_descriptions.insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) =
            eval.evaluate_with_diagnostics("$sourceDescriptions.petstore.getPetById");

        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("getPetById"));
        assert!(warnings[0]
            .message
            .contains("without loaded source document metadata"));
    }

    #[test]
    fn evaluate_backward_compat_still_returns_null() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$steps.missing.outputs.x"), Value::Null);
        assert_eq!(eval.evaluate("$foo.bar"), Value::Null);
    }

    // ── Bug #14: $inputs nested traversal ─────────────────────────

    #[test]
    fn inputs_nested_dot_path_traversal() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": {"baz": 42}}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        // Nested traversal
        assert_eq!(eval.evaluate("$inputs.foo.bar.baz"), json!(42));
        // One level deep
        assert_eq!(eval.evaluate("$inputs.foo.bar"), json!({"baz": 42}));
        // Top-level (flat key) still works
        assert_eq!(eval.evaluate("$inputs.foo"), json!({"bar": {"baz": 42}}));
    }

    #[test]
    fn inputs_flat_key_still_works() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("simple".to_string(), json!("hello"))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.simple"), json!("hello"));
    }

    #[test]
    fn inputs_nested_missing_sub_path_returns_null() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": 1}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.foo.missing"), Value::Null);
    }

    // ── Phase 1: NOT operator and parenthesis grouping ──────────────

    #[test]
    fn not_operator_negates_true() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert!(!eval.evaluate_condition("!true"));
    }

    #[test]
    fn not_operator_negates_false() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert!(eval.evaluate_condition("!false"));
    }

    #[test]
    fn not_operator_on_expression() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(404),
            ..EvalContext::default()
        });
        // !($statusCode == 200) should be true when status is 404
        assert!(eval.evaluate_condition("!($statusCode == 200)"));
        // !($statusCode == 404) should be false when status is 404
        assert!(!eval.evaluate_condition("!($statusCode == 404)"));
    }

    #[test]
    fn not_does_not_consume_ne_operator() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("a".to_string(), json!(1));
        ctx.inputs.insert("b".to_string(), json!(2));
        let eval = ExpressionEvaluator::new(ctx);
        // != must still work correctly — the `!` must NOT be consumed as NOT
        assert!(eval.evaluate_condition("$inputs.a != $inputs.b"));
    }

    #[test]
    fn paren_grouping_simple() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("($statusCode == 200)"));
    }

    #[test]
    fn paren_grouping_or() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        });
        assert!(eval.evaluate_condition("($statusCode == 200 || $statusCode == 201)"));
    }

    #[test]
    fn paren_grouping_with_and() {
        let mut ctx = EvalContext {
            status_code: Some(201),
            ..EvalContext::default()
        };
        ctx.inputs.insert("c".to_string(), json!(3));
        let eval = ExpressionEvaluator::new(ctx);
        // ($statusCode == 200 || $statusCode == 201) && $inputs.c == 3
        assert!(
            eval.evaluate_condition("($statusCode == 200 || $statusCode == 201) && $inputs.c == 3")
        );
        // Should be false when the && side fails
        assert!(!eval
            .evaluate_condition("($statusCode == 200 || $statusCode == 201) && $inputs.c == 999"));
    }

    #[test]
    fn nested_parens() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("a".to_string(), json!(1));
        ctx.inputs.insert("b".to_string(), json!(2));
        let eval = ExpressionEvaluator::new(ctx);
        assert!(eval.evaluate_condition("(($inputs.a == 1)) && $inputs.b == 2"));
    }

    #[test]
    fn not_on_paren_grouped_expression() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            response_body: Some(json!({"error": null})),
            ..EvalContext::default()
        });
        // !($response.body.error) should be true when error is null (falsy)
        assert!(eval.evaluate_condition("!($response.body.error)"));
        // !($statusCode == 500) should be true when status is 200
        assert!(eval.evaluate_condition("!($statusCode == 500)"));
    }

    // ── Phase 3: $workflows expression root ─────────────────────────

    #[test]
    fn workflows_expression_inputs() {
        let mut ctx = EvalContext::default();
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::from([("env".to_string(), json!("production"))]),
                outputs: BTreeMap::new(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.env"),
            json!("production")
        );
    }

    #[test]
    fn workflows_expression_outputs() {
        let mut ctx = EvalContext::default();
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::new(),
                outputs: BTreeMap::from([("token".to_string(), json!("abc-123"))]),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.token"),
            json!("abc-123")
        );
    }

    #[test]
    fn workflows_unknown_id_is_null() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.unknown.outputs.x");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("\"unknown\" not found"));
    }

    #[test]
    fn workflows_missing_sub_path_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.auth");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn workflows_invalid_sub_path_warns() {
        let mut ctx = EvalContext::default();
        ctx.workflows
            .insert("auth".to_string(), super::WorkflowEvalState::default());
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.auth.something.else");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("invalid"));
    }
}
