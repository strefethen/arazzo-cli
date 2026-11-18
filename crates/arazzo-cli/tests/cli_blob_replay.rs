#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0PLANTED-REPLAY-JPEG\x00\xFF\xD9";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nPLANTED-REPLAY-PNG\x00\xff";
const MARKER: &str = "PLANTED-REPLAY";

struct Fixture {
    dir: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-blob-replay-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));

        let server = Server::http("127.0.0.1:0").unwrap_or_else(|err| panic!("binding: {err}"));
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(format!("{} {}", request.method(), request.url()));
                let (data, media_type) = match request.url() {
                    "/photo" => (PNG.to_vec(), "image/png"),
                    _ => (br#"{"id":"p1"}"#.to_vec(), "application/json"),
                };
                let header = Header::from_bytes("Content-Type", media_type)
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request.respond(TinyResponse::from_data(data).with_header(header));
            }
        });

        let fixture = Self {
            dir,
            requests,
            stop,
            worker: Some(worker),
        };
        fixture.write("cat.jpg", JPEG);
        let operation =
            |id: &str| json!({"operationId":id, "responses":{"200":{"description":"ok"}}});
        fixture.write(
            "api.openapi.json",
            json!({"openapi":"3.0.3", "info":{"title":"replay", "version":"1"},
                "servers":[{"url":base_url}],
                "paths":{"/upload":{"post":operation("upload")},
                    "/photo":{"get":operation("photo")}}})
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

    fn trace(&self, name: &str) -> Value {
        serde_json::from_str(&self.read(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn record(&self, workflow: &str, args: &[&str]) -> Value {
        let mut all = vec![
            "--json",
            "run",
            "spec.arazzo.json",
            workflow,
            "--trace",
            "trace.json",
        ];
        all.extend_from_slice(args);
        let run = succeeded(&self.cli(&all));
        assert!(!self.take().is_empty(), "{workflow}: the live run is sent");
        run
    }

    fn replay(&self, trace: &str) -> Output {
        let output = self.cli(&["--json", "replay", trace]);
        assert!(self.take().is_empty(), "replay never touches the wire");
        output
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .requests
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
        "photo":{}, "scan":{"writeOnly":true}, "meta":{"type":"object"}
    }});
    let upload = |step_id: &str, content_type: &str, payload: &str| {
        json!({"stepId":step_id, "operationId":"upload",
            "requestBody":{"contentType":content_type, "payload":payload},
            "successCriteria":[{"condition":"$statusCode == 200"}],
            "outputs":{"id":"$response.body#/id", "sent":"$request.body"}})
    };
    let workflow = |id: &str, steps: Value, outputs: Value| json!({"workflowId":id, "inputs":inputs, "steps":steps, "outputs":outputs});
    let sent = |step_id: &str| {
        json!({"id":format!("$steps.{step_id}.outputs.id"),
            "sent":format!("$steps.{step_id}.outputs.sent")})
    };
    json!({
        "arazzo":"1.1.0", "info":{"title":"blob replay", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":[
            workflow("upload", json!([upload("upload", "image/jpeg", "$inputs.photo")]),
                sent("upload")),
            workflow("sensitive",
                json!([upload("upload", "application/octet-stream", "$inputs.scan")]),
                sent("upload")),
            workflow("roundtrip", json!([
                {"stepId":"download", "operationId":"photo",
                    "successCriteria":[{"condition":"$statusCode == 200"}],
                    "outputs":{"photo":"$response.body"}},
                upload("reupload", "image/png", "$steps.download.outputs.photo")
            ]), json!({"photo":"$steps.download.outputs.photo",
                "id":"$steps.reupload.outputs.id", "sent":"$steps.reupload.outputs.sent"})),
            workflow("shaped", json!([upload("upload", "application/json", "$inputs.meta")]),
                json!({"id":"$steps.upload.outputs.id", "meta":"$inputs.meta",
                    "sent":"$steps.upload.outputs.sent"}))
        ]
    })
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not JSON: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn succeeded(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    stdout_json(output)
}

fn failed(output: &Output, code: &str) -> String {
    assert!(!output.status.success(), "the command must fail");
    let error = stdout_json(output);
    assert_eq!(error["kind"], "error", "{error}");
    assert_eq!(error["code"], code, "{error}");
    error["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error text: {error}"))
        .to_owned()
}

fn descriptor_of(bytes: &[u8], media_type: Option<&str>, filename: Option<&str>) -> Value {
    arazzo_runtime::BytesDescriptor::of(
        bytes,
        media_type.map(str::to_owned),
        filename.map(str::to_owned),
    )
    .to_value()
}

fn assert_replays_like(fixture: &Fixture, trace: &str, live: &Value) -> Value {
    let output = fixture.replay(trace);
    let replay = succeeded(&output);
    assert_eq!(replay["outputs"], live["outputs"], "{replay}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(MARKER),
        "replay printed content"
    );
    replay
}

#[test]
fn an_upload_replays_with_the_same_outcome_and_output_descriptors() {
    let fixture = Fixture::new();
    let live = fixture.record("upload", &["--input-file", "photo=cat.jpg"]);
    let photo = descriptor_of(JPEG, None, Some("cat.jpg"));
    assert_eq!(live["outputs"], json!({"id":"p1", "sent":photo}));
    let trace = fixture.trace("trace.json");
    assert_eq!(trace["inputBlobs"], json!({"photo":photo}));
    assert_eq!(trace["steps"][0]["request"]["bodyBlobs"], json!({"":photo}));

    let replay = assert_replays_like(&fixture, "trace.json", &live);
    assert_eq!(replay["requestsChecked"], 1);

    let human = fixture.cli(&["replay", "trace.json"]);
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("Replay completed"), "{stdout}");
    assert!(
        stdout.contains(photo["sha256"].as_str().unwrap_or_default()),
        "{stdout}"
    );
}

#[test]
fn a_download_and_reupload_replays_with_the_same_outcome_and_output_descriptors() {
    let fixture = Fixture::new();
    let live = fixture.record("roundtrip", &[]);
    let photo = descriptor_of(PNG, Some("image/png"), None);
    assert_eq!(
        live["outputs"],
        json!({"id":"p1", "photo":photo, "sent":photo})
    );
    let trace = fixture.trace("trace.json");
    assert_eq!(trace["steps"][0]["response"]["bodyBlob"], photo);
    assert_eq!(trace["steps"][1]["request"]["bodyBlobs"], json!({"":photo}));
    assert!(!fixture.read("trace.json").contains(MARKER));

    let replay = assert_replays_like(&fixture, "trace.json", &live);
    assert_eq!(replay["requestsChecked"], 2);
}

#[test]
fn a_sensitive_input_replays_as_the_redacted_marker() {
    let fixture = Fixture::new();
    let live = fixture.record("sensitive", &["--input-file", "scan=cat.jpg"]);
    let redacted = json!({"kind":"bytes", "redacted":true});
    assert_eq!(live["outputs"], json!({"id":"p1", "sent":redacted}));
    let trace = fixture.trace("trace.json");
    assert_eq!(trace["inputBlobs"], json!({"scan":redacted}));
    assert_eq!(
        trace["steps"][0]["request"]["bodyBlobs"],
        json!({"":redacted})
    );

    assert_replays_like(&fixture, "trace.json", &live);

    let mut hashed = trace;
    hashed["steps"][0]["request"]["bodyBlobs"][""] = descriptor_of(JPEG, None, None);
    fixture.write("hashed.json", hashed.to_string().as_bytes());
    let message = failed(
        &fixture.replay("hashed.json"),
        "RUNTIME_REPLAY_REQUEST_MISMATCH",
    );
    assert!(message.ends_with("body mismatch"), "{message}");
}

#[test]
fn a_tampered_blob_hash_is_a_request_mismatch() {
    let fixture = Fixture::new();
    fixture.record("upload", &["--input-file", "photo=cat.jpg"]);
    let trace = fixture.trace("trace.json");
    let other = descriptor_of(b"other bytes", None, Some("cat.jpg"));

    let mut request = trace.clone();
    request["steps"][0]["request"]["bodyBlobs"][""] = other.clone();
    fixture.write("request.json", request.to_string().as_bytes());
    let mut input = trace;
    input["inputBlobs"]["photo"] = other;
    fixture.write("input.json", input.to_string().as_bytes());

    for tampered in ["request.json", "input.json"] {
        let message = failed(&fixture.replay(tampered), "RUNTIME_REPLAY_REQUEST_MISMATCH");
        assert!(
            message.ends_with("replay request drift at seq 1 attempt 1: body mismatch"),
            "{tampered}: {message}"
        );
    }
}

#[test]
fn a_descriptor_shaped_json_value_replays_as_json() {
    let fixture = Fixture::new();
    let shaped = descriptor_of(JPEG, None, Some("cat.jpg"));
    let input = format!("meta={shaped}");
    let live = fixture.record("shaped", &["--input-json", input.as_str()]);
    assert_eq!(
        live["outputs"],
        json!({"id":"p1", "meta":shaped, "sent":shaped})
    );
    let trace = fixture.trace("trace.json");
    assert!(trace.get("inputBlobs").is_none(), "{trace}");
    assert_eq!(trace["steps"][0]["request"]["body"], shaped);
    assert!(trace["steps"][0]["request"].get("bodyBlobs").is_none());
    assert_replays_like(&fixture, "trace.json", &live);

    fixture.record("upload", &["--input-file", "photo=cat.jpg"]);
    let mut unmarked = fixture.trace("trace.json");
    let descriptor = unmarked["inputBlobs"]["photo"].clone();
    unmarked["inputs"]["photo"] = descriptor;
    if let Some(trace) = unmarked.as_object_mut() {
        trace.remove("inputBlobs");
    }
    fixture.write("unmarked.json", unmarked.to_string().as_bytes());
    let message = failed(
        &fixture.replay("unmarked.json"),
        "RUNTIME_INVALID_REQUEST_BODY",
    );
    assert!(message.contains("got object"), "{message}");
}
