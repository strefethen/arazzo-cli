//! Attempt-owned context shared with post-step action evaluation.

use std::collections::BTreeMap;

use arazzo_expr::EvalContext;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct MessageSnapshot {
    pub(super) headers: BTreeMap<String, String>,
    /// `None` means no payload was supplied; `Some(Value::Null)` is explicit null.
    pub(super) payload: Option<Value>,
}

pub(super) fn with_message(
    mut context: EvalContext,
    message: Option<&MessageSnapshot>,
) -> EvalContext {
    if let Some(message) = message {
        context.message_headers.clone_from(&message.headers);
        context.message_payload.clone_from(&message.payload);
    }
    context
}
