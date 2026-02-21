use serde::{Deserialize, Serialize};

/// One workflow frame in the paused runtime stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugStackFrame {
    pub depth: usize,
    pub workflow_id: String,
    pub step_id: String,
}
