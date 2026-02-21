#![forbid(unsafe_code)]

mod dap_test_support;

use std::io::Cursor;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_initialize_and_disconnect_round_trip() {
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
            "command": "disconnect",
            "arguments": {}
        }),
    ]);
    let mut reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(&mut reader, &mut output);
    assert!(run.is_ok(), "running DAP loop");

    let messages = dap_test_support::decode_dap_stream(&output);
    assert_eq!(messages.len(), 4);

    assert_eq!(
        messages[0].get("type").and_then(|v| v.as_str()),
        Some("response")
    );
    assert_eq!(
        messages[0].get("command").and_then(|v| v.as_str()),
        Some("initialize")
    );
    assert_eq!(
        messages[0].get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        messages[0]
            .pointer("/body/supportsConfigurationDoneRequest")
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    assert_eq!(
        messages[1].get("type").and_then(|v| v.as_str()),
        Some("event")
    );
    assert_eq!(
        messages[1].get("event").and_then(|v| v.as_str()),
        Some("initialized")
    );

    assert_eq!(
        messages[2].get("type").and_then(|v| v.as_str()),
        Some("response")
    );
    assert_eq!(
        messages[2].get("command").and_then(|v| v.as_str()),
        Some("disconnect")
    );
    assert_eq!(
        messages[2].get("success").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        messages[3].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );
}
