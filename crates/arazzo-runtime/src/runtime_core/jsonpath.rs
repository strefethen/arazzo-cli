use super::*;

pub(super) fn evaluate_jsonpath_condition(
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
    // Also tracks quote state so ')' inside string literals is ignored.
    let mut depth = 0usize;
    let mut close = None;
    let mut in_quote: Option<char> = None;
    let mut prev_backslash = false;
    for (i, ch) in after_count.char_indices() {
        if let Some(q) = in_quote {
            if ch == q && !prev_backslash {
                in_quote = None;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
            continue;
        }
        if (ch == '\'' || ch == '"') && !prev_backslash {
            in_quote = Some(ch);
            prev_backslash = false;
            continue;
        }
        prev_backslash = ch == '\\' && !prev_backslash;
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
    let lhs = count_jsonpath_relative_nodes(context_value, path)? as f64;
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

fn count_jsonpath_relative_nodes(context_value: &Value, path: &str) -> Option<usize> {
    let normalized = normalize_jsonpath_path(path);
    if normalized.is_empty() {
        return Some(1);
    }
    arazzo_expr::count_resolved_path_nodes(context_value, &normalized).ok()
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
