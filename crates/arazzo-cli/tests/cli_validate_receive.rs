#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)] // Test fixtures and assertions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-validate-receive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, spec: &Value, document: &Value) {
        fs::write(self.0.join("flow.json"), spec.to_string()).unwrap();
        fs::write(self.0.join("orders.json"), document.to_string()).unwrap();
    }

    fn run(&self, flags: &[&str]) -> Output {
        // A bare filename exercises the same empty parent path as `run`.
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .current_dir(&self.0)
            .args(["validate", "flow.json"])
            .args(flags)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn spec() -> Value {
    json!({
        "arazzo": "1.1.0", "info": {"title": "Receive", "version": "1"},
        "sourceDescriptions": [{"name": "orders", "url": "./orders.json", "type": "asyncapi"}],
        "workflows": [{"workflowId": "flow", "steps": [{
            "stepId": "receive", "channelPath": "{$sourceDescriptions.orders.url}#/channels/orders",
            "action": "receive"
        }]}]
    })
}

fn document() -> Value {
    json!({
        "asyncapi": "3.0.0", "info": {"title": "Orders", "version": "1"},
        "servers": {"kafka": {"host": "broker.invalid:9092", "protocol": "kafka"}},
        "channels": {"orders": {"address": "orders", "messages": {
            "order": {"contentType": "application/json"}
        }}},
        "operations": {"receiveOrders": {"action": "receive", "channel": {"$ref": "#/channels/orders"}}}
    })
}

fn body(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_valid(fixture: &Fixture, flags: &[&str]) {
    let mut args = vec!["--json"];
    args.extend_from_slice(flags);
    let output = fixture.run(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let body = body(&output);
    assert_eq!(body["valid"], true);
    assert!(body
        .get("errors")
        .is_none_or(|errors| errors.as_array().unwrap().is_empty()));
}

fn assert_refused(fixture: &Fixture, code: &str, needle: &str) {
    let output = fixture.run(&["--json"]);
    assert!(!output.status.success());
    let body = body(&output);
    assert_eq!(body["kind"], "error");
    assert_eq!(body["code"], "VALIDATE_SPEC_VALIDATION");
    assert_eq!(body["valid"], false);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{body}");
    assert_eq!(errors[0]["kind"], "receiveTargetRefused");
    assert_eq!(errors[0]["path"], "workflows[0].steps[0]");
    let message = errors[0]["message"].as_str().unwrap();
    assert!(message.starts_with(&format!("{code}: ")), "{message}");
    assert!(message.contains(needle), "{message}");
    let human = fixture.run(&[]);
    assert!(!human.status.success());
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8_lossy(&human.stderr)
        .contains(&format!("workflows[0].steps[0]: {message}")));
}

#[test]
fn receive_refusals_report_runtime_codes_and_fixed_documents_validate() {
    for case in [
        "send",
        "channel",
        "file",
        "source",
        "content",
        "operation-send",
        "correlation",
    ] {
        let fixture = Fixture::new();
        let mut spec = spec();
        let mut doc = document();
        let step = &mut spec["workflows"][0]["steps"][0];
        let (code, needle) = match case {
            "send" => {
                step["action"] = json!("send");
                ("RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT", "send")
            }
            "channel" => {
                step["channelPath"] = json!("{$sourceDescriptions.orders.url}#/channels/missing");
                ("RUNTIME_MESSAGE_TRANSPORT_CONFIG", "missing")
            }
            "file" => {
                spec["sourceDescriptions"][0]["url"] = json!("./missing.json");
                ("RUNTIME_MESSAGE_TRANSPORT_CONFIG", "missing.json")
            }
            "source" => {
                step["channelPath"] =
                    json!("{$sourceDescriptions.undeclared.url}#/channels/orders");
                ("RUNTIME_MESSAGE_TRANSPORT_CONFIG", "undeclared")
            }
            "content" => {
                doc["channels"]["orders"]["messages"]["order"]["contentType"] = json!("text/plain");
                ("RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT", "text/plain")
            }
            "operation-send" => {
                step.as_object_mut().unwrap().remove("channelPath");
                step["operationId"] = json!("$sourceDescriptions.orders.receiveOrders");
                doc["operations"]["receiveOrders"]["action"] = json!("send");
                ("RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT", "send")
            }
            "correlation" => {
                step["correlationId"] = json!("42");
                ("RUNTIME_MESSAGE_TRANSPORT_CONFIG", "Correlation ID Object")
            }
            _ => unreachable!(),
        };
        fixture.write(&spec, &doc);
        assert_refused(&fixture, code, needle);
        fixture.write(&self::spec(), &document());
        assert_valid(&fixture, &[]);
    }
}

#[test]
fn non_asyncapi_receive_reports_structural_errors_before_runtime_admission() {
    let fixture = Fixture::new();
    let mut spec = spec();
    spec["sourceDescriptions"][0]["type"] = json!("openapi");
    let step = &mut spec["workflows"][0]["steps"][0];
    step.as_object_mut().unwrap().remove("channelPath");
    step["operationId"] = json!("$sourceDescriptions.orders.getEvent");
    step["correlationId"] = json!("fixed-id");
    fixture.write(
        &spec,
        &json!({
            "openapi": "3.0.3", "info": {"title": "Events", "version": "1"},
            "paths": {"/events": {"get": {"operationId": "getEvent", "responses": {
                "200": {"description": "Event"}
            }}}}
        }),
    );

    let output = fixture.run(&["--strict", "--json"]);
    assert!(!output.status.success());
    let body = body(&output);
    assert_eq!(body["kind"], "error");
    assert_eq!(body["code"], "VALIDATE_SPEC_VALIDATION");
    assert_eq!(body["valid"], false);
    let errors = body["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{body}");
    for (error, field) in errors.iter().zip(["action", "correlationId"]) {
        assert_eq!(error["kind"], "invalidAsyncStep");
        assert_eq!(
            error["path"],
            format!("workflow \"flow\" > step \"receive\".{field}")
        );
        assert_eq!(
            error["message"],
            format!(
                "{field} is only applicable to asyncapi steps; source \"orders\" is type openapi"
            )
        );
    }
    assert!(!String::from_utf8_lossy(&output.stdout).contains("RUNTIME_"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("RUNTIME_"));
}

#[test]
fn server_selection_is_shared_repeatable_and_offline() {
    let fixture = Fixture::new();
    let mut spec = spec();
    let mut doc = document();
    doc["servers"]["kafka-secure"] =
        json!({"host": "secure.invalid:9093", "protocol": "kafka-secure"});
    fixture.write(&spec, &doc);
    assert_refused(
        &fixture,
        "RUNTIME_MESSAGE_TRANSPORT_CONFIG",
        "several Kafka servers",
    );
    assert_valid(&fixture, &["--asyncapi-server", "orders=kafka-secure"]);
    // Operation targets use the same source selection as channel targets.
    let step = &mut spec["workflows"][0]["steps"][0];
    step.as_object_mut().unwrap().remove("channelPath");
    step["operationId"] = json!("$sourceDescriptions.orders.receiveOrders");
    spec["sourceDescriptions"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "other", "url": "./orders.json", "type": "asyncapi"
        }));
    let mut other = spec["workflows"][0]["steps"][0].clone();
    other["stepId"] = json!("other");
    other["operationId"] = json!("$sourceDescriptions.other.receiveOrders");
    spec["workflows"][0]["steps"]
        .as_array_mut()
        .unwrap()
        .push(other);
    fixture.write(&spec, &doc);
    assert_valid(
        &fixture,
        &[
            "--asyncapi-server",
            "orders=kafka-secure",
            "--asyncapi-server",
            "other=kafka",
        ],
    );
}

#[test]
fn malformed_or_duplicate_server_flags_use_validate_usage_code() {
    let fixture = Fixture::new();
    fixture.write(&spec(), &document());
    for flags in [
        vec!["--asyncapi-server", "orders"],
        vec!["--asyncapi-server", "=kafka"],
        vec!["--asyncapi-server", "orders="],
        vec![
            "--asyncapi-server",
            "orders=kafka",
            "--asyncapi-server",
            "orders=kafka",
        ],
    ] {
        let mut args = vec!["--json"];
        args.extend_from_slice(&flags);
        let output = fixture.run(&args);
        assert!(!output.status.success());
        let body = body(&output);
        assert_eq!(body["kind"], "error");
        assert_eq!(body["code"], "VALIDATE_ASYNCAPI_SERVER_FLAG");
        let human = fixture.run(&flags);
        assert!(!human.status.success());
        assert!(String::from_utf8_lossy(&human.stderr).contains(body["error"].as_str().unwrap()));
    }
}

#[test]
fn all_refused_steps_are_reported_once_and_strict_promotes_warnings() {
    let fixture = Fixture::new();
    let mut spec = spec();
    spec["unknownField"] = json!(true);
    let mut send = spec["workflows"][0]["steps"][0].clone();
    send["stepId"] = json!("send");
    send["action"] = json!("send");
    spec["workflows"][0]["steps"]
        .as_array_mut()
        .unwrap()
        .push(send.clone());
    spec["workflows"]
        .as_array_mut()
        .unwrap()
        .push(json!({"workflowId": "second", "steps": [send]}));
    fixture.write(&spec, &document());
    for strict in [false, true] {
        let flags = if strict {
            vec!["--json", "--strict"]
        } else {
            vec!["--json"]
        };
        let output = fixture.run(&flags);
        assert!(!output.status.success());
        let body = body(&output);
        assert_eq!(body["valid"], false);
        let errors = body["errors"].as_array().unwrap();
        assert_eq!(errors.len(), if strict { 3 } else { 2 }, "{body}");
        for (error, path) in errors
            .iter()
            .zip(["workflows[0].steps[1]", "workflows[1].steps[0]"])
        {
            assert_eq!(error["path"], path);
            assert!(error["message"]
                .as_str()
                .unwrap()
                .starts_with("RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT: "));
        }
        if strict {
            assert_eq!(errors[2]["kind"], "unknownField");
            assert!(body.get("warnings").is_none());
            let human = fixture.run(&["--strict"]);
            let stderr = String::from_utf8_lossy(&human.stderr);
            for error in errors {
                assert!(stderr.contains(error["message"].as_str().unwrap()));
            }
        } else {
            assert_eq!(body["warnings"][0]["kind"], "unknownField");
        }
    }
}

#[test]
fn http_only_validation_does_not_load_sources() {
    let fixture = Fixture::new();
    let mut spec = spec();
    spec["sourceDescriptions"][0]["url"] = json!("./missing.json");
    spec["workflows"][0]["steps"][0] = json!({"stepId": "http", "operationId": "get"});
    fixture.write(&spec, &document());
    assert_valid(&fixture, &[]);
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args(["--json", "validate", "examples/httpbin-get.arazzo.yaml"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(body(&output)["valid"], true);
}

#[test]
fn shipped_example_validates_three_workflows_and_missing_source_copy_fails() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("examples/kafka-order-followup.arazzo.yaml");
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["--json", "validate"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(body(&output)["workflows"], 3);
    let mut spec: Value = serde_yaml_ng::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    for source in spec["sourceDescriptions"].as_array_mut().unwrap() {
        if source["type"] == "asyncapi" {
            source["url"] = json!("./missing.json");
        }
    }
    let fixture = Fixture::new();
    fixture.write(&spec, &document());
    let output = fixture.run(&["--json"]);
    assert!(!output.status.success());
    let body = body(&output);
    assert_eq!(body["valid"], false);
    assert_eq!(body["errors"].as_array().unwrap().len(), 3);
    for error in body["errors"].as_array().unwrap() {
        let message = error["message"].as_str().unwrap();
        assert!(message.starts_with("RUNTIME_MESSAGE_TRANSPORT_CONFIG: "));
        assert!(message.contains("missing.json"));
    }
}
