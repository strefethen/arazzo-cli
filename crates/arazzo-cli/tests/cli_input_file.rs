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

const DATA: [u8; 4] = [0xFB, 0xFF, 0xBF, 0x00];
const DATA_SHA256: &str = "2f6ac74596674225e68d78775cfdf1b2bacfbcc181030eddee24a21595c1cc78";
const DATA_BASE64: &str = "+/+/AA==";
const DATA_BASE64URL: &str = "-_-_AA==";
const NOTE: &str = "héllo; wörld\r\n\t";

type Seen = (Option<String>, Vec<u8>);

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
            "arazzo-input-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("folder"))
            .unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));

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
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push((content_type, body));
                let header = Header::from_bytes("Content-Type", "application/json")
                    .unwrap_or_else(|()| panic!("header"));
                let _ = request.respond(TinyResponse::from_string("{}").with_header(header));
            }
        });

        let fixture = Self {
            dir,
            requests,
            stop,
            worker: Some(worker),
        };
        fixture.write("data.bin", &DATA);
        fixture.write("a;b.bin", &DATA);
        fixture.write("note.txt", NOTE.as_bytes());
        fixture.write(
            "api.openapi.json",
            json!({"openapi":"3.0.3", "info":{"title":"uploads", "version":"1"},
                "servers":[{"url":base_url}],
                "paths":{"/upload":{"post":{"operationId":"upload",
                    "responses":{"200":{"description":"ok"}}}}}})
            .to_string()
            .as_bytes(),
        );
        fixture.write("spec.arazzo.json", run_spec().to_string().as_bytes());
        fixture.write("suite.arazzo.json", test_spec().to_string().as_bytes());
        fixture
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.dir.join(name);
        fs::write(&path, bytes).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn dry_run(&self, workflow: &str, args: &[&str]) -> Output {
        let mut all = vec!["--json", "run", "spec.arazzo.json", workflow, "--dry-run"];
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

fn inputs() -> Value {
    json!({"type":"object", "properties":{
        "raw":{"description":"bytes"},
        "png":{"contentMediaType":"image/png"},
        "text":{"type":"string"},
        "binaryText":{"type":"string", "format":"binary"},
        "b64":{"type":"string", "contentEncoding":"base64", "contentMediaType":"image/png"},
        "b64url":{"type":"string", "contentEncoding":"base64url"},
        "printable":{"type":"string", "contentEncoding":"quoted-printable"},
        "count":{"type":"integer"},
        "listing":{"type":"array"},
        "referenced":{"$ref":"#/$defs/photo"},
        "listed":{"enum":["a"]},
        "fixed":{"const":"a"},
        "combined":{"anyOf":[{}]},
        "textCombined":{"type":"string", "allOf":[{}]},
        "encodedBytes":{"contentEncoding":"base64"},
        "scan":{"writeOnly":true},
        "defaulted":{"default":"fallback"},
        "binaryBytes":{"format":"binary"}
    }})
}

fn send(id: &str, content_type: &str, input: &str) -> Value {
    json!({"workflowId":id, "inputs":inputs(), "steps":[{"stepId":"send",
        "operationId":"upload",
        "requestBody":{"contentType":content_type, "payload":format!("$inputs.{input}")},
        "successCriteria":[{"condition":"$statusCode == 200"}]}]})
}

fn run_spec() -> Value {
    let mut required = send("required", "application/octet-stream", "raw");
    required["inputs"]["required"] = json!(["raw"]);
    let mut workflows = vec![required];
    for input in ["raw", "png", "scan", "defaulted", "binaryBytes"] {
        workflows.push(send(input, "application/octet-stream", input));
    }
    for input in ["text", "binaryText", "b64", "b64url"] {
        workflows.push(send(input, "text/plain", input));
    }
    json!({"arazzo":"1.1.0", "info":{"title":"input files", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":workflows})
}

fn test_spec() -> Value {
    let declared = |id: &str, name: &str, content_type: &str, schema: Value| {
        json!({"workflowId":id,
            "inputs":{"type":"object", "required":[name], "properties":{ (name): schema }},
            "steps":[{"stepId":"send", "operationId":"upload",
                "requestBody":{"contentType":content_type, "payload":format!("$inputs.{name}")},
                "successCriteria":[{"condition":"$statusCode == 200"}]}]})
    };
    json!({"arazzo":"1.1.0", "info":{"title":"input file suite", "version":"1"},
    "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
    "workflows":[
        declared("bytes", "photo", "application/octet-stream", json!({})),
        declared("encoded", "encoded", "text/plain",
            json!({"type":"string", "contentEncoding":"base64url"})),
        {"workflowId":"neither", "steps":[{"stepId":"send", "operationId":"upload",
            "successCriteria":[{"condition":"$statusCode == 200"}]}]}
    ]})
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

fn body(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = stdout_json(output);
    assert_eq!(plan["kind"], "dryRun", "{plan}");
    plan["requests"][0]["body"].clone()
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

fn bytes(media_type: Option<&str>, filename: &str) -> Value {
    let mut descriptor = json!({"kind":"bytes", "size":4, "sha256":DATA_SHA256});
    if let Some(media_type) = media_type {
        descriptor["mediaType"] = json!(media_type);
    }
    descriptor["filename"] = json!(filename);
    descriptor
}

#[test]
fn a_schema_with_no_type_loads_bytes_with_media_type_and_filename_precedence() {
    let fixture = Fixture::new();
    let cases: [(&str, &str, Value); 7] = [
        ("raw", "raw=data.bin", bytes(None, "data.bin")),
        ("png", "png=data.bin", bytes(Some("image/png"), "data.bin")),
        (
            "png",
            "png=data.bin;type=image/webp",
            bytes(Some("image/webp"), "data.bin"),
        ),
        (
            "raw",
            "raw=data.bin;filename=upload.jpg",
            bytes(None, "upload.jpg"),
        ),
        (
            "png",
            "png=data.bin;type=image/jpeg;filename=up.jpg",
            bytes(Some("image/jpeg"), "up.jpg"),
        ),
        (
            "png",
            "png=data.bin;filename=up.jpg;type=image/jpeg",
            bytes(Some("image/jpeg"), "up.jpg"),
        ),
        (
            "binaryBytes",
            "binaryBytes=data.bin",
            bytes(None, "data.bin"),
        ),
    ];
    for (workflow, flag, expected) in cases {
        assert_eq!(
            body(&fixture.dry_run(workflow, &["--input-file", flag])),
            expected,
            "{flag}"
        );
    }

    let nested = fixture.dir.join("data.bin");
    let flag = format!("raw={}", nested.display());
    assert_eq!(
        body(&fixture.dry_run("raw", &["--input-file", &flag])),
        bytes(None, "data.bin"),
        "filename defaults to the basename, never the directory"
    );
}

#[test]
fn a_path_with_an_unrecognized_semicolon_is_read_literally() {
    let fixture = Fixture::new();
    assert_eq!(
        body(&fixture.dry_run("raw", &["--input-file", "raw=a;b.bin"])),
        bytes(None, "a;b.bin")
    );
    assert_eq!(
        body(&fixture.dry_run(
            "png",
            &["--input-file", "png=a;b.bin;type=text/plain; charset=x"]
        )),
        bytes(Some("text/plain; charset=x"), "a;b.bin")
    );
    let missing = fixture.dry_run("raw", &["--input-file", "raw=data.bin;Type=x"]);
    let message = failed(&missing, "RUN_INPUT_FILE");
    assert!(
        message.starts_with("--input-file input \"raw\": opening data.bin;Type=x: "),
        "suffixes are case-sensitive: {message}"
    );
}

#[test]
fn a_string_schema_loads_base64_base64url_or_strict_utf8_text() {
    let fixture = Fixture::new();
    for (workflow, flag, expected) in [
        ("b64", "b64=data.bin", DATA_BASE64),
        ("b64url", "b64url=data.bin", DATA_BASE64URL),
        ("text", "text=note.txt", NOTE),
        ("binaryText", "binaryText=note.txt", NOTE),
    ] {
        assert_eq!(
            body(&fixture.dry_run(workflow, &["--input-file", flag])),
            json!(expected),
            "{flag}"
        );
    }
}

#[test]
fn every_refused_row_fails_before_the_run_naming_the_input() {
    let fixture = Fixture::new();
    let cases: [(&str, &str, &str); 17] = [
        (
            "count=data.bin",
            "count",
            "its schema declares type \"integer\"; a file loads only for a schema with no type (bytes) or type \"string\"",
        ),
        (
            "listing=data.bin",
            "listing",
            "its schema declares type \"array\"",
        ),
        (
            "printable=data.bin",
            "printable",
            "its schema declares contentEncoding \"quoted-printable\"; a file input supports base64 and base64url",
        ),
        (
            "referenced=data.bin",
            "referenced",
            "its schema declares $ref, which this runtime does not resolve for a file input",
        ),
        (
            "listed=data.bin",
            "listed",
            "its schema declares enum, which this runtime does not resolve for a file input",
        ),
        (
            "fixed=data.bin",
            "fixed",
            "its schema declares const, which this runtime does not resolve for a file input",
        ),
        (
            "combined=data.bin",
            "combined",
            "its schema declares anyOf, which this runtime does not resolve for a file input",
        ),
        (
            "textCombined=note.txt",
            "textCombined",
            "its schema declares allOf, which this runtime does not resolve for a file input",
        ),
        (
            "encodedBytes=data.bin",
            "encodedBytes",
            "its schema declares contentEncoding without type \"string\"",
        ),
        (
            "text=data.bin",
            "text",
            "data.bin is not valid UTF-8 (at byte 0), and the input's schema declares type \"string\" with no contentEncoding",
        ),
        (
            "text=note.txt;type=text/plain",
            "text",
            "the ;type= and ;filename= suffixes apply only to a bytes input",
        ),
        (
            "raw=folder",
            "raw",
            "folder is a directory, not a file",
        ),
        (
            "avatar=data.bin",
            "avatar",
            "",
        ),
        (
            "raw=data.bin;type=",
            "raw",
            "",
        ),
        (
            "raw=data.bin;type=a;type=b",
            "raw",
            "",
        ),
        (
            "raw=;filename=x",
            "raw",
            "",
        ),
        (
            "raw",
            "raw",
            "",
        ),
    ];
    for (flag, input, reason) in cases {
        let output = fixture.dry_run("raw", &["--input-file", flag]);
        let message = failed(&output, "RUN_INPUT_FILE");
        let named = message.contains(&format!("\"{input}\""))
            || message.starts_with(&format!("invalid --input-file \"{input}"));
        assert!(named, "{flag}: {message}");
        assert!(message.contains(reason), "{flag}: {message}");
    }

    let undeclared = failed(
        &fixture.dry_run("raw", &["--input-file", "avatar=data.bin"]),
        "RUN_INPUT_FILE",
    );
    assert_eq!(
        undeclared,
        "--input-file names input \"avatar\", which workflow \"raw\" does not declare in its inputs"
    );
    for (flag, value) in [("--input", "raw=x"), ("--input-json", "raw=\"x\"")] {
        let message = failed(
            &fixture.dry_run("raw", &["--input-file", "raw=data.bin", flag, value]),
            "RUN_INPUT_FILE",
        );
        assert_eq!(
            message,
            "input \"raw\" is given by --input-file and also by --input or --input-json; give it once"
        );
    }
    for (flag, expected) in [
        ("raw=data.bin;type=", ";type= needs a value"),
        (
            "raw=data.bin;type=a;type=b",
            ";type= is given more than once",
        ),
        ("raw=;filename=x", "expected <input-name>=<path>"),
    ] {
        let message = failed(
            &fixture.dry_run("raw", &["--input-file", flag]),
            "RUN_INPUT_FILE",
        );
        assert!(message.contains(expected), "{flag}: {message}");
    }
    assert!(fixture.take().is_empty(), "a refused input sends nothing");
}

#[test]
fn a_bytes_schema_default_is_never_bytes() {
    let fixture = Fixture::new();
    assert_eq!(body(&fixture.dry_run("defaulted", &[])), json!("fallback"));
    assert_eq!(
        body(&fixture.dry_run("defaulted", &["--input-file", "defaulted=data.bin"])),
        bytes(None, "data.bin")
    );
}

#[test]
fn a_required_input_supplied_only_by_input_file_passes_strict_validation() {
    let fixture = Fixture::new();
    let output = fixture.cli(&[
        "--json",
        "run",
        "spec.arazzo.json",
        "required",
        "--strict-inputs",
        "--input-file",
        "raw=data.bin",
    ]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("warning: input"));
    let seen = fixture.take();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, DATA);

    let missing = fixture.cli(&[
        "--json",
        "run",
        "spec.arazzo.json",
        "required",
        "--strict-inputs",
    ]);
    let message = failed(&missing, "RUNTIME_INPUT_VALIDATION");
    assert!(message.contains("\"raw\""), "{message}");
}

#[test]
fn a_write_only_blob_input_renders_the_redacted_marker() {
    let fixture = Fixture::new();
    let output = fixture.dry_run("scan", &["--input-file", "scan=data.bin", "--verbose"]);
    assert_eq!(body(&output), json!({"kind":"bytes", "redacted":true}));
    let all = [output.stdout, output.stderr].concat();
    let text = String::from_utf8_lossy(&all);
    assert!(!text.contains(DATA_SHA256), "{text}");
    assert!(!text.contains("data.bin"), "{text}");
    assert!(
        text.contains("Input files: {\"scan\": \"<bytes redacted>\"}"),
        "{text}"
    );
}

#[test]
fn the_test_command_loads_input_files_per_workflow() {
    let fixture = Fixture::new();
    let output = fixture.cli(&[
        "--json",
        "test",
        "suite.arazzo.json",
        "--strict-inputs",
        "--input-file",
        "photo=data.bin",
        "--input-file",
        "encoded=data.bin",
    ]);
    let result = stdout_json(&output);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["summary"]["passed"], 3, "{result}");
    let mut seen = fixture.take();
    seen.sort();
    assert_eq!(
        seen,
        [
            (None, Vec::new()),
            (Some("application/octet-stream".to_owned()), DATA.to_vec()),
            (
                Some("text/plain".to_owned()),
                DATA_BASE64URL.as_bytes().to_vec()
            ),
        ]
    );

    let refused = fixture.cli(&[
        "--json",
        "test",
        "suite.arazzo.json",
        "--input-file",
        "encoded=data.bin;type=image/png",
    ]);
    let result = stdout_json(&refused);
    assert!(!refused.status.success());
    assert_eq!(result["summary"]["suiteErrors"], 1, "{result}");
    let error = result["suites"][0]["error"].as_str().unwrap_or_default();
    assert!(
        error.starts_with("workflow \"encoded\": --input-file input \"encoded\": the ;type= and ;filename= suffixes apply only to a bytes input"),
        "{error}"
    );

    let collision = fixture.cli(&[
        "--json",
        "test",
        "suite.arazzo.json",
        "--input-file",
        "photo=data.bin",
        "--input",
        "photo=x",
    ]);
    assert_eq!(
        failed(&collision, "TEST_INPUT_FILE"),
        "input \"photo\" is given by --input-file and also by --input or --input-json; give it once"
    );
    let unreadable = fixture.cli(&[
        "--json",
        "test",
        "suite.arazzo.json",
        "--input-file",
        "photo=folder",
    ]);
    assert!(failed(&unreadable, "TEST_INPUT_FILE").ends_with("folder is a directory, not a file"));
    assert!(fixture.take().is_empty(), "a refused suite sends nothing");
}
