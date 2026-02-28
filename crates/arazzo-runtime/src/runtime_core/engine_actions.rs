use super::*;

impl Engine {
    pub(super) fn handle_step_result(&self, ctx: StepDecisionContext<'_>) -> RoutedDecision {
        let step = &ctx.workflow.steps[ctx.step_idx];

        if ctx.result.success {
            let success_actions = if step.on_success.is_empty() {
                &ctx.workflow.success_actions
            } else {
                &step.on_success
            };
            let action = match self.find_matching_action_with_debug(
                ActionSelectionContext {
                    workflow_id: ctx.workflow_id,
                    step,
                    branch: ActionBranch::Success,
                    vars: ctx.vars,
                    response: ctx.result.response.as_ref(),
                    depth: ctx.depth,
                },
                success_actions,
            ) {
                Ok(action) => action,
                Err(err) => return RoutedDecision::error(err),
            };
            if let Some(action) = action {
                return self.execute_action(
                    ExecuteActionContext {
                        workflow: ctx.workflow,
                        current_idx: ctx.step_idx,
                        is_failure_path: false,
                        retry_count: ctx.retry_count,
                        options: ctx.options,
                    },
                    action.action,
                    Some(SelectedActionDebugContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        vars: ctx.vars,
                        response: ctx.result.response.as_ref(),
                        depth: ctx.depth,
                        branch: ActionBranch::Success,
                        action_index: action.index,
                    }),
                );
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
        let action = match self.find_matching_action_with_debug(
            ActionSelectionContext {
                workflow_id: ctx.workflow_id,
                step,
                branch: ActionBranch::Failure,
                vars: ctx.vars,
                response: ctx.result.response.as_ref(),
                depth: ctx.depth,
            },
            failure_actions,
        ) {
            Ok(action) => action,
            Err(err) => return RoutedDecision::error(err),
        };
        if let Some(action) = action {
            return self.execute_action(
                ExecuteActionContext {
                    workflow: ctx.workflow,
                    current_idx: ctx.step_idx,
                    is_failure_path: true,
                    retry_count: ctx.retry_count,
                    options: ctx.options,
                },
                action.action,
                Some(SelectedActionDebugContext {
                    workflow_id: ctx.workflow_id,
                    step,
                    vars: ctx.vars,
                    response: ctx.result.response.as_ref(),
                    depth: ctx.depth,
                    branch: ActionBranch::Failure,
                    action_index: action.index,
                }),
            );
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
                if !evaluate_criterion(criterion, &eval, response) {
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

    fn find_matching_action_with_debug<'a>(
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
            self.debug_gate_action(&gate, ctx.branch, action_index, action)?;
            if action.criteria.is_empty() {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }

            let mut all_match = true;
            for (criterion_index, criterion) in action.criteria.iter().enumerate() {
                let evaluation = evaluate_criterion_detailed(criterion, &eval, ctx.response);
                self.debug_gate_action_criterion(
                    &gate,
                    ctx.branch,
                    action_index,
                    criterion_index,
                    &evaluation,
                )?;
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

    fn execute_action(
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
                if !action.step_id.is_empty() {
                    if let Some(idx) = self.find_step_index(ctx.workflow, &action.step_id) {
                        return RoutedDecision {
                            flow: FlowDecision::Next(idx),
                            trace: TraceDecision {
                                action_type: action.type_.to_string(),
                                target_step_id: action.step_id.clone(),
                                ..TraceDecision::with_path(TraceDecisionPath::GotoStep)
                            },
                        };
                    }
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::GotoTargetNotFound,
                            format!("goto: step \"{}\" not found", action.step_id),
                        )),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            target_step_id: action.step_id.clone(),
                            ..TraceDecision::with_path(TraceDecisionPath::Error)
                        },
                    };
                }
                if !action.workflow_id.is_empty() {
                    return RoutedDecision {
                        flow: FlowDecision::GotoWorkflow(action.workflow_id.clone()),
                        trace: TraceDecision {
                            action_type: action.type_.to_string(),
                            target_workflow_id: action.workflow_id.clone(),
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
                let mut limit = MAX_RETRIES_PER_STEP;
                if action.retry_limit > 0 {
                    limit = usize::try_from(action.retry_limit).unwrap_or(MAX_RETRIES_PER_STEP);
                }
                let current = ctx.retry_count.get(&ctx.current_idx).copied().unwrap_or(0);
                let will_execute_retry = current < limit;
                if let Some(debug) = debug_ctx {
                    if let Err(err) = self.debug_gate_retry_selected(
                        debug,
                        action,
                        current,
                        limit,
                        will_execute_retry,
                    ) {
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
                            retry_limit: Some(action.retry_limit),
                            ..TraceDecision::with_path(TraceDecisionPath::Error)
                        },
                    };
                }
                if action.retry_after > 0 {
                    if let Some(debug) = debug_ctx {
                        if let Err(err) = self.debug_gate_retry_delay(debug, action, current, limit)
                        {
                            return RoutedDecision::error(err);
                        }
                    }
                    if let Err(err) =
                        sleep_with_checks(Duration::from_secs(action.retry_after), ctx.options)
                    {
                        return RoutedDecision {
                            flow: FlowDecision::Error(err),
                            trace: TraceDecision {
                                action_type: action.type_.to_string(),
                                retry_after_seconds: Some(action.retry_after),
                                retry_limit: Some(action.retry_limit),
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
                        retry_limit: Some(action.retry_limit),
                        ..TraceDecision::with_path(TraceDecisionPath::Retry)
                    },
                }
            }
        }
    }

    pub(super) fn find_step_index(&self, workflow: &Workflow, step_id: &str) -> Option<usize> {
        self.index
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

#[derive(Debug, Clone, Copy)]
pub(super) struct StepDecisionContext<'a> {
    pub workflow_id: &'a str,
    pub workflow: &'a Workflow,
    pub step_idx: usize,
    pub result: &'a StepResult,
    pub vars: &'a VarStore,
    pub depth: usize,
    pub retry_count: &'a BTreeMap<usize, usize>,
    pub options: &'a ExecutionOptions,
}

#[derive(Debug, Clone, Copy)]
struct ExecuteActionContext<'a> {
    workflow: &'a Workflow,
    current_idx: usize,
    is_failure_path: bool,
    retry_count: &'a BTreeMap<usize, usize>,
    options: &'a ExecutionOptions,
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
