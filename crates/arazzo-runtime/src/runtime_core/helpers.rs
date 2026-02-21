use super::*;

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
    match criterion.type_.as_str() {
        "regex" => {
            let context_value = eval.evaluate_string(&criterion.context);
            match Regex::new(&criterion.condition) {
                Ok(re) => re.is_match(&context_value),
                Err(_) => false,
            }
        }
        "jsonpath" => {
            if let Some(resp) = response {
                if resp.content_type == "xml" {
                    return is_truthy(&extract_xpath(&resp.body, &criterion.condition));
                }
            }
            let value = eval.evaluate(&format!("$response.body.{}", criterion.condition));
            is_truthy(&value)
        }
        _ => eval.evaluate_condition(&criterion.condition),
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
        return RuntimeError(format!("step {step_id}: {err}"));
    }
    if let Some(resp) = &result.response {
        let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
        if body_preview.len() > 500 {
            body_preview.truncate(500);
            body_preview.push_str("...");
        }
        return RuntimeError(format!(
            "step {step_id}: success criteria not met (status={}, body={})",
            resp.status_code, body_preview
        ));
    }
    RuntimeError(format!("step {step_id}: success criteria not met"))
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
            .all(|step| step.workflow_id.is_empty())
}

pub(crate) fn has_control_flow(workflow: &Workflow) -> bool {
    for step in &workflow.steps {
        for action in &step.on_success {
            if matches!(action.type_.as_str(), "goto" | "retry" | "end") {
                return true;
            }
        }
        for action in &step.on_failure {
            if matches!(action.type_.as_str(), "goto" | "retry" | "end") {
                return true;
            }
        }
    }
    false
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
            return Err(RuntimeError(format!(
                "dependency cycle detected in workflow \"{}\"",
                workflow.workflow_id
            )));
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

    scan(&step.operation_path);
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
