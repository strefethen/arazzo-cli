#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn read_json_file(path: &Path) -> Value {
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(err) => panic!("reading json file {}: {err}", path.display()),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(v) => v,
        Err(err) => panic!("parsing json file {}: {err}", path.display()),
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(delta) => delta.as_nanos(),
        Err(_) => 0,
    };
    let mut path = std::env::temp_dir();
    path.push(format!("{}-{}-{}", prefix, std::process::id(), nanos));
    if let Err(err) = fs::create_dir_all(&path) {
        panic!("creating temp dir {}: {err}", path.display());
    }
    path
}

fn normalize_trace_snapshot(trace: &mut Value) {
    if let Some(tool) = trace.get_mut("tool").and_then(Value::as_object_mut) {
        tool.insert(
            "version".to_string(),
            Value::String("<TOOL_VERSION>".to_string()),
        );
    }
    if let Some(run) = trace.get_mut("run").and_then(Value::as_object_mut) {
        run.insert(
            "startedAt".to_string(),
            Value::String("<TIMESTAMP>".to_string()),
        );
        run.insert(
            "finishedAt".to_string(),
            Value::String("<TIMESTAMP>".to_string()),
        );
        run.insert("durationMs".to_string(), Value::Number(0.into()));
    }
    if let Some(steps) = trace.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps {
            if let Some(step_obj) = step.as_object_mut() {
                step_obj.insert("durationMs".to_string(), Value::Number(0.into()));
            }
        }
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
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );
    let body = stdout_json(&output);
    assert_snapshot("run-missing-workflow.json", &body);
}

#[test]
fn snapshot_run_trace_dry_run_contract() {
    let temp_dir = temp_dir("arazzo-trace-snapshot");
    let trace_path = temp_dir.join("trace.json");
    let trace_path_str = trace_path.to_string_lossy().to_string();

    let output = run([
        "--json",
        "run",
        "examples/httpbin-get.arazzo.yaml",
        "status-check",
        "--dry-run",
        "--input",
        "code=429",
        "--trace",
        &trace_path_str,
    ]
    .as_slice());
    assert!(output.status.success());

    let mut trace = read_json_file(&trace_path);
    normalize_trace_snapshot(&mut trace);
    assert_snapshot("run-trace-dry-run.json", &trace);

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn snapshot_run_trace_failure_contract() {
    let temp_dir = temp_dir("arazzo-trace-failure-snapshot");
    let trace_path = temp_dir.join("trace.json");
    let trace_path_str = trace_path.to_string_lossy().to_string();

    let output = run([
        "--json",
        "run",
        "examples/httpbin-get.arazzo.yaml",
        "missing-workflow",
        "--dry-run",
        "--trace",
        &trace_path_str,
    ]
    .as_slice());
    assert!(
        !output.status.success(),
        "JSON error output should exit with non-zero code"
    );

    let mut trace = read_json_file(&trace_path);
    normalize_trace_snapshot(&mut trace);
    assert_snapshot("run-trace-failure.json", &trace);

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn snapshot_replay_json_contract() {
    let temp_dir = temp_dir("arazzo-replay-snapshot");
    let trace_path = temp_dir.join("trace.json");
    let trace_path_str = trace_path.to_string_lossy().to_string();

    let trace_output = run([
        "--json",
        "run",
        "examples/httpbin-get.arazzo.yaml",
        "status-check",
        "--dry-run",
        "--input",
        "code=429",
        "--trace",
        &trace_path_str,
    ]
    .as_slice());
    assert!(trace_output.status.success());

    let replay_output = run(["--json", "replay", &trace_path_str].as_slice());
    assert!(replay_output.status.success());
    let body = stdout_json(&replay_output);
    assert_snapshot("replay.json", &body);

    let _ = fs::remove_dir_all(temp_dir);
}
