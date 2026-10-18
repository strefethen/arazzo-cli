#![forbid(unsafe_code)]

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-explicit-test-paths-{}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path().join(name);
        fs::write(&path, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
}

fn start_server() -> (String, Arc<AtomicUsize>) {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("binding local server: {err}"));
    let url = format!("http://{}", server.server_addr());
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            observed.fetch_add(1, Ordering::SeqCst);
            let _ = request.respond(tiny_http::Response::from_string("{}"));
        }
    });
    (url, requests)
}

fn write_openapi(dir: &TempDir, base_url: &str) -> PathBuf {
    let operations = [("/a", "fixtureGetA"), ("/selected", "fixtureSelected")];
    let paths = operations
        .into_iter()
        .map(|(path, operation_id)| {
            (
                path.to_string(),
                json!({"get": {"operationId": operation_id, "responses": {"200": {"description": "OK"}}}}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let document = json!({
        "openapi": "3.0.3",
        "info": {"title": "Local fixture", "version": "1.0.0"},
        "servers": [{"url": base_url}],
        "paths": paths,
    });
    dir.write("fixture.openapi.json", &document.to_string())
}

fn json_spec(openapi_path: &Path) -> String {
    json!({
        "arazzo": "1.1.0",
        "info": {"title": "Explicit file", "version": "1.0.0"},
        "sourceDescriptions": [{"name": "api", "url": openapi_path, "type": "openapi"}],
        "workflows": [{"workflowId": "wf", "steps": [{"stepId": "one", "operationId": "fixtureGetA"}]}],
    })
    .to_string()
}

fn yaml_spec(openapi_path: &Path) -> String {
    format!(
        "arazzo: 1.1.0\ninfo:\n  title: Explicit file\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: {}\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationId: fixtureGetA\n",
        openapi_path.display()
    )
}

fn results(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout is not JSON ({err}): {stdout}"))
}

fn test_success(path: &Path) -> Value {
    let path = path.to_string_lossy();
    let output = run(&["--json", "test", &path]);
    assert!(
        output.status.success(),
        "test failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = results(&output);
    assert_eq!(body["summary"]["totalSuites"], 1, "{body}");
    assert_eq!(body["summary"]["passed"], 1, "{body}");
    body
}

#[test]
fn explicit_json_yaml_and_extensionless_files_run_by_contents() {
    let (server_url, requests) = start_server();
    let dir = TempDir::new();
    let openapi = write_openapi(&dir, &server_url);
    let json = json_spec(&openapi);
    let yaml = yaml_spec(&openapi);

    for (name, contents) in [
        ("workflow.arazzo.json", json.as_str()),
        ("workflow.yaml", yaml.as_str()),
        ("workflow", yaml.as_str()),
    ] {
        let path = dir.write(name, contents);
        let body = test_success(&path);
        assert_eq!(body["summary"]["totalTests"], 1, "{name}: {body}");
    }

    assert_eq!(requests.load(Ordering::SeqCst), 3);
}

#[test]
fn directory_scanning_still_uses_the_shared_suffix_policy() {
    let (server_url, requests) = start_server();
    let dir = TempDir::new();
    let openapi = write_openapi(&dir, &server_url);
    let yaml = yaml_spec(&openapi);
    let json = json_spec(&openapi);

    let selected = dir.write("selected.arazzo.yaml", &yaml);
    dir.write("notes.yaml", &yaml);
    dir.write("notes.json", &json);

    let directory = dir.path().to_string_lossy();
    let output = run(&["--json", "test", &directory]);
    assert!(
        output.status.success(),
        "directory test failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = results(&output);
    assert_eq!(body["summary"]["totalSuites"], 1, "{body}");
    assert_eq!(
        body["suites"][0]["file"],
        selected.to_string_lossy().as_ref()
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[test]
fn explicitly_named_malformed_json_and_yaml_are_suite_errors() {
    let dir = TempDir::new();
    for (name, contents) in [("broken.json", "{"), ("broken.yaml", "arazzo: [")] {
        let path = dir.write(name, contents);
        let path = path.to_string_lossy();
        let output = run(&["--json", "test", &path]);
        assert_eq!(output.status.code(), Some(1), "{name}");
        let body = results(&output);
        assert_eq!(body["summary"]["suiteErrors"], 1, "{name}: {body}");
        assert!(body["suites"][0]["error"].is_string(), "{name}: {body}");
        assert!(
            !body["suites"][0]["error"]
                .as_str()
                .unwrap_or_default()
                .contains("not an Arazzo spec"),
            "filename rejection instead of parser error: {body}"
        );
    }
}

#[test]
fn missing_path_and_repeated_file_selection_keep_existing_behavior() {
    let dir = TempDir::new();
    let missing = dir
        .path()
        .join("missing.yaml")
        .to_string_lossy()
        .into_owned();
    let missing_output = run(&["--json", "test", &missing]);
    assert_eq!(missing_output.status.code(), Some(1));
    let missing_body = results(&missing_output);
    assert_eq!(missing_body["code"], "TEST_PATH_NOT_FOUND");

    let (server_url, requests) = start_server();
    let openapi = write_openapi(&dir, &server_url);
    let spec = dir.write("duplicate.yaml", &yaml_spec(&openapi));
    let spec = spec.to_string_lossy();
    let output = run(&["--json", "test", &spec, &spec]);
    assert!(
        output.status.success(),
        "duplicate selection failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = results(&output);
    assert_eq!(body["summary"]["totalSuites"], 1, "{body}");
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}
