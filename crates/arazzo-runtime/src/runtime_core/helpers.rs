use super::*;

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
}

pub(crate) fn extract_xpath(body: &[u8], expr: &str) -> Value {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };
    // Strip default namespace declarations so simple XPath expressions
    // work on both RSS 2.0 (no namespace) and Atom (xmlns="...") feeds.
    // Preserves prefixed namespaces like xmlns:media="...".
    let Ok(re) = Regex::new(r#"xmlns="[^"]*""#) else {
        return Value::Null;
    };
    let text = re.replace_all(text, "");
    let package = match sxd_document::parser::parse(&text) {
        Ok(p) => p,
        Err(_) => return Value::Null,
    };
    let doc = package.as_document();
    match sxd_xpath::evaluate_xpath(&doc, expr) {
        Ok(val) => {
            let s = val.string();
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Err(_) => Value::Null,
    }
}

pub(crate) fn evaluate_criterion(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> bool {
    evaluate_criterion_detailed(criterion, eval, response).matched
}

pub(crate) fn evaluate_criterion_detailed(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> CriterionEvaluation {
    let type_name = criterion.resolved_type_name();
    let mut context_value = if criterion.context.trim().is_empty() {
        default_criterion_context(response)
    } else {
        eval.evaluate(&criterion.context)
    };
    let mut error = None;

    let condition_result = match type_name.as_str() {
        "regex" => {
            let context_text = value_to_string(&context_value);
            match Regex::new(&criterion.condition) {
                Ok(re) => re.is_match(&context_text),
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
        _ => eval.evaluate_condition(&criterion.condition),
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
    }
}

pub(crate) fn evaluate_output_expression(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> Value {
    if expr.starts_with('/') {
        if let Some(resp) = response {
            return extract_xpath(&resp.body, expr);
        }
        return Value::Null;
    }

    if expr.starts_with('$') {
        return eval.evaluate(expr);
    }

    let json_path = to_json_path(expr);
    eval.evaluate(&format!("$response.body.{json_path}"))
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
    let close = after_count.find(')')?;
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
        let value = &trimmed[1..trimmed.len() - 1];
        return Some(Value::String(value.to_string()));
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
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
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
            out.push_str(value);
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

pub(super) fn resolve_payload(value: &serde_yaml::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(v) => Value::Bool(*v),
        serde_yaml::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::String(v) => {
            if v.starts_with('$') {
                eval.evaluate(v)
            } else if v.contains("{$") {
                Value::String(eval.interpolate_string(v))
            } else {
                Value::String(v.clone())
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(resolve_payload(item, eval));
            }
            Value::Array(out)
        }
        serde_yaml::Value::Mapping(map) => {
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
            body_preview.truncate(500);
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

pub(super) fn sleep_with_checks(
    delay: Duration,
    options: &ExecutionOptions,
) -> Result<(), RuntimeError> {
    if delay.is_zero() {
        return Ok(());
    }

    let start = Instant::now();
    loop {
        options.check()?;
        let elapsed = start.elapsed();
        if elapsed >= delay {
            return Ok(());
        }
        let remaining = delay - elapsed;
        std::thread::sleep(remaining.min(SLEEP_CHECK_INTERVAL));
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
    let pattern = Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")
        .unwrap_or_else(|err| panic!("failed to compile step-ref regex: {err}"));

    let mut scan = |s: &str| {
        for captures in pattern.captures_iter(s) {
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
        scan(&p.value);
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

fn scan_payload_refs(value: &serde_yaml::Value, scan: &mut impl FnMut(&str)) {
    match value {
        serde_yaml::Value::String(s) => {
            if s.starts_with('$') {
                scan(s);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                scan_payload_refs(item, scan);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_, v) in map {
                scan_payload_refs(v, scan);
            }
        }
        _ => {}
    }
}
