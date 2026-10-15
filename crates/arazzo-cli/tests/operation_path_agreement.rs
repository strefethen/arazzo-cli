#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The shared syntax contract through validate, run, steps and show.
//! Runtime-only pointer lookup is proved by canonical_operation_targets.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Supported,
    UnknownSource,
    Malformed,
}

const TABLE: &[(&str, Verdict)] = &[
    (
        "{$sourceDescriptions.api.url}#/paths/~1pets/get",
        Verdict::Supported,
    ),
    (
        "{$sourceDescriptions.api.url}#/paths/~1pets/post",
        Verdict::Supported,
    ),
    (
        "{$sourceDescriptions.api.url}#/paths/~1pets~1%7BpetId%7D/delete",
        Verdict::Supported,
    ),
    (
        "{$sourceDescriptions.missing.url}#/paths/~1pets/get",
        Verdict::UnknownSource,
    ),
    ("/pets", Verdict::Malformed),
    ("GET /pets", Verdict::Malformed),
    ("{api}./pets", Verdict::Malformed),
    ("POST {api}./pets", Verdict::Malformed),
    ("https://elsewhere.example.com/pets", Verdict::Malformed),
    (
        "GET {$sourceDescriptions.api.url}#/paths/~1pets/get",
        Verdict::Malformed,
    ),
    ("{$sourceDescriptions.api.url}", Verdict::Malformed),
    ("#/paths/~1pets/get", Verdict::Malformed),
    (
        "{$sourceDescriptions.api.url}#/paths/~2pets/get",
        Verdict::Malformed,
    ),
];

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-agreement-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("fixture directory");
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove fixture");
    }
}

fn write_spec(dir: &Path, operation_path: &str) -> PathBuf {
    let openapi = json!({"openapi": "3.0.3", "info": {"title": "Agreement", "version": "1"},
        "servers": [{"url": "https://api.example.com/v1"}],
        "paths": {"/pets": {"get": {"responses": {"200": {"description": "OK"}}}, "post": {"responses": {"200": {"description": "OK"}}}},
            "/pets/{petId}": {"delete": {"responses": {"200": {"description": "OK"}}}}}});
    fs::write(dir.join("api.json"), openapi.to_string()).expect("explicit OpenAPI");
    let spec = json!({"arazzo": "1.1.0", "info": {"title": "Agreement", "version": "1"},
        "sourceDescriptions": [{"name": "api", "url": "./api.json", "type": "openapi"}],
        "workflows": [{"workflowId": "wf", "steps": [{"stepId": "probe", "operationPath": operation_path}]}]});
    let path = dir.join("agreement.arazzo.yaml");
    fs::write(&path, spec.to_string()).expect("Arazzo fixture");
    path
}

fn run_json(args: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .expect("CLI");
    let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "{args:?}: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), body)
}

#[test]
fn runtime_and_validate_agree_on_every_operation_path_form() {
    for (target, expected) in TABLE {
        let dir = TempDir::new();
        let spec = write_spec(&dir.0, target);
        let path = spec.to_str().unwrap();
        let (valid, validation) = run_json(&["--json", "validate", path]);
        let (runs, plan) = run_json(&["--json", "run", path, "wf", "--dry-run"]);
        let (steps, rows) = run_json(&["--json", "steps", path, "wf"]);
        let (show, shown) = run_json(&["--json", "show", "wf", "--dir", dir.0.to_str().unwrap()]);
        if *expected == Verdict::Supported {
            assert!(
                valid && runs && steps && show,
                "{target}: {validation} {plan} {rows} {shown}"
            );
            assert_eq!(rows[0]["url"], *target);
            assert!(rows[0].get("method").is_none(), "{rows}");
            assert_eq!(shown["steps"][0]["url"], *target);
            assert!(shown["steps"][0].get("method").is_none(), "{shown}");
            assert_eq!(plan["requests"].as_array().unwrap().len(), 1);
        } else {
            assert!(
                !valid && !runs && !steps && !show,
                "{target}: {validation} {plan} {rows} {shown}"
            );
            let kind = if *expected == Verdict::UnknownSource {
                "invalidReference"
            } else {
                "unsupportedOperationPath"
            };
            assert!(
                validation["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["kind"] == kind),
                "{target}: {validation}"
            );
            assert_eq!(plan["code"], "RUN_SPEC_VALIDATION");
            assert_eq!(rows["code"], "STEPS_SPEC_VALIDATION");
            assert!(plan.get("requests").is_none());
        }
    }
}

#[test]
fn dry_run_json_rejects_the_custom_fixture_before_runtime_planning() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/unsupported-operation-path.arazzo.yaml");
    let (ok, body) = run_json(&[
        "--json",
        "run",
        path.to_str().unwrap(),
        "probe",
        "--dry-run",
    ]);
    assert!(!ok);
    assert_eq!(body["kind"], "error");
    assert_eq!(body["code"], "RUN_SPEC_VALIDATION");
    assert!(body["error"].as_str().unwrap().contains("list-pets"));
    assert!(body.get("requests").is_none());
}
