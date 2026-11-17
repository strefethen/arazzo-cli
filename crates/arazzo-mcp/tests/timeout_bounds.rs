#![allow(clippy::unwrap_used)]

use std::time::Duration;

use arazzo_mcp::{handlers, state::ServerState, tools, McpLimits};
use serde_json::{json, Value};

#[test]
fn rejects_invalid_values_before_workflow_lookup() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let configured = ServerState::load_configured(
        &[],
        Some(vec![]),
        McpLimits {
            max_http_timeout: Duration::from_secs(5),
            max_execution_timeout: Duration::from_millis(20_900),
            ..McpLimits::default()
        },
    )
    .unwrap();
    let empty = ServerState::empty();
    for (state, field, max) in [
        (&empty, "http_timeout_seconds", 30),
        (&empty, "execution_timeout_seconds", 300),
        (&configured, "http_timeout_seconds", 5),
        (&configured, "execution_timeout_seconds", 20),
    ] {
        for value in [
            Value::Null,
            json!(true),
            json!("canary-secret"),
            json!(0),
            json!(-1),
            json!(1.5),
            json!(1.0),
            json!(max + 1),
            json!(u64::MAX),
            serde_json::from_str("18446744073709551616").unwrap(),
            json!([]),
            json!({}),
        ] {
            let mut args = json!({"workflow_id":"missing"});
            args[field] = value;
            let result = handlers::run_workflow(state, &args, &runtime).unwrap();
            assert_eq!(result["isError"], true);
            let text = result["content"][0]["text"].as_str().unwrap();
            let error: Value = serde_json::from_str(text).unwrap();
            assert_eq!(error["code"], "MCP_INVALID_ARGUMENT");
            assert!(text.len() <= 512);
            assert!(!text.contains("canary-secret"));
        }
        for value in [None, Some(json!(1)), Some(json!(max))] {
            let mut args = json!({"workflow_id":"missing"});
            if let Some(value) = value {
                args[field] = value;
            }
            let result = handlers::run_workflow(state, &args, &runtime).unwrap();
            assert_eq!(result["content"][0]["text"], "workflow not found: missing");
        }
        let definitions = tools::definitions(state);
        let run = definitions
            .iter()
            .find(|d| d["name"] == "run_workflow")
            .unwrap();
        let schema = &run["inputSchema"]["properties"][field];
        assert_eq!(schema["minimum"], 1);
        assert_eq!(schema["maximum"], max);
        assert_eq!(schema["default"], max);
    }
}
