use std::path::Path;

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

pub fn stack_trace_body(frame_id: u64, frame_name: &str, source_path: &str, line: u32) -> Value {
    let source_name = Path::new(source_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workflow");
    json!({
        "stackFrames": [
            {
                "id": frame_id,
                "name": frame_name,
                "line": line,
                "column": 1,
                "source": {
                    "name": source_name,
                    "path": source_path
                }
            }
        ],
        "totalFrames": 1
    })
}

pub fn scopes_body(locals_ref: u64, watch_ref: u64) -> Value {
    json!({
        "scopes": [
            {
                "name": "Locals",
                "presentationHint": "locals",
                "variablesReference": locals_ref,
                "expensive": false
            },
            {
                "name": "Watch",
                "presentationHint": "registers",
                "variablesReference": watch_ref,
                "expensive": false
            }
        ]
    })
}

pub fn variables_body(entries: &[(&str, String)]) -> Value {
    let variables = entries
        .iter()
        .map(|(name, value)| {
            json!({
                "name": name,
                "value": value,
                "variablesReference": 0
            })
        })
        .collect::<Vec<_>>();
    json!({ "variables": variables })
}
