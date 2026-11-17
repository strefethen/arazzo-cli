#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::BytesDescriptor;
use regex::Regex;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const MARKER: &str = "PLANTED-OUTPUT-BYTES-41c9!";
const ENCODED: [&str; 3] = [
    "UExBTlRFRC1PVVRQVVQtQllURVMtNDFjOSE",
    "/9j/4AAQUExBTlRFRC1PVVRQVVQtQllURVMtNDFjOSEA/9k=",
    "_9j_4AAQUExBTlRFRC1PVVRQVVQtQllURVMtNDFjOSEA_9k=",
];

fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    bytes.extend_from_slice(MARKER.as_bytes());
    bytes.extend_from_slice(&[0x00, 0xFF, 0xD9]);
    bytes
}

struct Fixture {
    dir: PathBuf,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-blob-descriptors-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|err| panic!("binding: {err}"));
        let base_url = format!("http://{}", server.server_addr());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&bodies), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(body);
                let header = Header::from_bytes("Content-Type", "application/json")
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request
                    .respond(TinyResponse::from_string(r#"{"id":"p1"}"#).with_header(header));
            }
        });
        let fixture = Self {
            dir,
            bodies,
            stop,
            worker: Some(worker),
        };
        fixture.write("cat.jpg", &jpeg());
        fixture.write(
            "api.openapi.json",
            json!({"openapi":"3.0.3", "info":{"title":"uploads", "version":"1"},
                "servers":[{"url":base_url}],
                "paths":{"/upload":{"post":{"operationId":"upload",
                    "responses":{"200":{"description":"ok"}}}}}})
            .to_string()
            .as_bytes(),
        );
        fixture.write("spec.arazzo.json", spec().to_string().as_bytes());
        fixture
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.dir.join(name);
        fs::write(&path, bytes).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    }

    fn read(&self, name: &str) -> String {
        let path = self.dir.join(name);
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
    }

    fn run(&self, workflow: &str, input: &str, json_output: bool) -> Output {
        let file = format!("{input}=cat.jpg");
        let mut args = vec![
            "run",
            "spec.arazzo.json",
            workflow,
            "--input-file",
            file.as_str(),
            "--trace",
            "trace.json",
            "--export-run",
            "export.json",
        ];
        if json_output {
            args.insert(0, "--json");
        }
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(&args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn take(&self) -> Vec<Vec<u8>> {
        std::mem::take(
            &mut *self
                .bodies
                .lock()
                .unwrap_or_else(|_| panic!("reading requests")),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn spec() -> Value {
    let inputs = json!({"type":"object", "properties":{
        "photo":{}, "scan":{"writeOnly":true}}});
    let upload = |step_id: &str, payload: &str, outputs: Value| {
        json!({"stepId":step_id, "operationId":"upload",
            "requestBody":{"contentType":"application/octet-stream", "payload":payload},
            "successCriteria":[{"condition":"$statusCode == 200"}], "outputs":outputs})
    };
    json!({
        "arazzo":"1.1.0", "info":{"title":"blob descriptors", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":[
            {"workflowId":"carry", "inputs":inputs, "steps":[
                upload("first", "$inputs.photo",
                    json!({"photo":"$inputs.photo", "sent":"$request.body"})),
                upload("again", "$steps.first.outputs.sent",
                    json!({"id":"$response.body#/id"}))
            ], "outputs":{
                "id":"$steps.again.outputs.id",
                "photo":"$steps.first.outputs.photo",
                "sent":"$steps.first.outputs.sent"
            }},
            {"workflowId":"secret", "inputs":inputs, "steps":[
                upload("first", "$inputs.scan", json!({"copy":"$inputs.scan"}))
            ], "outputs":{"copy":"$steps.first.outputs.copy"}},
            {"workflowId":"named", "inputs":inputs, "steps":[
                upload("first", "$inputs.photo", json!({"apiKey":"$inputs.photo"}))
            ], "outputs":{"apiKey":"$steps.first.outputs.apiKey"}}
        ]
    })
}

fn descriptor() -> Value {
    BytesDescriptor::of(&jpeg(), None, Some("cat.jpg".to_owned())).to_value()
}

fn redacted() -> Value {
    json!({"kind":"bytes", "redacted":true})
}

fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("stdout JSON: {err}"))
}

fn parsed(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|err| panic!("{err}: {raw}"))
}

fn assert_no_bytes(label: &str, text: &[u8]) {
    let contains = |needle: &[u8]| text.windows(needle.len()).any(|window| window == needle);
    assert!(!contains(&jpeg()), "{label}: the bytes leaked");
    assert!(
        !contains(MARKER.as_bytes()),
        "{label}: planted bytes leaked"
    );
    for encoded in ENCODED {
        assert!(!contains(encoded.as_bytes()), "{label}: base64 leaked");
    }
}

fn published_descriptor_schema() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/schemas/trace-v2.schema.json");
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    parsed(&raw)["$defs"]["bytesDescriptor"].clone()
}

fn conforms(value: &Value, schema: &Value) -> bool {
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        return branches
            .iter()
            .filter(|branch| conforms(value, branch))
            .count()
            == 1;
    }
    if let Some(expected) = schema.get("const") {
        return value == expected;
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Some(object) = value.as_object() else {
                return false;
            };
            let properties = schema["properties"].as_object();
            let required = schema["required"].as_array().into_iter().flatten();
            required
                .filter_map(Value::as_str)
                .all(|name| object.contains_key(name))
                && object
                    .iter()
                    .all(|(name, item)| match properties.and_then(|p| p.get(name)) {
                        Some(property) => conforms(item, property),
                        None => schema["additionalProperties"] != json!(false),
                    })
        }
        Some("integer") => value.as_u64().is_some(),
        Some("string") => value.as_str().is_some_and(|text| {
            schema["pattern"].as_str().is_none_or(|pattern| {
                Regex::new(pattern)
                    .unwrap_or_else(|err| panic!("schema pattern {pattern}: {err}"))
                    .is_match(text)
            })
        }),
        _ => true,
    }
}

fn assert_published(label: &str, value: &Value) {
    assert!(
        conforms(value, &published_descriptor_schema()),
        "{label}: {value} does not match docs/schemas bytesDescriptor"
    );
}

#[test]
fn json_output_export_and_trace_render_blob_outputs_as_descriptors() {
    let fixture = Fixture::new();
    let output = fixture.run("carry", "photo", true);
    let result = json_of(&output);
    let descriptor = descriptor();
    assert_published("descriptor", &descriptor);
    assert_eq!(
        result,
        json!({"kind":"success", "outputs":{
            "id":"p1", "photo":descriptor, "sent":descriptor}})
    );
    assert_no_bytes("run --json stdout", &output.stdout);
    assert_no_bytes("run --json stderr", &output.stderr);
    assert_eq!(
        fixture.take(),
        [jpeg(), jpeg()],
        "both uploads carry the file"
    );

    let raw_export = fixture.read("export.json");
    assert_no_bytes("export", raw_export.as_bytes());
    let export = parsed(&raw_export);
    assert_eq!(export["outputs"], result["outputs"]);
    let first = &export["workflows"]["carry"]["steps"]["first"]["executions"][0];
    assert_eq!(
        first["outputs"],
        json!({"photo":descriptor, "sent":descriptor})
    );

    let raw_trace = fixture.read("trace.json");
    assert_no_bytes("trace", raw_trace.as_bytes());
    let trace = parsed(&raw_trace);
    let step = &trace["steps"][0];
    assert_eq!(
        step["outputBlobs"],
        json!({"photo":descriptor, "sent":descriptor})
    );
    assert!(step.get("outputs").is_none(), "{step}");
    assert_eq!(trace["steps"][1]["outputs"], json!({"id":"p1"}));
    for (name, value) in step["outputBlobs"].as_object().into_iter().flatten() {
        assert_published(name, value);
    }

    let text = fixture.run("carry", "photo", false);
    assert!(text.status.success(), "{text:?}");
    assert_no_bytes("run stdout", &text.stdout);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains(&format!(
            "sha256\":\"{}\"",
            descriptor["sha256"].as_str().unwrap_or_default()
        )),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
}

#[test]
fn a_sensitive_blob_output_is_the_redacted_marker_on_every_surface() {
    for (workflow, input, name, json_marker, sensitive_input) in [
        ("secret", "scan", "copy", redacted(), true),
        ("named", "photo", "apiKey", descriptor(), false),
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(workflow, input, true);
        let result = json_of(&output);
        assert_eq!(
            result["outputs"],
            json!({ (name): json_marker }),
            "{workflow}"
        );
        assert_no_bytes(workflow, &output.stdout);
        assert_eq!(fixture.take(), [jpeg()], "{workflow}");

        let raw_export = fixture.read("export.json");
        let raw_trace = fixture.read("trace.json");
        let sha = descriptor()["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        for (label, raw) in [("export", &raw_export), ("trace", &raw_trace)] {
            assert_no_bytes(label, raw.as_bytes());
            assert!(
                !sensitive_input || !raw.contains(&sha),
                "{workflow} {label}: a hash leaked"
            );
        }
        let export = parsed(&raw_export);
        assert_eq!(
            export["outputs"],
            json!({ (name): redacted() }),
            "{workflow}"
        );
        let trace = parsed(&raw_trace);
        assert_eq!(
            trace["steps"][0]["outputBlobs"],
            json!({ (name): redacted() }),
            "{workflow}"
        );
        assert_published(workflow, &redacted());
    }
}

#[test]
fn a_trace_that_recorded_a_blob_output_does_not_replay() {
    let fixture = Fixture::new();
    json_of(&fixture.run("carry", "photo", true));
    let mut trace = parsed(&fixture.read("trace.json"));
    trace["inputBlobs"] = json!({});
    trace["steps"][0]["request"]
        .as_object_mut()
        .map(|request| request.remove("bodyBlobs"));
    fixture.write("trace.json", trace.to_string().as_bytes());
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["--json", "replay", "trace.json"])
        .current_dir(&fixture.dir)
        .output()
        .unwrap_or_else(|err| panic!("running replay: {err}"));
    assert!(!output.status.success());
    let error: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("replay stdout: {err}"));
    assert_eq!(error["code"], "REPLAY_TRACE_LOAD", "{error}");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|text| text.contains("outputBlobs of workflow \"carry\" step \"first\"")),
        "{error}"
    );
}
