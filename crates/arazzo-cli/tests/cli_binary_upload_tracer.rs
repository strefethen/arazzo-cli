#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

const MARKER: &str = "PLANTED-JPEG-BYTES-7f3a!";
const JPEG_SHA256: &str = "9ac0c3c62d5a0bdb39a61a581b7993bec3bdbe1ceff5e24f41ad8da48f2e5ffa";
const ENCODINGS: [&str; 4] = [
    "/9j/4AAQUExBTlRFRC1KUEVHLUJZVEVTLTdmM2EhAP/+gMMo/9k=",
    "_9j_4AAQUExBTlRFRC1KUEVHLUJZVEVTLTdmM2EhAP_-gMMo_9k=",
    "UExBTlRFRC1KUEVHLUJZVEVTLTdmM2Eh",
    "/9j/4AAQ",
];

fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    bytes.extend_from_slice(MARKER.as_bytes());
    bytes.extend_from_slice(&[0x00, 0xFF, 0xFE, 0x80, 0xC3, 0x28, 0xFF, 0xD9]);
    bytes
}

fn descriptor() -> Value {
    json!({"kind":"bytes", "size":38, "sha256":JPEG_SHA256, "filename":"cat.jpg"})
}

fn redacted() -> Value {
    json!({"kind":"bytes", "redacted":true})
}

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    line: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

struct Fixture {
    dir: PathBuf,
    requests: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-binary-upload-{}-{}",
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
                let content_type = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Content-Type"))
                    .map(|header| header.value.as_str().to_owned());
                let redirect = request.url() == "/redirect";
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen {
                        line: format!("{} {}", request.method(), request.url()),
                        content_type,
                        body,
                    });
                let header = |name: &str, value: &str| {
                    Header::from_bytes(name, value).unwrap_or_else(|()| panic!("header {name}"))
                };
                let response = if redirect {
                    TinyResponse::from_string("")
                        .with_status_code(StatusCode(307))
                        .with_header(header("Location", "/upload"))
                } else {
                    TinyResponse::from_string(r#"{"id":"p1"}"#)
                        .with_header(header("Content-Type", "application/json"))
                };
                let _ = request.respond(response);
            }
        });

        let fixture = Self {
            dir,
            requests,
            stop,
            worker: Some(worker),
        };
        fixture.write("cat.jpg", &jpeg());
        fixture.write(
            "api.openapi.json",
            json!({"openapi":"3.0.3", "info":{"title":"uploads", "version":"1"},
            "servers":[{"url":base_url}],
            "paths":{
                "/upload":{"post":{"operationId":"upload",
                    "responses":{"200":{"description":"ok"}}}},
                "/redirect":{"post":{"operationId":"redirected",
                    "responses":{"307":{"description":"moved"}}}}
            }})
            .to_string()
            .as_bytes(),
        );
        fixture.write("spec.arazzo.json", spec().to_string().as_bytes());
        fixture
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.join(name);
        fs::write(&path, bytes).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
        path
    }

    fn read(&self, name: &str) -> String {
        let path = self.dir.join(name);
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn run(&self, workflow: &str, args: &[&str]) -> Output {
        let mut all = vec!["--json", "run", "spec.arazzo.json", workflow];
        all.extend_from_slice(args);
        self.cli(&all)
    }

    fn take(&self) -> Vec<Seen> {
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
        "photo":{"description":"the picture"},
        "caption":{"type":"string"},
        "scan":{"writeOnly":true},
        "tokenFile":{},
        "typed":{"type":"string"},
        "encoded":{"type":"string", "contentEncoding":"base64"},
        "listed":{"enum":["a", "b"]},
        "fixed":{"const":"a"},
        "combined":{"allOf":[{}]}
    }});
    let upload = |operation: &str, content_type: Option<&str>, payload: Value| {
        let mut body = json!({"payload":payload});
        if let Some(content_type) = content_type {
            body["contentType"] = json!(content_type);
        }
        json!({"stepId":"upload", "operationId":operation, "requestBody":body,
            "successCriteria":[{"condition":"$statusCode == 200"}],
            "outputs":{"id":"$response.body#/id"}})
    };
    let workflow = |id: &str, step: Value| {
        json!({"workflowId":id, "inputs":inputs, "steps":[step],
            "outputs":{"id":"$steps.upload.outputs.id"}})
    };
    let photo = json!("$inputs.photo");
    let mut echo = upload("upload", Some("image/jpeg"), photo.clone());
    echo["outputs"]["sent"] = json!("$request.body");
    let mut required = workflow(
        "required",
        upload("upload", Some("image/jpeg"), photo.clone()),
    );
    required["inputs"]["required"] = json!(["photo"]);
    json!({
        "arazzo":"1.1.0", "info":{"title":"binary upload tracer", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":[
            workflow("upload", upload("upload", Some("image/jpeg"), photo.clone())),
            workflow("redirected", upload("redirected", Some("image/jpeg"), photo.clone())),
            workflow("sensitive", upload("upload", Some("application/octet-stream"),
                json!("$inputs.scan"))),
            workflow("named", upload("upload", Some("application/octet-stream"),
                json!("$inputs.tokenFile"))),
            workflow("literal", upload("upload", Some("image/jpeg"), json!("./cat.jpg"))),
            workflow("caption", upload("upload", Some("text/plain"), json!("$inputs.caption"))),
            workflow("untyped", upload("upload", None, photo.clone())),
            workflow("json", upload("upload", Some("application/json"), photo.clone())),
            workflow("form", upload("upload", Some("application/x-www-form-urlencoded"),
                photo.clone())),
            workflow("pointer", upload("upload", Some("image/jpeg"),
                json!("$inputs.photo#/exif"))),
            workflow("echo", echo),
            required
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

fn assert_no_file_bytes(label: &str, text: &[u8]) {
    let planted = jpeg();
    let contains = |needle: &[u8]| text.windows(needle.len()).any(|window| window == needle);
    assert!(!contains(&planted), "{label}: the file's bytes leaked");
    assert!(
        !contains(MARKER.as_bytes()),
        "{label}: planted bytes leaked"
    );
    for encoded in ENCODINGS {
        assert!(
            !contains(encoded.as_bytes()),
            "{label}: base64 of the file leaked"
        );
    }
}

fn assert_output_has_no_file_bytes(label: &str, output: &Output) {
    assert_no_file_bytes(&format!("{label} stdout"), &output.stdout);
    assert_no_file_bytes(&format!("{label} stderr"), &output.stderr);
}

#[test]
fn the_file_reaches_the_wire_byte_for_byte() {
    let fixture = Fixture::new();
    let output = fixture.run("upload", &["--input-file", "photo=cat.jpg"]);
    let result = succeeded(&output);
    assert_eq!(
        result,
        json!({"kind":"success", "outputs":{"id":"p1"}}),
        "the response is read as usual"
    );
    assert_output_has_no_file_bytes("run --json", &output);

    let seen = fixture.take();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].line, "POST /upload");
    assert_eq!(seen[0].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(seen[0].body[..3], [0xFF, 0xD8, 0xFF]);
    assert_eq!(seen[0].body, jpeg());

    let absolute = fixture.dir.join("cat.jpg");
    let flag = format!("photo={}", absolute.display());
    let stepped = fixture.run("upload", &["--step", "upload", "--input-file", &flag]);
    succeeded(&stepped);
    assert_eq!(fixture.take()[0].body, jpeg());
}

#[test]
fn a_307_redirect_resends_identical_bytes() {
    let fixture = Fixture::new();
    let output = fixture.run("redirected", &["--input-file", "photo=cat.jpg"]);
    assert_eq!(succeeded(&output)["outputs"]["id"], "p1");
    let seen = fixture.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        ["POST /redirect", "POST /upload"]
    );
    for request in &seen {
        assert_eq!(request.content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(request.body, jpeg(), "{}", request.line);
    }
}

#[test]
fn trace_json_output_and_export_carry_the_descriptor_and_never_the_bytes() {
    let fixture = Fixture::new();
    let output = fixture.run(
        "upload",
        &[
            "--input-file",
            "photo=cat.jpg",
            "--input",
            "caption=cat",
            "--trace",
            "trace.json",
            "--export-run",
            "export.json",
            "--verbose",
        ],
    );
    assert_eq!(succeeded(&output)["kind"], "success");
    assert_output_has_no_file_bytes("run --json --verbose", &output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Input files: {\"photo\": \"<38 bytes>\"}"),
        "{stderr}"
    );
    assert_eq!(fixture.take()[0].body, jpeg());

    let raw_trace = fixture.read("trace.json");
    assert_no_file_bytes("trace", raw_trace.as_bytes());
    let trace: Value = serde_json::from_str(&raw_trace).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(trace["schemaVersion"], "trace.v2");
    assert_eq!(trace["inputs"], json!({"caption":"cat"}));
    assert_eq!(trace["inputBlobs"], json!({"photo":descriptor()}));
    let request = &trace["steps"][0]["request"];
    assert_eq!(request["bodyBlobs"], json!({"":descriptor()}));
    assert!(request.get("body").is_none(), "{request}");
    assert_eq!(request["headers"]["Content-Type"], "image/jpeg");

    let raw_export = fixture.read("export.json");
    assert_no_file_bytes("export", raw_export.as_bytes());
    let export: Value = serde_json::from_str(&raw_export).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        export["inputs"],
        json!({"caption":"cat", "photo":descriptor()})
    );
    assert_eq!(export["outputs"], json!({"id":"p1"}));
}

fn assert_redacted_everywhere(workflow: &str, input: &str) {
    let fixture = Fixture::new();
    let file = format!("{input}=cat.jpg");
    let args = [
        "--input-file",
        file.as_str(),
        "--trace",
        "trace.json",
        "--export-run",
        "export.json",
        "--verbose",
    ];
    let output = fixture.run(workflow, &args);
    succeeded(&output);
    assert_output_has_no_file_bytes("run", &output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!(
            "Input files: {{\"{input}\": \"<bytes redacted>\"}}"
        )),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let seen = fixture.take();
    assert_eq!(
        seen[0].content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(seen[0].body, jpeg(), "redaction never changes what is sent");

    let raw_trace = fixture.read("trace.json");
    let raw_export = fixture.read("export.json");
    for (label, raw) in [("trace", &raw_trace), ("export", &raw_export)] {
        assert_no_file_bytes(label, raw.as_bytes());
        assert!(
            !raw.contains(JPEG_SHA256),
            "{label}: a sensitive hash leaked"
        );
        assert!(
            !raw.contains("cat.jpg"),
            "{label}: a sensitive filename leaked"
        );
    }
    let trace: Value = serde_json::from_str(&raw_trace).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(trace["inputBlobs"], json!({ (input): redacted() }));
    assert_eq!(
        trace["steps"][0]["request"]["bodyBlobs"],
        json!({"":redacted()})
    );
    let export: Value = serde_json::from_str(&raw_export).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(export["inputs"], json!({ (input): redacted() }));

    let dry = fixture.run(workflow, &["--input-file", file.as_str(), "--dry-run"]);
    assert_eq!(succeeded(&dry)["requests"][0]["body"], redacted());
    assert!(!String::from_utf8_lossy(&dry.stdout).contains(JPEG_SHA256));
    assert!(fixture.take().is_empty());
}

#[test]
fn a_write_only_input_file_renders_the_redacted_marker_everywhere() {
    assert_redacted_everywhere("sensitive", "scan");
}

#[test]
fn an_input_file_named_like_a_credential_renders_the_redacted_marker_everywhere() {
    assert_redacted_everywhere("named", "tokenFile");
}

#[test]
fn replaying_a_trace_that_recorded_bytes_is_refused() {
    let fixture = Fixture::new();
    succeeded(&fixture.run(
        "upload",
        &["--input-file", "photo=cat.jpg", "--trace", "trace.json"],
    ));
    assert_eq!(fixture.take().len(), 1);

    let message = failed(
        &fixture.cli(&["--json", "replay", "trace.json"]),
        "REPLAY_TRACE_LOAD",
    );
    assert!(
        message.ends_with("trace.json records bytes at inputBlobs.photo; a trace keeps only a descriptor of bytes, and replay cannot restore them yet"),
        "{message}"
    );

    let mut trace: Value =
        serde_json::from_str(&fixture.read("trace.json")).unwrap_or_else(|err| panic!("{err}"));
    trace
        .as_object_mut()
        .unwrap_or_else(|| panic!("trace object"))
        .remove("inputBlobs");
    fixture.write("request-only.json", trace.to_string().as_bytes());
    let message = failed(
        &fixture.cli(&["--json", "replay", "request-only.json"]),
        "REPLAY_TRACE_LOAD",
    );
    assert!(
        message.ends_with("records bytes at request.bodyBlobs of workflow \"upload\" step \"upload\" (seq 1); a trace keeps only a descriptor of bytes, and replay cannot restore them yet"),
        "{message}"
    );

    let human = fixture.cli(&["replay", "trace.json"]);
    assert!(!human.status.success());
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8_lossy(&human.stderr).contains("records bytes at inputBlobs.photo"));
    assert!(fixture.take().is_empty(), "a refused replay sends nothing");
}

#[test]
fn dry_run_shows_the_descriptor_and_no_bytes() {
    let fixture = Fixture::new();
    let args = ["--input-file", "photo=cat.jpg", "--dry-run"];

    let json_run = fixture.run("upload", &args);
    let plan = succeeded(&json_run);
    assert_output_has_no_file_bytes("dry run --json", &json_run);
    assert_eq!(plan["kind"], "dryRun");
    assert_eq!(plan["requests"][0]["method"], "POST");
    assert_eq!(plan["requests"][0]["headers"]["Content-Type"], "image/jpeg");
    assert_eq!(plan["requests"][0]["body"], descriptor());

    let mut human_args = vec!["run", "spec.arazzo.json", "upload"];
    human_args.extend_from_slice(&args);
    let human = fixture.cli(&human_args);
    assert!(human.status.success());
    assert_output_has_no_file_bytes("dry run", &human);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        stdout.contains(&format!(
            "  Body: {{\"kind\":\"bytes\",\"size\":38,\"sha256\":\"{JPEG_SHA256}\",\"filename\":\"cat.jpg\"}}"
        )),
        "{stdout}"
    );

    assert!(fixture.take().is_empty(), "a dry run sends nothing");
}

#[test]
fn input_file_is_refused_before_the_run() {
    let fixture = Fixture::new();
    fixture.write("big.bin", &[7; 65]);
    fixture.write("exact.bin", &[7; 64]);
    let cases: [(&str, &[&str], &str); 13] = [
        (
            "undeclared name",
            &["--input-file", "avatar=cat.jpg"],
            "--input-file names input \"avatar\", which workflow \"upload\" does not declare in its inputs",
        ),
        (
            "also given by --input",
            &["--input-file", "photo=cat.jpg", "--input", "photo=x"],
            "input \"photo\" is given by --input-file and also by --input or --input-json; give it once",
        ),
        (
            "also given by --input-json",
            &["--input-json", "photo={\"a\":1}", "--input-file", "photo=cat.jpg"],
            "input \"photo\" is given by --input-file and also by --input or --input-json; give it once",
        ),
        (
            "given twice",
            &["--input-file", "photo=cat.jpg", "--input-file", "photo=cat.jpg"],
            "--input-file names input \"photo\" more than once",
        ),
        (
            "typed schema with a non-UTF-8 file",
            &["--input-file", "typed=cat.jpg"],
            "--input-file input \"typed\": cat.jpg is not valid UTF-8",
        ),
        (
            "enum schema",
            &["--input-file", "listed=cat.jpg"],
            "--input-file input \"listed\": its schema declares enum, which this runtime does not resolve for a file input",
        ),
        (
            "const schema",
            &["--input-file", "fixed=cat.jpg"],
            "--input-file input \"fixed\": its schema declares const, which this runtime does not resolve for a file input",
        ),
        (
            "applicator schema",
            &["--input-file", "combined=cat.jpg"],
            "--input-file input \"combined\": its schema declares allOf, which this runtime does not resolve for a file input",
        ),
        (
            "file over the cap",
            &["--input-file", "photo=big.bin", "--max-input-file-size", "64"],
            "--input-file input \"photo\": big.bin is larger than --max-input-file-size (64 bytes)",
        ),
        (
            "default cap is not unlimited",
            &["--input-file", "photo=cat.jpg", "--max-input-file-size", "37"],
            "--input-file input \"photo\": cat.jpg is larger than --max-input-file-size (37 bytes)",
        ),
        (
            "missing file",
            &["--input-file", "photo=absent.jpg"],
            "--input-file input \"photo\": opening absent.jpg: ",
        ),
        (
            "no path",
            &["--input-file", "photo"],
            "invalid --input-file \"photo\": expected <input-name>=<path>",
        ),
        (
            "empty path",
            &["--input-file", "photo="],
            "invalid --input-file \"photo=\": expected <input-name>=<path>",
        ),
    ];
    for (case, args, expected) in cases {
        let output = fixture.run("upload", args);
        let message = failed(&output, "RUN_INPUT_FILE");
        assert!(message.starts_with(expected), "{case}: {message}");
        assert!(fixture.take().is_empty(), "{case}: a request was sent");
    }

    let unknown_workflow = fixture.run("absent", &["--input-file", "photo=cat.jpg"]);
    let message = failed(&unknown_workflow, "RUN_INPUT_FILE");
    assert!(
        message.contains("workflow \"absent\" does not declare"),
        "{message}"
    );

    let at_the_cap = fixture.run(
        "upload",
        &[
            "--input-file",
            "photo=exact.bin",
            "--max-input-file-size",
            "64",
        ],
    );
    succeeded(&at_the_cap);
    assert_eq!(fixture.take()[0].body, vec![7; 64]);

    let human = fixture.cli(&[
        "run",
        "spec.arazzo.json",
        "upload",
        "--input-file",
        "avatar=cat.jpg",
    ]);
    assert!(!human.status.success());
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8_lossy(&human.stderr).contains("does not declare in its inputs"));
}

#[cfg(unix)]
#[test]
fn the_size_cap_is_enforced_while_reading_not_from_file_metadata() {
    let fixture = Fixture::new();
    let reported = fs::metadata("/dev/zero")
        .unwrap_or_else(|err| panic!("/dev/zero: {err}"))
        .len();
    assert_eq!(reported, 0, "the device reports no size, yet never ends");
    let output = fixture.run(
        "upload",
        &[
            "--input-file",
            "photo=/dev/zero",
            "--max-input-file-size",
            "4096",
        ],
    );
    assert_eq!(
        failed(&output, "RUN_INPUT_FILE"),
        "--input-file input \"photo\": /dev/zero is larger than --max-input-file-size (4096 bytes)"
    );
    assert!(fixture.take().is_empty());
}

#[test]
fn a_document_field_that_looks_like_a_path_is_sent_as_text() {
    let fixture = Fixture::new();
    assert!(Path::new(&fixture.dir).join("cat.jpg").exists());

    let literal = fixture.run("literal", &[]);
    succeeded(&literal);
    let seen = fixture.take();
    assert_eq!(seen[0].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(seen[0].body, b"./cat.jpg");

    let input = fixture.run("caption", &["--input", "caption=./cat.jpg"]);
    succeeded(&input);
    assert_eq!(fixture.take()[0].body, b"./cat.jpg");

    let json_input = fixture.run("caption", &["--input-json", "caption=\"cat.jpg\""]);
    succeeded(&json_input);
    assert_eq!(fixture.take()[0].body, b"cat.jpg");
}

#[test]
fn bytes_fail_closed_wherever_they_cannot_be_sent_or_read() {
    let fixture = Fixture::new();
    let photo = ["--input-file", "photo=cat.jpg"];

    for (workflow, expected) in [
        (
            "untyped",
            "requestBody.payload: the payload carries bytes, so an explicit contentType is required; request not sent",
        ),
        (
            "json",
            "requestBody.payload: contentType \"application/json\" cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded; request not sent",
        ),
        (
            "form",
            "requestBody.payload: contentType \"application/x-www-form-urlencoded\" cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded; request not sent",
        ),
    ] {
        let output = fixture.run(workflow, &photo);
        let message = failed(&output, "RUNTIME_INVALID_REQUEST_BODY");
        assert!(message.contains("step \"upload\""), "{workflow}: {message}");
        assert!(message.ends_with(expected), "{workflow}: {message}");
        assert_output_has_no_file_bytes(workflow, &output);
        assert!(fixture.take().is_empty(), "{workflow}: a request was sent");
    }

    let pointer = fixture.run("pointer", &photo);
    let message = failed(&pointer, "RUNTIME_VALUE_UNAVAILABLE");
    assert!(
        message.ends_with("step \"upload\": requestBody.payload: $inputs.photo#/exif: input \"photo\" is bytes, which a JSON Pointer cannot read; request not sent"),
        "{message}"
    );
    assert!(fixture.take().is_empty());

    let echo = fixture.run("echo", &photo);
    assert_eq!(succeeded(&echo)["outputs"], json!({"id":"p1"}));
    assert_output_has_no_file_bytes("echo", &echo);
    assert_eq!(fixture.take()[0].body, jpeg());
}

#[test]
fn a_required_bytes_input_passes_strict_input_validation() {
    let fixture = Fixture::new();
    let output = fixture.run(
        "required",
        &["--input-file", "photo=cat.jpg", "--strict-inputs"],
    );
    assert_eq!(succeeded(&output)["outputs"]["id"], "p1");
    assert_eq!(fixture.take()[0].body, jpeg());
}
