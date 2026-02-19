#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(delta) => delta.as_nanos(),
            Err(_) => 0,
        };
        let mut path = std::env::temp_dir();
        path.push(format!("{}-{}-{}", prefix, std::process::id(), nanos));
        if let Err(err) = fs::create_dir_all(&path) {
            panic!("creating temp dir {}: {err}", path.display());
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => {
            let mut path = repo_root();
            path.push("rust/target/debug/arazzo-cli");
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
    path.push("../../..");
    match fs::canonicalize(&path) {
        Ok(v) => v,
        Err(err) => panic!("canonicalizing repo root {}: {err}", path.display()),
    }
}

fn fixture_spec() -> PathBuf {
    let mut path = repo_root();
    path.push("examples/httpbin-get.arazzo.yaml");
    path
}

fn run(args: &[&str], current_dir: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new(cli_bin());
    cmd.args(args);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
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

#[test]
fn validate_json_reports_valid_metadata() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["--json", "validate", &spec_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(true)));
    assert_eq!(
        body.get("title"),
        Some(&Value::String("HTTPBin Demo".to_string()))
    );
    assert_eq!(body.get("workflows"), Some(&Value::Number(3.into())));
}

#[test]
fn list_json_contains_expected_workflows() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(["--json", "list", &spec_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected list output array, got: {body}"),
    };
    assert_eq!(rows.len(), 3);

    let mut ids = Vec::<String>::new();
    for row in rows {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        ids.push(id.to_string());
    }
    ids.sort();

    assert_eq!(
        ids,
        vec![
            "echo-headers".to_string(),
            "get-origin".to_string(),
            "status-check".to_string()
        ]
    );
}

#[test]
fn catalog_and_show_json_work_with_temp_directory() {
    let temp = TempDir::new("arazzo-catalog-test");
    let source = fixture_spec();
    let mut one = temp.path().to_path_buf();
    one.push("one.yaml");
    let mut two = temp.path().to_path_buf();
    two.push("two.yaml");

    if let Err(err) = fs::copy(&source, &one) {
        panic!("copying fixture to {}: {err}", one.display());
    }
    let second_content = match fs::read_to_string(&source) {
        Ok(v) => v,
        Err(err) => panic!("reading fixture {}: {err}", source.display()),
    };
    let second_content =
        second_content.replace("workflowId: status-check", "workflowId: status-check-two");
    if let Err(err) = fs::write(&two, second_content) {
        panic!("writing adjusted fixture {}: {err}", two.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let catalog = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(catalog.status.success());
    let catalog_body = stdout_json(&catalog);
    let rows = match catalog_body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {catalog_body}"),
    };
    assert_eq!(rows.len(), 2);

    let show = run(
        ["--json", "show", "status-check", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(show.status.success());
    let show_body = stdout_json(&show);
    assert_eq!(
        show_body.get("id"),
        Some(&Value::String("status-check".to_string()))
    );
    assert_eq!(show_body.get("steps"), Some(&Value::Number(2.into())));
}

#[test]
fn show_errors_on_duplicate_workflow_id() {
    let temp = TempDir::new("arazzo-show-dup-test");
    let source = fixture_spec();
    let mut one = temp.path().to_path_buf();
    one.push("a.yaml");
    let mut two = temp.path().to_path_buf();
    two.push("b.yaml");

    if let Err(err) = fs::copy(&source, &one) {
        panic!("copying fixture to {}: {err}", one.display());
    }
    if let Err(err) = fs::copy(&source, &two) {
        panic!("copying fixture to {}: {err}", two.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let show = run(
        ["show", "status-check", "--dir", &dir_str].as_slice(),
        Some(temp.path()),
    );
    assert!(!show.status.success());
    let stderr = String::from_utf8_lossy(&show.stderr);
    assert!(stderr.contains("found in multiple files"));
}

#[test]
fn run_dry_run_json_returns_request_plan() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=429",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected dry-run array, got: {body}"),
    };
    assert_eq!(rows.len(), 2);

    let first = &rows[0];
    assert_eq!(
        first.get("stepId"),
        Some(&Value::String("check-status".to_string()))
    );
    assert_eq!(first.get("method"), Some(&Value::String("GET".to_string())));
    let first_url = first.get("url").and_then(Value::as_str).unwrap_or_default();
    assert!(first_url.contains("/status/429"));

    let second = &rows[1];
    assert_eq!(
        second.get("stepId"),
        Some(&Value::String("handle-error".to_string()))
    );
    assert_eq!(
        second.get("method"),
        Some(&Value::String("GET".to_string()))
    );
    let second_url = second
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(second_url.contains("/get"));
}

#[test]
fn run_rejects_invalid_input_format() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();

    let output = run(
        [
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "bad",
        ]
        .as_slice(),
        None,
    );
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid input format"));
}

#[test]
fn validate_json_reports_invalid_spec_errors() {
    let temp = TempDir::new("arazzo-validate-invalid");
    let mut invalid = temp.path().to_path_buf();
    invalid.push("bad.yaml");
    let content = r#"
arazzo: 1.0.0
info:
  version: 1.0.0
sourceDescriptions:
  - name: s1
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    steps: []
"#;
    if let Err(err) = fs::write(&invalid, content) {
        panic!("writing invalid spec {}: {err}", invalid.display());
    }

    let invalid_str = invalid.to_string_lossy().to_string();
    let output = run(["--json", "validate", &invalid_str].as_slice(), None);
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert_eq!(body.get("valid"), Some(&Value::Bool(false)));
    let errors = body
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    assert!(!errors.is_empty());
}

#[test]
fn show_not_found_returns_non_zero_exit() {
    let temp = TempDir::new("arazzo-show-not-found");
    let source = fixture_spec();
    let mut file = temp.path().to_path_buf();
    file.push("spec.yaml");
    if let Err(err) = fs::copy(&source, &file) {
        panic!("copying fixture to {}: {err}", file.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(
        ["show", "missing-workflow", "--dir", &dir_str].as_slice(),
        None,
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
}

#[test]
fn run_json_reports_error_for_missing_workflow() {
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        ["--json", "run", &spec_str, "missing-workflow", "--dry-run"].as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    assert!(body.get("error").is_some());
}

#[test]
fn catalog_json_skips_invalid_yaml_files() {
    let temp = TempDir::new("arazzo-catalog-skip-invalid");
    let source = fixture_spec();
    let mut valid = temp.path().to_path_buf();
    valid.push("valid.yaml");
    if let Err(err) = fs::copy(&source, &valid) {
        panic!("copying fixture to {}: {err}", valid.display());
    }

    let mut invalid = temp.path().to_path_buf();
    invalid.push("invalid.yaml");
    if let Err(err) = fs::write(&invalid, "not: [valid") {
        panic!("writing invalid yaml {}: {err}", invalid.display());
    }

    let dir_str = temp.path().to_string_lossy().to_string();
    let output = run(["--json", "catalog", &dir_str].as_slice(), None);
    assert!(output.status.success());
    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected catalog array, got: {body}"),
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("file"),
        Some(&Value::String("valid.yaml".to_string()))
    );
}

#[test]
fn run_dry_run_expands_env_input_values() {
    std::env::set_var("ARAZZO_TEST_CODE", "204");
    let spec = fixture_spec();
    let spec_str = spec.to_string_lossy().to_string();
    let output = run(
        [
            "--json",
            "run",
            &spec_str,
            "status-check",
            "--dry-run",
            "--input",
            "code=$ARAZZO_TEST_CODE",
        ]
        .as_slice(),
        None,
    );
    assert!(output.status.success());

    let body = stdout_json(&output);
    let rows = match body.as_array() {
        Some(v) => v,
        None => panic!("expected dry-run array, got: {body}"),
    };
    assert!(!rows.is_empty());
    let first = &rows[0];
    let url = first.get("url").and_then(Value::as_str).unwrap_or_default();
    assert!(url.contains("/status/204"));
}
