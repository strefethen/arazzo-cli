use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Value produced by evaluating one watch expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WatchEvaluation {
    pub expression: String,
    pub value: Value,
}
