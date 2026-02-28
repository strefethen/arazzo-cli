use arazzo_expr::{EvalContext, ExpressionEvaluator};
use serde::{Deserialize, Serialize};

/// One executable checkpoint within a workflow step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum StepCheckpoint {
    #[default]
    Step,
    SuccessCriterion {
        index: usize,
    },
    OnSuccessAction {
        index: usize,
    },
    OnSuccessCriterion {
        action_index: usize,
        criterion_index: usize,
    },
    OnFailureAction {
        index: usize,
    },
    OnFailureCriterion {
        action_index: usize,
        criterion_index: usize,
    },
    OnSuccessRetrySelected {
        action_index: usize,
    },
    OnSuccessRetryDelay {
        action_index: usize,
    },
    OnFailureRetrySelected {
        action_index: usize,
    },
    OnFailureRetryDelay {
        action_index: usize,
    },
    Output {
        name: String,
    },
}

/// Canonical runtime step breakpoint identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StepBreakpoint {
    pub workflow_id: String,
    pub step_id: String,
    #[serde(default, skip_serializing_if = "is_step_checkpoint")]
    pub checkpoint: StepCheckpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl StepBreakpoint {
    pub fn new(workflow_id: impl Into<String>, step_id: impl Into<String>) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            step_id: step_id.into(),
            checkpoint: StepCheckpoint::Step,
            condition: None,
        }
    }

    pub fn at_success_criterion(mut self, index: usize) -> Self {
        self.checkpoint = StepCheckpoint::SuccessCriterion { index };
        self
    }

    pub fn at_output(mut self, name: impl Into<String>) -> Self {
        self.checkpoint = StepCheckpoint::Output { name: name.into() };
        self
    }

    pub fn at_on_success_action(mut self, index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnSuccessAction { index };
        self
    }

    pub fn at_on_success_criterion(mut self, action_index: usize, criterion_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnSuccessCriterion {
            action_index,
            criterion_index,
        };
        self
    }

    pub fn at_on_failure_action(mut self, index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnFailureAction { index };
        self
    }

    pub fn at_on_failure_criterion(mut self, action_index: usize, criterion_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnFailureCriterion {
            action_index,
            criterion_index,
        };
        self
    }

    pub fn at_on_success_retry_selected(mut self, action_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnSuccessRetrySelected { action_index };
        self
    }

    pub fn at_on_success_retry_delay(mut self, action_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnSuccessRetryDelay { action_index };
        self
    }

    pub fn at_on_failure_retry_selected(mut self, action_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnFailureRetrySelected { action_index };
        self
    }

    pub fn at_on_failure_retry_delay(mut self, action_index: usize) -> Self {
        self.checkpoint = StepCheckpoint::OnFailureRetryDelay { action_index };
        self
    }

    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    fn matches_identity(
        &self,
        workflow_id: &str,
        step_id: &str,
        checkpoint: &StepCheckpoint,
    ) -> bool {
        self.workflow_id == workflow_id && self.step_id == step_id && self.checkpoint == *checkpoint
    }
}

pub(crate) fn first_matching_breakpoint(
    breakpoints: &[StepBreakpoint],
    workflow_id: &str,
    step_id: &str,
    checkpoint: &StepCheckpoint,
    eval_ctx: &EvalContext,
) -> Option<StepBreakpoint> {
    for breakpoint in breakpoints {
        if !breakpoint.matches_identity(workflow_id, step_id, checkpoint) {
            continue;
        }
        if breakpoint_condition_matches(breakpoint, eval_ctx) {
            return Some(breakpoint.clone());
        }
    }
    None
}

fn breakpoint_condition_matches(breakpoint: &StepBreakpoint, eval_ctx: &EvalContext) -> bool {
    let Some(condition) = breakpoint.condition.as_deref() else {
        return true;
    };
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return true;
    }
    ExpressionEvaluator::new(eval_ctx.clone()).evaluate_condition(trimmed)
}

fn is_step_checkpoint(checkpoint: &StepCheckpoint) -> bool {
    matches!(checkpoint, StepCheckpoint::Step)
}
