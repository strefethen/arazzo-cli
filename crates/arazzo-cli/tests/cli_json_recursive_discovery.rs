#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-json-recursive-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap_or_else(|err| panic!("create fixture directory: {err}"));
        Self(path)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap_or_else(|| panic!("fixture parent")))
            .unwrap_or_else(|err| panic!("create fixture parent: {err}"));
        fs::write(&path, content).unwrap_or_else(|err| panic!("write fixture: {err}"));
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap_or_else(|err| panic!("remove fixture directory: {err}"));
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("run CLI: {err}"))
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "CLI stdout is not JSON: {error}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn nested_json_only_directory_executes_one_local_step() {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("bind local HTTP fixture: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let dir = TempDir::new();
    let openapi = json!({
        "openapi": "3.0.3",
        "info": {"title": "Local source", "version": "1.0.0"},
        "servers": [{"url": base_url}],
        "paths": {"/ping": {"get": {"operationId": "ping", "responses": {"200": {"description": "OK"}}}}}
    });
    let source = dir.write("source.openapi.json", &openapi.to_string());
    let spec = json!({
        "arazzo": "1.1.0",
        "info": {"title": "Local workflow", "version": "1.0.0"},
        "sourceDescriptions": [{"name": "api", "url": source, "type": "openapi"}],
        "workflows": [{"workflowId": "json-workflow", "steps": [{"stepId": "ping-step", "operationId": "ping"}]}]
    });
    let spec_path = dir.write("nested/workflow.arazzo.json", &spec.to_string());

    let responder = std::thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|err| panic!("receive local request: {err}"))
            .unwrap_or_else(|| panic!("CLI sent a local request"));
        assert_eq!(request.url(), "/ping");
        request
            .respond(tiny_http::Response::from_string("{}"))
            .unwrap_or_else(|err| panic!("respond locally: {err}"));
    });
    let directory = dir
        .path()
        .to_str()
        .unwrap_or_else(|| panic!("UTF-8 fixture path"));
    let output = run(&["--json", "test", directory]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = json_output(&output);
    assert_eq!(body["summary"]["totalSuites"], 1, "{body}");
    assert_eq!(body["summary"]["totalTests"], 1, "{body}");
    assert_eq!(body["summary"]["passed"], 1, "{body}");
    assert_eq!(
        body["suites"][0]["file"],
        spec_path
            .to_str()
            .unwrap_or_else(|| panic!("UTF-8 fixture path"))
    );
    responder
        .join()
        .unwrap_or_else(|_| panic!("local responder finished"));
}

#[test]
fn empty_directory_keeps_structured_no_specs_error() {
    let dir = TempDir::new();
    let directory = dir
        .path()
        .to_str()
        .unwrap_or_else(|| panic!("UTF-8 fixture path"));
    let output = run(&["--json", "test", directory]);
    assert_eq!(output.status.code(), Some(1));
    let body = json_output(&output);
    assert_eq!(body["kind"], "error");
    assert_eq!(body["code"], "TEST_NO_SPECS");
}

#[test]
fn cli_test_and_serve_help_name_all_recursive_suffixes() {
    for command in ["test", "serve"] {
        let output = run(&[command, "--help"]);
        assert!(output.status.success(), "{command} help failed");
        let help =
            String::from_utf8(output.stdout).unwrap_or_else(|err| panic!("UTF-8 help: {err}"));
        for suffix in [".arazzo.json", ".arazzo.yaml", ".arazzo.yml"] {
            assert!(
                help.contains(suffix),
                "{command} help missing {suffix}: {help}"
            );
        }
    }
}
