use super::*;

impl Engine {
    pub(super) async fn handle_step_result(&self, ctx: StepDecisionContext<'_>) -> RoutedDecision {
        let step = &ctx.workflow.steps[ctx.step_idx];

        if ctx.result.success {
            let success_actions = if step.on_success.is_empty() {
                &ctx.workflow.success_actions
            } else {
                &step.on_success
            };
            let action = match self
                .find_matching_action_with_debug(
                    ActionSelectionContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        branch: ActionBranch::Success,
                        vars: ctx.vars,
                        response: ctx.result.response.as_deref(),
                        depth: ctx.depth,
                    },
                    success_actions,
                )
                .await
            {
                Ok(action) => action,
                Err(err) => return RoutedDecision::error(err),
            };
            if let Some(action) = action {
                return self
                    .execute_action(
                        ExecuteActionContext {
                            workflow: ctx.workflow,
                            current_idx: ctx.step_idx,
                            is_failure_path: false,
                            retry_count: ctx.retry_count,
                            cancel: ctx.cancel,
                            is_timeout: ctx.is_timeout,
                            response: ctx.result.response.as_deref(),
                            vars: ctx.vars,
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
            }
            return RoutedDecision {
                flow: FlowDecision::Next(ctx.step_idx + 1),
                trace: TraceDecision::with_path(TraceDecisionPath::Next),
            };
        }

        let failure_actions = if step.on_failure.is_empty() {
            &ctx.workflow.failure_actions
        } else {
            &step.on_failure
        };
        let action = match self
            .find_matching_action_with_debug(
                ActionSelectionContext {
                    workflow_id: ctx.workflow_id,
                    step,
                    branch: ActionBranch::Failure,
                    vars: ctx.vars,
                    response: ctx.result.response.as_deref(),
                    depth: ctx.depth,
                },
                failure_actions,
            )
            .await
        {
            Ok(action) => action,
            Err(err) => return RoutedDecision::error(err),
        };
        if let Some(action) = action {
            return self
                .execute_action(
                    ExecuteActionContext {
                        workflow: ctx.workflow,
                        current_idx: ctx.step_idx,
                        is_failure_path: true,
                        retry_count: ctx.retry_count,
                        cancel: ctx.cancel,
                        is_timeout: ctx.is_timeout,
                        response: ctx.result.response.as_deref(),
                        vars: ctx.vars,
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
        }

        RoutedDecision::error(step_result_error(&step.step_id, ctx.result))
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

        for (action_index, action) in actions.iter().enumerate() {
            self.debug_gate_action(&gate, ctx.branch, action_index, action)
                .await?;
            if action.criteria.is_empty() {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }

            let mut all_match = true;
            for (criterion_index, criterion) in action.criteria.iter().enumerate() {
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
    ) -> RoutedDecision {
        match action.type_ {
            ActionType::End => {
                if ctx.is_failure_path {
                    RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::SuccessCriteriaFailed,
                            format!(
                                "step {}: workflow ended by onFailure action",
                                ctx.workflow.steps[ctx.current_idx].step_id
                            ),
                        )),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            ..TraceDecision::with_path(TraceDecisionPath::Done)
                        },
                    }
                } else {
                    RoutedDecision {
                        flow: FlowDecision::Done,
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            ..TraceDecision::with_path(TraceDecisionPath::Done)
                        },
                    }
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
                                action_type: action.type_.to_string(),
                                target_step_id: resolved_step_id,
                                ..TraceDecision::with_path(TraceDecisionPath::GotoStep)
                            },
                        };
                    }
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::GotoTargetNotFound,
                            format!("goto: step \"{resolved_step_id}\" not found"),
                        )),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            target_step_id: resolved_step_id,
                            ..TraceDecision::with_path(TraceDecisionPath::Error)
                        },
                    };
                }
                if !action.workflow_id.is_empty() {
                    let resolved_workflow_id = eval.interpolate_string(&action.workflow_id);
                    return RoutedDecision {
                        flow: FlowDecision::GotoWorkflow(resolved_workflow_id.clone()),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            target_workflow_id: resolved_workflow_id,
                            ..TraceDecision::with_path(TraceDecisionPath::GotoWorkflow)
                        },
                    };
                }
                RoutedDecision {
                    flow: FlowDecision::Error(RuntimeError::new(
                        RuntimeErrorKind::GotoTargetMissing,
                        "goto: no stepId or workflowId specified",
                    )),
                    trace: TraceDecision {
                        action_type: action.type_.to_string(),
                        ..TraceDecision::with_path(TraceDecisionPath::Error)
                    },
                }
            }
            ActionType::Retry => {
                let limit = action
                    .retry_limit
                    .map(|v| usize::try_from(v).unwrap_or(MAX_RETRIES_PER_STEP))
                    .unwrap_or(MAX_RETRIES_PER_STEP);
                let current = ctx.retry_count.get(&ctx.current_idx).copied().unwrap_or(0);
                let will_execute_retry = current < limit;
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
                        return RoutedDecision::error(err);
                    }
                }
                if current >= limit {
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::RetryLimitExceeded,
                            format!(
                                "step {}: max retries ({limit}) exceeded",
                                ctx.workflow.steps[ctx.current_idx].step_id
                            ),
                        )),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            retry_after_seconds: Some(action.retry_after),
                            retry_limit: action.retry_limit,
                            ..TraceDecision::with_path(TraceDecisionPath::Error)
                        },
                    };
                }

                // RetryScheduled observer event emitted by caller (execute_inner) after FlowDecision::Retry
                // Spec §4.6.6: Retry-After response header overrides configured retryAfter.
                let effective_delay =
                    compute_retry_after_delay(action, ctx.response.map(|r| &r.headers));
                if !effective_delay.is_zero() {
                    if let Some(debug) = debug_ctx {
                        if let Err(err) = self
                            .debug_gate_retry_delay(debug, action, current, limit)
                            .await
                        {
                            return RoutedDecision::error(err);
                        }
                    }
                    if let Err(err) =
                        sleep_with_cancel(effective_delay, ctx.cancel, ctx.is_timeout).await
                    {
                        return RoutedDecision {
                            flow: FlowDecision::Error(err),
                            trace: TraceDecision {
                                action_type: action.type_.to_string(),
                                retry_after_seconds: Some(action.retry_after),
                                retry_limit: action.retry_limit,
                                ..TraceDecision::with_path(TraceDecisionPath::Error)
                            },
                        };
                    }
                }
                RoutedDecision {
                    flow: FlowDecision::Retry(ctx.current_idx),
                    trace: TraceDecision {
                        action_type: action.type_.to_string(),
                        retry_after_seconds: Some(action.retry_after),
                        retry_limit: action.retry_limit,
                        ..TraceDecision::with_path(TraceDecisionPath::Retry)
                    },
                }
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
    Retry(usize),
    Done,
    GotoWorkflow(String),
    Error(RuntimeError),
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
}

#[derive(Debug)]
pub(super) struct StepDecisionContext<'a> {
    pub workflow_id: &'a str,
    pub workflow: &'a Workflow,
    pub step_idx: usize,
    pub result: &'a StepResult,
    pub vars: &'a VarStore,
    pub depth: usize,
    pub retry_count: &'a BTreeMap<usize, usize>,
    pub cancel: &'a CancellationToken,
    pub is_timeout: &'a AtomicBool,
}

#[derive(Debug)]
struct ExecuteActionContext<'a> {
    workflow: &'a Workflow,
    current_idx: usize,
    is_failure_path: bool,
    retry_count: &'a BTreeMap<usize, usize>,
    cancel: &'a CancellationToken,
    is_timeout: &'a AtomicBool,
    response: Option<&'a Response>,
    vars: &'a VarStore,
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

/// Compute the effective retry delay per Arazzo 1.0.1 §4.6.6.
///
/// When the response carries a `Retry-After` header with a valid integer-seconds
/// value, that value takes precedence over the configured `retryAfter` on the action.
/// Falls back to the configured value when the header is absent or malformed.
fn compute_retry_after_delay(
    action: &OnAction,
    headers: Option<&BTreeMap<String, String>>,
) -> Duration {
    let configured = Duration::from_secs(action.retry_after);
    let Some(hdrs) = headers else {
        return configured;
    };
    // Case-insensitive lookup for the Retry-After header.
    let raw = hdrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if raw.is_empty() {
        return configured;
    }
    // Integer form: number of seconds to wait.
    if let Ok(secs) = raw.parse::<u64>() {
        return Duration::from_secs(secs);
    }
    // HTTP-date form is uncommon for rate-limited APIs; fall back to configured.
    configured
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_header_integer_overrides_config() {
        let action = OnAction {
            retry_after: 10,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "2".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers));
        assert_eq!(delay, Duration::from_secs(2));
    }

    #[test]
    fn retry_after_header_respected_when_no_config() {
        let action = OnAction {
            retry_after: 0,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("retry-after".to_string(), "3".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers));
        assert_eq!(delay, Duration::from_secs(3));
    }

    #[test]
    fn retry_after_config_used_when_no_header() {
        let action = OnAction {
            retry_after: 5,
            ..OnAction::default()
        };
        let headers = BTreeMap::new();
        let delay = compute_retry_after_delay(&action, Some(&headers));
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn retry_after_malformed_header_falls_back() {
        let action = OnAction {
            retry_after: 4,
            ..OnAction::default()
        };
        let mut headers = BTreeMap::new();
        headers.insert("Retry-After".to_string(), "not-a-number".to_string());
        let delay = compute_retry_after_delay(&action, Some(&headers));
        assert_eq!(delay, Duration::from_secs(4));
    }

    #[test]
    fn retry_after_no_headers_at_all() {
        let action = OnAction {
            retry_after: 7,
            ..OnAction::default()
        };
        let delay = compute_retry_after_delay(&action, None);
        assert_eq!(delay, Duration::from_secs(7));
    }
}
