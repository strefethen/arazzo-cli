use std::collections::HashMap;
use std::sync::LazyLock;

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

use super::*;

/// Characters to percent-encode in path segment values per RFC 3986 §3.3.
/// Allows unreserved chars (§2.3), sub-delimiters (§2.2), ':', and '@'
/// (all part of the `pchar` production). Non-ASCII bytes are always encoded.
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

static STEP_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")
        .unwrap_or_else(|err| panic!("failed to compile step-ref regex: {err}"))
});

static XMLNS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"xmlns(?::\w+)?="[^"]*""#)
        .unwrap_or_else(|err| panic!("failed to compile xmlns regex: {err}"))
});

static NS_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(/?)[\w-]+:")
        .unwrap_or_else(|err| panic!("failed to compile ns-prefix regex: {err}"))
});

/// Cache for compiled regular expressions used in criterion evaluation.
///
/// Wraps a `Mutex<HashMap>` for safe concurrent access from parallel steps.
/// Regex compilation is expensive (µs) while matching is cheap (ns), so
/// caching yields 100–500x speedup for repeated evaluations of the same pattern.
pub(crate) struct RegexCache {
    cache: Mutex<HashMap<String, Regex>>,
}

impl RegexCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Compile a regex (or return cached) and test whether it matches `text`.
    ///
    /// The lock is held for the duration of the match, but matching takes
    /// nanoseconds so contention is negligible.
    pub(crate) fn is_match(&self, pattern: &str, text: &str) -> Result<bool, regex::Error> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(re) = cache.get(pattern) {
            return Ok(re.is_match(text));
        }
        let re = Regex::new(pattern)?;
        let result = re.is_match(text);
        cache.insert(pattern.to_string(), re);
        Ok(result)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CriterionEvaluation {
    pub type_name: String,
    pub type_version: Option<String>,
    pub condition: String,
    pub condition_result: bool,
    pub matched: bool,
    pub context_expr: String,
    pub context_value: Value,
    pub error: Option<String>,
    pub warnings: Vec<arazzo_expr::ExpressionWarning>,
}

pub(crate) fn extract_xpath(body: &[u8], expr: &str) -> Value {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };
    let text = XMLNS_RE.replace_all(text, "");
    let text = NS_PREFIX_RE.replace_all(&text, "<$1");
    let mut doc = match uppsala::parse(&text) {
        Ok(d) => d,
        Err(_) => return Value::Null,
    };
    doc.prepare_xpath();
    let eval = uppsala::XPathEvaluator::new();
    let root = doc.root();
    match eval.evaluate(&doc, root, expr) {
        Ok(uppsala::XPathValue::String(s)) if !s.is_empty() => Value::String(s),
        Ok(uppsala::XPathValue::NodeSet(nodes)) if !nodes.is_empty() => {
            let s = doc.text_content_deep(nodes[0]);
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Ok(uppsala::XPathValue::Number(n)) => {
            let s = n.to_string();
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Ok(uppsala::XPathValue::Boolean(b)) => Value::String(b.to_string()),
        _ => Value::Null,
    }
}

pub(crate) fn evaluate_criterion(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    regex_cache: &RegexCache,
) -> bool {
    evaluate_criterion_detailed(criterion, eval, response, regex_cache).matched
}

pub(crate) fn evaluate_criterion_detailed(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    regex_cache: &RegexCache,
) -> CriterionEvaluation {
    let type_name = criterion.resolved_type_name();
    let mut expr_warnings = Vec::new();
    let mut context_value = if criterion.context.trim().is_empty() {
        default_criterion_context(response)
    } else {
        let (val, warnings) = eval.evaluate_with_diagnostics(&criterion.context);
        expr_warnings = warnings;
        val
    };
    let mut error = None;

    let condition_result = match type_name.as_str() {
        "regex" => {
            let context_text = value_to_string(&context_value);
            match regex_cache.is_match(&criterion.condition, &context_text) {
                Ok(matched) => matched,
                Err(err) => {
                    error = Some(format!("invalid regex: {err}"));
                    false
                }
            }
        }
        "jsonpath" => {
            if context_value.is_null() {
                false
            } else {
                evaluate_jsonpath_condition(eval, &context_value, &criterion.condition)
            }
        }
        "xpath" => {
            let xml_text = match &context_value {
                Value::String(text) => text.clone(),
                Value::Null => match response {
                    Some(resp) => String::from_utf8_lossy(&resp.body).to_string(),
                    None => String::new(),
                },
                other => other.to_string(),
            };
            context_value = Value::String(xml_text.clone());
            is_truthy(&extract_xpath(xml_text.as_bytes(), &criterion.condition))
        }
        _ => {
            let (result, cond_warnings) =
                eval.evaluate_condition_with_diagnostics(&criterion.condition);
            expr_warnings.extend(cond_warnings);
            result
        }
    };

    CriterionEvaluation {
        type_name,
        type_version: criterion.declared_type_version().map(ToString::to_string),
        condition: criterion.condition.clone(),
        condition_result,
        matched: condition_result,
        context_expr: criterion.context.clone(),
        context_value,
        error,
        warnings: expr_warnings,
    }
}

pub(crate) fn evaluate_output_expression(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> Value {
    evaluate_output_expression_detailed(expr, eval, response).0
}

pub(crate) fn evaluate_output_expression_detailed(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    if expr.starts_with('/') {
        if let Some(resp) = response {
            return (extract_xpath(&resp.body, expr), Vec::new());
        }
        return (Value::Null, Vec::new());
    }

    if expr.starts_with('$') {
        return eval.evaluate_with_diagnostics(expr);
    }

    let json_path = to_json_path(expr);
    eval.evaluate_with_diagnostics(&format!("$response.body.{json_path}"))
}

fn default_criterion_context(response: Option<&Response>) -> Value {
    match response {
        Some(resp) => {
            if let Some(json) = &resp.body_json {
                json.clone()
            } else if !resp.body.is_empty() {
                Value::String(String::from_utf8_lossy(&resp.body).to_string())
            } else {
                Value::Null
            }
        }
        None => Value::Null,
    }
}

fn evaluate_jsonpath_condition(
    eval: &ExpressionEvaluator,
    context_value: &Value,
    condition: &str,
) -> bool {
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return false;
    }

    if let Some(predicate) = parse_jsonpath_filter_predicate(trimmed) {
        return evaluate_jsonpath_filter_predicate(context_value, predicate);
    }

    let mut scoped_ctx = eval.context().clone();
    scoped_ctx.response_body = Some(context_value.clone());
    let scoped_eval = ExpressionEvaluator::new(scoped_ctx);

    let normalized = normalize_jsonpath_path(trimmed);
    if normalized.is_empty() {
        return !context_value.is_null();
    }

    let value = scoped_eval.evaluate(&format!("$response.body.{normalized}"));
    is_truthy(&value)
}

fn parse_jsonpath_filter_predicate(condition: &str) -> Option<&str> {
    if !(condition.starts_with("$[?") && condition.ends_with(']')) {
        return None;
    }
    let mut inner = condition.strip_prefix("$[?")?.strip_suffix(']')?.trim();
    if inner.starts_with('(') && inner.ends_with(')') && inner.len() >= 2 {
        inner = inner[1..inner.len() - 1].trim();
    }
    if inner.is_empty() {
        None
    } else {
        Some(inner)
    }
}

fn evaluate_jsonpath_filter_predicate(context_value: &Value, predicate: &str) -> bool {
    let candidates = match context_value {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        value => vec![value],
    };

    for candidate in candidates {
        if evaluate_single_jsonpath_predicate(candidate, predicate) {
            return true;
        }
    }
    false
}

fn evaluate_single_jsonpath_predicate(candidate: &Value, predicate: &str) -> bool {
    let predicate = strip_wrapping_parens(predicate.trim());

    if let Some(parts) = split_predicate(predicate, "||") {
        return parts
            .iter()
            .any(|part| evaluate_single_jsonpath_predicate(candidate, part));
    }
    if let Some(parts) = split_predicate(predicate, "&&") {
        return parts
            .iter()
            .all(|part| evaluate_single_jsonpath_predicate(candidate, part));
    }

    if let Some(result) = evaluate_jsonpath_count_predicate(candidate, predicate) {
        return result;
    }
    if let Some(result) = evaluate_jsonpath_comparison_predicate(candidate, predicate) {
        return result;
    }
    if predicate.starts_with('@') || predicate.starts_with('$') {
        return is_truthy(&extract_jsonpath_relative(candidate, predicate));
    }
    false
}

fn strip_wrapping_parens(input: &str) -> &str {
    let mut trimmed = input.trim();
    loop {
        if !(trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2) {
            return trimmed;
        }
        if !is_fully_parenthesized(trimmed) {
            return trimmed;
        }
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
}

fn is_fully_parenthesized(input: &str) -> bool {
    let mut depth = 0usize;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
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
            '(' => depth = depth.saturating_add(1),
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx != input.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_predicate<'a>(input: &'a str, delimiter: &str) -> Option<Vec<&'a str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut found = false;

    for (idx, ch) in input.char_indices() {
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
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }

        if paren_depth == 0 && bracket_depth == 0 && input[idx..].starts_with(delimiter) {
            let part = input[start..idx].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = idx + delimiter.len();
            found = true;
        }
    }

    if !found {
        return None;
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

fn evaluate_jsonpath_count_predicate(context_value: &Value, predicate: &str) -> Option<bool> {
    let trimmed = predicate.trim();
    let after_count = trimmed.strip_prefix("count")?.trim_start();
    if !after_count.starts_with('(') {
        return None;
    }
    // Find the matching close paren using depth tracking to handle
    // nested expressions like count(@.items[?(@.active)]).
    let mut depth = 0usize;
    let mut close = None;
    for (i, ch) in after_count.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let path = after_count[1..close].trim();
    let remainder = after_count[close + 1..].trim();
    let (op, rhs) = parse_leading_comparison(remainder)?;
    let rhs_num = rhs.parse::<f64>().ok()?;
    let value = extract_jsonpath_relative(context_value, path);
    let lhs = count_jsonpath_nodes(&value) as f64;
    Some(compare_with_op(&lhs, &rhs_num, op))
}

fn evaluate_jsonpath_comparison_predicate(context_value: &Value, predicate: &str) -> Option<bool> {
    let (left_raw, op, right_raw) = split_comparison_expression(predicate)?;
    if !(left_raw.starts_with('@') || left_raw.starts_with('$')) {
        return None;
    }

    let left = extract_jsonpath_relative(context_value, left_raw);
    let right = if right_raw.starts_with('@') || right_raw.starts_with('$') {
        extract_jsonpath_relative(context_value, right_raw)
    } else {
        parse_literal_value(right_raw)?
    };

    Some(compare_json_values(&left, &right, op))
}

fn extract_jsonpath_relative(context_value: &Value, path: &str) -> Value {
    let normalized = normalize_jsonpath_path(path);
    if normalized.is_empty() {
        return context_value.clone();
    }
    let eval = ExpressionEvaluator::new(EvalContext {
        response_body: Some(context_value.clone()),
        ..EvalContext::default()
    });
    eval.evaluate(&format!("$response.body.{normalized}"))
}

fn normalize_jsonpath_path(path: &str) -> String {
    let trimmed = path.trim();
    if let Some(value) = trimmed.strip_prefix("$.") {
        return value.to_string();
    }
    if trimmed == "$" {
        return String::new();
    }
    if let Some(value) = trimmed.strip_prefix('$') {
        return value.trim_start_matches('.').to_string();
    }
    if let Some(value) = trimmed.strip_prefix("@.") {
        return value.to_string();
    }
    if trimmed == "@" {
        return String::new();
    }
    if let Some(value) = trimmed.strip_prefix('@') {
        return value.trim_start_matches('.').to_string();
    }
    trimmed.to_string()
}

fn parse_leading_comparison(input: &str) -> Option<(&str, &str)> {
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some(rhs) = input.strip_prefix(op) {
            return Some((op, rhs.trim()));
        }
    }
    None
}

fn split_comparison_expression(input: &str) -> Option<(&str, &str, &str)> {
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some(idx) = find_operator_outside_quotes(input, op) {
            let left = input[..idx].trim();
            let right = input[idx + op.len()..].trim();
            if left.is_empty() || right.is_empty() {
                return None;
            }
            return Some((left, op, right));
        }
    }
    None
}

fn find_operator_outside_quotes(input: &str, needle: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
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

fn parse_literal_value(input: &str) -> Option<Value> {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("null") {
        return Some(Value::Null);
    }
    if trimmed.eq_ignore_ascii_case("true") {
        return Some(Value::Bool(true));
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return Some(Value::Bool(false));
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(json!(value));
    }
    if let Ok(value) = trimmed.parse::<f64>() {
        return Some(json!(value));
    }
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        let unescaped = inner
            .replace("\\\"", "\"")
            .replace("\\'", "'")
            .replace("\\\\", "\\")
            .replace("\\n", "\n")
            .replace("\\t", "\t");
        return Some(Value::String(unescaped));
    }
    None
}

fn compare_json_values(left: &Value, right: &Value, op: &str) -> bool {
    match op {
        "==" => left == right,
        "!=" => left != right,
        ">" | "<" | ">=" | "<=" => {
            if let (Some(lhs), Some(rhs)) = (left.as_f64(), right.as_f64()) {
                return compare_with_op(&lhs, &rhs, op);
            }
            compare_with_op(&value_to_string(left), &value_to_string(right), op)
        }
        _ => false,
    }
}

fn compare_with_op<T: PartialOrd + PartialEq>(lhs: &T, rhs: &T, op: &str) -> bool {
    match op {
        "==" => lhs == rhs,
        "!=" => lhs != rhs,
        ">" => lhs > rhs,
        "<" => lhs < rhs,
        ">=" => lhs >= rhs,
        "<=" => lhs <= rhs,
        _ => false,
    }
}

fn count_jsonpath_nodes(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::Array(items) => items.len(),
        Value::Object(items) => items.len(),
        _ => 1,
    }
}

/// Parse `{sourceName}./path` prefix from an operationPath.
/// Returns None if no `{name}.` prefix is found — the dot after `}` is required
/// to distinguish source references from path parameter placeholders like `/{id}/resource`.
pub(super) fn parse_source_prefix(op_path: &str) -> Option<(&str, &str)> {
    if !op_path.starts_with('{') {
        return None;
    }
    let close = op_path.find('}')?;
    let name = &op_path[1..close];
    if name.is_empty() {
        return None;
    }
    let remaining = &op_path[close + 1..];
    let path = remaining.strip_prefix('.')?;
    Some((name, path))
}

pub(crate) fn parse_method(operation_path: &str) -> (&str, &str) {
    let Some(idx) = operation_path.find(' ') else {
        return ("", operation_path);
    };
    if idx == 0 || idx > 7 {
        return ("", operation_path);
    }
    let candidate = &operation_path[..idx];
    let valid = matches!(
        candidate,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
    );
    if valid {
        return (candidate, &operation_path[idx + 1..]);
    }
    ("", operation_path)
}

pub(super) fn replace_path_params(path: &str, params: &BTreeMap<String, String>) -> String {
    let mut remaining = path;
    let mut out = String::with_capacity(path.len());

    loop {
        let Some(open) = remaining.find('{') else {
            out.push_str(remaining);
            break;
        };
        let Some(close_rel) = remaining[open + 1..].find('}') else {
            out.push_str(remaining);
            break;
        };
        let close = open + 1 + close_rel;
        out.push_str(&remaining[..open]);
        let key = &remaining[open + 1..close];
        if let Some(value) = params.get(key) {
            let encoded = utf8_percent_encode(value, PATH_SEGMENT_ENCODE_SET).to_string();
            out.push_str(&encoded);
        } else {
            out.push_str(&remaining[open..=close]);
        }
        remaining = &remaining[close + 1..];
    }

    out
}

pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

pub(super) fn resolve_payload(value: &serde_yml::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yml::Value::Null => Value::Null,
        serde_yml::Value::Bool(v) => Value::Bool(*v),
        serde_yml::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else {
                Value::Null
            }
        }
        serde_yml::Value::String(v) => eval.resolve_value(v),
        serde_yml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(resolve_payload(item, eval));
            }
            Value::Array(out)
        }
        serde_yml::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                out.insert(key, resolve_payload(v, eval));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

pub(super) fn to_json_path(expr: &str) -> String {
    if let Some(path) = expr.strip_prefix("$response.body.") {
        return path.to_string();
    }
    if let Some(path) = expr.strip_prefix("$response.body") {
        return path.trim_start_matches('.').to_string();
    }
    expr.to_string()
}

pub(super) fn step_result_error(step_id: &str, result: &StepResult) -> RuntimeError {
    if let Some(err) = &result.err {
        return RuntimeError::new(
            RuntimeErrorKind::SuccessCriteriaFailed,
            format!("step {step_id}: {err}"),
        );
    }
    if let Some(resp) = &result.response {
        let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
        if body_preview.len() > 500 {
            let mut end = 500;
            while !body_preview.is_char_boundary(end) {
                end -= 1;
            }
            body_preview.truncate(end);
            body_preview.push_str("...");
        }
        return RuntimeError::new(
            RuntimeErrorKind::SuccessCriteriaFailed,
            format!(
                "step {step_id}: success criteria not met (status={}, body={})",
                resp.status_code, body_preview
            ),
        );
    }
    RuntimeError::new(
        RuntimeErrorKind::SuccessCriteriaFailed,
        format!("step {step_id}: success criteria not met"),
    )
}

pub(super) async fn sleep_with_cancel(
    delay: Duration,
    cancel: &CancellationToken,
    is_timeout: &AtomicBool,
) -> Result<(), RuntimeError> {
    if delay.is_zero() {
        return Ok(());
    }

    tokio::select! {
        () = tokio::time::sleep(delay) => Ok(()),
        () = cancel.cancelled() => {
            if is_timeout.load(Ordering::Acquire) {
                Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionTimeout,
                    "execution timeout exceeded",
                ))
            } else {
                Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionCancelled,
                    "execution cancelled",
                ))
            }
        },
    }
}

pub(super) fn can_execute_parallel(workflow: &Workflow) -> bool {
    !has_control_flow(workflow)
        && workflow
            .steps
            .iter()
            .all(|step| !matches!(&step.target, Some(StepTarget::WorkflowId(_))))
}

fn actions_have_control_flow(actions: &[OnAction]) -> bool {
    actions.iter().any(|a| {
        matches!(
            a.type_,
            ActionType::Goto | ActionType::Retry | ActionType::End
        )
    })
}

pub(crate) fn has_control_flow(workflow: &Workflow) -> bool {
    actions_have_control_flow(&workflow.success_actions)
        || actions_have_control_flow(&workflow.failure_actions)
        || workflow.steps.iter().any(|step| {
            actions_have_control_flow(&step.on_success)
                || actions_have_control_flow(&step.on_failure)
        })
}

pub(crate) fn build_levels(workflow: &Workflow) -> Result<Vec<Vec<usize>>, RuntimeError> {
    let mut step_id_to_index = BTreeMap::<String, usize>::new();
    for (idx, step) in workflow.steps.iter().enumerate() {
        step_id_to_index.insert(step.step_id.clone(), idx);
    }

    let mut deps = vec![BTreeSet::<usize>::new(); workflow.steps.len()];
    for (idx, step) in workflow.steps.iter().enumerate() {
        for dep_id in extract_step_refs(step) {
            if let Some(dep_idx) = step_id_to_index.get(&dep_id) {
                deps[idx].insert(*dep_idx);
            }
        }
    }

    let mut indegree = deps.iter().map(BTreeSet::len).collect::<Vec<_>>();
    let mut assigned = vec![false; workflow.steps.len()];
    let mut remaining = workflow.steps.len();
    let mut levels = Vec::<Vec<usize>>::new();

    while remaining > 0 {
        let mut level = Vec::new();
        for idx in 0..workflow.steps.len() {
            if !assigned[idx] && indegree[idx] == 0 {
                level.push(idx);
            }
        }
        if level.is_empty() {
            return Err(RuntimeError::new(
                RuntimeErrorKind::DependencyCycle,
                format!(
                    "dependency cycle detected in workflow \"{}\"",
                    workflow.workflow_id
                ),
            ));
        }
        for idx in &level {
            assigned[*idx] = true;
            remaining -= 1;
            for dep_idx in 0..deps.len() {
                if deps[dep_idx].remove(idx) {
                    indegree[dep_idx] -= 1;
                }
            }
        }
        levels.push(level);
    }

    Ok(levels)
}

pub(crate) fn extract_step_refs(step: &Step) -> Vec<String> {
    let mut refs = BTreeSet::<String>::new();

    let mut scan = |s: &str| {
        for captures in STEP_REF_RE.captures_iter(s) {
            if let Some(m) = captures.get(1) {
                refs.insert(m.as_str().to_string());
            }
        }
    };

    match &step.target {
        Some(StepTarget::OperationPath(p)) => scan(p),
        Some(StepTarget::OperationId(id)) => scan(id),
        _ => {}
    }
    for p in &step.parameters {
        let v = p.value_as_str();
        scan(&v);
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            scan_payload_refs(payload, &mut scan);
        }
    }
    for c in &step.success_criteria {
        scan(&c.condition);
        scan(&c.context);
    }
    for expr in step.outputs.values() {
        scan(expr);
    }
    for action in &step.on_success {
        for c in &action.criteria {
            scan(&c.condition);
        }
    }
    for action in &step.on_failure {
        for c in &action.criteria {
            scan(&c.condition);
        }
    }

    refs.into_iter().collect()
}

fn scan_payload_refs(value: &serde_yml::Value, scan: &mut impl FnMut(&str)) {
    match value {
        serde_yml::Value::String(s) => {
            if s.starts_with('$') {
                scan(s);
            } else if s.contains("{$") {
                for (pos, _) in s.match_indices("{$") {
                    if let Some(end) = s[pos + 1..].find('}') {
                        let ref_expr = &s[pos + 1..pos + 1 + end];
                        scan(ref_expr);
                    }
                }
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                scan_payload_refs(item, scan);
            }
        }
        serde_yml::Value::Mapping(map) => {
            for (_, v) in map {
                scan_payload_refs(v, scan);
            }
        }
        _ => {}
    }
}

/// Compute the transitive set of step indices that `target_step_id` depends on
/// (via `$steps.*` references). Returns a `BTreeSet` of step indices that must
/// execute before the target, **not** including the target itself.
pub(crate) fn compute_transitive_deps(
    workflow: &Workflow,
    target_step_id: &str,
) -> Result<BTreeSet<usize>, RuntimeError> {
    let mut id_to_idx = BTreeMap::<&str, usize>::new();
    for (idx, step) in workflow.steps.iter().enumerate() {
        id_to_idx.insert(&step.step_id, idx);
    }

    let target_idx = *id_to_idx.get(target_step_id).ok_or_else(|| {
        RuntimeError::new(
            RuntimeErrorKind::StepNotFound,
            format!(
                "step \"{}\" not found in workflow \"{}\"",
                target_step_id, workflow.workflow_id
            ),
        )
    })?;

    // BFS from target step over extract_step_refs edges
    let mut visited = BTreeSet::<usize>::new();
    let mut queue = std::collections::VecDeque::<usize>::new();
    queue.push_back(target_idx);

    while let Some(idx) = queue.pop_front() {
        let refs = extract_step_refs(&workflow.steps[idx]);
        for ref_id in &refs {
            if let Some(&dep_idx) = id_to_idx.get(ref_id.as_str()) {
                if dep_idx != target_idx && visited.insert(dep_idx) {
                    queue.push_back(dep_idx);
                }
            }
        }
    }

    Ok(visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arazzo_spec::{CriterionExpressionType, CriterionType};
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn parse_method_supports_known_verbs() {
        assert_eq!(parse_method("GET /items"), ("GET", "/items"));
        assert_eq!(parse_method("POST /items"), ("POST", "/items"));
        assert_eq!(parse_method("DELETE /items/1"), ("DELETE", "/items/1"));
        assert_eq!(parse_method("PATCH /items/1"), ("PATCH", "/items/1"));
        assert_eq!(parse_method("HEAD /health"), ("HEAD", "/health"));
        assert_eq!(parse_method("OPTIONS /api"), ("OPTIONS", "/api"));
        assert_eq!(parse_method("/items"), ("", "/items"));
        assert_eq!(parse_method(""), ("", ""));
        assert_eq!(parse_method("UNKNOWN /items"), ("", "UNKNOWN /items"));
    }

    #[test]
    fn evaluate_criterion_modes() {
        let cache = RegexCache::new();
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            response_body: Some(json!({
                "name":"alice",
                "ok":true,
                "pets":[{"id":1}],
                "items":[
                    {"id":1,"ok":false,"pets":[]},
                    {"id":2,"ok":true,"pets":[{"id":"a"}]}
                ]
            })),
            ..EvalContext::default()
        });

        let plain = SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        };
        assert!(evaluate_criterion(&plain, &eval, None, &cache));

        let regex = SuccessCriterion {
            type_: Some(CriterionType::Name("regex".to_string())),
            context: "$response.body.name".to_string(),
            condition: "^[a-z]+$".to_string(),
        };
        assert!(evaluate_criterion(&regex, &eval, None, &cache));

        let jsonpath = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body".to_string(),
            condition: "$.name".to_string(),
        };
        assert!(evaluate_criterion(&jsonpath, &eval, None, &cache));

        let jp_existence = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body".to_string(),
            condition: "$.ok".to_string(),
        };
        assert!(evaluate_criterion(&jp_existence, &eval, None, &cache));

        let jp_nested = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body".to_string(),
            condition: "$.pets[0].id".to_string(),
        };
        assert!(evaluate_criterion(&jp_nested, &eval, None, &cache));

        let jp_missing = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body".to_string(),
            condition: "$.nonexistent".to_string(),
        };
        assert!(!evaluate_criterion(&jp_missing, &eval, None, &cache));

        let jp_filter_at_ok = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(@.ok == true)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_filter_at_ok, &eval, None, &cache));

        let jp_filter_none = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(@.id == 999)]".to_string(),
        };
        assert!(!evaluate_criterion(&jp_filter_none, &eval, None, &cache));

        let jp_count = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(count(@.pets) > 0)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_count, &eval, None, &cache));

        let jp_and = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(@.ok == true && @.id == 2)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_and, &eval, None, &cache));

        let jp_or = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(@.id == 99 || @.id == 1)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_or, &eval, None, &cache));

        let jp_comparison = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(@.id > 1)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_comparison, &eval, None, &cache));

        let jp_root_count = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "jsonpath".to_string(),
                version: "draft-goessner-dispatch-jsonpath-00".to_string(),
            })),
            context: "$response.body.items".to_string(),
            condition: "$[?(count($) > 0)]".to_string(),
        };
        assert!(evaluate_criterion(&jp_root_count, &eval, None, &cache));
    }

    #[test]
    fn evaluate_criterion_xpath_uses_context_and_condition() {
        let cache = RegexCache::new();
        let criterion = SuccessCriterion {
            type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                type_: "xpath".to_string(),
                version: "xpath-10".to_string(),
            })),
            context: "$response.body".to_string(),
            condition: "//item[1]/title".to_string(),
        };
        let response = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: br#"<?xml version="1.0"?><rss><channel><item><title>Hello</title></item></channel></rss>"#
                .to_vec(),
            body_json: None,
            content_type: ContentType::Xml,
        };
        let eval = ExpressionEvaluator::new(EvalContext::default());

        assert!(evaluate_criterion(
            &criterion,
            &eval,
            Some(&response),
            &cache
        ));
    }

    #[test]
    fn extract_step_refs_and_control_flow() {
        let step = Step {
            step_id: "s2".to_string(),
            target: Some(StepTarget::OperationPath(
                "/items/$steps.s1.outputs.id".to_string(),
            )),
            parameters: vec![arazzo_spec::Parameter {
                name: "q".to_string(),
                in_: Some(ParamLocation::Query),
                value: serde_yml::Value::String("$steps.s1.outputs.query".to_string()),
                ..arazzo_spec::Parameter::default()
            }],
            outputs: BTreeMap::from([("val".to_string(), "$steps.s1.outputs.value".to_string())]),
            on_failure: vec![OnAction {
                type_: ActionType::Retry,
                criteria: vec![SuccessCriterion {
                    condition: "$steps.s1.outputs.code == 429".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..OnAction::default()
            }],
            ..Step::default()
        };

        let refs = extract_step_refs(&step);
        assert_eq!(refs, vec!["s1".to_string()]);

        let wf_no_flow = Workflow {
            workflow_id: "no-flow".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        };
        assert!(!has_control_flow(&wf_no_flow));

        let wf_with_flow = Workflow {
            workflow_id: "with-flow".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                on_failure: vec![OnAction {
                    type_: ActionType::Goto,
                    step_id: "fallback".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        };
        assert!(has_control_flow(&wf_with_flow));
    }

    #[test]
    fn build_levels_supports_independent_chain_and_cycle() {
        let independent = Workflow {
            workflow_id: "independent".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let independent_levels = match build_levels(&independent) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(independent_levels, vec![vec![0, 1]]);

        let chain = Workflow {
            workflow_id: "chain".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/one".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/two".to_string())),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.s1.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let chain_levels = match build_levels(&chain) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(chain_levels, vec![vec![0], vec![1]]);

        let cycle = Workflow {
            workflow_id: "cycle".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.s2.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.s1.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let cycle_result = build_levels(&cycle);
        let cycle_err = match cycle_result {
            Ok(_) => panic!("expected cycle detection error"),
            Err(err) => err,
        };
        assert!(cycle_err.message.contains("dependency cycle detected"));
        assert_eq!(cycle_err.kind, RuntimeErrorKind::DependencyCycle);
    }

    #[test]
    fn build_levels_supports_diamond_dependency() {
        let workflow = Workflow {
            workflow_id: "diamond".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    parameters: vec![
                        arazzo_spec::Parameter {
                            name: "x".to_string(),
                            in_: Some(ParamLocation::Query),
                            value: serde_yml::Value::String("$steps.a.outputs.id".to_string()),
                            ..arazzo_spec::Parameter::default()
                        },
                        arazzo_spec::Parameter {
                            name: "y".to_string(),
                            in_: Some(ParamLocation::Query),
                            value: serde_yml::Value::String("$steps.b.outputs.id".to_string()),
                            ..arazzo_spec::Parameter::default()
                        },
                    ],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let levels = match build_levels(&workflow) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(levels, vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn compute_transitive_deps_no_deps_returns_empty() {
        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let deps = match compute_transitive_deps(&workflow, "a") {
            Ok(d) => d,
            Err(e) => panic!("no-deps step a should resolve: {e}"),
        };
        assert!(deps.is_empty(), "step with no refs should have no deps");

        let deps = match compute_transitive_deps(&workflow, "b") {
            Ok(d) => d,
            Err(e) => panic!("no-deps step b should resolve: {e}"),
        };
        assert!(deps.is_empty(), "step with no refs should have no deps");
    }

    #[test]
    fn compute_transitive_deps_direct_dependency() {
        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    outputs: BTreeMap::from([(
                        "val".to_string(),
                        "$steps.a.outputs.result".to_string(),
                    )]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let deps = match compute_transitive_deps(&workflow, "b") {
            Ok(d) => d,
            Err(e) => panic!("direct dep should resolve: {e}"),
        };
        assert_eq!(deps.len(), 1);
        assert!(deps.contains(&0), "b depends on a (index 0)");
    }

    #[test]
    fn compute_transitive_deps_transitive_chain() {
        // c -> b -> a (transitive)
        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    outputs: BTreeMap::from([(
                        "val".to_string(),
                        "$steps.a.outputs.result".to_string(),
                    )]),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    target: Some(StepTarget::OperationPath("/c".to_string())),
                    outputs: BTreeMap::from([(
                        "val".to_string(),
                        "$steps.b.outputs.val".to_string(),
                    )]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let deps = match compute_transitive_deps(&workflow, "c") {
            Ok(d) => d,
            Err(e) => panic!("transitive chain should resolve: {e}"),
        };
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&0), "c transitively depends on a");
        assert!(deps.contains(&1), "c directly depends on b");
    }

    #[test]
    fn compute_transitive_deps_diamond() {
        // d -> b, d -> c, b -> a, c -> a
        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.a.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "y".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.a.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "d".to_string(),
                    outputs: BTreeMap::from([
                        ("v1".to_string(), "$steps.b.outputs.r".to_string()),
                        ("v2".to_string(), "$steps.c.outputs.r".to_string()),
                    ]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let deps = match compute_transitive_deps(&workflow, "d") {
            Ok(d) => d,
            Err(e) => panic!("diamond deps should resolve: {e}"),
        };
        assert_eq!(deps.len(), 3, "d depends on a, b, c");
        assert!(deps.contains(&0));
        assert!(deps.contains(&1));
        assert!(deps.contains(&2));
    }

    #[test]
    fn compute_transitive_deps_unknown_step_errors() {
        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "a".to_string(),
                ..Step::default()
            }],
            ..Workflow::default()
        };

        let err = match compute_transitive_deps(&workflow, "missing") {
            Ok(_) => panic!("unknown step should error"),
            Err(e) => e,
        };
        assert_eq!(err.kind, RuntimeErrorKind::StepNotFound);
    }

    #[test]
    fn test_xpath_extraction() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title>First Story</title>
      <link>https://example.com/1</link>
    </item>
    <item>
      <title>Second Story</title>
      <link>https://example.com/2</link>
    </item>
  </channel>
</rss>"#;
        assert_eq!(
            extract_xpath(xml, "//item[1]/title"),
            Value::String("First Story".to_string())
        );
        assert_eq!(
            extract_xpath(xml, "//item[2]/title"),
            Value::String("Second Story".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_cdata() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title><![CDATA[Story with <special> chars]]></title>
    </item>
  </channel>
</rss>"#;
        assert_eq!(
            extract_xpath(xml, "//item[1]/title"),
            Value::String("Story with <special> chars".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_atom() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
  <title>top scoring links : technology</title>
  <entry>
    <title>First Reddit Post</title>
    <link href="https://reddit.com/1"/>
  </entry>
  <entry>
    <title>Second Reddit Post</title>
    <link href="https://reddit.com/2"/>
  </entry>
</feed>"#;
        assert_eq!(
            extract_xpath(xml, "//entry[1]/title"),
            Value::String("First Reddit Post".to_string())
        );
        assert_eq!(
            extract_xpath(xml, "//entry[2]/title"),
            Value::String("Second Reddit Post".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_no_match() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel></channel></rss>"#;
        assert_eq!(extract_xpath(xml, "//item[1]/title"), Value::Null);
    }

    #[test]
    fn test_xpath_extraction_invalid_xml() {
        let body = b"this is not xml at all <broken>";
        assert_eq!(extract_xpath(body, "//item[1]/title"), Value::Null);
    }

    proptest! {
        #[test]
        fn parse_method_round_trips_known_verbs(
            method in prop_oneof![
                Just("GET"),
                Just("POST"),
                Just("PUT"),
                Just("PATCH"),
                Just("DELETE"),
                Just("HEAD"),
                Just("OPTIONS"),
            ],
            path in "[a-zA-Z0-9/_\\-\\?=&]{0,32}",
        ) {
            let operation_path = format!("{method} /{path}");
            let (parsed_method, parsed_path) = parse_method(&operation_path);
            prop_assert_eq!(parsed_method, method);
            prop_assert_eq!(parsed_path, format!("/{path}"));
        }

        #[test]
        fn build_levels_respects_dependency_order_for_generated_dags(
            size in 1usize..8usize,
            mask in any::<u64>(),
        ) {
            let mut bit_index = 0u32;
            let mut steps = Vec::<Step>::new();
            for idx in 0..size {
                let mut parameters = Vec::new();
                for dep in 0..idx {
                    let has_edge = ((mask >> bit_index) & 1) == 1;
                    bit_index = bit_index.saturating_add(1);
                    if has_edge {
                        parameters.push(arazzo_spec::Parameter {
                            name: format!("p{dep}"),
                            in_: Some(ParamLocation::Query),
                            value: serde_yml::Value::String(format!(
                                "$steps.s{dep}.outputs.value"
                            )),
                            ..arazzo_spec::Parameter::default()
                        });
                    }
                }

                steps.push(Step {
                    step_id: format!("s{idx}"),
                    target: Some(StepTarget::OperationPath(format!("/s{idx}"))),
                    parameters,
                    ..Step::default()
                });
            }

            let workflow = Workflow {
                workflow_id: "wf".to_string(),
                steps,
                ..Workflow::default()
            };

            let levels = build_levels(&workflow).unwrap_or_else(|err| {
                panic!("expected DAG levels, got error: {err}");
            });

            let mut flattened = Vec::<usize>::new();
            for level in &levels {
                for step_idx in level {
                    flattened.push(*step_idx);
                }
            }
            let mut sorted = flattened.clone();
            sorted.sort_unstable();
            prop_assert_eq!(sorted, (0..size).collect::<Vec<_>>());

            let mut rank = vec![usize::MAX; size];
            for (level_idx, level) in levels.iter().enumerate() {
                for step_idx in level {
                    rank[*step_idx] = level_idx;
                }
            }

            for step_idx in 0..size {
                let refs = extract_step_refs(&workflow.steps[step_idx]);
                for dep in refs {
                    let dep_idx = dep
                        .trim_start_matches('s')
                        .parse::<usize>()
                        .unwrap_or(usize::MAX);
                    prop_assert!(dep_idx < size);
                    prop_assert!(rank[dep_idx] < rank[step_idx]);
                }
            }
        }
    }
}
