use serde_json::{json, Value};

use super::requests::DapBreakpoint;

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

pub fn set_breakpoints_body(breakpoints: &[DapBreakpoint]) -> Value {
    let mapped = breakpoints
        .iter()
        .map(|bp| {
            json!({
                "verified": true,
                "line": bp.line
            })
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
