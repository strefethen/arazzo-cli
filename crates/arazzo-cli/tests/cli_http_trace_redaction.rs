#![forbid(unsafe_code)]

mod common;

use arazzo_runtime::BytesDescriptor;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{fs, thread, time::Duration};

#[test]
fn persisted_http_preview_masks_secrets_without_masking_live_outputs() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap_or_else(|_| panic!("bind loopback"));
    let dir = std::env::temp_dir().join(format!("arazzo-http-preview-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap_or_else(|_| panic!("fixture directory"));
    common::write_in(&dir, &format!("http://{}", server.server_addr()));
    let body = format!(
        r#"{{"a_token":"FAKE-HTTP-CANARY","b_nested":{{"password":"FAKE-NESTED-CANARY"}},"c_name":"visible","zz_padding":"{}"}}"#,
        "é".repeat(3000)
    );
    let body_bytes = body.len();
    let handle = thread::spawn(move || {
        for _ in 0..3 {
            let request = server
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("receive"))
                .unwrap_or_else(|| panic!("CLI request"));
            request
                .respond(
                    tiny_http::Response::from_string(&body).with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json")
                            .unwrap_or_else(|_| panic!("header")),
                    ),
                )
                .unwrap_or_else(|_| panic!("response"));
        }
    });
    let spec = dir.join("workflow.yaml");
    fs::write(&spec, "arazzo: 1.0.1\ninfo:\n  title: HTTP preview\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: fixture.openapi.json\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationId: fixtureGetOne\n        outputs:\n          token: $response.body#/a_token\n    outputs:\n      token: $steps.one.outputs.token\n").unwrap_or_else(|_| panic!("write workflow"));
    let mut artifacts = Vec::new();
    for limit in [0, 2048, 4096] {
        let trace = dir.join(format!("trace-{limit}.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(["--json", "run"])
            .arg(&spec)
            .args(["wf", "--trace"])
            .arg(&trace)
            .args(["--trace-max-body-bytes", &limit.to_string()])
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|_| panic!("run CLI"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let live: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("live JSON"));
        assert_eq!(live["outputs"]["token"], "FAKE-HTTP-CANARY");
        artifacts.push((
            limit,
            fs::read_to_string(trace).unwrap_or_else(|_| panic!("persisted trace")),
        ));
    }
    handle.join().unwrap_or_else(|_| panic!("fixture thread"));
    for (limit, raw) in artifacts {
        assert!(!raw.contains("FAKE-HTTP-CANARY"));
        assert!(!raw.contains("FAKE-NESTED-CANARY"));
        let trace: Value = serde_json::from_str(&raw).unwrap_or_else(|_| panic!("trace JSON"));
        let response = &trace["steps"][0]["response"];
        assert_eq!(response["bodyBytes"], body_bytes);
        assert!(response.get("bodyLossy").is_none());
        assert!(response.get("bodyPreview").is_none());
        assert_eq!(response["mediaType"], "application/json");
        let text = response["body"].as_str().unwrap_or_else(|| panic!("body"));
        assert!(text.len() <= limit + 3);
        if limit > 0 {
            assert!(text.contains("[REDACTED]"));
            assert!(text.contains("visible"));
        }
        assert_valid_trace_v2(&trace);
    }
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRPLANTED-PNG-BYTES\xFF\xFE";

struct RawServer {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RawServer {
    fn start(status: u16, media_type: Option<&'static str>, body: &'static [u8]) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap_or_else(|_| panic!("bind"));
        let url = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let mut response = tiny_http::Response::from_data(body)
                    .with_status_code(tiny_http::StatusCode(status));
                if let Some(media_type) = media_type {
                    response.add_header(
                        tiny_http::Header::from_bytes("Content-Type", media_type)
                            .unwrap_or_else(|()| panic!("content type")),
                    );
                }
                let _ = request.respond(response);
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(server: &RawServer, criterion: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-trace-v2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|_| panic!("fixture directory"));
        common::write_in(&dir, &server.url);
        fs::write(
            dir.join("workflow.yaml"),
            format!("arazzo: 1.1.0\ninfo:\n  title: trace v2\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: fixture.openapi.json\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationId: fixtureGetOne\n        successCriteria:\n          - condition: {criterion}\n"),
        )
        .unwrap_or_else(|_| panic!("write workflow"));
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn run(&self, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        if json {
            command.arg("--json");
        }
        command
            .args(["run", "workflow.yaml", "wf", "--trace"])
            .arg(self.path("trace.json"))
            .arg("--export-run")
            .arg(self.path("run.json"))
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|_| panic!("run CLI"))
    }

    fn read(&self, name: &str) -> (String, Value) {
        let raw = fs::read_to_string(self.path(name)).unwrap_or_else(|_| panic!("read {name}"));
        let value = serde_json::from_str(&raw).unwrap_or_else(|_| panic!("parse {name}"));
        (raw, value)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, byte)| {
            acc | u32::from(*byte) << (16 - 8 * i)
        });
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

fn assert_no_content(raw: &str, body: &[u8], planted: &str) {
    assert!(!raw.contains(planted), "planted bytes leaked: {raw}");
    assert!(!raw.contains('\u{fffd}'), "lossy text leaked: {raw}");
    for offset in 0..3 {
        let encoded = base64(&body[offset..]);
        let stable = &encoded[..encoded.len().saturating_sub(4)];
        assert!(!raw.contains(stable), "base64 content leaked: {raw}");
    }
}

#[test]
fn a_latin_1_text_body_is_recorded_decoded_and_redacted() {
    let body: &'static [u8] = b"caf\xE9 token=FAKE-LATIN-CANARY";
    let server = RawServer::start(200, Some("text/plain; charset=iso-8859-1"), body);
    let fixture = Fixture::new(&server, "$statusCode == 200");
    let output = fixture.run(true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (raw, trace) = fixture.read("trace.json");
    assert_eq!(trace["schemaVersion"], "trace.v2");
    let response = &trace["steps"][0]["response"];
    assert_eq!(response["body"], "caf\u{e9} token=[REDACTED]");
    assert_eq!(response["mediaType"], "text/plain;charset=iso-8859-1");
    assert_eq!(response["bodyBytes"], body.len());
    assert!(!raw.contains("FAKE-LATIN-CANARY"), "{raw}");
    assert!(!raw.contains('\u{fffd}'), "{raw}");
    assert_valid_trace_v2(&trace);
}

#[test]
fn bytes_and_undecodable_bodies_are_descriptors_without_content() {
    let cases: [(Option<&'static str>, &'static [u8], &str, bool); 3] = [
        (Some("image/png"), PNG, "PLANTED-PNG-BYTES", false),
        (
            Some("application/json"),
            b"{\"k\":\"PLANTED-JSON-BYTES\xFF\"}",
            "PLANTED-JSON-BYTES",
            true,
        ),
        (
            None,
            b"PLANTED-UNTYPED-BYTES {}",
            "PLANTED-UNTYPED-BYTES",
            false,
        ),
    ];
    for (media_type, body, planted, decode_error) in cases {
        let server = RawServer::start(200, media_type, body);
        let fixture = Fixture::new(&server, "$statusCode == 200");
        let output = fixture.run(true);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let (raw, trace) = fixture.read("trace.json");
        assert_no_content(&raw, body, planted);
        let response = &trace["steps"][0]["response"];
        assert!(response.get("body").is_none(), "{response}");
        assert_eq!(
            response["bodyBlob"],
            serde_json::to_value(BytesDescriptor::of(
                body,
                media_type.map(str::to_owned),
                None
            ))
            .unwrap_or_else(|_| panic!("descriptor"))
        );
        assert_eq!(
            response.get("bodyError").is_some(),
            decode_error,
            "{response}"
        );
        assert_eq!(
            response.get("mediaType").and_then(Value::as_str),
            media_type
        );
        assert_valid_trace_v2(&trace);
        let (export_raw, export) = fixture.read("run.json");
        assert_no_content(&export_raw, body, planted);
        let exported = &export["workflows"]["wf"]["steps"]["one"]["executions"][0]["response"];
        assert_eq!(
            exported.get("contentType").and_then(Value::as_str),
            media_type
        );
    }
}

#[test]
fn criteria_failures_describe_png_and_latin_bodies_without_lossy_bytes() {
    let cases: [(&'static str, &'static [u8], String, &str); 2] = [
        (
            "image/png",
            PNG,
            format!("body=<{} bytes, image/png>)", PNG.len()),
            "PLANTED-PNG-BYTES",
        ),
        (
            "text/plain; charset=iso-8859-1",
            b"caf\xE9 token=FAKE-LATIN-CANARY",
            "body=caf\u{e9} token=[REDACTED])".to_owned(),
            "FAKE-LATIN-CANARY",
        ),
    ];
    for (media_type, body, described, secret) in cases {
        let described = described.as_str();
        let server = RawServer::start(200, Some(media_type), body);
        let fixture = Fixture::new(&server, "$statusCode == 201");
        let human = fixture.run(false);
        assert!(!human.status.success());
        let stderr = String::from_utf8(human.stderr).unwrap_or_else(|_| panic!("UTF-8 stderr"));
        assert!(stderr.contains(described), "{stderr}");
        let json = fixture.run(true);
        assert!(!json.status.success());
        let stdout = String::from_utf8(json.stdout).unwrap_or_else(|_| panic!("UTF-8 stdout"));
        let error: Value = serde_json::from_str(&stdout).unwrap_or_else(|_| panic!("{stdout}"));
        assert_eq!(error["code"], "RUNTIME_SUCCESS_CRITERIA_FAILED");
        assert!(
            error["error"]
                .as_str()
                .unwrap_or_default()
                .contains(described),
            "{error}"
        );
        let (export_raw, export) = fixture.read("run.json");
        let (trace_raw, _) = fixture.read("trace.json");
        for text in [&stderr, &stdout, &export_raw, &trace_raw] {
            assert!(!text.contains('\u{fffd}'), "lossy text leaked: {text}");
            assert!(!text.contains(secret), "content leaked: {text}");
        }
        let response = &export["workflows"]["wf"]["steps"]["one"]["executions"][0]["response"];
        assert_eq!(
            response["contentType"],
            media_type.replace("; ", ";"),
            "{response}"
        );
    }
}

#[test]
fn the_v2_schema_rejects_v1_response_records() {
    let schema = trace_v2_schema();
    let mut trace = json!({
        "schemaVersion": "trace.v2",
        "tool": {"name": "arazzo", "version": "0"},
        "run": {"specPath": "s", "workflowId": "wf", "parallel": false, "dryRun": false,
            "timeoutMs": 0, "startedAt": "", "finishedAt": "", "durationMs": 0, "status": "success"},
        "inputs": {},
        "steps": [{"seq": 1, "workflowId": "wf", "stepId": "one", "attempt": 1, "kind": "http",
            "durationMs": 0, "decision": {"path": "next"},
            "response": {"statusCode": 200, "bodyBytes": 2, "mediaType": "application/json", "body": "{}"}}]
    });
    assert_eq!(validate(&schema, &schema, &trace, ""), Vec::<String>::new());
    let rejected: [(&str, Value); 5] = [
        ("/steps/0/response/contentType", json!("json")),
        ("/steps/0/response/bodyLossy", json!(true)),
        ("/steps/0/response/bodyPreview", json!("{}")),
        (
            "/steps/0/response/bodyBlob",
            json!({"kind": "bytes", "size": 2, "sha256": "00"}),
        ),
        ("/schemaVersion", json!("trace.v1")),
    ];
    for (pointer, value) in rejected {
        let mut bad = trace.clone();
        let (parent, key) = pointer
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("pointer"));
        let target = if parent.is_empty() {
            &mut bad
        } else {
            bad.pointer_mut(parent)
                .unwrap_or_else(|| panic!("{parent}"))
        };
        target[key] = value;
        assert!(
            !validate(&schema, &schema, &bad, "").is_empty(),
            "{pointer} must be rejected"
        );
    }
    let response = trace
        .pointer_mut("/steps/0/response")
        .unwrap_or_else(|| panic!("response"));
    response["bodyError"] = json!("not JSON");
    assert!(!validate(&schema, &schema, &trace, "").is_empty());
}

fn trace_v2_schema() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schemas/trace-v2.schema.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|_| panic!("schema")))
        .unwrap_or_else(|_| panic!("schema JSON"))
}

fn assert_valid_trace_v2(trace: &Value) {
    let schema = trace_v2_schema();
    let errors = validate(&schema, &schema, trace, "");
    assert!(errors.is_empty(), "{errors:?}\n{trace}");
}

fn validate(root: &Value, schema: &Value, value: &Value, at: &str) -> Vec<String> {
    let schema = match schema {
        Value::Bool(true) => return Vec::new(),
        Value::Bool(false) => return vec![format!("{at}: not allowed")],
        Value::Object(schema) => schema,
        other => panic!("schema node {other} is not a schema"),
    };
    let mut errors = Vec::new();
    for (keyword, rule) in schema {
        match keyword.as_str() {
            "$schema" | "$id" | "$defs" | "title" | "description" => {}
            "$ref" => {
                let pointer = rule
                    .as_str()
                    .and_then(|r| r.strip_prefix('#'))
                    .unwrap_or_else(|| panic!("local $ref only"));
                let target = root.pointer(pointer).unwrap_or_else(|| panic!("{pointer}"));
                errors.extend(validate(root, target, value, at));
            }
            "type" => {
                let ok = match rule.as_str().unwrap_or_default() {
                    "object" => value.is_object(),
                    "array" => value.is_array(),
                    "string" => value.is_string(),
                    "boolean" => value.is_boolean(),
                    "number" => value.is_number(),
                    "integer" => value.is_i64() || value.is_u64(),
                    other => panic!("type {other}"),
                };
                if !ok {
                    errors.push(format!("{at}: expected {rule}"));
                }
            }
            "const" if value != rule => errors.push(format!("{at}: expected {rule}")),
            "enum" if !rule.as_array().is_some_and(|items| items.contains(value)) => {
                errors.push(format!("{at}: not in {rule}"));
            }
            "const" | "enum" => {}
            "minimum" => {
                if value
                    .as_f64()
                    .zip(rule.as_f64())
                    .is_some_and(|(v, m)| v < m)
                {
                    errors.push(format!("{at}: below {rule}"));
                }
            }
            "pattern" => {
                let pattern = regex::Regex::new(rule.as_str().unwrap_or_default())
                    .unwrap_or_else(|_| panic!("pattern"));
                if value.as_str().is_some_and(|text| !pattern.is_match(text)) {
                    errors.push(format!("{at}: does not match {rule}"));
                }
            }
            "required" => {
                if let Some(object) = value.as_object() {
                    for name in rule
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        if !object.contains_key(name) {
                            errors.push(format!("{at}: missing {name}"));
                        }
                    }
                }
            }
            "dependentRequired" => {
                if let (Some(object), Some(rules)) = (value.as_object(), rule.as_object()) {
                    for (name, needs) in rules {
                        for need in needs
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            if object.contains_key(name) && !object.contains_key(need) {
                                errors.push(format!("{at}: {name} requires {need}"));
                            }
                        }
                    }
                }
            }
            "properties" | "additionalProperties" => {
                let Some(object) = value.as_object() else {
                    continue;
                };
                let declared = schema.get("properties").and_then(Value::as_object);
                for (name, member) in object {
                    let child = format!("{at}/{name}");
                    match (keyword.as_str(), declared.and_then(|d| d.get(name))) {
                        ("properties", Some(sub)) => {
                            errors.extend(validate(root, sub, member, &child))
                        }
                        ("additionalProperties", None) => {
                            errors.extend(validate(root, rule, member, &child));
                        }
                        _ => {}
                    }
                }
            }
            "items" => {
                for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                    errors.extend(validate(root, rule, item, &format!("{at}/{index}")));
                }
            }
            "oneOf" => {
                let matches = rule
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|sub| validate(root, sub, value, at).is_empty())
                    .count();
                if matches != 1 {
                    errors.push(format!("{at}: matches {matches} oneOf branches"));
                }
            }
            "not" => {
                if validate(root, rule, value, at).is_empty() {
                    errors.push(format!("{at}: matches a forbidden schema"));
                }
            }
            other => panic!("the test validator does not implement {other}"),
        }
    }
    errors
}
