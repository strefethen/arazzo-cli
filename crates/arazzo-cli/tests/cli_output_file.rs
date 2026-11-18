#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00PLANTED-PNG-BYTES\xFF\xFE\x00\x01";

struct Fixture {
    dir: PathBuf,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-output-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|err| panic!("binding: {err}"));
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (counted, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                counted.fetch_add(1, Ordering::Relaxed);
                let (status, content_type, body): (u16, &str, Vec<u8>) = match request.url() {
                    "/image" => (200, "image/png", PNG.to_vec()),
                    "/info" => (200, "application/json", br#"{"name":"cat"}"#.to_vec()),
                    _ => (500, "text/plain", b"boom".to_vec()),
                };
                let header = Header::from_bytes("Content-Type", content_type)
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request.respond(
                    TinyResponse::from_data(body)
                        .with_status_code(status)
                        .with_header(header),
                );
            }
        });
        let fixture = Self {
            dir,
            requests,
            stop,
            worker: Some(worker),
        };
        let operation =
            |id: &str| json!({"get":{"operationId":id, "responses":{"200":{"description":"ok"}}}});
        fixture.write(
            "api.openapi.json",
            json!({"openapi":"3.0.3", "info":{"title":"downloads", "version":"1"},
                "servers":[{"url":base_url}],
                "paths":{"/image":operation("image"), "/info":operation("info"),
                    "/fail":operation("fail")}})
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

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn run(&self, workflow: &str, flags: &[&str]) -> Output {
        let mut args = vec!["--json", "run", "spec.arazzo.json", workflow];
        args.extend_from_slice(flags);
        Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(&args)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .unwrap_or_else(|err| panic!("listing {}: {err}", self.dir.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        names.sort();
        names
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
    let step = |step_id: &str, operation: &str, outputs: Value| {
        json!({"stepId":step_id, "operationId":operation,
            "successCriteria":[{"condition":"$statusCode == 200"}], "outputs":outputs})
    };
    json!({
        "arazzo":"1.1.0", "info":{"title":"downloads", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":"./api.openapi.json", "type":"openapi"}],
        "workflows":[
            {"workflowId":"download", "steps":[
                step("get", "image", json!({"image":"$response.body"}))
            ], "outputs":{"image":"$steps.get.outputs.image"}},
            {"workflowId":"mixed", "steps":[
                step("get", "image", json!({"image":"$response.body"})),
                step("info", "info", json!({"name":"$response.body#/name"}))
            ], "outputs":{"image":"$steps.get.outputs.image", "name":"$steps.info.outputs.name"}},
            {"workflowId":"failing", "steps":[
                step("get", "image", json!({"image":"$response.body"})),
                step("fail", "fail", json!({}))
            ], "outputs":{"image":"$steps.get.outputs.image"}},
            {"workflowId":"ghost", "steps":[
                step("get", "image", json!({"image":"$response.body"}))
            ], "outputs":{"image":"$steps.get.outputs.image",
                "ghost":"$steps.get.outputs.nothing"}},
            {"workflowId":"pathy",
                "inputs":{"type":"object", "properties":{"dest":{"type":"string"}}},
                "steps":[step("get", "image", json!({"image":"$response.body"}))],
                "outputs":{"image":"$steps.get.outputs.image", "file":"$inputs.dest"}}
        ]
    })
}

fn error_of(output: &Output) -> Value {
    assert!(
        !output.status.success(),
        "expected failure, stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout JSON: {err}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_output_file_error(output: &Output, needle: &str) {
    let error = error_of(output);
    assert_eq!(error["code"], "RUN_OUTPUT_FILE", "{error}");
    let message = error["error"].as_str().unwrap_or_default();
    assert!(message.contains(needle), "{needle:?} not in {message:?}");
}

#[test]
fn a_downloaded_png_is_written_byte_identical_and_replaces_an_existing_file() {
    let fixture = Fixture::new();
    fixture.write("cat.png", b"stale contents that must disappear");
    let output = fixture.run("download", &["--output-file", "image=cat.png"]);
    assert_success(&output);
    assert_eq!(
        fs::read(fixture.path("cat.png")).unwrap_or_else(|err| panic!("reading cat.png: {err}")),
        PNG
    );
    assert_eq!(fixture.requests(), 1);
    assert!(fixture.leftovers().is_empty(), "{:?}", fixture.leftovers());
    let printed: Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("stdout JSON: {err}"));
    assert_eq!(printed["outputs"]["image"]["kind"], "bytes");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PLANTED-PNG-BYTES"));
}

#[test]
fn a_fresh_target_is_created() {
    let fixture = Fixture::new();
    assert_success(&fixture.run("download", &["--output-file", "image=fresh.png"]));
    assert_eq!(
        fs::read(fixture.path("fresh.png")).unwrap_or_else(|err| panic!("reading: {err}")),
        PNG
    );
}

#[test]
fn bytes_are_only_descriptors_without_the_flag() {
    let fixture = Fixture::new();
    assert_success(&fixture.run("download", &[]));
    assert!(!fixture.path("cat.png").exists());
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn an_undeclared_output_name_fails_before_any_request() {
    let fixture = Fixture::new();
    let output = fixture.run("download", &["--output-file", "other=cat.png"]);
    assert_output_file_error(&output, "not a declared output");
    assert_eq!(fixture.requests(), 0);
    assert!(!fixture.path("cat.png").exists());
}

#[test]
fn a_missing_target_directory_fails_before_any_request() {
    let fixture = Fixture::new();
    let output = fixture.run("download", &["--output-file", "image=no-such-dir/cat.png"]);
    assert_output_file_error(&output, "no-such-dir");
    assert_eq!(fixture.requests(), 0);
}

#[test]
fn a_directory_target_fails_before_any_request() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("taken")).unwrap_or_else(|err| panic!("creating directory: {err}"));
    let output = fixture.run("download", &["--output-file", "image=taken"]);
    assert_output_file_error(&output, "is a directory");
    assert_eq!(fixture.requests(), 0);
}

#[cfg(unix)]
#[test]
fn an_unwritable_target_directory_fails_before_any_request_and_leaves_no_probe() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let locked = fixture.path("locked");
    fs::create_dir(&locked).unwrap_or_else(|err| panic!("creating directory: {err}"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555))
        .unwrap_or_else(|err| panic!("locking directory: {err}"));
    let writable_anyway = fs::File::create(locked.join("probe")).is_ok();
    let output = fixture.run("download", &["--output-file", "image=locked/cat.png"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|err| panic!("unlocking directory: {err}"));
    if writable_anyway {
        return;
    }
    assert_output_file_error(&output, "not writable");
    assert_eq!(fixture.requests(), 0);
    assert!(!locked.join("cat.png").exists());
}

#[test]
fn malformed_duplicate_and_dry_run_flags_fail_before_any_request() {
    let fixture = Fixture::new();
    assert_output_file_error(
        &fixture.run("download", &["--output-file", "image"]),
        "expected name=path",
    );
    assert_output_file_error(
        &fixture.run("download", &["--output-file", "image="]),
        "expected name=path",
    );
    assert_output_file_error(
        &fixture.run(
            "download",
            &[
                "--output-file",
                "image=a.png",
                "--output-file",
                "image=b.png",
            ],
        ),
        "more than once",
    );
    assert_output_file_error(
        &fixture.run(
            "mixed",
            &[
                "--output-file",
                "image=a.png",
                "--output-file",
                "name=./a.png",
            ],
        ),
        "more than one output the target",
    );
    assert_output_file_error(
        &fixture.run("download", &["--dry-run", "--output-file", "image=a.png"]),
        "--dry-run",
    );
    assert_eq!(fixture.requests(), 0);
    assert!(!fixture.path("a.png").exists());
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn a_non_bytes_output_is_an_error_and_writes_no_file() {
    let fixture = Fixture::new();
    let output = fixture.run(
        "mixed",
        &[
            "--output-file",
            "image=cat.png",
            "--output-file",
            "name=name.txt",
        ],
    );
    assert_output_file_error(&output, "\"name\" is not bytes");
    assert!(!fixture.path("cat.png").exists());
    assert!(!fixture.path("name.txt").exists());
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn an_absent_output_is_an_error_and_writes_no_file() {
    let fixture = Fixture::new();
    let output = fixture.run(
        "ghost",
        &[
            "--output-file",
            "image=cat.png",
            "--output-file",
            "ghost=ghost.bin",
        ],
    );
    let error = error_of(&output);
    assert_eq!(error["code"], "RUN_OUTPUT_FILE", "{error}");
    assert!(error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("ghost"));
    assert!(!fixture.path("cat.png").exists());
    assert!(!fixture.path("ghost.bin").exists());
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn a_failed_run_writes_no_file_and_leaves_an_existing_one_alone() {
    let fixture = Fixture::new();
    fixture.write("cat.png", b"original");
    let output = fixture.run("failing", &["--output-file", "image=cat.png"]);
    let error = error_of(&output);
    assert_ne!(error["code"], "RUN_OUTPUT_FILE", "{error}");
    assert_eq!(
        fs::read(fixture.path("cat.png")).unwrap_or_else(|err| panic!("reading: {err}")),
        b"original"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn a_path_named_in_a_document_writes_nothing() {
    let fixture = Fixture::new();
    let planted = fixture.path("planted.txt");
    let dest = format!("dest={}", planted.display());
    assert_success(&fixture.run("pathy", &["-i", dest.as_str()]));
    assert!(!planted.exists());
    assert_success(&fixture.run(
        "pathy",
        &["-i", dest.as_str(), "--output-file", "image=cat.png"],
    ));
    assert!(!planted.exists());
    assert_eq!(
        fs::read(fixture.path("cat.png")).unwrap_or_else(|err| panic!("reading: {err}")),
        PNG
    );
}

#[test]
fn replay_has_no_output_file_flag() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args([
            "--json",
            "replay",
            "trace.json",
            "--output-file",
            "image=cat.png",
        ])
        .current_dir(&fixture.dir)
        .output()
        .unwrap_or_else(|err| panic!("running replay: {err}"));
    let error = error_of(&output);
    assert_eq!(error["code"], "CLI_USAGE", "{error}");
    assert!(error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("--output-file"));
    let help = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["replay", "--help"])
        .output()
        .unwrap_or_else(|err| panic!("running replay --help: {err}"));
    assert!(!String::from_utf8_lossy(&help.stdout).contains("output-file"));
}
