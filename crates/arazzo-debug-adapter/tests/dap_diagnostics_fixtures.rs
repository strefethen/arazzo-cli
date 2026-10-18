#![forbid(unsafe_code)]

mod dap_test_support;

use arazzo_debug_adapter::run_dap_stdio;
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/debugger-diagnostics")
        .join(name)
}

fn line(name: &str, text: &str) -> usize {
    std::fs::read_to_string(fixture(name))
        .unwrap_or_else(|err| panic!("fixture: {err}"))
        .lines()
        .position(|line| line.contains(text))
        .unwrap_or_else(|| panic!("fixture line missing: {text}"))
        + 1
}

fn request(command: &str, arguments: Value) -> Value {
    json!({"seq": 0, "type": "request", "command": command, "arguments": arguments})
}

fn run(mut requests: Vec<Value>) -> Vec<Value> {
    for (index, request) in requests.iter_mut().enumerate() {
        request["seq"] = json!(index + 1);
    }
    let mut output = Vec::new();
    run_dap_stdio(
        Cursor::new(dap_test_support::encode_dap_stream(&requests)),
        &mut output,
    )
    .unwrap_or_else(|err| panic!("DAP session: {err}"));
    dap_test_support::decode_dap_stream(&output)
}

fn response(messages: &[Value], seq: u64) -> &Value {
    messages
        .iter()
        .find(|message| message["type"] == "response" && message["request_seq"] == seq)
        .unwrap_or_else(|| panic!("missing response {seq}: {messages:#?}"))
}

fn launch(file: &str, workflow: &str, stop: bool, inputs: Value) -> Value {
    request(
        "launch",
        json!({"spec": fixture(file), "workflowId": workflow, "dryRun": true, "stopOnEntry": stop, "inputs": inputs}),
    )
}

#[test]
fn missing_watches_are_errors_and_explicit_null_is_a_value() {
    let messages = run(vec![
        launch(
            "outputs.arazzo.yaml",
            "address-forecast",
            true,
            json!({"alerts":{"features":[]}, "explicitNull":null}),
        ),
        request("configurationDone", json!({})),
        request(
            "evaluate",
            json!({"expression":"$inputs.alerts#/features/0/properties/headline", "frameId":100}),
        ),
        request(
            "evaluate",
            json!({"expression":"missing_alias", "frameId":100}),
        ),
        request(
            "evaluate",
            json!({"expression":"$inputs.explicitNull", "frameId":100}),
        ),
        request(
            "evaluate",
            json!({"expression":"$inputs.alerts ==", "frameId":100}),
        ),
        request("scopes", json!({"frameId":999})),
        request("variables", json!({"variablesReference":999})),
        request(
            "evaluate",
            json!({"expression":"$inputs.alerts#/features/0/properties/severity", "frameId":100}),
        ),
        request("disconnect", json!({})),
    ]);
    for seq in [3, 4, 6, 7, 8, 9] {
        assert_eq!(response(&messages, seq)["success"], false, "{messages:#?}");
        assert!(!response(&messages, seq)["message"]
            .as_str()
            .unwrap_or_else(|| panic!("missing diagnostic string"))
            .is_empty());
    }
    let diagnostic = response(&messages, 3)["message"]
        .as_str()
        .unwrap_or_else(|| panic!("missing diagnostic string"));
    for text in [
        "first_alert_headline",
        "$inputs.alerts#/features/0/properties/headline",
        "address-forecast",
        "check-active-alerts",
    ] {
        assert!(diagnostic.contains(text), "{diagnostic}");
    }
    assert_eq!(response(&messages, 5)["success"], true);
    assert_eq!(response(&messages, 5)["body"]["result"], "null");
}

#[test]
fn step_and_workflow_output_diagnostics_precede_completion() {
    let messages = run(vec![
        launch(
            "outputs.arazzo.yaml",
            "address-forecast",
            false,
            json!({"alerts":{"features":[]}, "explicitNull":null}),
        ),
        request("configurationDone", json!({})),
    ]);
    let diagnostics: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message["body"]["data"]["kind"] == "unresolvedOutput")
        .collect();
    assert_eq!(diagnostics.len(), 3, "{messages:#?}");
    let exited = messages
        .iter()
        .position(|message| message["event"] == "exited")
        .unwrap_or_else(|| panic!("missing exit event: {messages:#?}"));
    assert!(diagnostics.iter().all(|(index, _)| *index < exited));
    assert!(diagnostics
        .iter()
        .any(|(_, message)| message["body"]["output"]
            .as_str()
            .unwrap_or_else(|| panic!("missing diagnostic string"))
            .contains("workflow_headline")));
    assert!(diagnostics
        .iter()
        .all(|(_, message)| !message["body"]["output"]
            .as_str()
            .unwrap_or_else(|| panic!("missing diagnostic string"))
            .contains("explicit_null")));
}

#[test]
fn selected_parent_frame_uses_parent_values_and_preserves_variable_references() {
    let messages = run(vec![
        launch(
            "frames.arazzo.yaml",
            "parent",
            false,
            json!({"label":"parent-value"}),
        ),
        request(
            "setBreakpoints",
            json!({"source":{"path":fixture("frames.arazzo.yaml")}, "breakpoints":[{"line":line("frames.arazzo.yaml", "stepId: inspect-child")}]}),
        ),
        request("configurationDone", json!({})),
        request("stackTrace", json!({"threadId":1})),
        request(
            "evaluate",
            json!({"expression":"$inputs.label", "frameId":100}),
        ),
        request(
            "evaluate",
            json!({"expression":"$inputs.label", "frameId":101}),
        ),
        request("scopes", json!({"frameId":100})),
        request("scopes", json!({"frameId":101})),
        request("variables", json!({"variablesReference":2})),
        request("variables", json!({"variablesReference":5})),
        request("continue", json!({"threadId":1})),
        request("variables", json!({"variablesReference":2})),
        request("disconnect", json!({})),
    ]);
    assert_eq!(
        response(&messages, 4)["body"]["totalFrames"],
        2,
        "{messages:#?}"
    );
    assert_eq!(response(&messages, 5)["body"]["result"], "parent-value");
    assert_eq!(response(&messages, 6)["body"]["result"], "child-value");
    assert_eq!(
        response(&messages, 9)["body"]["variables"][0]["value"],
        "parent-value"
    );
    assert_eq!(
        response(&messages, 10)["body"]["variables"][0]["value"],
        "child-value"
    );
    assert_eq!(response(&messages, 12)["success"], false);
}

#[test]
fn unreadable_or_malformed_sources_and_invalid_conditions_are_not_verified() {
    for file in ["does-not-exist.arazzo.yaml", "malformed.yaml"] {
        let messages = run(vec![
            request(
                "setBreakpoints",
                json!({"source":{"path":fixture(file)},"breakpoints":[{"line":2}]}),
            ),
            request("disconnect", json!({})),
        ]);
        let bp = &response(&messages, 1)["body"]["breakpoints"][0];
        assert_eq!(bp["verified"], false);
        assert!(bp["message"]
            .as_str()
            .unwrap_or_else(|| panic!("missing diagnostic string"))
            .contains(file));
    }
    let messages = run(vec![
        request(
            "setBreakpoints",
            json!({"source":{"path":fixture("outputs.arazzo.yaml")},"breakpoints":[{"line":line("outputs.arazzo.yaml", "stepId: check-active-alerts"),"condition":"$inputs.alerts =="}]}),
        ),
        request("disconnect", json!({})),
    ]);
    let bp = &response(&messages, 1)["body"]["breakpoints"][0];
    assert_eq!(bp["verified"], false);
    assert!(bp["message"]
        .as_str()
        .unwrap_or_else(|| panic!("missing diagnostic string"))
        .contains("$inputs.alerts =="));
}

#[test]
fn breakpoint_evaluation_errors_name_the_condition_and_location() {
    let condition = "($inputs.explicitNull).missing == 1";
    let messages = run(vec![
        launch(
            "outputs.arazzo.yaml",
            "address-forecast",
            false,
            json!({"alerts":{"features":[]}, "explicitNull":null}),
        ),
        request(
            "setBreakpoints",
            json!({"source":{"path":fixture("outputs.arazzo.yaml")}, "breakpoints":[{"line":line("outputs.arazzo.yaml", "stepId: check-active-alerts"), "condition":condition}]}),
        ),
        request("configurationDone", json!({})),
    ]);
    assert_eq!(
        response(&messages, 2)["body"]["breakpoints"][0]["verified"],
        true
    );
    let diagnostic = messages
        .iter()
        .find(|message| message["event"] == "output" && message["body"]["category"] == "stderr")
        .unwrap_or_else(|| panic!("missing breakpoint diagnostic: {messages:#?}"))["body"]
        ["output"]
        .as_str()
        .unwrap_or_else(|| panic!("missing diagnostic string"));
    for text in [condition, "address-forecast", "check-active-alerts"] {
        assert!(diagnostic.contains(text), "{diagnostic}");
    }
    assert!(messages
        .iter()
        .any(|message| message["event"] == "exited" && message["body"]["exitCode"] == 1));
}
