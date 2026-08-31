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
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
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
    PathBuf::from(env!("CARGO_BIN_EXE_arazzo-cli"))
}

fn write_fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new("arazzo-cli-input-validation");
    let openapi_path = temp.path().join("openapi.yaml");
    let arazzo_path = temp.path().join("workflow.arazzo.yaml");
    fs::write(
        &openapi_path,
        r#"
openapi: 3.0.3
info:
  title: Input validation fixture
  version: 1.0.0
servers:
  - url: https://example.invalid
paths:
  /preferences:
    get:
      operationId: getPreference
      responses:
        '200':
          description: Success
"#,
    )
    .unwrap_or_else(|err| panic!("writing {}: {err}", openapi_path.display()));
    fs::write(
        &arazzo_path,
        r#"
arazzo: 1.1.0
info:
  title: Input validation fixture
  version: 1.0.0
sourceDescriptions:
  - name: local-api
    url: ./openapi.yaml
    type: openapi
workflows:
  - workflowId: preference
    inputs:
      type: object
      properties:
        preference:
          type: string
          default: both
          enum: [sunrise, sunset, both]
    steps:
      - stepId: request
        operationId: getPreference
"#,
    )
    .unwrap_or_else(|err| panic!("writing {}: {err}", arazzo_path.display()));
    (temp, arazzo_path)
}

fn run(spec_path: &Path, extra_args: &[&str]) -> std::process::Output {
    let mut command = Command::new(cli_bin());
    command.args(["--json", "run"]);
    command.arg(spec_path);
    command.args(["preference", "--dry-run"]);
    command.args(extra_args);
    command
        .output()
        .unwrap_or_else(|err| panic!("running input-validation fixture: {err}"))
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "parsing JSON stdout failed: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn run_strict_inputs_accepts_property_enum_members_and_default() {
    let (_temp, spec_path) = write_fixture();

    for input in [None, Some("sunrise"), Some("sunset"), Some("both")] {
        let input_arg = input.map(|value| format!("preference={value}"));
        let mut args = vec!["--strict-inputs"];
        if let Some(input_arg) = input_arg.as_deref() {
            args.extend(["--input", input_arg]);
        }
        let output = run(&spec_path, &args);
        assert!(
            output.status.success(),
            "input {input:?} should succeed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let body = stdout_json(&output);
        assert_eq!(body.get("kind"), Some(&Value::String("dryRun".to_string())));
        assert_eq!(body["requests"].as_array().map(Vec::len), Some(1));
    }
}

#[test]
fn run_strict_inputs_returns_redacted_runtime_code_for_enum_non_member() {
    let (_temp, spec_path) = write_fixture();
    let output = run(
        &spec_path,
        &["--strict-inputs", "--input", "preference=invalid"],
    );

    assert!(!output.status.success());
    let body = stdout_json(&output);
    assert_eq!(body.get("kind"), Some(&Value::String("error".to_string())));
    assert_eq!(
        body.get("code"),
        Some(&Value::String("RUNTIME_INPUT_VALIDATION".to_string()))
    );
    let diagnostic = format!(
        "{}{}",
        body.get("error")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(diagnostic.contains("value is not one of the declared enum values"));
    for value in ["invalid", "sunrise", "sunset", "both"] {
        assert!(!diagnostic.contains(value));
    }
}

#[test]
fn run_non_strict_inputs_warns_and_continues_for_enum_non_member() {
    let (_temp, spec_path) = write_fixture();
    let output = run(&spec_path, &["--input", "preference=invalid"]);

    assert!(
        output.status.success(),
        "non-strict run should continue: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let body = stdout_json(&output);
    assert_eq!(body.get("kind"), Some(&Value::String("dryRun".to_string())));
    assert_eq!(body["requests"].as_array().map(Vec::len), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.trim(),
        "warning: input \"preference\": value is not one of the declared enum values"
    );
    for value in ["invalid", "sunrise", "sunset", "both"] {
        assert!(!stderr.contains(value));
    }
}
