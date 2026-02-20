#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => {
            let mut path = repo_root();
            path.push("target/debug/arazzo-cli");
            if path.exists() {
                path
            } else {
                panic!(
                    "CLI binary path not found at {}; CARGO_BIN_EXE_arazzo-cli missing",
                    path.display()
                );
            }
        }
    }
}

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../..");
    match fs::canonicalize(&path) {
        Ok(v) => v,
        Err(err) => panic!("canonicalizing repo root {}: {err}", path.display()),
    }
}

fn run(args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(cli_bin());
    cmd.current_dir(repo_root());
    cmd.args(args);
    match cmd.output() {
        Ok(output) => output,
        Err(err) => panic!("running command {args:?}: {err}"),
    }
}

fn stdout_json(output: &std::process::Output) -> Value {
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(value) => value,
        Err(err) => panic!(
            "parsing JSON stdout failed: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        ),
    }
}

fn load_snapshot(name: &str) -> Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/snapshots");
    path.push(name);
    let raw = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(err) => panic!("reading snapshot {}: {err}", path.display()),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => v,
        Err(err) => panic!("parsing snapshot {}: {err}", path.display()),
    }
}

fn assert_snapshot(name: &str, actual: &Value) {
    let expected = load_snapshot(name);
    if actual != &expected {
        let expected_pretty = match serde_json::to_string_pretty(&expected) {
            Ok(v) => v,
            Err(err) => panic!("serializing expected snapshot {name}: {err}"),
        };
        let actual_pretty = match serde_json::to_string_pretty(actual) {
            Ok(v) => v,
            Err(err) => panic!("serializing actual output {name}: {err}"),
        };
        panic!(
            "snapshot mismatch for {name}\nexpected:\n{expected_pretty}\nactual:\n{actual_pretty}"
        );
    }
}

#[test]
fn snapshot_validate_json_contract() {
    let output = run(["--json", "validate", "examples/httpbin-get.arazzo.yaml"].as_slice());
    assert!(output.status.success());
    let mut body = stdout_json(&output);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("file".to_string(), Value::String("<SPEC_PATH>".to_string()));
    }
    assert_snapshot("validate.json", &body);
}

#[test]
fn snapshot_list_json_contract() {
    let output = run(["--json", "list", "examples/httpbin-get.arazzo.yaml"].as_slice());
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_snapshot("list.json", &body);
}

#[test]
fn snapshot_run_dry_run_json_contract() {
    let output = run([
        "--json",
        "run",
        "examples/httpbin-get.arazzo.yaml",
        "status-check",
        "--dry-run",
        "--input",
        "code=429",
    ]
    .as_slice());
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_snapshot("run-dry-run.json", &body);
}

#[test]
fn snapshot_run_missing_workflow_json_contract() {
    let output = run([
        "--json",
        "run",
        "examples/httpbin-get.arazzo.yaml",
        "missing-workflow",
        "--dry-run",
    ]
    .as_slice());
    assert!(output.status.success());
    let body = stdout_json(&output);
    assert_snapshot("run-missing-workflow.json", &body);
}
