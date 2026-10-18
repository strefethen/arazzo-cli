#![forbid(unsafe_code)]

//! The `--json` error object `main` prints for every command:
//! `{"kind":"error","error":…,"code":…}` on stdout, the plain message on
//! stderr without `--json`. The sweep at the end has one `sweep_<name>` case
//! per subcommand; a unit test in `src/cli.rs` fails when one is missing.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

struct TempDir {
    path: PathBuf,
}

static TEMP_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let seq = TEMP_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-json-errors-{prefix}-{}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        common::write_in(&path, "https://example.com");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file(&self, name: &str, contents: &str) -> String {
        let path = self.path.join(name);
        fs::write(&path, contents).unwrap_or_else(|err| panic!("writing {name}: {err}"));
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Asserts stdout is exactly one error object with `code` and returns its
/// `error` message.
fn assert_error_object(output: &Output, code: &str) -> String {
    let text = stdout(output);
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("stdout is not one JSON value ({err}): {text}"));
    assert_eq!(body["kind"], "error", "{body}");
    assert_eq!(body["code"], code, "{body}");
    let keys: Vec<&str> = body
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(keys.len(), 3, "unexpected fields: {body}");
    body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error must be a string: {body}"))
        .to_string()
}

// ── AC1: clap usage errors ──────────────────────────────────────

#[test]
fn unknown_flag_with_json_prints_cli_usage_and_exits_2() {
    let output = run(&["--json", "list", "--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    let message = assert_error_object(&output, "CLI_USAGE");
    assert!(message.contains("--bogus"), "{message}");
    assert!(!message.contains('\u{1b}'), "message must be unstyled");
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));

    let after = run(&["list", "--bogus", "--json"]);
    assert_eq!(after.status.code(), Some(2));
    assert_error_object(&after, "CLI_USAGE");
}

#[test]
fn missing_argument_with_json_prints_cli_usage_and_exits_2() {
    let output = run(&["--json", "replay"]);
    assert_eq!(output.status.code(), Some(2));
    let message = assert_error_object(&output, "CLI_USAGE");
    assert!(message.contains("<TRACE>"), "{message}");
}

#[test]
fn usage_errors_without_json_keep_clap_output() {
    for args in [&["list", "--bogus"][..], &["replay"][..]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stdout(&output).is_empty(), "{args:?}: {}", stdout(&output));
        let err = stderr(&output);
        assert!(err.starts_with("error: "), "{args:?}: {err}");
        assert!(err.contains("Usage:"), "{args:?}: {err}");
    }
}

#[test]
fn json_after_the_terminator_does_not_switch_usage_errors_to_json() {
    let output = run(&["list", "--bogus", "--", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
    assert!(stderr(&output).starts_with("error: "));
}

#[test]
fn help_and_version_keep_clap_behavior_with_json() {
    for args in [&["--json", "--help"][..], &["--help"][..]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(stdout(&output).contains("Usage:"), "{args:?}");
    }
    let plain = run(&["--help"]);
    let json = run(&["--json", "--help"]);
    assert_eq!(plain.stdout, json.stdout);

    let version = run(&["--json", "--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(
        stdout(&version).starts_with("arazzo "),
        "{}",
        stdout(&version)
    );
}

// ── AC2: one case per `test` failure row ─────────────────────────

/// Runs `test` with `args` three ways: `--json`, `--format json`, and plain.
/// The two JSON forms print the object with `code`; plain prints the same
/// message on stderr and nothing on stdout.
fn assert_test_failure(args: &[&str], code: &str) -> String {
    let mut json_args = vec!["--json", "test"];
    json_args.extend_from_slice(args);
    let json = run(&json_args);
    assert_eq!(json.status.code(), Some(1), "{json_args:?}");
    let message = assert_error_object(&json, code);

    let mut format_args = vec!["test", "--format", "json"];
    format_args.extend_from_slice(args);
    let format = run(&format_args);
    assert_eq!(format.status.code(), Some(1), "{format_args:?}");
    assert_eq!(assert_error_object(&format, code), message);

    let mut plain_args = vec!["test"];
    plain_args.extend_from_slice(args);
    let plain = run(&plain_args);
    assert_eq!(plain.status.code(), Some(1), "{plain_args:?}");
    assert!(stdout(&plain).is_empty(), "{}", stdout(&plain));
    assert_eq!(stderr(&plain), format!("{message}\n"));
    message
}

#[test]
fn test_missing_path_is_test_path_not_found() {
    let dir = TempDir::new("not-found");
    let missing = dir.path().join("missing.arazzo.yaml");
    let message = assert_test_failure(&[&missing.to_string_lossy()], "TEST_PATH_NOT_FOUND");
    assert!(message.starts_with("path does not exist: "), "{message}");
}

#[cfg(unix)]
#[test]
fn test_unscannable_directory_is_test_path_scan() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new("scan");
    let locked = dir.path().join("locked");
    fs::create_dir_all(&locked).unwrap_or_else(|err| panic!("mkdir: {err}"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
        .unwrap_or_else(|err| panic!("chmod: {err}"));
    // Root reads any directory; there is nothing to assert then.
    let readable = fs::read_dir(&locked).is_ok();
    if !readable {
        let message = assert_test_failure(&[&locked.to_string_lossy()], "TEST_PATH_SCAN");
        assert!(message.starts_with("scanning "), "{message}");
    }
    let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
}

#[test]
fn explicitly_named_non_spec_file_is_a_suite_parse_error() {
    let dir = TempDir::new("not-arazzo");
    let file = dir.file("notes.txt", "hello");
    let output = run(&["--json", "test", &file]);
    assert_eq!(output.status.code(), Some(1));

    let text = stdout(&output);
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("stdout is not one JSON value ({err}): {text}"));
    assert_eq!(body["kind"], "results", "{body}");
    assert_eq!(body["summary"]["totalSuites"], 1, "{body}");
    assert_eq!(body["summary"]["totalTests"], 0, "{body}");
    assert_eq!(body["summary"]["suiteErrors"], 1, "{body}");

    let error = body["suites"][0]["error"]
        .as_str()
        .unwrap_or_else(|| panic!("suite must report its parse error: {body}"));
    assert!(error.contains("parsing arazzo yaml"), "{error}");
    assert!(error.contains("hello"), "{error}");
}

#[test]
fn test_empty_directory_is_test_no_specs() {
    let dir = TempDir::new("no-specs");
    let message = assert_test_failure(&[&dir.path().to_string_lossy()], "TEST_NO_SPECS");
    assert!(message.starts_with("no test specs found in: "), "{message}");
}

#[test]
fn test_bad_input_values_are_test_input_format() {
    let dir = TempDir::new("input");
    let spec = dir.file("suite.arazzo.yaml", "not checked before inputs");
    let message = assert_test_failure(&[&spec, "-i", "novalue"], "TEST_INPUT_FORMAT");
    assert!(message.starts_with("invalid input format"), "{message}");
    let message = assert_test_failure(&[&spec, "--input-json", "k={"], "TEST_INPUT_FORMAT");
    assert!(
        message.starts_with("invalid JSON input format"),
        "{message}"
    );
}

#[test]
fn test_unreadable_openapi_is_test_openapi_read_file() {
    let dir = TempDir::new("openapi");
    let spec = dir.file("suite.arazzo.yaml", "not checked before OpenAPI");
    let missing = dir.path().join("missing.yaml");
    let message = assert_test_failure(
        &[&spec, "--openapi", &missing.to_string_lossy()],
        "TEST_OPENAPI_READ_FILE",
    );
    assert!(message.starts_with("reading OpenAPI file "), "{message}");
}

#[test]
fn test_invalid_filter_is_test_filter_invalid() {
    let dir = TempDir::new("filter");
    let spec = dir.file("suite.arazzo.yaml", "not checked before the filter");
    let message = assert_test_failure(&[&spec, "--filter", "("], "TEST_FILTER_INVALID");
    assert!(message.starts_with("invalid --filter regex: "), "{message}");
}

// ── AC3: replay failures keep today's object ─────────────────────

#[test]
fn replay_trace_load_failure_is_the_same_object_as_before() {
    let dir = TempDir::new("replay");
    let missing = dir.path().join("missing-trace.json");
    let missing = missing.to_string_lossy();

    let plain = run(&["replay", &missing]);
    assert_eq!(plain.status.code(), Some(1));
    assert!(stdout(&plain).is_empty());
    let err = stderr(&plain);
    let message = err.strip_suffix('\n').unwrap_or(&err);

    let output = run(&["--json", "replay", &missing]);
    assert_eq!(output.status.code(), Some(1));
    let body: Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|err| panic!("replay stdout is not JSON: {err}"));
    assert_eq!(
        body,
        json!({"kind": "error", "error": message, "code": "REPLAY_TRACE_LOAD"})
    );
    // The pre-change printer was `serde_json::to_writer_pretty` plus a
    // newline, in field order kind, error, code.
    let expected = format!(
        "{{\n  \"kind\": \"error\",\n  \"error\": {},\n  \"code\": \"REPLAY_TRACE_LOAD\"\n}}\n",
        serde_json::to_string(message).unwrap_or_default()
    );
    assert_eq!(stdout(&output), expected);
}

// ── AC4: failing cases print results, never an error object ──────

#[test]
fn test_run_with_failures_prints_results_and_exits_1() {
    let dir = TempDir::new("results");
    dir.file("broken.arazzo.yaml", "arazzo: [not a spec");
    let path = dir.path().to_string_lossy().into_owned();

    let json = run(&["--json", "test", &path]);
    assert_eq!(json.status.code(), Some(1));
    let body: Value = serde_json::from_str(&stdout(&json))
        .unwrap_or_else(|err| panic!("test stdout is not JSON: {err}"));
    assert_eq!(body["kind"], "results", "{body}");
    assert_eq!(body["summary"]["suiteErrors"], 1, "{body}");
    assert!(body.get("code").is_none(), "{body}");

    for format in ["tap", "junit"] {
        let output = run(&["test", "--format", format, &path]);
        assert_eq!(output.status.code(), Some(1), "{format}");
        let text = stdout(&output);
        assert!(!text.is_empty(), "{format}");
        assert!(!text.contains("\"kind\""), "{format}: {text}");
    }
}

// ── run failures that printed plain text before ac-9d64c ─────────

/// A local server answering 500 on `/fail` and 200 elsewhere.
fn start_server() -> String {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("binding local server: {err}"));
    let url = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let status = if request.url().starts_with("/fail") {
                500
            } else {
                200
            };
            let response = tiny_http::Response::from_string("{}")
                .with_status_code(tiny_http::StatusCode(status));
            let _ = request.respond(response);
        }
    });
    url
}

fn run_spec(dir: &TempDir, source_url: &str, operation_path: &str) -> String {
    dir.file(
        "run.arazzo.yaml",
        &format!(
            "arazzo: 1.1.0\ninfo:\n  title: Run errors\n  version: 1.0.0\nsourceDescriptions:\n  - name: api\n    url: {source_url}\n    type: openapi\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: one\n        operationId: {operation_path}\n        successCriteria:\n          - condition: $statusCode == 200\n"
        ),
    )
}

#[test]
fn run_bad_inputs_are_run_input_format() {
    let dir = TempDir::new("run-input");
    let spec = run_spec(
        &dir,
        &common::fixture_url("http://127.0.0.1:9"),
        "fixtureGetOne",
    );
    for flags in [
        ["-i", "bad"],
        ["--input-json", "key=notjson"],
        ["--input-json", "bad"],
    ] {
        let output = run(&["--json", "run", &spec, "wf", flags[0], flags[1]]);
        assert_eq!(output.status.code(), Some(1), "{flags:?}");
        let message = assert_error_object(&output, "RUN_INPUT_FORMAT");
        assert!(message.contains("input format"), "{flags:?}: {message}");

        let plain = run(&["run", &spec, "wf", flags[0], flags[1]]);
        assert_eq!(plain.status.code(), Some(1), "{flags:?}");
        assert!(stdout(&plain).is_empty(), "{flags:?}");
        assert_eq!(stderr(&plain), format!("{message}\n"), "{flags:?}");
    }
}

#[test]
fn run_missing_local_source_carries_the_engine_code() {
    let dir = TempDir::new("run-engine");
    let spec = run_spec(&dir, "./missing.openapi.yaml", "fixtureGetOne");
    let output = run(&["--json", "run", &spec, "wf"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout(&output);
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("stdout is not one JSON value ({err}): {text}"));
    let code = body["code"].as_str().unwrap_or_default();
    assert!(code.starts_with("RUNTIME_SOURCE_DESCRIPTION_"), "{body}");
    let message = assert_error_object(&output, code);
    assert!(
        message.starts_with("creating runtime engine: "),
        "{message}"
    );
}

#[test]
fn run_trace_write_failure_after_criteria_failure_keeps_both_causes() {
    let dir = TempDir::new("run-trace-fail");
    let url = start_server();
    let spec = run_spec(&dir, &common::fixture_url(&url), "fixtureGetFail");
    let trace = dir.path().join("trace-dir");
    fs::create_dir(&trace).unwrap_or_else(|err| panic!("creating trace dir: {err}"));
    let trace = trace.to_string_lossy();

    let output = run(&["--json", "run", &spec, "wf", "--trace", &trace]);
    assert_eq!(output.status.code(), Some(1));
    let message = assert_error_object(&output, "RUN_TRACE_WRITE");
    let (run_error, trace_error) = message
        .split_once("; writing trace: ")
        .unwrap_or_else(|| panic!("message must join both causes: {message}"));
    assert!(!run_error.is_empty(), "{message}");
    assert!(!trace_error.is_empty(), "{message}");
}

#[test]
fn run_trace_write_failure_after_success_carries_the_trace_error_alone() {
    let dir = TempDir::new("run-trace-ok");
    let url = start_server();
    let spec = run_spec(&dir, &common::fixture_url(&url), "fixtureGetOne");
    let trace = dir.path().join("trace-dir");
    fs::create_dir(&trace).unwrap_or_else(|err| panic!("creating trace dir: {err}"));
    let trace = trace.to_string_lossy();

    let output = run(&["--json", "run", &spec, "wf", "--trace", &trace]);
    assert_eq!(output.status.code(), Some(1));
    let message = assert_error_object(&output, "RUN_TRACE_WRITE");
    assert!(message.starts_with("writing trace: "), "{message}");
    assert!(!message.contains("; "), "{message}");
}

// ── validate failures keep their fields beside the error object ──

/// Runs `validate` with `--json` and plain; asserts the shared object with
/// `code` plus `valid: false` and `file`, and that plain mode prints the same
/// message on stderr. Returns the parsed object.
fn assert_validate_failure(file: &str, extra: &[&str], code: &str) -> Value {
    let mut json_args = vec!["--json"];
    json_args.extend_from_slice(extra);
    json_args.extend_from_slice(&["validate", file]);
    let output = run(&json_args);
    assert_eq!(output.status.code(), Some(1), "{json_args:?}");
    let text = stdout(&output);
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("stdout is not one JSON value ({err}): {text}"));
    assert_eq!(body["kind"], "error", "{body}");
    assert_eq!(body["code"], code, "{body}");
    assert_eq!(body["valid"], false, "{body}");
    assert_eq!(body["file"], file, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.starts_with("validation failed: "), "{body}");
    // Field order in the raw text: the shared object first, then the validate fields.
    let at = |key: &str| text.find(&format!("\"{key}\":")).unwrap_or(usize::MAX);
    assert!(
        at("kind") < at("error") && at("error") < at("code"),
        "{text}"
    );
    assert!(
        at("code") < at("errors") && at("code") < at("valid"),
        "{text}"
    );

    let mut plain_args = extra.to_vec();
    plain_args.extend_from_slice(&["validate", file]);
    let plain = run(&plain_args);
    assert_eq!(plain.status.code(), Some(1), "{plain_args:?}");
    assert!(stdout(&plain).is_empty(), "{}", stdout(&plain));
    assert_eq!(stderr(&plain), format!("{message}\n"));
    body
}

#[test]
fn validate_missing_file_is_validate_spec_read_file() {
    let dir = TempDir::new("validate-missing");
    let missing = dir.path().join("missing.arazzo.yaml");
    let body = assert_validate_failure(&missing.to_string_lossy(), &[], "VALIDATE_SPEC_READ_FILE");
    assert_eq!(body["errors"][0]["source"], "readFile", "{body}");
    assert!(body.get("warnings").is_none(), "{body}");
}

#[test]
fn validate_unparseable_yaml_is_validate_spec_parse_yaml() {
    let dir = TempDir::new("validate-yaml");
    let file = dir.file("bad.arazzo.yaml", "arazzo: [not a spec");
    let body = assert_validate_failure(&file, &[], "VALIDATE_SPEC_PARSE_YAML");
    assert_eq!(body["errors"][0]["source"], "parseYaml", "{body}");
}

#[test]
fn validate_invalid_document_is_validate_spec_validation() {
    let dir = TempDir::new("validate-invalid");
    let file = dir.file(
        "invalid.arazzo.yaml",
        "arazzo: 1.0.1\ninfo: {title: t, version: '1'}\nsourceDescriptions: []\nworkflows: []\n",
    );
    let body = assert_validate_failure(&file, &[], "VALIDATE_SPEC_VALIDATION");
    assert_eq!(body["errors"][0]["source"], "validation", "{body}");
}

#[test]
fn validate_strict_promotion_is_validate_spec_validation() {
    let file = format!(
        "{}/../../testdata/unsupported-operation-path.arazzo.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    let body = assert_validate_failure(&file, &["--strict"], "VALIDATE_SPEC_VALIDATION");
    assert_eq!(body["errors"].as_array().map(Vec::len), Some(1), "{body}");
    assert!(body.get("warnings").is_none(), "{body}");
}

#[test]
fn schema_validate_describes_the_error_branch() {
    let output = run(&["schema", "validate"]);
    assert_eq!(output.status.code(), Some(0));
    let schema: Value = serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|err| panic!("schema is not JSON: {err}"));
    let branches = schema["anyOf"]
        .as_array()
        .unwrap_or_else(|| panic!("schema must be anyOf success or error: {schema}"));
    assert_eq!(branches.len(), 2, "{schema}");
    let error = &schema["$defs"]["ValidateErrorResult"]["properties"];
    for field in [
        "kind", "error", "code", "valid", "file", "errors", "warnings",
    ] {
        assert!(error.get(field).is_some(), "missing {field}: {schema}");
    }
    assert!(schema["$defs"]["ValidateResult"]["properties"]
        .get("valid")
        .is_some());
}

// ── list, steps, show, catalog failures ─────────────────────────

fn example_spec() -> String {
    format!(
        "{}/../../examples/httpbin-get.arazzo.yaml",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Runs `args` with `--json` and plain; asserts the bare error object with
/// `code` and exit 1, and that plain mode prints the same message on stderr
/// with empty stdout. Returns the message.
fn assert_command_failure(args: &[&str], code: &str) -> String {
    let mut json_args = vec!["--json"];
    json_args.extend_from_slice(args);
    let output = run(&json_args);
    assert_eq!(output.status.code(), Some(1), "{json_args:?}");
    let message = assert_error_object(&output, code);
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));

    let plain = run(args);
    assert_eq!(plain.status.code(), Some(1), "{args:?}");
    assert!(stdout(&plain).is_empty(), "{}", stdout(&plain));
    assert_eq!(stderr(&plain), format!("{message}\n"));
    message
}

#[test]
fn list_missing_file_is_list_spec_read_file() {
    let dir = TempDir::new("list-missing");
    let missing = dir.path().join("missing.arazzo.yaml");
    let message =
        assert_command_failure(&["list", &missing.to_string_lossy()], "LIST_SPEC_READ_FILE");
    assert!(message.starts_with("reading arazzo file: "), "{message}");
}

#[test]
fn steps_unparseable_yaml_is_steps_spec_parse_yaml() {
    let dir = TempDir::new("steps-yaml");
    let file = dir.file("bad.arazzo.yaml", "arazzo: [not a spec");
    assert_command_failure(&["steps", &file, "any"], "STEPS_SPEC_PARSE_YAML");
}

#[test]
fn steps_missing_workflow_is_steps_workflow_not_found() {
    let spec = example_spec();
    let message = assert_command_failure(
        &["steps", &spec, "no-such-workflow"],
        "STEPS_WORKFLOW_NOT_FOUND",
    );
    assert_eq!(
        message,
        format!("workflow \"no-such-workflow\" not found in {spec}")
    );
}

#[test]
fn show_unreadable_dir_is_show_dir_read() {
    let dir = TempDir::new("show-dir");
    let missing = dir.path().join("missing");
    let message = assert_command_failure(
        &["show", "any", "--dir", &missing.to_string_lossy()],
        "SHOW_DIR_READ",
    );
    assert!(message.starts_with("reading directory "), "{message}");
}

#[test]
fn show_missing_workflow_is_show_workflow_not_found() {
    let dir = TempDir::new("show-missing");
    let dir_str = dir.path().to_string_lossy().into_owned();
    let message = assert_command_failure(
        &["show", "no-such-workflow", "--dir", &dir_str],
        "SHOW_WORKFLOW_NOT_FOUND",
    );
    assert_eq!(
        message,
        format!("workflow \"no-such-workflow\" not found in {dir_str}")
    );
}

#[test]
fn show_duplicate_workflow_is_show_workflow_ambiguous() {
    let dir = TempDir::new("show-ambiguous");
    let spec = fs::read_to_string(example_spec())
        .unwrap_or_else(|err| panic!("reading example spec: {err}"));
    dir.file("a.arazzo.yaml", &spec);
    dir.file("b.arazzo.yaml", &spec);
    let message = assert_command_failure(
        &["show", "get-origin", "--dir", &dir.path().to_string_lossy()],
        "SHOW_WORKFLOW_AMBIGUOUS",
    );
    assert!(message.contains("found in multiple files"), "{message}");
}

#[test]
fn catalog_unreadable_dir_is_catalog_dir_read() {
    let dir = TempDir::new("catalog-dir");
    let missing = dir.path().join("missing");
    let message =
        assert_command_failure(&["catalog", &missing.to_string_lossy()], "CATALOG_DIR_READ");
    assert!(message.starts_with("reading directory "), "{message}");
}

#[test]
fn schemas_for_list_steps_show_catalog_describe_the_error_branch() {
    for (command, success_type) in [
        ("list", "array"),
        ("steps", "array"),
        ("catalog", "array"),
        ("show", "WorkflowDetail"),
    ] {
        let output = run(&["schema", command]);
        assert_eq!(output.status.code(), Some(0), "{command}");
        let schema: Value = serde_json::from_str(&stdout(&output))
            .unwrap_or_else(|err| panic!("{command} schema is not JSON: {err}"));
        let branches = schema["anyOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{command} schema must be anyOf: {schema}"));
        assert_eq!(branches.len(), 2, "{schema}");
        let success = branches[0].to_string();
        assert!(success.contains(success_type), "{command}: {success}");
        assert_eq!(
            branches[1],
            json!({"$ref": "#/$defs/ErrorObject"}),
            "{schema}"
        );
        let error = &schema["$defs"]["ErrorObject"];
        assert_eq!(
            error["required"],
            json!(["kind", "error", "code"]),
            "{schema}"
        );
    }
}

// ── generate is YAML-only ───────────────────────────────────────

#[test]
fn generate_json_is_unsupported_with_and_without_output() {
    let dir = TempDir::new("generate-json");
    let out = dir.path().join("out.arazzo.yaml");
    let out = out.to_string_lossy().into_owned();
    for args in [
        vec!["--json", "generate", "--spec", "missing.openapi.yaml"],
        vec![
            "--json",
            "generate",
            "--spec",
            "missing.openapi.yaml",
            "--output",
            &out,
        ],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let message = assert_error_object(&output, "GENERATE_JSON_UNSUPPORTED");
        assert_eq!(message, "generate writes YAML only; remove --json");
        assert!(!Path::new(&out).exists(), "{args:?} must not write a file");
    }
}

#[test]
fn generate_output_must_be_yaml_or_yml() {
    let dir = TempDir::new("generate-output-ext");
    for name in ["out.json", "out", "out.yaml.txt"] {
        let out = dir.path().join(name);
        let out_str = out.to_string_lossy().into_owned();
        let output = run(&[
            "generate",
            "--spec",
            "missing.openapi.yaml",
            "--output",
            &out_str,
        ]);
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(stdout(&output).is_empty(), "{}", stdout(&output));
        let message = stderr(&output);
        assert!(message.contains(&out_str), "{message}");
        assert!(message.contains(".yaml or .yml"), "{message}");
        assert!(!out.exists(), "{name} must not be created");
    }
    // Accepted extensions get past the check and fail on the missing spec.
    for name in ["out.yml", "out.YAML"] {
        let out = dir.path().join(name);
        let out_str = out.to_string_lossy().into_owned();
        let output = run(&[
            "generate",
            "--spec",
            "missing.openapi.yaml",
            "--output",
            &out_str,
        ]);
        let message = stderr(&output);
        assert!(
            message.contains("reading OpenAPI spec"),
            "{name}: {message}"
        );
        assert!(!message.contains(".yaml or .yml"), "{name}: {message}");
    }
}

#[test]
fn schema_generate_is_an_unknown_command() {
    let output = run(&["schema", "generate"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("unknown command"),
        "{}",
        stderr(&output)
    );
}

// ── Sweep: one failing `--json` invocation per subcommand ────────

/// Runs `args` with `--json` first and asserts a non-zero exit and that
/// stdout is exactly one JSON object with `kind: "error"`, a non-empty
/// `error`, and `code` equal to `code`. Command-specific fields may follow.
fn assert_sweep(args: &[&str], code: &str) {
    let mut json_args = vec!["--json"];
    json_args.extend_from_slice(args);
    let output = run(&json_args);
    assert!(!output.status.success(), "{json_args:?} must fail");
    let text = stdout(&output);
    let body: Value = serde_json::from_str(&text).unwrap_or_else(|err| {
        panic!("{json_args:?}: stdout is not one JSON value ({err}): {text}")
    });
    assert!(body.is_object(), "{json_args:?}: {body}");
    assert_eq!(body["kind"], "error", "{json_args:?}: {body}");
    assert!(
        body["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{json_args:?}: error must be a non-empty string: {body}"
    );
    assert_eq!(body["code"], code, "{json_args:?}: {body}");
}

const SWEEP_MISSING: &str = "/nonexistent/arazzo-cli-sweep/missing.arazzo.yaml";
const SWEEP_MISSING_DIR: &str = "/nonexistent/arazzo-cli-sweep/dir";

#[test]
fn sweep_run() {
    assert_sweep(&["run", SWEEP_MISSING, "wf"], "RUN_SPEC_READ_FILE");
}

#[test]
fn sweep_replay() {
    assert_sweep(
        &["replay", "/nonexistent/arazzo-cli-sweep/trace.json"],
        "REPLAY_TRACE_LOAD",
    );
}

#[test]
fn sweep_validate() {
    assert_sweep(&["validate", SWEEP_MISSING], "VALIDATE_SPEC_READ_FILE");
}

#[test]
fn sweep_list() {
    assert_sweep(&["list", SWEEP_MISSING], "LIST_SPEC_READ_FILE");
}

#[test]
fn sweep_catalog() {
    assert_sweep(&["catalog", SWEEP_MISSING_DIR], "CATALOG_DIR_READ");
}

#[test]
fn sweep_show() {
    assert_sweep(&["show", "wf", "--dir", SWEEP_MISSING_DIR], "SHOW_DIR_READ");
}

#[test]
fn sweep_steps() {
    assert_sweep(&["steps", SWEEP_MISSING, "wf"], "STEPS_SPEC_READ_FILE");
}

#[test]
fn sweep_generate() {
    assert_sweep(
        &["generate", "--spec", "missing.openapi.yaml"],
        "GENERATE_JSON_UNSUPPORTED",
    );
}

#[test]
fn sweep_schema() {
    assert_sweep(&["schema", "bogus"], "SCHEMA_UNKNOWN_COMMAND");
    let plain = run(&["schema", "bogus"]);
    assert_eq!(plain.status.code(), Some(1));
    assert!(stdout(&plain).is_empty(), "{}", stdout(&plain));
    assert!(
        stderr(&plain).starts_with("unknown command: \"bogus\""),
        "{}",
        stderr(&plain)
    );
}

#[test]
fn sweep_test() {
    assert_sweep(&["test", SWEEP_MISSING], "TEST_PATH_NOT_FOUND");
}

#[test]
fn sweep_serve() {
    assert_sweep(&["serve"], "SERVE_NO_SPECS");
    assert_sweep(
        &["serve", "--dir", SWEEP_MISSING_DIR],
        "SERVE_SPEC_DISCOVERY",
    );
    assert_sweep(&["serve", SWEEP_MISSING], "SERVE_MCP");
}

#[test]
fn sweep_usage_error() {
    assert_sweep(&["no-such-subcommand"], "CLI_USAGE");
}
