use super::*;

impl Engine {
    pub(super) async fn handle_step_result(&self, ctx: StepDecisionContext<'_>) -> RoutedDecision {
        let step = &ctx.workflow.steps[ctx.step_idx];
        if ctx.cancel.is_cancelled() {
            return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
        }

        if ctx.result.success {
            let success_actions =
                applicable_actions(&step.on_success, &ctx.workflow.success_actions);
            let action = match self
                .find_matching_action_with_debug(
                    ActionSelectionContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        branch: ActionBranch::Success,
                        vars: ctx.vars,
                        response: ctx.result.response.as_deref(),
                        depth: ctx.depth,
                        cancel: ctx.cancel,
                        is_timeout: ctx.is_timeout,
                    },
                    success_actions,
                )
                .await
            {
                Ok(action) => action,
                Err(err) => return RoutedDecision::error(err),
            };
            if ctx.cancel.is_cancelled() {
                return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
            }
            if let Some(action) = action {
                let decision = self
                    .execute_action(
                        ExecuteActionContext {
                            workflow: ctx.workflow,
                            current_idx: ctx.step_idx,
                            is_failure_path: false,
                            retry_count: ctx.retry_count,
                            retry_site: RetrySite::new(ctx.step_idx, action.index),
                            cancel: ctx.cancel,
                            is_timeout: ctx.is_timeout,
                            response: ctx.result.response.as_deref(),
                            vars: ctx.vars,
                            original_err_kind: ctx.result.err_kind,
                        },
                        action.action,
                        Some(SelectedActionDebugContext {
                            workflow_id: ctx.workflow_id,
                            step,
                            vars: ctx.vars,
                            response: ctx.result.response.as_deref(),
                            depth: ctx.depth,
                            branch: ActionBranch::Success,
                            action_index: action.index,
                        }),
                    )
                    .await;
                if ctx.cancel.is_cancelled() {
                    return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
                }
                return match decision {
                    ActionExecution::Routed(decision) => decision,
                    ActionExecution::RetryExhausted(exhausted) => {
                        Self::retry_limit_exceeded_decision(step, exhausted)
                    }
                };
            }
            return RoutedDecision {
                flow: FlowDecision::Next(ctx.step_idx + 1),
                trace: TraceDecision::with_path(TraceDecisionPath::Next),
            };
        }

        let failure_actions = applicable_actions(&step.on_failure, &ctx.workflow.failure_actions);
        // Failure Action Object §5.8.8.1 requires an exhausted retry to yield
        // to subsequent failure actions. Continue the same routing pass from
        // the action after the exhausted retry; do not evaluate earlier actions
        // against the same failed response again.
        let mut next_action_index = 0;
        let mut exhausted_retry = None;
        loop {
            if ctx.cancel.is_cancelled() {
                return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
            }
            let action = match self
                .find_matching_action_with_debug_from(
                    ActionSelectionContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        branch: ActionBranch::Failure,
                        vars: ctx.vars,
                        response: ctx.result.response.as_deref(),
                        depth: ctx.depth,
                        cancel: ctx.cancel,
                        is_timeout: ctx.is_timeout,
                    },
                    failure_actions,
                    next_action_index,
                )
                .await
            {
                Ok(action) => action,
                Err(err) => return RoutedDecision::error(err),
            };
            if ctx.cancel.is_cancelled() {
                return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
            }
            let Some(action) = action else {
                return match exhausted_retry {
                    Some(exhausted) => Self::retry_limit_exceeded_decision(step, exhausted),
                    None => RoutedDecision::error(step_result_error(&step.step_id, ctx.result)),
                };
            };

            let decision = self
                .execute_action(
                    ExecuteActionContext {
                        workflow: ctx.workflow,
                        current_idx: ctx.step_idx,
                        is_failure_path: true,
                        retry_count: ctx.retry_count,
                        retry_site: RetrySite::new(ctx.step_idx, action.index),
                        cancel: ctx.cancel,
                        is_timeout: ctx.is_timeout,
                        response: ctx.result.response.as_deref(),
                        vars: ctx.vars,
                        original_err_kind: ctx.result.err_kind,
                    },
                    action.action,
                    Some(SelectedActionDebugContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        vars: ctx.vars,
                        response: ctx.result.response.as_deref(),
                        depth: ctx.depth,
                        branch: ActionBranch::Failure,
                        action_index: action.index,
                    }),
                )
                .await;
            if ctx.cancel.is_cancelled() {
                return RoutedDecision::error(control::cancellation_error(ctx.is_timeout));
            }

            match decision {
                ActionExecution::Routed(decision) => return decision,
                ActionExecution::RetryExhausted(exhausted) => {
                    exhausted_retry = Some(exhausted);
                    next_action_index = action.index.saturating_add(1);
                }
            }
        }
    }

    fn retry_limit_exceeded_decision(step: &Step, exhausted: ExhaustedRetry) -> RoutedDecision {
        RoutedDecision {
            flow: FlowDecision::Error(RuntimeError::new(
                RuntimeErrorKind::RetryLimitExceeded,
                format!(
                    "step {}: max retries ({}) exceeded",
                    step.step_id, exhausted.effective_limit
                ),
            )),
            trace: TraceDecision {
                action_type: ActionType::Retry.to_string(),
                retry_after_seconds: Some(exhausted.retry_after),
                retry_limit: exhausted.configured_limit,
                ..TraceDecision::with_path(TraceDecisionPath::Error)
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn find_matching_action<'a>(
        &self,
        actions: &'a [OnAction],
        vars: &VarStore,
        response: Option<&Response>,
    ) -> Option<&'a OnAction> {
        let eval = ExpressionEvaluator::new(self.make_eval_context(vars, response));
        for action in actions {
            if action.criteria.is_empty() {
                return Some(action);
            }
            let mut all_match = true;
            for criterion in &action.criteria {
                if !evaluate_criterion(criterion, &eval, response, &self.inner.regex_cache) {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Some(action);
            }
        }
        None
    }

    async fn find_matching_action_with_debug<'a>(
        &self,
        ctx: ActionSelectionContext<'_>,
        actions: &'a [OnAction],
    ) -> Result<Option<MatchedActionRef<'a>>, RuntimeError> {
        self.find_matching_action_with_debug_from(ctx, actions, 0)
            .await
    }

    async fn find_matching_action_with_debug_from<'a>(
        &self,
        ctx: ActionSelectionContext<'_>,
        actions: &'a [OnAction],
        start_index: usize,
    ) -> Result<Option<MatchedActionRef<'a>>, RuntimeError> {
        if ctx.cancel.is_cancelled() {
            return Err(control::cancellation_error(ctx.is_timeout));
        }
        let eval = ExpressionEvaluator::new(self.make_eval_context(ctx.vars, ctx.response));
        let current_outputs = ctx.vars.step_outputs(&ctx.step.step_id);
        let gate = DebugGateContext {
            workflow_id: ctx.workflow_id,
            step_id: &ctx.step.step_id,
            vars: ctx.vars,
            response: ctx.response,
            request: None,
            current_outputs: &current_outputs,
            depth: ctx.depth,
        };

        for (action_index, action) in actions.iter().enumerate().skip(start_index) {
            if ctx.cancel.is_cancelled() {
                return Err(control::cancellation_error(ctx.is_timeout));
            }
            self.debug_gate_action(&gate, ctx.branch, action_index, action)
                .await?;
            if ctx.cancel.is_cancelled() {
                return Err(control::cancellation_error(ctx.is_timeout));
            }
            if action.criteria.is_empty() {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }

            let mut all_match = true;
            for (criterion_index, criterion) in action.criteria.iter().enumerate() {
                if ctx.cancel.is_cancelled() {
                    return Err(control::cancellation_error(ctx.is_timeout));
                }
                let evaluation = evaluate_criterion_detailed(
                    criterion,
                    &eval,
                    ctx.response,
                    &self.inner.regex_cache,
                );
                self.debug_gate_action_criterion(
                    &gate,
                    ctx.branch,
                    action_index,
                    criterion_index,
                    &evaluation,
                )
                .await?;
                if ctx.cancel.is_cancelled() {
                    return Err(control::cancellation_error(ctx.is_timeout));
                }
                if !evaluation.matched {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }
        }
        Ok(None)
    }

    async fn execute_action(
        &self,
        ctx: ExecuteActionContext<'_>,
        action: &OnAction,
        debug_ctx: Option<SelectedActionDebugContext<'_>>,
    ) -> ActionExecution {
        if ctx.cancel.is_cancelled() {
            return RoutedDecision::error(control::cancellation_error(ctx.is_timeout)).into();
        }
        match action.action_type() {
            ActionType::End => {
                if ctx.is_failure_path {
                    // Preserve the original error kind (e.g. HttpRequest, ExecutionTimeout)
                    // instead of replacing it with a generic SuccessCriteriaFailed.
                    let err_kind = ctx
                        .original_err_kind
                        .unwrap_or(RuntimeErrorKind::SuccessCriteriaFailed);
                    RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            err_kind,
                            format!(
                                "step {}: workflow ended by onFailure action",
                                ctx.workflow.steps[ctx.current_idx].step_id
                            ),
                        )),
                        trace: TraceDecision {
                            action_type: action.action_type().to_string(),
                            ..TraceDecision::with_path(TraceDecisionPath::Done)
                        },
                    }
                    .into()
                } else {
                    RoutedDecision {
                        flow: FlowDecision::Done,
                        trace: TraceDecision {
                            action_type: action.action_type().to_string(),
                            ..TraceDecision::with_path(TraceDecisionPath::Done)
                        },
                    }
                    .into()
                }
            }
            ActionType::Goto => {
                // Resolve expressions in goto targets (e.g. "step_{$inputs.target}").
                let eval = ExpressionEvaluator::new(self.make_eval_context(ctx.vars, ctx.response));
                if !action.step_id.is_empty() {
                    let resolved_step_id = eval.interpolate_string(&action.step_id);
                    if let Some(idx) = self.find_step_index(ctx.workflow, &resolved_step_id) {
                        return RoutedDecision {
                            flow: FlowDecision::Next(idx),
                            trace: TraceDecision {
                                action_type: action.action_type().to_string(),
                                target_step_id: resolved_step_id,
                                ..TraceDecision::with_path(TraceDecisionPath::GotoStep)
                            },
                        }
                        .into();
                    }
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::GotoTargetNotFound,
                            format!("goto: step \"{resolved_step_id}\" not found"),
                        )),
                        trace: TraceDecision {
                            action_type: action.action_type().to_string(),
                            target_step_id: resolved_step_id,
                            ..TraceDecision::with_path(TraceDecisionPath::Error)
                        },
                    }
                    .into();
                }
                if !action.workflow_id.is_empty() {
                    let resolved_workflow_id = eval.interpolate_string(&action.workflow_id);
                    let inputs = map_action_parameters(action, &eval);
                    return RoutedDecision {
                        flow: FlowDecision::GotoWorkflow {
                            workflow_id: resolved_workflow_id.clone(),
                            inputs,
                        },
                        trace: TraceDecision {
                            action_type: action.action_type().to_string(),
                            target_workflow_id: resolved_workflow_id,
                            ..TraceDecision::with_path(TraceDecisionPath::GotoWorkflow)
                        },
                    }
                    .into();
                }
                RoutedDecision {
                    flow: FlowDecision::Error(RuntimeError::new(
                        RuntimeErrorKind::GotoTargetMissing,
                        "goto: no stepId or workflowId specified",
                    )),
                    trace: TraceDecision {
                        action_type: action.action_type().to_string(),
                        ..TraceDecision::with_path(TraceDecisionPath::Error)
                    },
                }
                .into()
            }
            ActionType::Retry => {
                // Failure Action Object: "If a stepId or workflowId are
                // specified, then the reference is executed and the context is
                // returned, after which the current step is retried." The ids
                // resolve here (mirroring the Goto arm); the call itself runs
                // at the FlowDecision::Retry consumption sites, where `vars`
                // is mutable, after the retry delay and immediately before the
                // retried attempt.
                let eval = ExpressionEvaluator::new(self.make_eval_context(ctx.vars, ctx.response));
                let reference = if !action.step_id.is_empty() {
                    Some(RetryReference::Step {
                        step_id: eval.interpolate_string(&action.step_id),
                    })
                } else if !action.workflow_id.is_empty() {
                    Some(RetryReference::Workflow {
                        workflow_id: eval.interpolate_string(&action.workflow_id),
                        inputs: map_action_parameters(action, &eval),
                    })
                } else {
                    None
                };
                let (trace_target_step, trace_target_workflow) = match &reference {
                    Some(RetryReference::Step { step_id }) => (step_id.clone(), String::new()),
                    Some(RetryReference::Workflow { workflow_id, .. }) => {
                        (String::new(), workflow_id.clone())
                    }
                    None => (String::new(), String::new()),
                };
                let limit = effective_retry_limit(action.retry_limit);
                let current = ctx.retry_count.get(&ctx.retry_site).copied().unwrap_or(0);
                let will_execute_retry = current < limit;
                if let Err(err) = validate_retry_after_value(action.retry_after) {
                    return RoutedDecision::error(err).into();
                }
                if current >= limit {
                    if let Some(debug) = debug_ctx {
                        if let Err(err) = self
                            .debug_gate_retry_selected(
                                debug,
                                action,
                                current,
                                limit,
                                will_execute_retry,
                            )
                            .await
                        {
                            return RoutedDecision::error(err).into();
                        }
                        if ctx.cancel.is_cancelled() {
                            return RoutedDecision::error(control::cancellation_error(
                                ctx.is_timeout,
                            ))
                            .into();
                        }
                    }
                    return ActionExecution::RetryExhausted(ExhaustedRetry {
                        effective_limit: limit,
                        retry_after: action.retry_after,
                        configured_limit: action.retry_limit,
                    });
                }

                // A finite configured decimal is converted only when it is the
                // fallback. A valid integer Retry-After header takes precedence.
                let effective_delay = match compute_retry_after_delay(
                    action,
                    ctx.response.map(|response| &response.headers),
                ) {
                    Ok(delay) => delay,
                    Err(err) => return RoutedDecision::error(err).into(),
                };

                if let Some(debug) = debug_ctx {
                    if let Err(err) = self
                        .debug_gate_retry_selected(
                            debug,
                            action,
                            current,
                            limit,
                            will_execute_retry,
                        )
                        .await
                    {
                        return RoutedDecision::error(err).into();
                    }
                    if ctx.cancel.is_cancelled() {
                        return RoutedDecision::error(control::cancellation_error(ctx.is_timeout))
                            .into();
                    }
                }
                if !effective_delay.is_zero() {
                    if ctx.cancel.is_cancelled() {
                        return RoutedDecision::error(control::cancellation_error(ctx.is_timeout))
                            .into();
                    }
                    if let Some(debug) = debug_ctx {
                        if let Err(err) = self
                            .debug_gate_retry_delay(
                                debug,
                                action,
                                current,
                                limit,
                                effective_delay.as_secs_f64(),
                            )
                            .await
                        {
                            return RoutedDecision::error(err).into();
                        }
                        if ctx.cancel.is_cancelled() {
                            return RoutedDecision::error(control::cancellation_error(
                                ctx.is_timeout,
                            ))
                            .into();
                        }
                    }
                    if let Err(err) =
                        sleep_with_cancel(effective_delay, ctx.cancel, ctx.is_timeout).await
                    {
                        return RoutedDecision {
                            flow: FlowDecision::Error(err),
                            trace: TraceDecision {
                                action_type: action.action_type().to_string(),
                                retry_after_seconds: Some(action.retry_after),
                                retry_limit: action.retry_limit,
                                ..TraceDecision::with_path(TraceDecisionPath::Error)
                            },
                        }
                        .into();
                    }
                }
                if ctx.cancel.is_cancelled() {
                    return RoutedDecision::error(control::cancellation_error(ctx.is_timeout))
                        .into();
                }
                RoutedDecision {
                    flow: FlowDecision::Retry {
                        step_idx: ctx.current_idx,
                        retry_site: ctx.retry_site,
                        retry_limit: limit,
                        delay_seconds: effective_delay.as_secs_f64(),
                        reference,
                    },
                    trace: TraceDecision {
                        action_type: action.action_type().to_string(),
                        target_step_id: trace_target_step,
                        target_workflow_id: trace_target_workflow,
                        retry_after_seconds: Some(action.retry_after),
                        retry_limit: action.retry_limit,
                        ..TraceDecision::with_path(TraceDecisionPath::Retry)
                    },
                }
                .into()
            }
        }
    }

    pub(super) fn find_step_index(&self, workflow: &Workflow, step_id: &str) -> Option<usize> {
        self.inner
            .index
            .step_indexes
            .get(&workflow.workflow_id)
            .and_then(|index| index.get(step_id).copied())
    }
}

#[derive(Debug)]
pub(super) enum FlowDecision {
    Next(usize),
    Retry {
        step_idx: usize,
        /// Internal `(step index, effective failure-action index)` site.
        retry_site: RetrySite,
        /// Configured or defaulted retry budget. This is not part of the
        /// public trace schema, which preserves the configured Option value.
        retry_limit: u64,
        /// Effective delay after a valid HTTP Retry-After override, if any.
        delay_seconds: f64,
        /// Recovery reference from the retry action's `stepId`/`workflowId`.
        /// Executed call-and-return at the consumption site before the step
        /// at `step_idx` is retried; `None` for a plain retry.
        reference: Option<RetryReference>,
    },
    Done,
    GotoWorkflow {
        workflow_id: String,
        /// Callee inputs mapped from the action's `parameters`. `None` means
        /// the action declared no parameters, in which case the caller's own
        /// inputs transfer unchanged (the pre-1.1 behavior).
        inputs: Option<BTreeMap<String, Value>>,
    },
    Error(RuntimeError),
}

/// Resolved `stepId`/`workflowId` recovery reference carried by a retry
/// decision. Failure Action Object: "If a stepId or workflowId are specified,
/// then the reference is executed and the context is returned, after which
/// the current step is retried."
#[derive(Debug)]
pub(super) enum RetryReference {
    Step {
        step_id: String,
    },
    Workflow {
        workflow_id: String,
        /// Callee inputs mapped from the action's `parameters`; `None` means
        /// no parameters were declared and the caller's inputs forward
        /// unchanged (same contract as `FlowDecision::GotoWorkflow`).
        inputs: Option<BTreeMap<String, Value>>,
    },
}

/// Maps a success/failure action's `parameters` to callee workflow inputs.
///
/// Success/Failure Action Object (1.1.0): parameters "MUST be passed to a
/// workflow as referenced by workflowId" — resolved through the shared
/// value/Selector resolver, they become the callee's input map. An action
/// without parameters returns `None`, which keeps forwarding the caller's
/// inputs.
fn map_action_parameters(
    action: &OnAction,
    eval: &ExpressionEvaluator,
) -> Option<BTreeMap<String, Value>> {
    if action.parameters.is_empty() {
        return None;
    }
    let mut mapped = BTreeMap::new();
    for param in &action.parameters {
        let (value, param_warnings) = resolve_value_source(&param.value, eval);
        for warning in param_warnings {
            eprintln!("warning: action parameter {:?}: {warning}", param.name);
        }
        mapped.insert(param.name.clone(), value);
    }
    Some(mapped)
}

#[derive(Debug)]
pub(super) struct RoutedDecision {
    pub flow: FlowDecision,
    pub trace: TraceDecision,
}

impl RoutedDecision {
    pub fn error(err: RuntimeError) -> Self {
        Self {
            flow: FlowDecision::Error(err),
            trace: TraceDecision::with_path(TraceDecisionPath::Error),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MatchedActionRef<'a> {
    index: usize,
    action: &'a OnAction,
}

#[derive(Debug, Clone, Copy)]
struct ExhaustedRetry {
    effective_limit: u64,
    retry_after: f64,
    configured_limit: Option<u64>,
}

#[derive(Debug)]
enum ActionExecution {
    Routed(RoutedDecision),
    RetryExhausted(ExhaustedRetry),
}

impl From<RoutedDecision> for ActionExecution {
    fn from(decision: RoutedDecision) -> Self {
        Self::Routed(decision)
    }
}

/// Internal identity for retry budgets. A retry is scoped to its step and its
/// index in that step's effective failure-action list.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct RetrySite {
    step_index: usize,
    action_index: usize,
}

impl RetrySite {
    fn new(step_index: usize, action_index: usize) -> Self {
        Self {
            step_index,
            action_index,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SelectedActionDebugContext<'a> {
    pub workflow_id: &'a str,
    pub step: &'a Step,
    pub vars: &'a VarStore,
    pub response: Option<&'a Response>,
    pub depth: usize,
    pub branch: ActionBranch,
    pub action_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct ActionSelectionContext<'a> {
    workflow_id: &'a str,
    step: &'a Step,
    branch: ActionBranch,
    vars: &'a VarStore,
    response: Option<&'a Response>,
    depth: usize,
    cancel: &'a CancellationToken,
    is_timeout: &'a AtomicBool,
}

#[derive(Debug)]
pub(super) struct StepDecisionContext<'a> {
    pub workflow_id: &'a str,
    pub workflow: &'a Workflow,
    pub step_idx: usize,
    pub result: &'a StepResult,
    pub vars: &'a VarStore,
    pub depth: usize,
    pub retry_count: &'a BTreeMap<RetrySite, u64>,
    pub cancel: &'a CancellationToken,
    pub is_timeout: &'a AtomicBool,
}

#[derive(Debug)]
struct ExecuteActionContext<'a> {
    workflow: &'a Workflow,
    current_idx: usize,
    is_failure_path: bool,
    retry_count: &'a BTreeMap<RetrySite, u64>,
    retry_site: RetrySite,
    cancel: &'a CancellationToken,
    is_timeout: &'a AtomicBool,
    response: Option<&'a Response>,
    vars: &'a VarStore,
    /// Original error kind from a runtime error (e.g. HttpRequest, ExecutionTimeout).
    /// Used by the `End` action on the failure path to preserve the root cause
    /// instead of replacing it with a generic `SuccessCriteriaFailed`.
    original_err_kind: Option<RuntimeErrorKind>,
}

/// The action list a step's result is routed through: the step's own list
/// when it declares one, otherwise the workflow's. Parallel levels read the
/// same list to order a step after the outputs its criteria consult.
pub(super) fn applicable_actions<'a>(
    step_actions: &'a [OnAction],
    workflow_actions: &'a [OnAction],
) -> &'a [OnAction] {
    if step_actions.is_empty() {
        workflow_actions
    } else {
        step_actions
    }
}

/// Returns the effective Arazzo retry limit for all runtime consumers.
///
/// Failure Action Object §5.8.8.1: an omitted `retryLimit` means one retry;
/// an explicit value is retained exactly.
pub(super) fn effective_retry_limit(configured_limit: Option<u64>) -> u64 {
    configured_limit.unwrap_or(DEFAULT_RETRY_LIMIT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionBranch {
    Success,
    Failure,
}

impl ActionBranch {
    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "onSuccess",
            Self::Failure => "onFailure",
        }
    }

    pub fn action_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessAction {
                index: action_index,
            },
            Self::Failure => StepCheckpoint::OnFailureAction {
                index: action_index,
            },
        }
    }

    pub fn criterion_checkpoint(
        self,
        action_index: usize,
        criterion_index: usize,
    ) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessCriterion {
                action_index,
                criterion_index,
            },
            Self::Failure => StepCheckpoint::OnFailureCriterion {
                action_index,
                criterion_index,
            },
        }
    }

    pub fn retry_selected_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessRetrySelected { action_index },
            Self::Failure => StepCheckpoint::OnFailureRetrySelected { action_index },
        }
    }

    pub fn retry_delay_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessRetryDelay { action_index },
            Self::Failure => StepCheckpoint::OnFailureRetryDelay { action_index },
        }
    }
}

/// Compute the effective retry delay per Arazzo 1.1.0 §5.8.8.
///
/// When the response carries a `Retry-After` header with a valid integer-seconds
/// value, that value takes precedence over the configured `retryAfter` on the action.
/// Falls back to the configured value when the header is absent or malformed.
fn compute_retry_after_delay(
    action: &OnAction,
    headers: Option<&BTreeMap<String, String>>,
) -> Result<Duration, RuntimeError> {
    validate_retry_after_value(action.retry_after)?;
    let Some(hdrs) = headers else {
        return configured_retry_after_delay(action.retry_after);
    };
    // Case-insensitive lookup for the Retry-After header.
    let raw = hdrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if raw.is_empty() {
        return configured_retry_after_delay(action.retry_after);
    }
    // Integer form: number of seconds to wait.
    if let Ok(secs) = raw.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    // HTTP-date form is uncommon for rate-limited APIs; fall back to configured.
    configured_retry_after_delay(action.retry_after)
}

fn validate_retry_after_value(retry_after: f64) -> Result<(), RuntimeError> {
    if retry_after.is_finite() && retry_after >= 0.0 {
        return Ok(());
    }
    Err(RuntimeError::new(
        RuntimeErrorKind::InputValidation,
        "retryAfter must be a finite non-negative number",
    ))
}

fn configured_retry_after_delay(retry_after: f64) -> Result<Duration, RuntimeError> {
    Duration::try_from_secs_f64(retry_after).map_err(|_| {
        RuntimeError::new(
            RuntimeErrorKind::InputValidation,
            "retryAfter must be a finite non-negative duration representable by the runtime",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_retry_limit_preserves_u64_max() {
        assert_eq!(effective_retry_limit(Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn retry_after_header_integer_overrides_config() {
        let action = OnAction {
            retry_after: 10.0,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "2".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("integer header delay must convert: {err}"));
        assert_eq!(delay, Duration::from_secs(2));
    }

    #[test]
    fn retry_after_header_respected_when_no_config() {
        let action = OnAction {
            retry_after: 0.0,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("retry-after".to_string(), "3".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("header override must convert: {err}"));
        assert_eq!(delay, Duration::from_secs(3));
    }

    #[test]
    fn retry_after_config_used_when_no_header() {
        let action = OnAction {
            retry_after: 5.0,
            ..OnAction::default()
        };
        let headers = BTreeMap::new();
        let delay = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("configured integer delay must convert: {err}"));
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn retry_after_malformed_header_falls_back() {
        let action = OnAction {
            retry_after: 4.0,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "not-a-number".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("malformed header fallback must convert: {err}"));
        assert_eq!(delay, Duration::from_secs(4));
    }

    #[test]
    fn retry_after_no_headers_at_all() {
        let action = OnAction {
            retry_after: 7.0,
            ..OnAction::default()
        };
        let delay = compute_retry_after_delay(&action, None)
            .unwrap_or_else(|err| panic!("configured integer delay must convert: {err}"));
        assert_eq!(delay, Duration::from_secs(7));
    }

    #[test]
    fn retry_after_fractional_config_and_header_zero_preserve_effective_delay() {
        let action = OnAction {
            retry_after: 0.25,
            ..OnAction::default()
        };
        let fallback = compute_retry_after_delay(&action, None)
            .unwrap_or_else(|err| panic!("fractional configured delay must convert: {err}"));
        assert_eq!(fallback, Duration::from_millis(250));

        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "0".to_string());
        let overridden = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("Retry-After: 0 must override: {err}"));
        assert!(overridden.is_zero());

        headers.insert("Retry-After".to_string(), "invalid".to_string());
        let malformed_fallback = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("malformed header must use fractional fallback: {err}"));
        assert_eq!(malformed_fallback, Duration::from_millis(250));
    }

    #[test]
    fn retry_after_nonfinite_typed_model_fails_even_with_header_override() {
        for retry_after in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let action = OnAction {
                retry_after,
                ..OnAction::default()
            };
            let mut headers = BTreeMap::new();
            headers.insert("Retry-After".to_string(), "0".to_string());
            let err = match compute_retry_after_delay(&action, Some(&headers)) {
                Ok(delay) => panic!(
                    "invalid configured retryAfter must fail before header override, got {delay:?}"
                ),
                Err(err) => err,
            };
            assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
            assert_eq!(err.code(), "RUNTIME_INPUT_VALIDATION");
        }
    }

    #[test]
    fn retry_after_finite_overflow_fails_only_when_used_as_fallback() {
        let action = OnAction {
            retry_after: f64::MAX,
            ..OnAction::default()
        };
        let err = match compute_retry_after_delay(&action, None) {
            Ok(delay) => {
                panic!(
                    "finite retryAfter overflow must fail without a header override, got {delay:?}"
                )
            }
            Err(err) => err,
        };
        assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
        assert_eq!(err.code(), "RUNTIME_INPUT_VALIDATION");

        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "0".to_string());
        let overridden = compute_retry_after_delay(&action, Some(&headers))
            .unwrap_or_else(|err| panic!("Retry-After: 0 must override finite overflow: {err}"));
        assert!(overridden.is_zero());

        headers.insert("Retry-After".to_string(), "malformed".to_string());
        let malformed = match compute_retry_after_delay(&action, Some(&headers)) {
            Ok(delay) => {
                panic!(
                    "malformed header must use the overflowing configured fallback, got {delay:?}"
                )
            }
            Err(err) => err,
        };
        assert_eq!(malformed.kind, RuntimeErrorKind::InputValidation);
    }
}
