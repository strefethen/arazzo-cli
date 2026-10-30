#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const AUDIO_SOURCE: &str = "$response.body#/audio_url";
const AUDIO_MARKER: &str = "<unresolved:$response.body#/audio_url>";
const AUDIO_PRODUCER: &str = "dry-run unresolved output producer/fetch.outputs.audio: $response.body#/audio_url requires a response; transport was skipped";
const AUDIO_CONSUMER: &str = "dry-run unresolved consumer/play/requestBody.payload/streamUrl: producer/fetch.outputs.audio depends on $response.body#/audio_url; transport was skipped";
const DIRECT_PRODUCER: &str = "dry-run unresolved output direct/fetch.outputs.audio: $response.body#/audio_url requires a response; transport was skipped";
const DIRECT_CONSUMER: &str = "dry-run unresolved direct/play/parameters[0](query:streamUrl): direct/fetch.outputs.audio depends on $response.body#/audio_url; transport was skipped";
const PARENT_PRODUCER: &str = "dry-run unresolved output child-create/create-item.outputs.created_url: $response.body#/url requires a response; transport was skipped";
const PARENT_CONSUMER: &str = "dry-run unresolved parent-flow/verify-child-output/parameters[0](query:child_url): child-create/create-item.outputs.created_url depends on $response.body#/url; transport was skipped";
const PARENT_VERIFY: &str = "dry-run unresolved output parent-flow/verify-child-output.outputs.child_url: $response.body#/args/child_url requires a response; transport was skipped";

struct Fixture {
    directory: PathBuf,
    server: tiny_http::Server,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "arazzo-cli-dry-run-composition-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory)
            .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
        let server = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("bind fixture server: {error}"));
        let base = format!("http://{}", server.server_addr());
        let source = json!({
            "openapi": "3.0.3",
            "info": {"title": "Dry-run composition", "version": "1"},
            "servers": [{"url": base}],
            "paths": {
                "/produce": {"get": {"operationId": "produce", "responses": {"200": {"description": "OK"}}}},
                "/consume": {"post": {"operationId": "consume", "responses": {"200": {"description": "OK"}}}},
                "/get": {"get": {"responses": {"200": {"description": "OK"}}}},
                "/post": {"post": {"responses": {"200": {"description": "OK"}}}}
            }
        });
        fs::write(directory.join("source.json"), source.to_string())
            .unwrap_or_else(|error| panic!("write source: {error}"));
        Self {
            directory,
            server,
            base,
        }
    }

    fn write_audition(&self, genuine_missing: bool) {
        let mut payload = json!({"streamUrl": "$inputs.streamUrl"});
        if genuine_missing {
            payload["missing"] = json!("$steps.absent.outputs.value");
        }
        self.write(json!({
            "arazzo": "1.1.0",
            "info": {"title": "Dry-run composition", "version": "1"},
            "sourceDescriptions": [{"name": "api", "url": "source.json", "type": "openapi"}],
            "workflows": [
                {"workflowId": "audition", "steps": [
                    {"stepId": "produce", "workflowId": "producer"},
                    {"stepId": "consume", "workflowId": "consumer", "parameters": [
                        {"name": "streamUrl", "value": "$steps.produce.outputs.streamUrl"}
                    ]}
                ]},
                {"workflowId": "producer", "steps": [
                    {"stepId": "fetch", "operationId": "produce", "outputs": {"audio": AUDIO_SOURCE}}
                ], "outputs": {"streamUrl": "$steps.fetch.outputs.audio"}},
                {"workflowId": "consumer", "inputs": {
                    "type": "object", "properties": {"streamUrl": {"type": "string"}},
                    "required": ["streamUrl"]
                }, "steps": [
                    {"stepId": "play", "operationId": "consume", "requestBody": {
                        "contentType": "application/json", "payload": payload
                    }}
                ]},
                {"workflowId": "direct", "steps": [
                    {"stepId": "fetch", "operationId": "produce", "outputs": {"audio": AUDIO_SOURCE}},
                    {"stepId": "play", "operationId": "consume", "parameters": [
                        {"name": "streamUrl", "in": "query", "value": "$steps.fetch.outputs.audio"}
                    ]}
                ]}
            ]
        }));
    }

    fn write_parent_flow(&self) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/sub-workflow.arazzo.yaml");
        let document = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read parent-flow fixture: {error}"));
        let mut spec: Value = serde_yaml_ng::from_str(&document)
            .unwrap_or_else(|error| panic!("parse parent-flow fixture: {error}"));
        assert_eq!(
            spec["workflows"][1]["steps"][0]["outputs"]["created_url"],
            "$response.body#/url"
        );
        spec["sourceDescriptions"][0]["url"] = json!("source.json");
        self.write(spec);
    }

    fn write(&self, spec: Value) {
        fs::write(self.directory.join("workflow.json"), spec.to_string())
            .unwrap_or_else(|error| panic!("write workflow: {error}"));
    }

    fn run(&self, workflow: &str, mode: Option<&str>, json: bool, strict: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        command.current_dir(&self.directory);
        if json {
            command.arg("--json");
        }
        command.args([
            "run",
            "workflow.json",
            workflow,
            "--dry-run",
            "--execution-timeout",
            "2s",
        ]);
        if workflow == "parent-flow" {
            command.args(["--input", "item_name=widget"]);
        }
        if let Some(mode) = mode {
            command.args(["--expr-diagnostics", mode]);
        }
        if strict {
            command.arg("--strict-inputs");
        }
        command
            .output()
            .unwrap_or_else(|error| panic!("run CLI composition fixture: {error}"))
    }

    fn assert_zero_calls(&self) {
        assert!(
            self.server
                .try_recv()
                .unwrap_or_else(|error| panic!("check dry-run transport: {error}"))
                .is_none(),
            "dry-run sent a request"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn body(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("one JSON result required: {error}; {output:?}"))
}

fn wrapped(workflow: &str, step: &str, warning: &str) -> String {
    format!("workflow \"{workflow}\" step \"{step}\": {warning}")
}

fn assert_human_request(output: &Output, request: &str, warnings: &[&str]) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let section = stdout
        .split("\n\n")
        .find(|section| section.starts_with(request))
        .unwrap_or_else(|| panic!("missing request {request:?}: {stdout}"));
    let expected = warnings
        .iter()
        .map(|warning| format!("  Warning: {warning}"))
        .collect::<Vec<_>>();
    assert!(section.ends_with(&expected.join("\n")), "{section}");
    for warning in warnings {
        assert_eq!(stdout.matches(warning).count(), 1, "{stdout}");
    }
}

fn assert_audition_plan(fixture: &Fixture, output: &Output, json: bool) {
    assert!(output.status.success(), "{output:?}");
    if json {
        let result = body(output);
        assert_eq!(result["kind"], "dryRun", "{result}");
        let requests = result["requests"]
            .as_array()
            .unwrap_or_else(|| panic!("request array required: {result}"));
        assert_eq!(requests.len(), 2, "{result}");
        assert_eq!(requests[0]["stepId"], "fetch");
        assert_eq!(requests[0]["warnings"], json!([AUDIO_PRODUCER]));
        assert_eq!(requests[1]["stepId"], "play");
        assert_eq!(requests[1]["body"], json!({"streamUrl": AUDIO_MARKER}));
        assert_eq!(requests[1]["warnings"], json!([AUDIO_CONSUMER]));
    } else {
        assert_human_request(
            output,
            &format!("GET {}/produce", fixture.base),
            &[AUDIO_PRODUCER],
        );
        assert_human_request(
            output,
            &format!("POST {}/consume", fixture.base),
            &[AUDIO_CONSUMER],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(AUDIO_MARKER), "{stdout}");
        let producer = stdout.find(AUDIO_PRODUCER).unwrap_or(usize::MAX);
        let consumer = stdout.find("POST ").unwrap_or(0);
        assert!(producer < consumer, "{stdout}");
    }
    fixture.assert_zero_calls();
}

#[test]
fn audition_keeps_required_typed_child_input_and_request_local_warnings() {
    let fixture = Fixture::new();
    fixture.write_audition(false);
    for strict in [false, true] {
        for json in [false, true] {
            let output = fixture.run("audition", None, json, strict);
            assert_audition_plan(&fixture, &output, json);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("warning:"), "{stderr}");
            assert!(!stderr.contains("input"), "{stderr}");
            if json {
                assert!(body(&output).get("warnings").is_none());
            }
        }
    }
}

#[test]
fn direct_step_alias_retains_encoded_marker_and_separate_warnings() {
    let fixture = Fixture::new();
    fixture.write_audition(false);
    for json in [false, true] {
        let output = fixture.run("direct", None, json, false);
        assert!(output.status.success(), "{output:?}");
        if json {
            let result = body(&output);
            assert_eq!(result["kind"], "dryRun");
            assert_eq!(result["requests"].as_array().map(Vec::len), Some(2));
            assert_eq!(result["requests"][0]["warnings"], json!([DIRECT_PRODUCER]));
            assert_eq!(result["requests"][1]["warnings"], json!([DIRECT_CONSUMER]));
            let url = url::Url::parse(
                result["requests"][1]["url"]
                    .as_str()
                    .unwrap_or_else(|| panic!("request URL required: {result}")),
            )
            .unwrap_or_else(|error| panic!("parse request URL: {error}"));
            assert_eq!(
                url.query_pairs().collect::<Vec<_>>(),
                [("streamUrl".into(), AUDIO_MARKER.into())]
            );
            assert!(!url.as_str().contains(AUDIO_MARKER));
        } else {
            assert_human_request(
                &output,
                &format!("GET {}/produce", fixture.base),
                &[DIRECT_PRODUCER],
            );
            assert_human_request(
                &output,
                &format!("POST {}/consume?", fixture.base),
                &[DIRECT_CONSUMER],
            );
        }
        fixture.assert_zero_calls();
    }
}

#[test]
fn audition_warn_and_error_modes_keep_existing_diagnostic_policy() {
    let fixture = Fixture::new();
    fixture.write_audition(false);
    let warnings = [
        wrapped("producer", "fetch", AUDIO_PRODUCER),
        wrapped("consumer", "play", AUDIO_CONSUMER),
    ];
    for mode in ["warn", "error"] {
        for json in [false, true] {
            let output = fixture.run("audition", Some(mode), json, true);
            assert_eq!(output.status.code(), Some(i32::from(mode == "error")));
            if json {
                let result = body(&output);
                assert_eq!(result["warnings"], json!(warnings));
                if mode == "error" {
                    assert_eq!(result["kind"], "error");
                    assert_eq!(result["code"], "RUNTIME_EXPRESSION_DIAGNOSTICS");
                    assert_eq!(
                        result["error"],
                        "expression diagnostics reported 2 warning(s)"
                    );
                    assert!(result.get("requests").is_none());
                } else {
                    assert_audition_plan(&fixture, &output, true);
                }
                assert!(!String::from_utf8_lossy(&output.stderr).contains("warning:"));
            } else if mode == "warn" {
                assert_audition_plan(&fixture, &output, false);
                let stderr = String::from_utf8_lossy(&output.stderr);
                for warning in &warnings {
                    assert_eq!(stderr.matches(&format!("warning: {warning}")).count(), 1);
                }
            } else {
                assert!(output.stdout.is_empty(), "{output:?}");
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("expression diagnostics reported 2 warning(s)"));
            }
            fixture.assert_zero_calls();
        }
    }
}

#[test]
fn genuine_unresolved_value_keeps_admission_error_precedence() {
    let fixture = Fixture::new();
    fixture.write_audition(true);
    for mode in [None, Some("warn"), Some("error")] {
        for json in [false, true] {
            let output = fixture.run("audition", mode, json, true);
            assert!(!output.status.success(), "{output:?}");
            let message = if json {
                let result = body(&output);
                assert_eq!(result["kind"], "error");
                assert_eq!(result["code"], "RUNTIME_UNRESOLVED_REQUEST_VALUE");
                assert!(result.get("requests").is_none());
                result["error"]
                    .as_str()
                    .unwrap_or_else(|| panic!("error message required: {result}"))
                    .to_string()
            } else {
                String::from_utf8_lossy(&output.stderr).into_owned()
            };
            assert!(message.contains("requestBody.payload/missing"), "{message}");
            assert!(message.contains("request not sent"), "{message}");
            assert!(
                !message.contains("expression diagnostics reported"),
                "{message}"
            );
            fixture.assert_zero_calls();
        }
    }
}

#[test]
fn canonical_parent_flow_keeps_child_url_in_default_warn_and_error_modes() {
    let fixture = Fixture::new();
    fixture.write_parent_flow();
    let warnings = [
        wrapped("child-create", "create-item", PARENT_PRODUCER),
        wrapped("parent-flow", "verify-child-output", PARENT_CONSUMER),
        wrapped("parent-flow", "verify-child-output", PARENT_VERIFY),
    ];
    for mode in [None, Some("warn"), Some("error")] {
        for json in [false, true] {
            let output = fixture.run("parent-flow", mode, json, true);
            assert_eq!(output.status.code(), Some(i32::from(mode == Some("error"))));
            if json {
                let result = body(&output);
                if mode == Some("error") {
                    assert_eq!(result["kind"], "error");
                    assert_eq!(result["code"], "RUNTIME_EXPRESSION_DIAGNOSTICS");
                    assert!(result.get("requests").is_none());
                } else {
                    assert_eq!(result["kind"], "dryRun");
                    let requests = &result["requests"];
                    assert_eq!(requests.as_array().map(Vec::len), Some(2));
                    assert_eq!(requests[0]["stepId"], "create-item");
                    assert_eq!(requests[0]["warnings"], json!([PARENT_PRODUCER]));
                    assert_eq!(requests[1]["stepId"], "verify-child-output");
                    assert_eq!(
                        requests[1]["warnings"],
                        json!([PARENT_CONSUMER, PARENT_VERIFY])
                    );
                    let url = url::Url::parse(requests[1]["url"].as_str().unwrap_or(""))
                        .unwrap_or_else(|error| panic!("parse parent request URL: {error}"));
                    assert_eq!(
                        url.query_pairs().collect::<Vec<_>>(),
                        [(
                            "child_url".into(),
                            "<unresolved:$response.body#/url>".into()
                        )]
                    );
                }
                if mode.is_some() {
                    assert_eq!(result["warnings"], json!(warnings));
                } else {
                    assert!(result.get("warnings").is_none());
                }
            } else if mode != Some("error") {
                assert_human_request(
                    &output,
                    &format!("POST {}/post", fixture.base),
                    &[PARENT_PRODUCER],
                );
                assert_human_request(
                    &output,
                    &format!("GET {}/get?", fixture.base),
                    &[PARENT_CONSUMER, PARENT_VERIFY],
                );
            } else {
                assert!(output.stdout.is_empty(), "{output:?}");
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("expression diagnostics reported 3 warning(s)"));
            }
            fixture.assert_zero_calls();
        }
    }
}
