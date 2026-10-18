#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn invalid_spec_reports_startup_error_before_termination() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-startup-failure-{nanos}.yaml"));
    fs::write(&path, "{}\n").unwrap_or_else(|err| panic!("writing invalid spec: {err}"));
    let input = dap_test_support::encode_dap_stream(&[
        json!({ "seq": 1, "type": "request", "command": "initialize", "arguments": {} }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": { "spec": path.to_string_lossy(), "stopOnEntry": true }
        }),
        json!({ "seq": 3, "type": "request", "command": "configurationDone", "arguments": {} }),
        json!({ "seq": 4, "type": "request", "command": "disconnect", "arguments": {} }),
    ]);
    let mut output = Vec::new();
    let result = run_dap_stdio(Cursor::new(input), &mut output);
    fs::remove_file(&path).unwrap_or_else(|err| panic!("removing invalid spec: {err}"));
    assert!(result.is_ok(), "DAP loop should exit cleanly: {result:?}");

    let messages = dap_test_support::decode_dap_stream(&output);
    let error_index = messages
        .iter()
        .position(|message| message["event"] == "output" && message["body"]["category"] == "stderr")
        .unwrap_or_else(|| {
            panic!("startup error must reach the extension output channel: {messages:#?}")
        });
    let failure_index = messages
        .iter()
        .position(|message| {
            message["command"] == "configurationDone" && message["success"] == false
        })
        .unwrap_or_else(|| panic!("configurationDone must fail: {messages:#?}"));
    let terminated_index = messages
        .iter()
        .position(|message| message["event"] == "terminated")
        .unwrap_or_else(|| panic!("failed startup must terminate: {messages:#?}"));
    let diagnostic = messages[failure_index]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("failed response must include its diagnostic: {messages:#?}"));
    assert!(diagnostic.contains("loading arazzo spec for debug"));
    assert!(diagnostic.contains(path.to_string_lossy().as_ref()));
    assert_eq!(
        messages[error_index]["body"]["output"],
        format!("Arazzo debug: {diagnostic}\n")
    );
    assert!(error_index < failure_index && failure_index < terminated_index);
    assert!(messages.iter().all(|message| message["event"] != "stopped"));
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["event"] == "terminated")
            .count(),
        1
    );
}
