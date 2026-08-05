#![forbid(unsafe_code)]

mod dap_test_support;

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::json;

/// A workflow that fails at runtime (here: an operationId that no loaded
/// OpenAPI spec defines) must surface the error to the DAP client instead of
/// ending with a bare `terminated`. Regression test for the "hit continue and
/// the session just stops with no output" report on issue #2: the engine
/// monitor used to discard the engine's `Err` and emit only `terminated`.
#[test]
fn engine_failure_emits_error_output_exited_and_terminated() {
    let spec_path = write_temp_spec();

    let input = dap_test_support::encode_dap_stream(&[
        json!({ "seq": 1, "type": "request", "command": "initialize", "arguments": {} }),
        json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "spec": spec_path.to_string_lossy(),
                "workflowId": "failing",
                "stopOnEntry": true
            }
        }),
        json!({ "seq": 3, "type": "request", "command": "configurationDone", "arguments": {} }),
        // Resume from the entry pause; the first step fails to resolve its
        // operationId, the workflow errors, and the session must end loudly.
        json!({ "seq": 4, "type": "request", "command": "continue", "arguments": {} }),
        // NO disconnect — failure events must arrive on their own.
    ]);

    let reader = Cursor::new(input);
    let mut output = Vec::<u8>::new();

    let run = run_dap_stdio(reader, &mut output);
    assert!(run.is_ok(), "DAP loop should exit cleanly: {run:?}");

    let messages = dap_test_support::decode_dap_stream(&output);
    let event_name =
        |m: &serde_json::Value| m.get("event").and_then(|v| v.as_str()).map(str::to_string);

    let error_output_idx = messages.iter().position(|m| {
        event_name(m).as_deref() == Some("output")
            && m.pointer("/body/category").and_then(|v| v.as_str()) == Some("stderr")
            && m.pointer("/body/output")
                .and_then(|v| v.as_str())
                .is_some_and(|text| text.contains("MissingOperation"))
    });
    let exited_idx = messages
        .iter()
        .position(|m| event_name(m).as_deref() == Some("exited"));
    let terminated_idx = messages
        .iter()
        .position(|m| event_name(m).as_deref() == Some("terminated"));

    let Some(error_output_idx) = error_output_idx else {
        panic!(
            "expected a stderr output event naming the unresolved operationId.\n\
             Messages received ({}):\n{:#?}",
            messages.len(),
            messages
        );
    };
    let Some(exited_idx) = exited_idx else {
        panic!("expected an exited event, got: {messages:#?}");
    };
    let Some(terminated_idx) = terminated_idx else {
        panic!("expected a terminated event, got: {messages:#?}");
    };

    let exit_code = messages[exited_idx]
        .pointer("/body/exitCode")
        .and_then(|v| v.as_i64());
    assert_eq!(exit_code, Some(1), "workflow failure reports exit code 1");

    assert!(
        error_output_idx < exited_idx && exited_idx < terminated_idx,
        "failure events must arrive as output -> exited -> terminated, got \
         output@{error_output_idx}, exited@{exited_idx}, terminated@{terminated_idx}"
    );

    let _ = fs::remove_file(spec_path);
}

fn write_temp_spec() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = std::env::temp_dir().join(format!("arazzo-debug-engine-failure-{nanos}.yaml"));
    // Absolute base URL (legacy semantics): no OpenAPI document is loaded, so
    // the step's operationId cannot resolve and the workflow fails before any
    // HTTP request is attempted. Port 9 (discard) is never contacted.
    let spec = r#"
arazzo: "1.0.0"
info:
  title: Engine Failure Test
  version: "1.0.0"
sourceDescriptions:
  - name: test
    url: http://127.0.0.1:9
    type: openapi
workflows:
  - workflowId: failing
    steps:
      - stepId: fetch-data
        operationId: MissingOperation
        successCriteria:
          - condition: $statusCode == 200
"#;
    fs::write(&path, spec).unwrap_or_else(|err| panic!("writing temp spec: {err}"));
    path
}
