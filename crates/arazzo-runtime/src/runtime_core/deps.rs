use super::*;

static STEP_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")
        .unwrap_or_else(|err| panic!("failed to compile step-ref regex: {err}"))
});

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
            a.action_type(),
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

    for dependency in &step.depends_on {
        if !dependency.is_empty() && !dependency.starts_with('$') {
            refs.insert(dependency.clone());
        }
    }

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
        scan_value_source_refs(&p.value, &mut scan);
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            scan_value_source_refs(payload, &mut scan);
        }
        for replacement in &body.replacements {
            scan_value_source_refs(&replacement.value, &mut scan);
        }
    }
    for c in &step.success_criteria {
        scan(&c.condition);
        scan(&c.context);
    }
    for output in step.outputs.values() {
        match output {
            OutputValue::RuntimeExpression(expression) => scan(expression),
            OutputValue::Selector(selector) => scan(&selector.context),
        }
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

fn scan_value_source_refs(value: &ValueSource, scan: &mut impl FnMut(&str)) {
    match value {
        ValueSource::Selector(selector) => scan(&selector.context),
        ValueSource::Literal(value) => scan_literal_refs(value, scan),
    }
}

fn scan_literal_refs(value: &serde_yaml_ng::Value, scan: &mut impl FnMut(&str)) {
    match value {
        serde_yaml_ng::Value::String(s) => {
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
        serde_yaml_ng::Value::Sequence(seq) => {
            for item in seq {
                scan_value_source_refs(&item.clone().into(), scan);
            }
        }
        serde_yaml_ng::Value::Mapping(map) => {
            for (_, v) in map {
                scan_value_source_refs(&v.clone().into(), scan);
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
