#![forbid(unsafe_code)]

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/http-body");
const SUCCESS: &str = "success.arazzo.yaml";
const FAILURES: &str = "failures.arazzo.yaml";
const JPEG_MARKER: &str = "PLANTED-SYSTEM-JPEG-41c9!";
const OTHER_MARKER: &str = "PLANTED-SYSTEM-OTHER-41c9!";
const PNG_MARKER: &str = "PLANTED-SYSTEM-PNG-41c9!";
const SECRET: &str = "FAKE-LATIN-CANARY-41c9";
const PHOTO: [&str; 2] = ["--input-file", "photo=cat.jpg"];
const PHOTO_PART: &str = "photo=cat.jpg;type=image/jpeg;filename=c.jpg";
const TAGS: &str = r#"tags=["a","b"]"#;
const GZIP_JSON: &[u8] = br#"{"name":"gzip-cat","sizes":[1,2]}"#;
const OUTPUT_TARGET: &str = "out.bin";

fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    bytes.extend_from_slice(JPEG_MARKER.as_bytes());
    bytes.extend_from_slice(&[0x00, 0xFF, 0xFE, 0x80, 0xC3, 0x28, 0x0D, 0x0A, 0xFF, 0xD9]);
    bytes
}

fn other_jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
    bytes.extend_from_slice(OTHER_MARKER.as_bytes());
    bytes.extend_from_slice(&[0x00, 0xFF, 0xD9]);
    bytes
}

fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec();
    bytes.extend_from_slice(PNG_MARKER.as_bytes());
    bytes.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x01]);
    bytes
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(u32::MAX, |crc, byte| {
        (0..8).fold(crc ^ u32::from(*byte), |crc, _| {
            if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            }
        })
    })
}

fn gzip_stored(bytes: &[u8]) -> Vec<u8> {
    let length = u16::try_from(bytes.len()).unwrap_or_else(|_| panic!("one stored block"));
    let mut out = vec![0x1F, 0x8B, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xFF, 0x01];
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&(!length).to_le_bytes());
    out.extend_from_slice(bytes);
    out.extend_from_slice(&crc32(bytes).to_le_bytes());
    out.extend_from_slice(&u32::from(length).to_le_bytes());
    out
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

#[derive(Debug, Clone)]
struct Seen {
    line: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

#[derive(Default)]
struct Photos {
    last: Option<u64>,
}

impl Photos {
    fn put(&mut self, body: &[u8]) -> u16 {
        let mut hasher = DefaultHasher::new();
        body.hash(&mut hasher);
        let hash = hasher.finish();
        match self.last {
            None => {
                self.last = Some(hash);
                201
            }
            Some(last) if last == hash => 204,
            Some(_) => 409,
        }
    }
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

fn reply(status: u16, content_type: Option<&'static str>, body: &[u8]) -> Reply {
    Reply {
        status,
        headers: content_type
            .map(|value| vec![("Content-Type", value)])
            .unwrap_or_default(),
        body: body.to_vec(),
    }
}

fn route(path: &str, body: &[u8], photos: &mut Photos) -> Reply {
    let latin1 = |text: &[u8]| reply(200, Some("text/plain; charset=iso-8859-1"), text);
    match path {
        "/upload" => reply(200, Some("application/json"), br#"{"id":"p1"}"#),
        "/redirect" => Reply {
            status: 307,
            headers: vec![("Location", "/upload")],
            body: Vec::new(),
        },
        "/photo" => reply(photos.put(body), None, b""),
        "/cat.png" => reply(200, Some("image/png"), &png()),
        "/latin1" => latin1(b"caf\xE9 cr\xE8me br\xFBl\xE9e"),
        "/secret" => latin1(&latin1_secret()),
        "/gzip" => Reply {
            status: 200,
            headers: vec![
                ("Content-Type", "application/json"),
                ("Content-Encoding", "gzip"),
            ],
            body: gzip_stored(GZIP_JSON),
        },
        "/bad-json" => reply(200, Some("application/json"), b"{not json"),
        "/bad-charset" => reply(200, Some("text/plain; charset=utf-8"), b"caf\xE9"),
        "/unknown-charset" => reply(200, Some("text/plain; charset=x-no-such-charset"), b"cafe"),
        "/compress" => Reply {
            status: 200,
            headers: vec![
                ("Content-Type", "application/json"),
                ("Content-Encoding", "compress"),
            ],
            body: b"\x1F\x9D{}".to_vec(),
        },
        "/corrupt-gzip" => Reply {
            status: 200,
            headers: vec![
                ("Content-Type", "application/json"),
                ("Content-Encoding", "gzip"),
            ],
            body: b"\x1F\x8B\x08\x00not a deflate stream".to_vec(),
        },
        _ => reply(404, Some("text/plain"), b"no such route"),
    }
}

fn latin1_secret() -> Vec<u8> {
    let mut bytes = b"caf\xE9 token=".to_vec();
    bytes.extend_from_slice(SECRET.as_bytes());
    bytes
}

fn openapi(base_url: &str) -> Value {
    let operations = [
        ("/upload", "post", "upload"),
        ("/redirect", "post", "redirected"),
        ("/photo", "put", "putPhoto"),
        ("/cat.png", "get", "download"),
        ("/latin1", "get", "latin1"),
        ("/secret", "get", "secret"),
        ("/gzip", "get", "gzipped"),
        ("/bad-json", "get", "badJson"),
        ("/bad-charset", "get", "badCharset"),
        ("/unknown-charset", "get", "unknownCharset"),
        ("/compress", "get", "compressed"),
        ("/corrupt-gzip", "get", "corruptGzip"),
    ];
    let mut paths = serde_json::Map::new();
    for (path, method, id) in operations {
        paths.insert(
            path.to_owned(),
            json!({ (method): {"operationId": id, "responses": {"200": {"description": "ok"}}} }),
        );
    }
    json!({"openapi": "3.0.3", "info": {"title": "http body system", "version": "1"},
        "servers": [{"url": base_url}], "paths": paths})
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
            "arazzo-http-body-system-{}-{}",
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
            let mut photos = Photos::default();
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
                let path = request
                    .url()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let answer = route(&path, &body, &mut photos);
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen {
                        line: format!("{} {path}", request.method()),
                        content_type,
                        body,
                    });
                let mut response = TinyResponse::from_data(answer.body)
                    .with_status_code(StatusCode(answer.status));
                for (name, value) in answer.headers {
                    response.add_header(
                        Header::from_bytes(name, value)
                            .unwrap_or_else(|()| panic!("header {name}")),
                    );
                }
                let _ = request.respond(response);
            }
        });
        let fixture = Self {
            dir,
            requests,
            stop,
            worker: Some(worker),
        };
        for document in [SUCCESS, FAILURES] {
            let source = Path::new(FIXTURES).join(document);
            let bytes = fs::read(&source)
                .unwrap_or_else(|err| panic!("reading {}: {err}", source.display()));
            fixture.write(document, &bytes);
        }
        fixture.write(
            "api.openapi.json",
            openapi(&base_url).to_string().as_bytes(),
        );
        fixture.write("cat.jpg", &jpeg());
        fixture.write("other.jpg", &other_jpeg());
        fixture.write("big.bin", &[7; 65]);
        fixture.write("notes.txt", b"plain notes");
        fixture
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.path(name);
        fs::write(&path, bytes).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn read(&self, name: &str) -> Vec<u8> {
        let path = self.path(name);
        fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
    }

    fn trace(&self, name: &str) -> Value {
        serde_json::from_slice(&self.read(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn run(&self, document: &str, workflow: &str, args: &[&str]) -> Output {
        let mut all = vec!["--json", "run", document, workflow];
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

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not one JSON value: {err}; stdout={} stderr={}",
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
    assert!(
        !output.status.success(),
        "the command must fail: stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let error = stdout_json(output);
    assert_eq!(error["kind"], "error", "{error}");
    assert_eq!(error["code"], code, "{error}");
    error["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error text: {error}"))
        .to_owned()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn assert_no_body_bytes(label: &str, text: &[u8]) {
    for (name, body, marker) in [
        ("JPEG", jpeg(), JPEG_MARKER),
        ("other JPEG", other_jpeg(), OTHER_MARKER),
        ("PNG", png(), PNG_MARKER),
    ] {
        assert!(!contains(text, &body), "{label}: the {name} bytes leaked");
        assert!(
            !contains(text, marker.as_bytes()),
            "{label}: planted {name} bytes leaked"
        );
        for offset in 0..3 {
            let encoded = base64(&body[offset..]);
            let stable = &encoded[..encoded.len() - 4];
            assert!(
                !contains(text, stable.as_bytes()),
                "{label}: base64 of the {name} leaked"
            );
        }
    }
    assert!(
        !contains(text, b"boundary=arazzo-"),
        "{label}: the wire boundary leaked"
    );
}

fn assert_output_has_no_body_bytes(label: &str, output: &Output) {
    assert_no_body_bytes(&format!("{label} stdout"), &output.stdout);
    assert_no_body_bytes(&format!("{label} stderr"), &output.stderr);
}

fn descriptor(bytes: &[u8], media_type: Option<&str>, filename: Option<&str>) -> Value {
    arazzo_runtime::BytesDescriptor::of(
        bytes,
        media_type.map(str::to_owned),
        filename.map(str::to_owned),
    )
    .to_value()
}

#[derive(Debug, PartialEq)]
struct Part {
    name: String,
    filename: Option<String>,
    media_type: Option<String>,
    bytes: Vec<u8>,
}

fn text_part(name: &str, value: &str) -> Part {
    Part {
        name: name.to_owned(),
        filename: None,
        media_type: None,
        bytes: value.as_bytes().to_vec(),
    }
}

fn parse_multipart(seen: &Seen) -> Vec<Part> {
    let content_type = seen.content_type.clone().unwrap_or_default();
    let boundary = multer::parse_boundary(&content_type)
        .unwrap_or_else(|err| panic!("{content_type:?}: {err}"));
    let body = seen.body.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap_or_else(|err| panic!("runtime: {err}"));
    runtime.block_on(async move {
        let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) });
        let mut multipart = multer::Multipart::new(stream, boundary);
        let mut parts = Vec::new();
        while let Some(field) = multipart
            .next_field()
            .await
            .unwrap_or_else(|err| panic!("parsing multipart: {err}"))
        {
            let name = field.name().unwrap_or_default().to_owned();
            let filename = field.file_name().map(str::to_owned);
            let media_type = field.content_type().map(ToString::to_string);
            let bytes = field
                .bytes()
                .await
                .unwrap_or_else(|err| panic!("reading part: {err}"))
                .to_vec();
            parts.push(Part {
                name,
                filename,
                media_type,
                bytes,
            });
        }
        parts
    })
}

const SUCCESS_RUNS: [(&str, &[&str]); 7] = [
    ("upload-jpeg", &PHOTO),
    (
        "reupload",
        &[
            "--input-file",
            "photo=cat.jpg",
            "--input-file",
            "other=other.jpg",
        ],
    ),
    (
        "multipart",
        &[
            "--input-file",
            PHOTO_PART,
            "--input",
            "caption=cat",
            "--input-json",
            TAGS,
        ],
    ),
    ("roundtrip", &[]),
    ("decoding", &[]),
    ("redirected", &PHOTO),
    ("secret", &[]),
];

fn record(fixture: &Fixture, workflow: &str, args: &[&str]) -> (Output, String) {
    let trace = format!("trace-{workflow}.json");
    let mut all = args.to_vec();
    all.extend_from_slice(&["--trace", trace.as_str(), "--verbose"]);
    let output = fixture.run(SUCCESS, workflow, &all);
    succeeded(&output);
    assert!(
        !fixture.take().is_empty(),
        "{workflow}: the live run is sent"
    );
    (output, trace)
}

#[test]
fn item_1_a_jpeg_input_file_reaches_the_server_byte_identical_as_image_jpeg() {
    let fixture = Fixture::new();
    let output = fixture.run(SUCCESS, "upload-jpeg", &PHOTO);
    assert_eq!(
        succeeded(&output),
        json!({"kind":"success", "outputs":{"id":"p1",
            "sent":descriptor(&jpeg(), None, Some("cat.jpg"))}})
    );
    let seen = fixture.take();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].line, "POST /upload");
    assert_eq!(seen[0].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(seen[0].body[..3], [0xFF, 0xD8, 0xFF]);
    assert_eq!(seen[0].body, jpeg());
}

#[test]
fn item_2_identical_and_different_reuploads_observe_204_and_409() {
    let fixture = Fixture::new();
    let output = fixture.run(SUCCESS, "reupload", SUCCESS_RUNS[1].1);
    assert_eq!(
        succeeded(&output)["outputs"],
        json!({"first":201, "identical":204, "different":409})
    );
    let seen = fixture.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        ["PUT /photo", "PUT /photo", "PUT /photo"]
    );
    assert_eq!(seen[0].body, jpeg());
    assert_eq!(seen[1].body, jpeg());
    assert_eq!(seen[2].body, other_jpeg());
    for request in &seen {
        assert_eq!(request.content_type.as_deref(), Some("image/jpeg"));
    }
}

#[test]
fn item_3_a_multipart_upload_recovers_every_part_with_the_file_last() {
    let fixture = Fixture::new();
    let output = fixture.run(SUCCESS, "multipart", SUCCESS_RUNS[2].1);
    assert_eq!(succeeded(&output)["outputs"], json!({"id":"p1"}));
    let seen = fixture.take();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0]
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")),
        "{:?}",
        seen[0].content_type
    );
    assert_eq!(
        parse_multipart(&seen[0]),
        vec![
            text_part("caption", "cat"),
            text_part("tags", "a"),
            text_part("tags", "b"),
            Part {
                name: "photo".to_owned(),
                filename: Some("c.jpg".to_owned()),
                media_type: Some("image/jpeg".to_owned()),
                bytes: jpeg(),
            },
        ]
    );
}

#[test]
fn item_4_a_downloaded_png_is_reuploaded_and_written_byte_identical() {
    let fixture = Fixture::new();
    let output = fixture.run(SUCCESS, "roundtrip", &["--output-file", "png=copy.png"]);
    assert_eq!(
        succeeded(&output)["outputs"],
        json!({"id":"p1", "png":descriptor(&png(), Some("image/png"), None)})
    );
    let seen = fixture.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        ["GET /cat.png", "POST /upload"]
    );
    assert_eq!(seen[1].content_type.as_deref(), Some("image/png"));
    assert_eq!(seen[1].body, png());
    assert_eq!(fixture.read("copy.png"), png());
}

#[test]
fn item_5_latin1_and_gzip_responses_decode_and_a_307_resends_identical_bytes() {
    let fixture = Fixture::new();
    let decoded = fixture.run(SUCCESS, "decoding", &[]);
    assert_eq!(
        succeeded(&decoded)["outputs"],
        json!({"text":"caf\u{e9} cr\u{e8}me br\u{fb}l\u{e9}e", "name":"gzip-cat", "sizes":[1, 2]})
    );
    assert_eq!(fixture.take().len(), 2);

    let redirected = fixture.run(SUCCESS, "redirected", &PHOTO);
    assert_eq!(succeeded(&redirected)["outputs"], json!({"id":"p1"}));
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
fn item_6_every_run_replays_its_own_trace_to_the_same_outcomes_and_descriptors() {
    let fixture = Fixture::new();
    for (workflow, args) in SUCCESS_RUNS {
        let (live, trace_name) = record(&fixture, workflow, args);
        let live = stdout_json(&live);
        let trace = fixture.trace(&trace_name);
        let steps = trace["steps"]
            .as_array()
            .unwrap_or_else(|| panic!("{workflow}: trace steps"));
        for step in steps {
            assert!(
                step["criteria"]
                    .as_array()
                    .is_none_or(|criteria| criteria.iter().all(|c| c["result"] == true)),
                "{workflow}: {step}"
            );
        }

        let replay = fixture.cli(&["--json", "replay", trace_name.as_str()]);
        assert!(
            fixture.take().is_empty(),
            "{workflow}: replay sent a request"
        );
        let replayed = succeeded(&replay);
        assert_eq!(replayed["kind"], "success", "{workflow}: {replayed}");
        assert_eq!(replayed["outputs"], live["outputs"], "{workflow}");
        assert_eq!(
            replayed["requestsChecked"],
            json!(steps.len()),
            "{workflow}: {replayed}"
        );
        assert_output_has_no_body_bytes(workflow, &replay);
    }

    let multipart = fixture.trace("trace-multipart.json");
    assert_eq!(
        multipart["steps"][0]["request"]["bodyBlobs"]["/photo"],
        descriptor(&jpeg(), Some("image/jpeg"), Some("c.jpg"))
    );
    let upload = fixture.trace("trace-upload-jpeg.json");
    assert_eq!(
        upload["steps"][0]["request"]["bodyBlobs"][""],
        descriptor(&jpeg(), None, Some("cat.jpg"))
    );
    let roundtrip = fixture.trace("trace-roundtrip.json");
    let png_descriptor = descriptor(&png(), Some("image/png"), None);
    assert_eq!(
        roundtrip["steps"][0]["response"]["bodyBlob"],
        png_descriptor
    );
    assert_eq!(
        roundtrip["steps"][1]["request"]["bodyBlobs"][""],
        png_descriptor
    );
}

#[test]
fn item_7_traces_and_json_output_hold_no_body_bytes_and_redact_a_latin1_secret() {
    let fixture = Fixture::new();
    for (workflow, args) in SUCCESS_RUNS {
        let (output, trace_name) = record(&fixture, workflow, args);
        assert_output_has_no_body_bytes(workflow, &output);
        let raw = fixture.read(&trace_name);
        assert_no_body_bytes(&trace_name, &raw);
        for text in [&output.stdout, &output.stderr, &raw] {
            assert!(
                !contains(text, SECRET.as_bytes()),
                "{workflow}: secret leaked"
            );
            assert!(
                !contains(text, "\u{fffd}".as_bytes()),
                "{workflow}: lossy text"
            );
        }
    }

    let secret = fixture.trace("trace-secret.json");
    let response = &secret["steps"][0]["response"];
    assert_eq!(response["mediaType"], "text/plain;charset=iso-8859-1");
    assert_eq!(response["body"], "caf\u{e9} token=[REDACTED]");
    assert_eq!(
        secret["steps"][0]["criteria"][1]["result"], true,
        "the regex criterion read the decoded text: {secret}"
    );
}

struct FailurePath {
    document: &'static str,
    workflow: &'static str,
    args: &'static [&'static str],
    code: &'static str,
    step: Option<&'static str>,
    fragments: &'static [&'static str],
    requests: usize,
}

fn assert_failure_path(row: &FailurePath) {
    let fixture = Fixture::new();
    let output = fixture.run(row.document, row.workflow, row.args);
    let message = failed(&output, row.code);
    if let Some(step) = row.step {
        assert!(
            message.starts_with(&format!("step {step}: ")),
            "{}: the diagnostic names no step: {message}",
            row.workflow
        );
    }
    for fragment in row.fragments {
        assert!(
            message.contains(fragment),
            "{}: {fragment:?} not in {message:?}",
            row.workflow
        );
    }
    assert_eq!(
        fixture.take().len(),
        row.requests,
        "{}: requests the server received",
        row.workflow
    );
    assert_output_has_no_body_bytes(row.workflow, &output);
    assert!(
        !fixture.path(OUTPUT_TARGET).exists(),
        "{}: a file was written",
        row.workflow
    );
}

macro_rules! failure_paths {
    ($($name:ident => $row:expr;)+) => {
        $(
            #[test]
            fn $name() {
                assert_failure_path(&$row);
            }
        )+
    };
}

const BYTES_PHOTO: &[&str] = &["--input-file", "photo=cat.jpg", "--input", "caption=cat"];
const VALUE_UNAVAILABLE: &str = "RUNTIME_VALUE_UNAVAILABLE";
const INVALID_REQUEST_BODY: &str = "RUNTIME_INVALID_REQUEST_BODY";
const INPUT_FILE: &str = "RUN_INPUT_FILE";
const OUTPUT_FILE: &str = "RUN_OUTPUT_FILE";
const NOT_SENT: &str = "; request not sent";

const fn failure(
    workflow: &'static str,
    args: &'static [&'static str],
    code: &'static str,
    step: Option<&'static str>,
    fragments: &'static [&'static str],
    requests: usize,
) -> FailurePath {
    FailurePath {
        document: FAILURES,
        workflow,
        args,
        code,
        step,
        fragments,
        requests,
    }
}

failure_paths! {
    failure_json_pointer_into_bytes => failure(
        "pointer", BYTES_PHOTO, VALUE_UNAVAILABLE, Some("upload"),
        &["$inputs.photo#/exif: input \"photo\" is bytes, which a JSON Pointer cannot read", NOT_SENT],
        0);
    failure_interpolation_of_bytes => failure(
        "interpolation", BYTES_PHOTO, VALUE_UNAVAILABLE, Some("upload"),
        &["requestBody.payload: $inputs.photo: input \"photo\" is bytes, which only a whole request body payload or a whole output can carry", NOT_SENT],
        0);
    failure_criterion_context_reaching_bytes => failure(
        "criterion-context", &[], VALUE_UNAVAILABLE, Some("download"),
        &["successCriteria[0]: $response.body: response body is bytes (image/png)"],
        1);
    failure_criterion_condition_operand_reaching_bytes => failure(
        "criterion-operand", &[], VALUE_UNAVAILABLE, Some("download"),
        &["successCriteria[0]: $response.body: response body is bytes (image/png)"],
        1);
    failure_selector_context_in_a_payload_reaching_bytes => failure(
        "selector-context", BYTES_PHOTO, VALUE_UNAVAILABLE, Some("upload"),
        &["requestBody.payload: $inputs.photo: input \"photo\" is bytes", NOT_SENT],
        0);
    failure_selector_context_in_an_output_reaching_bytes => failure(
        "selector-output", &[], VALUE_UNAVAILABLE, Some("download"),
        &["output \"width\": $response.body: response body is bytes (image/png)"],
        1);
    failure_operation_parameter_reaching_bytes => failure(
        "parameter", BYTES_PHOTO, VALUE_UNAVAILABLE, Some("download"),
        &["parameters[0](query:q): $inputs.photo: input \"photo\" is bytes", NOT_SENT],
        0);
    failure_replacement_target_reaching_bytes => failure(
        "replacement-target", BYTES_PHOTO, VALUE_UNAVAILABLE, Some("upload"),
        &["requestBody.replacements[0].target: requestBody.payload/photo is bytes, which a replacement target cannot read", NOT_SENT],
        0);
    failure_bytes_without_a_content_type => failure(
        "bytes-without-content-type", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["requestBody.payload: the payload carries bytes, so an explicit contentType is required", NOT_SENT],
        0);
    failure_multipart_payload_without_a_content_type => failure(
        "multipart-without-content-type", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["requestBody.payload/photo: the payload carries bytes, so an explicit contentType is required", NOT_SENT],
        0);
    failure_bytes_under_a_json_content_type => failure(
        "bytes-under-json", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["contentType \"application/json\" cannot carry bytes", NOT_SENT],
        0);
    failure_bytes_under_a_form_content_type => failure(
        "bytes-under-form", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["contentType \"application/x-www-form-urlencoded\" cannot carry bytes", NOT_SENT],
        0);
    failure_object_payload_under_a_text_content_type => failure(
        "object-under-text", &[], INVALID_REQUEST_BODY, Some("upload"),
        &["contentType \"text/plain\" sends only a string, number, boolean or bytes payload; got object"],
        0);
    failure_array_payload_under_a_text_content_type => failure(
        "array-under-text", &[], INVALID_REQUEST_BODY, Some("upload"),
        &["contentType \"text/plain\" sends only a string, number, boolean or bytes payload; got array"],
        0);
    failure_non_map_multipart_payload => failure(
        "multipart-non-map", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["contentType \"multipart/form-data\" needs an object payload, one form-data part per entry; got bytes", NOT_SENT],
        0);
    failure_boundary_written_in_a_multipart_content_type => failure(
        "multipart-boundary", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["carries a boundary parameter; the runtime chooses the multipart boundary", NOT_SENT],
        0);
    failure_content_type_header_parameter_on_a_step_with_a_body => failure(
        "content-type-header", BYTES_PHOTO, INVALID_REQUEST_BODY, Some("upload"),
        &["parameters[0](header:Content-Type): the runtime sets Content-Type from requestBody.contentType alone", NOT_SENT],
        0);
    failure_input_file_whose_schema_permits_neither_bytes_nor_an_encoding => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--input-file", "count=cat.jpg"], INPUT_FILE, None,
        &["--input-file input \"count\": its schema declares type \"integer\""],
        0);
    failure_input_file_naming_an_undeclared_input => failure(
        "upload", &["--input-file", "avatar=cat.jpg"], INPUT_FILE, None,
        &["--input-file names input \"avatar\", which workflow \"upload\" does not declare in its inputs"],
        0);
    failure_input_file_colliding_with_input => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--input", "photo=x"], INPUT_FILE, None,
        &["input \"photo\" is given by --input-file and also by --input or --input-json; give it once"],
        0);
    failure_input_file_colliding_with_input_json => failure(
        "upload", &["--input-json", "photo={\"a\":1}", "--input-file", "photo=cat.jpg"], INPUT_FILE, None,
        &["input \"photo\" is given by --input-file and also by --input or --input-json; give it once"],
        0);
    failure_input_file_over_the_size_cap => failure(
        "upload", &["--input-file", "photo=big.bin", "--max-input-file-size", "64"], INPUT_FILE, None,
        &["--input-file input \"photo\": big.bin is larger than --max-input-file-size (64 bytes)"],
        0);
    failure_input_file_invalid_utf8_for_a_string_input => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--input-file", "typed=cat.jpg"], INPUT_FILE, None,
        &["--input-file input \"typed\": cat.jpg is not valid UTF-8"],
        0);
    failure_input_file_with_an_unknown_content_encoding => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--input-file", "encoded=notes.txt"], INPUT_FILE, None,
        &["--input-file input \"encoded\": its schema declares contentEncoding \"base32\""],
        0);
    failure_output_file_naming_an_undeclared_output => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--output-file", "other=out.bin"], OUTPUT_FILE, None,
        &["--output-file names \"other\", which is not a declared output of the workflow"],
        0);
    failure_output_file_with_an_unusable_target_directory => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--output-file", "sent=no-such-dir/out.bin"], OUTPUT_FILE, None,
        &["--output-file target directory no-such-dir is unusable"],
        0);
    failure_output_file_whose_output_is_not_bytes_when_the_run_ends => failure(
        "upload", &["--input-file", "photo=cat.jpg", "--output-file", "id=out.bin"], OUTPUT_FILE, None,
        &["--output-file output \"id\" is not bytes; nothing was written"],
        1);
    failure_response_body_whose_json_fails_to_decode => failure(
        "bad-json", &[], VALUE_UNAVAILABLE, Some("read"),
        &["output \"field\": $response.body#/field: response body is not valid JSON (application/json)"],
        1);
    failure_response_body_whose_charset_fails_to_decode => failure(
        "bad-charset", &[], VALUE_UNAVAILABLE, Some("read"),
        &["output \"text\": $response.body: response body is not valid UTF-8 text"],
        1);
    failure_response_body_in_an_unknown_charset => failure(
        "unknown-charset", &[], VALUE_UNAVAILABLE, Some("read"),
        &["output \"text\": $response.body: response charset \"x-no-such-charset\" is not a supported encoding label"],
        1);
    failure_response_body_whose_content_coding_is_not_decoded => failure(
        "undecodable-coding", &[], VALUE_UNAVAILABLE, Some("read"),
        &["output \"field\": $response.body#/field: response Content-Encoding \"compress\" is not a coding the runtime decodes"],
        1);
    failure_response_body_whose_gzip_coding_fails_to_decode => failure(
        "corrupt-gzip", &[], "RUNTIME_HTTP_RESPONSE_READ", Some("read"),
        &["reading response body: error decoding response body"],
        1);
}

#[test]
fn failure_replay_of_a_trace_written_by_an_older_trace_schema() {
    let fixture = Fixture::new();
    let (_, trace_name) = record(&fixture, "upload-jpeg", &PHOTO);
    let mut trace = fixture.trace(&trace_name);
    assert_eq!(trace["schemaVersion"], "trace.v2");
    trace["schemaVersion"] = json!("trace.v1");
    fixture.write("trace-v1.json", trace.to_string().as_bytes());

    let output = fixture.cli(&["--json", "replay", "trace-v1.json"]);
    let message = failed(&output, "REPLAY_TRACE_LOAD");
    assert_eq!(
        message,
        "unsupported trace schema version \"trace.v1\": this version replays only \"trace.v2\"; re-record the trace with run --trace"
    );
    assert!(fixture.take().is_empty(), "replay sent a request");
}
