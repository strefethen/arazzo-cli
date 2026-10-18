#![forbid(unsafe_code)]

mod dap_test_support;

use std::io::Cursor;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_evaluate_before_launch_reports_error() {
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
            "command": "evaluate",
            "arguments": {
                "expression": "$inputs.code"
            }
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "disconnect",
            "arguments": {}
        }),
    ]);
    let reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(reader, &mut output);
    assert!(run.is_ok(), "running DAP loop");

    let messages = dap_test_support::decode_dap_stream(&output);
    assert_eq!(messages.len(), 5);

    let evaluate = &messages[2];
    assert_eq!(
        evaluate.get("type").and_then(|v| v.as_str()),
        Some("response")
    );
    assert_eq!(
        evaluate.get("command").and_then(|v| v.as_str()),
        Some("evaluate")
    );
    assert_eq!(evaluate["success"], false);
    assert_eq!(
        evaluate["message"],
        "cannot evaluate expression: debug runtime has not started"
    );
    assert_eq!(
        messages[4].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );
}
