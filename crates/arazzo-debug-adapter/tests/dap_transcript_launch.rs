#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

#[test]
fn dap_launch_lifecycle_populates_debug_views() {
    let spec_path = match write_temp_spec() {
        Ok(path) => path,
        Err(err) => panic!("creating launch transcript temp spec: {err}"),
    };
    let spec_path_str = spec_path.to_string_lossy().into_owned();

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
            "command": "launch",
            "arguments": {
                "spec": spec_path_str,
                "workflowId": "get-hackernews"
            }
        }),
        json!({
            "seq": 3,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": { "path": spec_path.to_string_lossy() },
                "breakpoints": [
                    { "line": 7 },
                    { "line": 9 }
                ]
            }
        }),
        json!({
            "seq": 4,
            "type": "request",
            "command": "setExceptionBreakpoints",
            "arguments": {}
        }),
        json!({
            "seq": 5,
            "type": "request",
            "command": "configurationDone",
            "arguments": {}
        }),
        json!({
            "seq": 6,
            "type": "request",
            "command": "threads",
            "arguments": {}
        }),
        json!({
            "seq": 7,
            "type": "request",
            "command": "stackTrace",
            "arguments": {}
        }),
        json!({
            "seq": 8,
            "type": "request",
            "command": "scopes",
            "arguments": { "frameId": 1 }
        }),
        json!({
            "seq": 9,
            "type": "request",
            "command": "variables",
            "arguments": { "variablesReference": 1 }
        }),
        json!({
            "seq": 10,
            "type": "request",
            "command": "continue",
            "arguments": {}
        }),
        json!({
            "seq": 11,
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
    assert_eq!(messages.len(), 15);

    assert_eq!(
        messages[2].get("command").and_then(|v| v.as_str()),
        Some("launch")
    );
    assert_eq!(
        messages[4].get("command").and_then(|v| v.as_str()),
        Some("setExceptionBreakpoints")
    );
    assert_eq!(
        messages[5].get("command").and_then(|v| v.as_str()),
        Some("configurationDone")
    );
    assert_eq!(
        messages[6].get("event").and_then(|v| v.as_str()),
        Some("stopped")
    );
    assert_eq!(
        messages[7]
            .pointer("/body/threads/0/id")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        messages[8]
            .pointer("/body/stackFrames/0/source/path")
            .and_then(|v| v.as_str()),
        Some(spec_path.to_string_lossy().as_ref())
    );
    assert_eq!(
        messages[8]
            .pointer("/body/stackFrames/0/line")
            .and_then(|v| v.as_u64()),
        Some(13)
    );
    assert_eq!(
        messages[9]
            .pointer("/body/scopes/0/variablesReference")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    let variables = messages[10]
        .pointer("/body/variables")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let workflow = variables
        .iter()
        .find(|entry| entry.get("name").and_then(|v| v.as_str()) == Some("workflowId"));
    assert!(workflow.is_some());
    assert_eq!(
        workflow
            .and_then(|entry| entry.get("value"))
            .and_then(|v| v.as_str()),
        Some("get-hackernews")
    );
    assert_eq!(
        messages[11].get("command").and_then(|v| v.as_str()),
        Some("continue")
    );
    let post_continue_event = messages[12].get("event").and_then(|v| v.as_str());
    if post_continue_event == Some("stopped") {
        let reason = messages[12]
            .pointer("/body/reason")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(reason == "breakpoint" || reason == "step" || reason == "pause");
    } else {
        assert_eq!(post_continue_event, Some("terminated"));
    }
    assert_eq!(
        messages[13].get("command").and_then(|v| v.as_str()),
        Some("disconnect")
    );
    assert_eq!(
        messages[14].get("event").and_then(|v| v.as_str()),
        Some("terminated")
    );

    let _ = fs::remove_file(spec_path);
}

fn write_temp_spec() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-launch-{nanos}.yaml"));
    let spec = r#"
arazzo: "1.0.0"
info:
  title: Demo
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: https://example.com
    type: openapi
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: https://example.com/rss
      - stepId: parse-rss
        operationPath: https://example.com/parse
"#;
    fs::write(&path, spec).map_err(|err| format!("writing temp spec: {err}"))?;
    Ok(path)
}
