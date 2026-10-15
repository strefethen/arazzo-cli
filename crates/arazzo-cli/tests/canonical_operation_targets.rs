#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Imported and generated targets execute the same bound OpenAPI operations
//! through the shipped CLI, including real method/path/server capture.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct Fixture {
    dir: PathBuf,
    server: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-canonical-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let listener = tiny_http::Server::http("127.0.0.1:0").expect("loopback server");
        let server = format!("http://{}", listener.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                if let Some(request) = listener
                    .recv_timeout(Duration::from_millis(20))
                    .expect("receive request")
                {
                    captured.lock().expect("request log").push((
                        request.method().as_str().to_string(),
                        request.url().to_string(),
                    ));
                    request
                        .respond(tiny_http::Response::from_string("[]"))
                        .expect("respond");
                }
            }
        });
        Self {
            dir,
            server,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir.join(name);
        fs::write(&path, contents).expect("write fixture");
        path
    }

    fn description(&self, name: &str, path: &str, id: Option<&str>) -> PathBuf {
        let mut operation = json!({"responses": {"200": {"description": "OK"}}});
        if let Some(id) = id {
            operation["operationId"] = json!(id);
        }
        let document = json!({
            "openapi": "3.0.3", "info": {"title": "Canonical API", "version": "1.0.0"},
            "servers": [{"url": "http://127.0.0.1:9/wrong-root"}],
            "paths": {path: {"servers": [{"url": "http://127.0.0.1:9/wrong-path"}], "get": {
                "servers": [{"url": format!("{}/effective", self.server)}],
                "responses": operation["responses"], "operationId": operation.get("operationId")
            }}}
        });
        // An absent ID must be absent, not JSON null, in the OpenAPI 3.0 input.
        let mut document = document;
        if id.is_none() {
            document["paths"][path]["get"]
                .as_object_mut()
                .unwrap()
                .remove("operationId");
        }
        // CRUD generation deliberately groups at least two operations. A real
        // sibling POST keeps that preexisting grouping contract; the proof
        // selects the generated list step in isolation.
        document["paths"][path]["post"] = json!({"responses": {"200": {"description": "OK"}}});
        if id.is_some() {
            document["paths"][path]["post"]["operationId"] = json!("createPet");
        }
        self.write(name, &document.to_string())
    }

    fn imported(&self, source: &Path, field: &str, target: &str) -> PathBuf {
        let spec = json!({"arazzo": "1.1.0", "info": {"title": "Imported", "version": "1.0.0"},
            "sourceDescriptions": [{"name": "api", "url": source.file_name().unwrap().to_str().unwrap(), "type": "openapi"}],
            "workflows": [{"workflowId": "wf", "steps": [{"stepId": "probe", field: target}]}]});
        self.write("imported.arazzo.yaml", &spec.to_string())
    }

    fn captured(&self) -> Vec<(String, String)> {
        self.requests.lock().expect("requests").clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().expect("join server");
        fs::remove_dir_all(&self.dir).expect("remove fixture directory");
    }
}

fn cli(args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .expect("run CLI");
    let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{args:?}: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), body)
}

fn prove_execution(
    spec: &Path,
    workflow: &str,
    source: &Path,
    expected_url: &str,
    step: Option<&str>,
) {
    let spec = spec.to_str().unwrap();
    let source = source.to_str().unwrap();
    let (ok, validation) = cli(&["--json", "validate", spec]);
    assert!(ok, "validation: {validation}");
    let mut run_args = vec!["--json", "run", spec, workflow, "--openapi", source];
    if let Some(step) = step {
        run_args.extend(["--step", step, "--no-deps"]);
    }
    let mut plan_args = run_args.clone();
    plan_args.push("--dry-run");
    let (ok, plan) = cli(&plan_args);
    assert!(ok, "plan: {plan}");
    assert_eq!(plan["requests"].as_array().unwrap().len(), 1);
    assert_eq!(plan["requests"][0]["method"], "GET");
    assert_eq!(plan["requests"][0]["url"], expected_url);
    let (ok, run) = cli(&run_args);
    assert!(ok, "execution: {run}");
}

#[test]
fn imported_and_generated_id_and_no_id_targets_execute_the_same_provided_operation() {
    for id in [Some("listPets"), None] {
        let fixture = Fixture::new();
        let source = fixture.description("api.openapi.json", "/pets", id);
        let imported = fixture.imported(
            &source,
            "operationPath",
            "{$sourceDescriptions.api.url}#/paths/~1pets/get",
        );
        let expected = format!("{}/effective/pets", fixture.server);
        prove_execution(&imported, "wf", &source, &expected, None);
        let generated = fixture.dir.join("generated.arazzo.yaml");
        let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args([
                "generate",
                "--spec",
                source.to_str().unwrap(),
                "--output",
                generated.to_str().unwrap(),
            ])
            .output()
            .expect("generate");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = fs::read_to_string(&generated).expect("generated spec");
        let emitted: Value = serde_yaml_ng::from_str(&text).expect("generated YAML");
        let target = emitted["workflows"][0]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["stepId"] == "list-pets")
            .expect("generated list step");
        if id.is_some() {
            assert_eq!(
                target["operationId"],
                "$sourceDescriptions.canonical-api.listPets"
            );
        } else {
            assert_eq!(
                target["operationPath"],
                "{$sourceDescriptions.canonical-api.url}#/paths/~1pets/get"
            );
        }
        prove_execution(
            &generated,
            "crud-pets",
            &source,
            &expected,
            Some("list-pets"),
        );
        assert_eq!(
            fixture.captured(),
            vec![("GET".to_string(), "/effective/pets".to_string()); 2]
        );
    }
}

#[test]
fn escaped_pointer_and_overlapping_sources_keep_method_path_and_server_identity() {
    let fixture = Fixture::new();
    let first = fixture.description("first.json", "/a~b/☃", None);
    let second = fixture.description("second.json", "/a~b/☃", None);
    let mut second_doc: Value = serde_json::from_slice(&fs::read(&second).unwrap()).unwrap();
    second_doc["paths"]["/a~b/☃"]["get"]["servers"] =
        json!([{"url": format!("{}/selected", fixture.server)}]);
    fs::write(&second, second_doc.to_string()).unwrap();
    let spec = json!({"arazzo": "1.1.0", "info": {"title": "Isolation", "version": "1.0.0"},
        "sourceDescriptions": [{"name": "first", "url": "first.json", "type": "openapi"}, {"name": "second", "url": "second.json", "type": "openapi"}],
        "workflows": [{"workflowId": "wf", "steps": [{"stepId": "selected", "operationPath": "{$sourceDescriptions.second.url}#/paths/~1a~0b~1%E2%98%83/get"}]}]});
    let spec = fixture.write("isolation.arazzo.yaml", &spec.to_string());
    let (ok, run) = cli(&[
        "--json",
        "run",
        spec.to_str().unwrap(),
        "wf",
        "--openapi",
        first.to_str().unwrap(),
        "--openapi",
        second.to_str().unwrap(),
    ]);
    assert!(ok, "{run}");
    assert_eq!(
        fixture.captured(),
        [("GET".to_string(), "/selected/a~b/%E2%98%83".to_string())]
    );
}

#[test]
fn invalid_or_unbound_targets_are_refused_before_any_traffic() {
    let fixture = Fixture::new();
    let source = fixture.description("api.json", "/pets", Some("listPets"));
    for target in [
        "/pets",
        "GET /pets",
        "{api}./pets",
        "https://example.invalid/pets",
        "{$sourceDescriptions.api.url}#/paths/~1missing/get",
        "{$sourceDescriptions.api.url}#/paths/~1pets",
        "{$sourceDescriptions.missing.url}#/paths/~1pets/get",
    ] {
        let spec = fixture.imported(&source, "operationPath", target);
        let (ok, error) = cli(&[
            "--json",
            "run",
            spec.to_str().unwrap(),
            "wf",
            "--openapi",
            source.to_str().unwrap(),
        ]);
        assert!(!ok, "{target}: {error}");
        assert_eq!(error["kind"], "error");
        assert!(fixture.captured().is_empty(), "{target} sent a request");
    }
    // A supplied document whose identity does not match the declared source
    // cannot answer either a canonical pointer or an operationId.
    for (field, target) in [
        (
            "operationPath",
            "{$sourceDescriptions.api.url}#/paths/~1pets/get",
        ),
        ("operationId", "listPets"),
    ] {
        let spec = fixture.imported(Path::new("https://unbound.invalid/api.json"), field, target);
        let mut value: Value = serde_json::from_slice(&fs::read(&spec).unwrap()).unwrap();
        value["sourceDescriptions"][0]["url"] = json!("https://unbound.invalid/api.json");
        fs::write(&spec, value.to_string()).unwrap();
        let (ok, error) = cli(&[
            "--json",
            "run",
            spec.to_str().unwrap(),
            "wf",
            "--openapi",
            source.to_str().unwrap(),
        ]);
        assert!(!ok, "unbound {field}: {error}");
        assert_eq!(error["code"], "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND");
        assert!(fixture.captured().is_empty());
    }
}
