#![forbid(unsafe_code)]

mod dap_test_support;

use std::io::Cursor;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_set_breakpoints_returns_verified_locations() {
    let input = dap_test_support::encode_dap_stream(&[
        json!({
            "seq": 1,
            "type": "request",
            "command": "initialize",
            "arguments": {}
        }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": "/tmp/workflow.arazzo.yaml" },
                "breakpoints": [
                    { "line": 8 },
                    { "line": 12 }
                ]
            }
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "disconnect",
            "arguments": {}
        }),
    ]);
    let mut reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(&mut reader, &mut output);
    assert!(run.is_ok(), "running DAP loop");

    let messages = dap_test_support::decode_dap_stream(&output);
    assert_eq!(messages.len(), 5);

    let set_breakpoints = &messages[2];
    assert_eq!(
        set_breakpoints.get("command").and_then(|v| v.as_str()),
        Some("setBreakpoints")
    );
    assert_eq!(
        set_breakpoints.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    let breakpoints = set_breakpoints
        .pointer("/body/breakpoints")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].get("line").and_then(|v| v.as_u64()), Some(8));
    assert_eq!(
        breakpoints[0].get("verified").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        breakpoints[1].get("line").and_then(|v| v.as_u64()),
        Some(12)
    );
    assert_eq!(
        breakpoints[1].get("verified").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        messages[4].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );
}
