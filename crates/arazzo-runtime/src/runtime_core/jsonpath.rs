use super::*;

/// Result of evaluating a JSONPath criterion condition.
pub(super) enum JsonPathOutcome {
    /// The condition was evaluated against the context value.
    Matched(bool),
    /// The condition uses JSONPath syntax outside the supported subset; the
    /// message names the offending construct.
    Unsupported(String),
}

/// Evaluates a JSONPath criterion condition against `context_value`.
///
/// Supported subset of the JSONPath grammar:
/// - dot paths and bracket indexing (`$.items[0].name`)
/// - filter predicates (`$[?(@.price > 10)]`) with `&&`, `||`, comparison
///   operators (`==`, `!=`, `>`, `<`, `>=`, `<=`), and `count(...)`
/// - bare existence checks (`$.name`, `@.name`)
///
/// Unsupported constructs — including recursive descent (`..`) and array
/// slices (`[a:b]`) — return [`JsonPathOutcome::Unsupported`] with a diagnostic
/// instead of silently evaluating to `false`.
pub(super) fn evaluate_jsonpath_condition(
    _eval: &ExpressionEvaluator,
    context_value: &Value,
    condition: &str,
) -> JsonPathOutcome {
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return JsonPathOutcome::Matched(false);
    }

    if let Some(reason) = detect_unsupported_jsonpath(trimmed) {
        return JsonPathOutcome::Unsupported(reason);
    }

    if let Some(predicate) = parse_jsonpath_filter_predicate(trimmed) {
        return JsonPathOutcome::Matched(evaluate_jsonpath_filter_predicate(
            context_value,
            predicate,
        ));
    }

    match arazzo_expr::select_json_path(context_value, trimmed) {
        Ok(selection) => JsonPathOutcome::Matched(is_truthy(&selection.value)),
        Err(err) => JsonPathOutcome::Unsupported(err.to_string()),
    }
}

/// Returns a diagnostic when `condition` uses JSONPath syntax outside the
/// supported subset: recursive descent (`..`) or array slices (`[a:b]`).
/// Quoted string literals are ignored so filter
/// predicates like `$[?(@.name == "a..b")]` are not misflagged.
fn detect_unsupported_jsonpath(condition: &str) -> Option<String> {
    let masked = mask_quoted_spans(condition);

    if masked.contains("..") {
        return Some(format!(
            "recursive descent \"..\" is not supported (in {condition:?})"
        ));
    }
    let mut rest = masked.as_str();
    while let Some(open) = rest.find('[') {
        let segment = &rest[open + 1..];
        let Some(close) = segment.find(']') else {
            break;
        };
        let inner = &segment[..close];
        if inner.contains(':')
            && inner
                .chars()
                .all(|c| c.is_ascii_digit() || c == ':' || c == '-' || c.is_whitespace())
        {
            return Some(format!(
                "array slice \"[{inner}]\" is not supported (in {condition:?})"
            ));
        }
        rest = &segment[close + 1..];
    }
    None
}

/// Replaces the contents of single- and double-quoted string literals with
/// `_` so syntax detection only sees structural characters.
fn mask_quoted_spans(condition: &str) -> String {
    let mut out = String::with_capacity(condition.len());
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for ch in condition.chars() {
        if let Some(quote) = in_quote {
            if escaped {
                escaped = false;
                out.push('_');
                continue;
            }
            if ch == '\\' {
                escaped = true;
                out.push('_');
                continue;
            }
            if ch == quote {
                in_quote = None;
                out.push(ch);
            } else {
                out.push('_');
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        }
        out.push(ch);
    }
    out
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
    arazzo_expr::select_json_path(context_value, path)
        .map(|selection| selection.value)
        .unwrap_or(Value::Null)
}

fn count_jsonpath_relative_nodes(context_value: &Value, path: &str) -> Option<usize> {
    arazzo_expr::select_json_path(context_value, path)
        .map(|selection| selection.match_count)
        .ok()
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

#[cfg(test)]
mod tests {
    use arazzo_expr::EvalContext;
    use serde_json::json;

    use super::*;

    fn evaluate(context_value: &Value, condition: &str) -> JsonPathOutcome {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        evaluate_jsonpath_condition(&eval, context_value, condition)
    }

    #[test]
    fn recursive_descent_reports_unsupported() {
        match evaluate(&json!({"foo": 1}), "$..foo") {
            JsonPathOutcome::Unsupported(reason) => {
                assert!(!reason.is_empty());
                assert!(reason.contains(".."), "reason should name the construct");
            }
            JsonPathOutcome::Matched(result) => {
                panic!("expected unsupported diagnostic, got Matched({result})")
            }
        }
    }

    #[test]
    fn wildcard_selects_values() {
        match evaluate(&json!([1, 2, 3]), "$[*]") {
            JsonPathOutcome::Matched(result) => assert!(result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("wildcard should be supported, got: {reason}")
            }
        }
    }

    #[test]
    fn array_slice_reports_unsupported() {
        match evaluate(&json!({"items": [1, 2, 3]}), "$.items[0:2]") {
            JsonPathOutcome::Unsupported(reason) => {
                assert!(!reason.is_empty());
                assert!(reason.contains("0:2"), "reason should name the construct");
            }
            JsonPathOutcome::Matched(result) => {
                panic!("expected unsupported diagnostic, got Matched({result})")
            }
        }
    }

    #[test]
    fn supported_filter_predicate_still_evaluates() {
        match evaluate(&json!([{"price": 15}]), "$[?(@.price > 10)]") {
            JsonPathOutcome::Matched(result) => assert!(result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("filter predicate should stay supported, got: {reason}")
            }
        }
        match evaluate(&json!([{"price": 5}]), "$[?(@.price > 10)]") {
            JsonPathOutcome::Matched(result) => assert!(!result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("filter predicate should stay supported, got: {reason}")
            }
        }
    }

    #[test]
    fn supported_count_predicate_still_evaluates() {
        // count(...) counts resolved nodes: a present path resolves to one node.
        match evaluate(&json!({"items": [1, 2, 3]}), "$[?(count(@.items) == 1)]") {
            JsonPathOutcome::Matched(result) => assert!(result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("count predicate should stay supported, got: {reason}")
            }
        }
        match evaluate(&json!({"items": [1, 2, 3]}), "$[?(count(@.missing) == 1)]") {
            JsonPathOutcome::Matched(result) => assert!(!result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("count predicate should stay supported, got: {reason}")
            }
        }
    }

    #[test]
    fn quoted_literals_are_not_misflagged() {
        match evaluate(&json!([{"name": "a..b"}]), r#"$[?(@.name == "a..b")]"#) {
            JsonPathOutcome::Matched(result) => assert!(result),
            JsonPathOutcome::Unsupported(reason) => {
                panic!("quoted literal should not trigger detection: {reason}")
            }
        }
    }
}
