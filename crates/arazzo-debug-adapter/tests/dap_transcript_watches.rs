#![forbid(unsafe_code)]

mod dap_test_support;

use std::io::Cursor;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_evaluate_watch_expression_returns_body() {
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
    let mut reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(&mut reader, &mut output);
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
    assert_eq!(
        evaluate.get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    let result = evaluate.pointer("/body/result").and_then(|v| v.as_str());
    assert_eq!(result, Some("evaluation not connected yet: $inputs.code"));
    assert_eq!(
        evaluate
            .pointer("/body/variablesReference")
            .and_then(|v| v.as_u64()),
        Some(0)
    );
    assert_eq!(
        messages[4].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );
}
