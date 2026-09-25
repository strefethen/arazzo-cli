use super::*;

static STEP_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")
        .unwrap_or_else(|err| panic!("failed to compile step-ref regex: {err}"))
});

/// A declaration that keeps a workflow's steps out of parallel levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParallelBlocker {
    pub(crate) reason: SequentialFallbackReason,
    /// Step that declares it; empty for a workflow-level action.
    pub(crate) step_id: String,
    /// Completes the sentence "workflow … runs sequentially because".
    pub(crate) detail: String,
}

/// The first declaration that keeps `workflow` out of parallel levels, or
/// `None` when every step can run in dependency order.
///
/// A plain `retry` failure action re-runs only the step that failed, and that
/// step's prerequisites all completed in earlier levels, so its retries stay
/// inside the step's own task. Every other action moves execution elsewhere:
/// a success action can only be `end` or `goto`, and a retry naming a `stepId`
/// or `workflowId` runs that reference first. A step that calls a workflow
/// needs the invocation's shared execution context. Workflow-level lists block
/// even when every step declares its own, so the answer does not depend on how
/// step and workflow lists combine.
pub(crate) fn parallel_blocker(workflow: &Workflow) -> Option<ParallelBlocker> {
    for step in &workflow.steps {
        if let Some(StepTarget::WorkflowId(target)) = &step.target {
            return Some(ParallelBlocker {
                reason: SequentialFallbackReason::SubWorkflowStep,
                step_id: step.step_id.clone(),
                detail: format!("step \"{}\" calls workflow \"{target}\"", step.step_id),
            });
        }
        for (actions, branch) in [
            (&step.on_success, ActionBranch::Success),
            (&step.on_failure, ActionBranch::Failure),
        ] {
            let site = ActionListSite::Step {
                step_id: &step.step_id,
                branch,
            };
            if let Some(blocker) = action_list_blocker(actions, site) {
                return Some(blocker);
            }
        }
    }
    [
        (&workflow.success_actions, ActionBranch::Success),
        (&workflow.failure_actions, ActionBranch::Failure),
    ]
    .into_iter()
    .find_map(|(actions, branch)| action_list_blocker(actions, ActionListSite::Workflow { branch }))
}

/// Where an action list is declared, as fallback messages name it.
#[derive(Debug, Clone, Copy)]
enum ActionListSite<'a> {
    Step {
        step_id: &'a str,
        branch: ActionBranch,
    },
    Workflow {
        branch: ActionBranch,
    },
}

fn action_list_blocker(actions: &[OnAction], site: ActionListSite<'_>) -> Option<ParallelBlocker> {
    let (step_id, branch) = match site {
        ActionListSite::Step { step_id, branch } => (step_id, branch),
        ActionListSite::Workflow { branch } => ("", branch),
    };
    let action = actions
        .iter()
        .find(|action| branch == ActionBranch::Success || !is_plain_retry(action))?;
    let subject = match site {
        ActionListSite::Step { step_id, branch } => {
            format!("step \"{step_id}\" has an {} action", branch.label())
        }
        ActionListSite::Workflow { branch } => {
            let list = match branch {
                ActionBranch::Success => "successActions",
                ActionBranch::Failure => "failureActions",
            };
            format!("the workflow's {list} include an action")
        }
    };
    let action_type = action.action_type();
    let (reason, consequence) =
        if branch == ActionBranch::Failure && action_type == ActionType::Retry {
            let (field, target) = if action.step_id.is_empty() {
                ("workflowId", "workflow")
            } else {
                ("stepId", "step")
            };
            (
                SequentialFallbackReason::RetryReference,
                format!(" with a {field}, which runs another {target} before retrying"),
            )
        } else {
            (
                SequentialFallbackReason::ControlFlowAction,
                ", which redirects control flow".to_string(),
            )
        };
    Some(ParallelBlocker {
        reason,
        step_id: step_id.to_string(),
        detail: format!("{subject} of type \"{action_type}\"{consequence}"),
    })
}

/// A `retry` that names no recovery reference re-runs only its own step.
fn is_plain_retry(action: &OnAction) -> bool {
    action.action_type() == ActionType::Retry
        && action.step_id.is_empty()
        && action.workflow_id.is_empty()
}

/// Groups `workflow`'s steps into dependency levels for parallel execution.
///
/// A step's edges come from what it reads while it runs and from the action
/// criteria its result is routed through: a level routes each step inside the
/// step's own task against the outputs of earlier levels, so a routing
/// criterion that reads another step's outputs needs that step in an earlier
/// level, just like the step's request inputs do.
pub(crate) fn build_levels(workflow: &Workflow) -> Result<Vec<Vec<usize>>, RuntimeError> {
    let mut step_id_to_index = BTreeMap::<String, usize>::new();
    for (idx, step) in workflow.steps.iter().enumerate() {
        step_id_to_index.insert(step.step_id.clone(), idx);
    }

    let mut deps = vec![BTreeSet::<usize>::new(); workflow.steps.len()];
    for (idx, step) in workflow.steps.iter().enumerate() {
        let mut refs = routing_refs(workflow, step);
        refs.extend(execution_refs(step));
        for dep_id in refs {
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

/// `$steps` references read while routing `step`'s result: the criteria,
/// condition and context, of the success and failure actions it is routed
/// through, including the workflow-level lists it inherits.
///
/// The step's own id is left out. In a workflow that runs in parallel levels
/// only failed attempts are routed through actions, and a failed attempt
/// records no outputs, so sequential execution finds none of the step's own
/// outputs there either. The edge would only turn a valid workflow into a
/// dependency cycle.
fn routing_refs(workflow: &Workflow, step: &Step) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    let lists = [
        applicable_actions(&step.on_success, &workflow.success_actions),
        applicable_actions(&step.on_failure, &workflow.failure_actions),
    ];
    for criterion in lists
        .into_iter()
        .flatten()
        .flat_map(|action| &action.criteria)
    {
        scan_step_refs(&criterion.condition, &mut refs);
        scan_step_refs(&criterion.context, &mut refs);
    }
    refs.remove(&step.step_id);
    refs
}

fn scan_step_refs(text: &str, refs: &mut BTreeSet<String>) {
    for captures in STEP_REF_RE.captures_iter(text) {
        if let Some(m) = captures.get(1) {
            refs.insert(m.as_str().to_string());
        }
    }
}

/// Every `$steps` reference `step` declares: what it reads while it runs plus
/// its own action criteria conditions. Single-step execution resolves its
/// dependencies from these.
pub(crate) fn extract_step_refs(step: &Step) -> Vec<String> {
    let mut refs = execution_refs(step);
    for action in step.on_success.iter().chain(&step.on_failure) {
        for c in &action.criteria {
            scan_step_refs(&c.condition, &mut refs);
        }
    }
    refs.into_iter().collect()
}

/// `$steps` references `step` reads while it runs: `dependsOn`, its target,
/// parameters, request body, success criteria, and outputs.
fn execution_refs(step: &Step) -> BTreeSet<String> {
    let mut refs = BTreeSet::<String>::new();

    for dependency in &step.depends_on {
        if !dependency.is_empty() && !dependency.starts_with('$') {
            refs.insert(dependency.clone());
        }
    }

    let mut scan = |s: &str| scan_step_refs(s, &mut refs);

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

    refs
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

#[cfg(test)]
mod tests {
    use super::*;

    fn step(step_id: &str) -> Step {
        Step {
            step_id: step_id.to_string(),
            target: Some(StepTarget::OperationPath(format!("/{step_id}"))),
            ..Step::default()
        }
    }

    fn action(type_: ActionType) -> OnAction {
        OnAction {
            name: format!("{type_}-action"),
            type_: Some(type_),
            ..OnAction::default()
        }
    }

    fn criterion(context: &str, condition: &str) -> SuccessCriterion {
        SuccessCriterion {
            context: context.to_string(),
            condition: condition.to_string(),
            ..SuccessCriterion::default()
        }
    }

    fn workflow(steps: Vec<Step>) -> Workflow {
        Workflow {
            workflow_id: "wf".to_string(),
            steps,
            ..Workflow::default()
        }
    }

    fn blocker(wf: &Workflow) -> ParallelBlocker {
        parallel_blocker(wf).unwrap_or_else(|| panic!("expected a parallel blocker for {wf:?}"))
    }

    #[test]
    fn plain_failure_retries_do_not_block_parallel_levels() {
        let mut retrying = step("a");
        retrying.on_failure = vec![action(ActionType::Retry), action(ActionType::Retry)];
        let mut wf = workflow(vec![retrying, step("b")]);
        wf.failure_actions = vec![action(ActionType::Retry)];

        assert_eq!(parallel_blocker(&wf), None);
    }

    #[test]
    fn goto_and_end_actions_block_with_the_declaring_step() {
        let mut ending = step("b");
        ending.on_failure = vec![action(ActionType::Retry), action(ActionType::End)];
        let wf = workflow(vec![step("a"), ending]);

        assert_eq!(
            blocker(&wf),
            ParallelBlocker {
                reason: SequentialFallbackReason::ControlFlowAction,
                step_id: "b".to_string(),
                detail: "step \"b\" has an onFailure action of type \"end\", which redirects control flow"
                    .to_string(),
            }
        );

        let mut going = step("a");
        going.on_failure = vec![OnAction {
            step_id: "a".to_string(),
            ..action(ActionType::Goto)
        }];
        let blocked = blocker(&workflow(vec![going]));
        assert_eq!(blocked.reason, SequentialFallbackReason::ControlFlowAction);
        assert_eq!(blocked.step_id, "a");
    }

    #[test]
    fn every_success_action_blocks_even_a_nonconformant_retry() {
        let mut succeeding = step("a");
        succeeding.on_success = vec![action(ActionType::Retry)];

        let blocked = blocker(&workflow(vec![succeeding]));
        assert_eq!(blocked.reason, SequentialFallbackReason::ControlFlowAction);
        assert_eq!(
            blocked.detail,
            "step \"a\" has an onSuccess action of type \"retry\", which redirects control flow"
        );
    }

    #[test]
    fn retry_references_block_because_they_run_another_step_or_workflow() {
        let mut recovering = step("a");
        recovering.on_failure = vec![OnAction {
            step_id: "login".to_string(),
            ..action(ActionType::Retry)
        }];
        let blocked = blocker(&workflow(vec![recovering, step("login")]));
        assert_eq!(blocked.reason, SequentialFallbackReason::RetryReference);
        assert_eq!(
            blocked.detail,
            "step \"a\" has an onFailure action of type \"retry\" with a stepId, which runs another step before retrying"
        );

        let mut wf = workflow(vec![step("a")]);
        wf.failure_actions = vec![OnAction {
            workflow_id: "refresh-token".to_string(),
            ..action(ActionType::Retry)
        }];
        assert_eq!(
            blocker(&wf),
            ParallelBlocker {
                reason: SequentialFallbackReason::RetryReference,
                step_id: String::new(),
                detail: "the workflow's failureActions include an action of type \"retry\" with a workflowId, which runs another workflow before retrying"
                    .to_string(),
            }
        );
    }

    #[test]
    fn workflow_level_success_actions_and_subworkflow_steps_block() {
        let mut wf = workflow(vec![step("a")]);
        wf.success_actions = vec![action(ActionType::End)];
        let blocked = blocker(&wf);
        assert_eq!(blocked.reason, SequentialFallbackReason::ControlFlowAction);
        assert_eq!(blocked.step_id, "");

        let calling = Step {
            step_id: "call".to_string(),
            target: Some(StepTarget::WorkflowId("child".to_string())),
            ..Step::default()
        };
        assert_eq!(
            blocker(&workflow(vec![step("a"), calling])),
            ParallelBlocker {
                reason: SequentialFallbackReason::SubWorkflowStep,
                step_id: "call".to_string(),
                detail: "step \"call\" calls workflow \"child\"".to_string(),
            }
        );
    }

    #[test]
    fn failure_criteria_order_levels_through_condition_and_context() {
        let mut by_context = step("b");
        by_context.on_failure = vec![OnAction {
            criteria: vec![criterion("$steps.a.outputs.code", "^5")],
            ..action(ActionType::Retry)
        }];
        let wf = workflow(vec![step("a"), by_context]);
        // extract_step_refs does not read action criteria contexts; routing does.
        assert!(extract_step_refs(&wf.steps[1]).is_empty());
        assert_eq!(
            build_levels(&wf).unwrap_or_else(|err| panic!("levels: {err}")),
            vec![vec![0], vec![1]]
        );
    }

    #[test]
    fn inherited_workflow_failure_criteria_order_only_inheriting_steps() {
        let mut own_list = step("c");
        own_list.on_failure = vec![action(ActionType::Retry)];
        let mut wf = workflow(vec![step("a"), step("b"), own_list]);
        wf.failure_actions = vec![OnAction {
            criteria: vec![criterion("", "$steps.b.outputs.retryable == true")],
            ..action(ActionType::Retry)
        }];

        // "a" inherits the workflow list, so it waits for "b". "b" inherits it
        // too, but a step's routing never sees its own outputs, so no self-edge.
        assert_eq!(
            build_levels(&wf).unwrap_or_else(|err| panic!("levels: {err}")),
            vec![vec![1, 2], vec![0]]
        );

        wf.failure_actions[0].criteria = vec![criterion("", "$steps.c.outputs.retryable == true")];
        // "c" routes through its own list, so only "a" and "b" wait for it.
        assert_eq!(
            build_levels(&wf).unwrap_or_else(|err| panic!("levels: {err}")),
            vec![vec![2], vec![0, 1]]
        );
    }

    #[test]
    fn only_routing_self_references_are_dropped_from_levels() {
        let mut routed_on_itself = step("a");
        routed_on_itself.on_failure = vec![OnAction {
            criteria: vec![criterion("", "$steps.a.outputs.code == 503")],
            ..action(ActionType::Retry)
        }];
        let wf = workflow(vec![routed_on_itself]);
        assert_eq!(
            build_levels(&wf).unwrap_or_else(|err| panic!("levels: {err}")),
            vec![vec![0]]
        );
        // Single-step dependency resolution still reads the condition.
        assert_eq!(extract_step_refs(&wf.steps[0]), vec!["a".to_string()]);

        let reads_itself = Step {
            parameters: vec![Parameter {
                name: "id".to_string(),
                in_: Some(ParamLocation::Query),
                value: serde_yaml_ng::Value::String("$steps.a.outputs.id".to_string()).into(),
                ..Parameter::default()
            }],
            ..step("a")
        };
        assert_eq!(
            build_levels(&workflow(vec![reads_itself]))
                .map_err(|err| err.kind)
                .err(),
            Some(RuntimeErrorKind::DependencyCycle)
        );
    }
}
