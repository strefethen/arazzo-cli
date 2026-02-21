use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ResolvedBreakpoint {
    pub line: u32,
    pub verified: bool,
    pub message: Option<String>,
}

pub fn response_with_body(seq: u64, command: &str, body: Value, request_seq: u64) -> Value {
    json!({
        "type": "response",
        "seq": seq,
        "request_seq": request_seq,
        "success": true,
        "command": command,
        "body": body
    })
}

pub fn error_response(seq: u64, command: &str, request_seq: u64, message: String) -> Value {
    json!({
        "type": "response",
        "seq": seq,
        "request_seq": request_seq,
        "success": false,
        "command": command,
        "message": message,
        "body": {}
    })
}

pub fn initialize_capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsEvaluateForHovers": true,
        "supportsStepBack": false,
        "supportsSetVariable": false,
        "supportsConditionalBreakpoints": true
    })
}

pub fn set_breakpoints_body(breakpoints: &[ResolvedBreakpoint]) -> Value {
    let mapped = breakpoints
        .iter()
        .map(|bp| {
            let mut mapped = json!({
                "verified": bp.verified,
                "line": bp.line
            });
            if let Some(message) = bp.message.as_ref() {
                mapped["message"] = json!(message);
            }
            mapped
        })
        .collect::<Vec<_>>();
    json!({ "breakpoints": mapped })
}

pub fn continue_body() -> Value {
    json!({
        "allThreadsContinued": true
    })
}

pub fn empty_body() -> Value {
    json!({})
}

pub fn evaluate_body(result: String) -> Value {
    json!({
        "result": result,
        "variablesReference": 0
    })
}

pub fn threads_body(thread_id: u64, thread_name: &str) -> Value {
    json!({
        "threads": [
            {
                "id": thread_id,
                "name": thread_name
            }
        ]
    })
}
