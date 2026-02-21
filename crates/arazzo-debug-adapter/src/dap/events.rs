use serde_json::{json, Value};

pub fn initialized_event(seq: u64) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "event": "initialized",
        "body": {}
    })
}

pub fn stopped_event(seq: u64, thread_id: u64, reason: &str) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "event": "stopped",
        "body": {
            "reason": reason,
            "threadId": thread_id,
            "allThreadsStopped": true
        }
    })
}

pub fn terminated_event(seq: u64) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "event": "terminated",
        "body": {}
    })
}
