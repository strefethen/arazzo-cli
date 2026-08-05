#![forbid(unsafe_code)]

//! One `operationPath` table, driven through both surfaces that read the field.
//!
//! `arazzo-runtime` resolves `operationPath` into a request URL and
//! `arazzo-validate` checks the Source Description it names. They used to hold
//! separate prefix parsers and disagreed on shipped input: the validator's
//! never stripped a leading `"<METHOD> "` token, so its unknown-source check
//! was silently skipped for all 27 `"<METHOD> {source}./path"` steps across
//! seven `examples/` files. Both now route through
//! `arazzo_spec::classify_operation_path`; this test is what keeps that true.
//!
//! Every row is driven through the shipped CLI rather than through library
//! internals, so agreement is asserted on what users actually run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// What both surfaces must independently conclude about one `operationPath`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Resolvable: validate is silent, the runtime plans a request.
    Supported,
    /// Names a Source Description that the document does not declare.
    UnknownSource,
    /// Specification-valid, but this runtime cannot resolve it.
    UnsupportedForm,
}

/// The shared table. Adding a row here exercises both surfaces at once.
const TABLE: &[(&str, Verdict)] = &[
    ("/pets", Verdict::Supported),
    ("GET /pets", Verdict::Supported),
    ("/pets/{petId}", Verdict::Supported),
    ("{api}./pets", Verdict::Supported),
    ("POST {api}./pets", Verdict::Supported),
    ("https://elsewhere.example.com/pets", Verdict::Supported),
    (
        "DELETE https://elsewhere.example.com/pets/7",
        Verdict::Supported,
    ),
    ("{missing}./pets", Verdict::UnknownSource),
    // The regression this consolidation fixes: the method token used to hide
    // the source reference from the validator entirely.
    ("POST {missing}./pet", Verdict::UnknownSource),
    (
        "{$sourceDescriptions.api.url}#/paths/~1pets/get",
        Verdict::UnsupportedForm,
    ),
    (
        "GET {$sourceDescriptions.api.url}#/paths/~1pets/get",
        Verdict::UnsupportedForm,
    ),
    ("{$sourceDescriptions.api.url}", Verdict::UnsupportedForm),
    ("#/paths/~1pets/get", Verdict::UnsupportedForm),
];

const UNSUPPORTED_CODE: &str = "RUNTIME_UNSUPPORTED_OPERATION_PATH_FORM";
const UNKNOWN_SOURCE_CODE: &str = "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND";
/// `run` validates before executing, so an unknown source is now reported by
/// the validation gate rather than by the engine. Same verdict, earlier stage —
/// and reaching that gate at all is the regression this ticket fixes for
/// `"<METHOD> {source}./path"` values.
const RUN_VALIDATION_CODE: &str = "RUN_SPEC_VALIDATION";
const UNSUPPORTED_KIND: &str = "unsupportedOperationPath";
const UNKNOWN_SOURCE_KIND: &str = "invalidReference";

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
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => panic!("CARGO_BIN_EXE_arazzo-cli missing"),
    }
}

fn write_spec(dir: &Path, operation_path: &str) -> String {
    let path = dir.join("agreement.arazzo.yaml");
    let spec = format!(
        r#"arazzo: 1.0.1
info:
  title: operationPath agreement
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/v1
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: probe
        operationPath: {operation_path:?}
"#
    );
    if let Err(err) = fs::write(&path, spec) {
        panic!("writing {}: {err}", path.display());
    }
    path.to_string_lossy().into_owned()
}

fn run_json(args: &[&str]) -> (bool, Value) {
    let output = match Command::new(cli_bin()).args(args).output() {
        Ok(output) => output,
        Err(err) => panic!("running arazzo-cli {args:?}: {err}"),
    };
    let body = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(body) => body,
        Err(err) => panic!(
            "parsing JSON from {args:?}: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    };
    (output.status.success(), body)
}

fn diagnostic_kinds(body: &Value, field: &str) -> Vec<String> {
    body.get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("kind").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// What `arazzo validate` concludes about one `operationPath`.
fn validate_verdict(spec: &str) -> Verdict {
    let (ok, body) = run_json(&["--json", "validate", spec]);
    let errors = diagnostic_kinds(&body, "errors");
    let warnings = diagnostic_kinds(&body, "warnings");

    if errors.iter().any(|kind| kind == UNKNOWN_SOURCE_KIND) {
        assert!(!ok, "an unknown sourceDescription must fail validate");
        return Verdict::UnknownSource;
    }
    if warnings.iter().any(|kind| kind == UNSUPPORTED_KIND) {
        assert!(
            ok,
            "an unresolvable operationPath is a warning, not a validation failure"
        );
        return Verdict::UnsupportedForm;
    }
    assert!(ok, "unexpected validation failure: {body}");
    Verdict::Supported
}

/// What `arazzo run --dry-run` concludes about the same value.
fn runtime_verdict(spec: &str) -> Verdict {
    let (ok, body) = run_json(&["--json", "run", spec, "wf", "--dry-run"]);
    if ok {
        assert_eq!(
            body.get("kind").and_then(Value::as_str),
            Some("dryRun"),
            "body={body}"
        );
        return Verdict::Supported;
    }
    let message = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match body.get("code").and_then(Value::as_str) {
        Some(UNSUPPORTED_CODE) => Verdict::UnsupportedForm,
        Some(UNKNOWN_SOURCE_CODE) => Verdict::UnknownSource,
        Some(RUN_VALIDATION_CODE) if message.contains("unknown sourceDescription") => {
            Verdict::UnknownSource
        }
        other => panic!("unexpected runtime failure code {other:?}; body={body}"),
    }
}

#[test]
fn runtime_and_validate_agree_on_every_operation_path_form() {
    let temp = TempDir::new("arazzo-operation-path-agreement");

    for (operation_path, expected) in TABLE {
        let spec = write_spec(&temp.path, operation_path);

        let from_validate = validate_verdict(&spec);
        let from_runtime = runtime_verdict(&spec);

        assert_eq!(
            from_validate, from_runtime,
            "validate and runtime disagree on {operation_path:?}"
        );
        assert_eq!(
            from_validate, *expected,
            "unexpected verdict for {operation_path:?}"
        );
    }
}

/// The stable `RUNTIME_*` code the `--json` contract exposes for the new
/// failure, asserted on the shipped fixture rather than a generated one.
#[test]
fn dry_run_json_surfaces_the_stable_unsupported_code() {
    let spec = "testdata/unsupported-operation-path.arazzo.yaml";
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let spec_path = repo_root.join(spec);

    let (ok, body) = run_json(&[
        "--json",
        "run",
        &spec_path.to_string_lossy(),
        "probe",
        "--dry-run",
    ]);

    assert!(!ok, "an unresolvable operationPath must exit non-zero");
    assert_eq!(body.get("kind").and_then(Value::as_str), Some("error"));
    assert_eq!(
        body.get("code").and_then(Value::as_str),
        Some(UNSUPPORTED_CODE),
        "body={body}"
    );

    let message = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(message.contains("list-pets"), "message was: {message}");
    assert!(
        message.contains("{$sourceDescriptions.petstore.url}#/paths/~1pets/get"),
        "message was: {message}"
    );
    // The literal expression and pointer text must never reach a URL.
    assert!(
        !message.contains("https://petstore.example.com/v1{$"),
        "the concatenated URL must not be constructed: {message}"
    );
}
