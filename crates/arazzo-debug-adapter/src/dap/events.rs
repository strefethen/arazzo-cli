use serde_json::{json, Value};

pub fn initialized_event(seq: u64) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "event": "initialized",
        "body": {}
    })
}
