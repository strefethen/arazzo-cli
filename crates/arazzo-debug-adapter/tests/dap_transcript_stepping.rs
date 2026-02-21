#![forbid(unsafe_code)]

mod dap_test_support;

use std::io::Cursor;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_stepping_commands_ack_success() {
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
            "command": "continue",
            "arguments": {}
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "next",
            "arguments": {}
        }),
        json!({
            "seq": 4,
            "type": "request",
            "command": "stepIn",
            "arguments": {}
        }),
        json!({
            "seq": 5,
            "type": "request",
            "command": "stepOut",
            "arguments": {}
        }),
        json!({
            "seq": 6,
            "type": "request",
            "command": "pause",
            "arguments": {}
        }),
        json!({
            "seq": 7,
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
    assert_eq!(messages.len(), 9);

    assert_response_ok(&messages[2], "continue");
    assert_eq!(
        messages[2]
            .pointer("/body/allThreadsContinued")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_response_ok(&messages[3], "next");
    assert_response_ok(&messages[4], "stepIn");
    assert_response_ok(&messages[5], "stepOut");
    assert_response_ok(&messages[6], "pause");
    assert_response_ok(&messages[7], "disconnect");
    assert_eq!(
        messages[8].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );
}

fn assert_response_ok(message: &serde_json::Value, command: &str) {
    assert_eq!(
        message.get("type").and_then(|v| v.as_str()),
        Some("response")
    );
    assert_eq!(
        message.get("command").and_then(|v| v.as_str()),
        Some(command)
    );
    assert_eq!(message.get("success").and_then(|v| v.as_bool()), Some(true));
}
