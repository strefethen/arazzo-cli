use arazzo_expr::{EvalContext, ExpressionEvaluator};
use serde::{Deserialize, Serialize};

/// Canonical runtime step breakpoint identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StepBreakpoint {
    pub workflow_id: String,
    pub step_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl StepBreakpoint {
    pub fn new(workflow_id: impl Into<String>, step_id: impl Into<String>) -> Self {
        Self {
            workflow_id: workflow_id.into(),
            step_id: step_id.into(),
            condition: None,
        }
    }

    pub fn with_condition(mut self, condition: impl Into<String>) -> Self {
        self.condition = Some(condition.into());
        self
    }

    fn matches_identity(&self, workflow_id: &str, step_id: &str) -> bool {
        self.workflow_id == workflow_id && self.step_id == step_id
    }
}

pub(crate) fn first_matching_breakpoint(
    breakpoints: &[StepBreakpoint],
    workflow_id: &str,
    step_id: &str,
    eval_ctx: &EvalContext,
) -> Option<StepBreakpoint> {
    for breakpoint in breakpoints {
        if !breakpoint.matches_identity(workflow_id, step_id) {
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
