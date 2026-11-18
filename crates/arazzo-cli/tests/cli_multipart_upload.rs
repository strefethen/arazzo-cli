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

const MARKER: &str = "PLANTED-MULTIPART-JPEG-5d2e!";
const PHOTO: &str = "photo=cat.jpg;type=image/jpeg;filename=c.jpg";

fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    bytes.extend_from_slice(MARKER.as_bytes());
    bytes.extend_from_slice(&[0x00, 0x0D, 0x0A, 0xFF, 0xD9]);
    bytes
}

#[derive(Debug, Clone)]
struct Seen {
    content_type: String,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq)]
struct Field {
    name: String,
    filename: Option<String>,
    media_type: Option<String>,
    bytes: Vec<u8>,
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
            "arazzo-multipart-upload-{}-{}",
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
                    .map(|header| header.value.as_str().to_owned())
                    .unwrap_or_default();
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen { content_type, body });
                let header = Header::from_bytes("Content-Type", "application/json")
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request
                    .respond(TinyResponse::from_string(r#"{"id":"p1"}"#).with_header(header));
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

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut all = vec![
            "--json",
            "run",
            "spec.arazzo.json",
            "upload",
            "--input-file",
            PHOTO,
            "--input",
            "caption=cat",
            "--input-json",
            r#"tags=["a","b"]"#,
        ];
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
    json!({
        "arazzo":"1.1.0", "info":{"title":"multipart upload", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":[{"workflowId":"upload",
            "inputs":{"type":"object", "properties":{"photo":{},
                "caption":{"type":"string"}, "tags":{"type":"array"}}},
            "steps":[{"stepId":"upload", "operationId":"upload",
                "requestBody":{"contentType":"multipart/form-data", "payload":{
                    "caption":"$inputs.caption", "tags":"$inputs.tags",
                    "photo":"$inputs.photo"}},
                "successCriteria":[{"condition":"$statusCode == 200"}],
                "outputs":{"id":"$response.body#/id"}}],
            "outputs":{"id":"$steps.upload.outputs.id"}}]
    })
}

fn succeeded(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("stdout is not JSON: {err}"))
}

fn assert_no_file_bytes(label: &str, text: &[u8]) {
    let contains = |needle: &[u8]| text.windows(needle.len()).any(|window| window == needle);
    assert!(!contains(&jpeg()), "{label}: the file's bytes leaked");
    assert!(
        !contains(MARKER.as_bytes()),
        "{label}: planted bytes leaked"
    );
    assert!(!contains(b"boundary="), "{label}: the boundary leaked");
}

fn assert_output_has_no_file_bytes(label: &str, output: &Output) {
    assert_no_file_bytes(&format!("{label} stdout"), &output.stdout);
    assert_no_file_bytes(&format!("{label} stderr"), &output.stderr);
}

fn parse(seen: &Seen) -> Vec<Field> {
    let boundary = multer::parse_boundary(&seen.content_type)
        .unwrap_or_else(|err| panic!("{:?}: {err}", seen.content_type));
    let body = seen.body.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap_or_else(|err| panic!("runtime: {err}"));
    runtime.block_on(async move {
        let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) });
        let mut multipart = multer::Multipart::new(stream, boundary);
        let mut fields = Vec::new();
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
            fields.push(Field {
                name,
                filename,
                media_type,
                bytes,
            });
        }
        fields
    })
}

fn text(name: &str, value: &str) -> Field {
    Field {
        name: name.to_owned(),
        filename: None,
        media_type: None,
        bytes: value.as_bytes().to_vec(),
    }
}

#[test]
fn a_jpeg_caption_and_tags_upload_records_and_replays_without_the_bytes() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--trace", "trace.json", "--verbose"]);
    assert_eq!(
        succeeded(&output),
        json!({"kind":"success", "outputs":{"id":"p1"}})
    );
    assert_output_has_no_file_bytes("run --json --verbose", &output);

    let seen = fixture.take();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        parse(&seen[0]),
        vec![
            text("caption", "cat"),
            text("tags", "a"),
            text("tags", "b"),
            Field {
                name: "photo".to_owned(),
                filename: Some("c.jpg".to_owned()),
                media_type: Some("image/jpeg".to_owned()),
                bytes: jpeg(),
            },
        ],
        "the file part is last, after the caption and one part per tag"
    );

    let raw_trace = fixture.read("trace.json");
    assert_no_file_bytes("trace", raw_trace.as_bytes());
    let trace: Value = serde_json::from_str(&raw_trace).unwrap_or_else(|err| panic!("{err}"));
    let request = &trace["steps"][0]["request"];
    assert_eq!(request["headers"]["Content-Type"], "multipart/form-data");
    assert_eq!(
        request["body"],
        json!({"caption":"cat", "tags":["a", "b"], "photo":null})
    );
    let photo = &request["bodyBlobs"]["/photo"];
    assert_eq!(photo["size"], json!(jpeg().len()));
    assert_eq!(photo["mediaType"], "image/jpeg");
    assert_eq!(photo["filename"], "c.jpg");
    assert_eq!(trace["inputBlobs"]["photo"]["sha256"], photo["sha256"]);

    let replay = fixture.cli(&["--json", "replay", "trace.json"]);
    let replayed = succeeded(&replay);
    assert_eq!(replayed["outputs"], json!({"id":"p1"}), "{replayed}");
    assert_eq!(replayed["requestsChecked"], 1, "{replayed}");
    assert_output_has_no_file_bytes("replay --json", &replay);
    assert!(fixture.take().is_empty(), "replay never touches the wire");
}

#[test]
fn a_dry_run_shows_the_parts_with_the_file_descriptor_and_no_bytes() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--dry-run"]);
    let planned = succeeded(&output);
    assert_output_has_no_file_bytes("dry run --json", &output);
    assert!(fixture.take().is_empty());
    let request = &planned["requests"][0];
    assert_eq!(request["headers"]["Content-Type"], "multipart/form-data");
    let body = &request["body"];
    assert_eq!(body["caption"], "cat");
    assert_eq!(body["tags"], json!(["a", "b"]));
    assert_eq!(body["photo"]["kind"], "bytes");
    assert_eq!(body["photo"]["filename"], "c.jpg");
    assert_eq!(body["photo"]["mediaType"], "image/jpeg");

    let human = fixture.cli(&[
        "run",
        "spec.arazzo.json",
        "upload",
        "--input-file",
        PHOTO,
        "--input",
        "caption=cat",
        "--input-json",
        r#"tags=["a","b"]"#,
        "--dry-run",
    ]);
    assert!(human.status.success());
    assert_output_has_no_file_bytes("dry run", &human);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("\"filename\":\"c.jpg\""), "{stdout}");
}
