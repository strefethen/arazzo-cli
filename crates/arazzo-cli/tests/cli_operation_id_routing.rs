#![forbid(unsafe_code)]

//! The `run` contract for source-qualified `operationId` targets.
//!
//! GitHub issue #5 is a two-source document whose steps all went to the first
//! source's host. `--dry-run` is where an operator sees that before sending
//! anything, so the plan it prints is pinned here, together with the `--json`
//! error contract for every `operationId` the runtime refuses.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const ALPHA_BASE: &str = "https://alpha.example.com/v1";
const BETA_BASE: &str = "https://beta.example.com/v2";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-cli-operation-id-routing-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let target = self.path.join(name);
        fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
        target
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn cli_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        return PathBuf::from(path);
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/arazzo-cli");
    assert!(path.exists(), "CLI binary not found at {}", path.display());
    path
}

fn openapi_document(base: &str, path: &str) -> String {
    [
        "openapi: \"3.0.3\"".to_string(),
        "info:".to_string(),
        "  title: fixture".to_string(),
        "  version: \"1.0.0\"".to_string(),
        "servers:".to_string(),
        format!("  - url: {base}"),
        "paths:".to_string(),
        format!("  {path}:"),
        "    get:".to_string(),
        "      operationId: getPet".to_string(),
        "      responses:".to_string(),
        "        \"200\":".to_string(),
        "          description: OK".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// A two-source Arazzo document — the issue #5 shape — with one step per
/// supplied `operationId` value.
fn arazzo_document(operation_ids: &[&str]) -> String {
    let mut lines = vec![
        "arazzo: 1.0.0".to_string(),
        "info:".to_string(),
        "  title: operationId routing".to_string(),
        "  version: 1.0.0".to_string(),
        "sourceDescriptions:".to_string(),
        "  - name: alpha".to_string(),
        "    url: ./alpha.openapi.yaml".to_string(),
        "    type: openapi".to_string(),
        "  - name: beta".to_string(),
        "    url: ./beta.openapi.yaml".to_string(),
        "    type: openapi".to_string(),
        "workflows:".to_string(),
        "  - workflowId: wf".to_string(),
        "    steps:".to_string(),
    ];
    for (idx, operation_id) in operation_ids.iter().enumerate() {
        lines.push(format!("      - stepId: step{idx}"));
        lines.push(format!("        operationId: \"{operation_id}\""));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Writes both OpenAPI documents plus an Arazzo document naming
/// `operation_ids`, and returns the path to run.
fn fixture(dir: &TempDir, operation_ids: &[&str]) -> PathBuf {
    dir.write("alpha.openapi.yaml", &openapi_document(ALPHA_BASE, "/pets"));
    dir.write(
        "beta.openapi.yaml",
        &openapi_document(BETA_BASE, "/animals"),
    );
    dir.write("routing.arazzo.yaml", &arazzo_document(operation_ids))
}

fn run_json(spec: &Path, extra: &[&str]) -> (bool, Value) {
    let mut args: Vec<String> = vec![
        "--json".to_string(),
        "run".to_string(),
        spec.display().to_string(),
        "wf".to_string(),
    ];
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    let output = Command::new(cli_bin())
        .args(&args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"));
    let parsed = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "parsing JSON stdout failed: {err}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), parsed)
}

/// Reads a string field, returning it owned.
///
/// Owned rather than borrowed on purpose: the conformance manifest's source
/// scanner reads `'a` as an opening char literal and blanks the rest of the
/// file, so a lifetime annotation anywhere in an evidence-bearing test target
/// makes its tests invisible to that gate.
fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string {field} in {value}"))
        .to_string()
}

/// The issue #5 repro: two steps, two sources, two hosts. Before the fix both
/// planned URLs carried the first source's host.
#[test]
fn dry_run_plans_a_distinct_host_per_qualified_source() {
    let dir = TempDir::new();
    let spec = fixture(
        &dir,
        &[
            "$sourceDescriptions.alpha.getPet",
            "$sourceDescriptions.beta.getPet",
        ],
    );

    let (ok, output) = run_json(&spec, &["--dry-run"]);
    assert!(ok, "dry run failed: {output}");
    assert_eq!(string_field(&output, "kind"), "dryRun");

    let requests = output
        .get("requests")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a requests array in {output}"));
    assert_eq!(requests.len(), 2, "planned requests: {output}");

    let urls: Vec<String> = requests
        .iter()
        .map(|request| string_field(request, "url"))
        .collect();
    assert_eq!(
        urls,
        vec![format!("{ALPHA_BASE}/pets"), format!("{BETA_BASE}/animals")]
    );
    assert_ne!(
        urls[0], urls[1],
        "the two steps must not plan the same host"
    );
}

/// Every refusal reaches `--json` as the documented error object with a
/// stable `code`, so a caller can branch on the cause instead of matching
/// message text.
#[test]
fn refused_operation_ids_carry_their_stable_code_in_json() {
    let cases: &[(&str, &str)] = &[
        // Unqualified, with two non-arazzo sources defined.
        ("getPet", "RUNTIME_OPERATION_ID_AMBIGUOUS"),
        // Qualified at a source the document does not declare.
        (
            "$sourceDescriptions.gamma.getPet",
            "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND",
        ),
        // Qualified at a source that does not define the operation.
        (
            "$sourceDescriptions.alpha.listPets",
            "RUNTIME_OPERATION_ID_NOT_FOUND",
        ),
        // Reaching for the qualified form without satisfying the
        // `source-reference` production: no operation segment, and an
        // operation segment outside the `CHAR` rule.
        (
            "$sourceDescriptions.getPet",
            "RUNTIME_UNSUPPORTED_OPERATION_ID_FORM",
        ),
        (
            "$sourceDescriptions.alpha.get{Pet}",
            "RUNTIME_UNSUPPORTED_OPERATION_ID_FORM",
        ),
        // A dotted operation name is legal grammar naming a real source, so it
        // reaches lookup and fails there — not at the form.
        (
            "$sourceDescriptions.alpha.v2.getPet",
            "RUNTIME_OPERATION_ID_NOT_FOUND",
        ),
    ];

    for (operation_id, code) in cases {
        let dir = TempDir::new();
        let spec = fixture(&dir, &[operation_id]);
        // A dry run is enough: the refusal happens before a request is built,
        // so it does not depend on sending anything.
        let (ok, output) = run_json(&spec, &["--dry-run"]);
        assert!(!ok, "expected {operation_id} to fail, got {output}");
        assert_eq!(
            string_field(&output, "kind"),
            "error",
            "output for {operation_id}"
        );
        assert_eq!(
            string_field(&output, "code"),
            *code,
            "code for {operation_id}"
        );
        assert!(
            string_field(&output, "error").contains(operation_id),
            "the message must quote the target; output for {operation_id}: {output}"
        );
    }
}

/// The human-readable path carries the same refusal — an operator running
/// without `--json` still learns which forms are accepted.
#[test]
fn refused_unqualified_operation_id_names_both_supported_forms() {
    let dir = TempDir::new();
    let spec = fixture(&dir, &["getPet"]);
    let output = Command::new(cli_bin())
        .args(["run", &spec.display().to_string(), "wf", "--dry-run"])
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"));

    assert!(!output.status.success(), "expected a non-zero exit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("$sourceDescriptions.<name>.getPet"),
        "stderr was: {text}"
    );
    assert!(text.contains("{<name>}./<path>"), "stderr was: {text}");
}
