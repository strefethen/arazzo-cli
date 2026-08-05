#![forbid(unsafe_code)]

//! Golden baseline sweep over every Arazzo document in `examples/` and
//! `testdata/`.
//!
//! This pins what each document does *today* — its validate outcome and
//! diagnostics, and the HTTP method and request URL each step resolves to under
//! `--dry-run` — so that a change to validation or URL construction shows up as
//! a failing test naming the file, workflow, and step, rather than as a
//! close-time checklist item nobody remembers to run.
//!
//! The document list is discovered from the filesystem, never hard-coded: a new
//! `*.arazzo.yaml` or `*.arazzo.yml` under either directory fails this test
//! until it has a golden entry.
//!
//! Regenerating is deliberate and never automatic:
//!
//! ```text
//! UPDATE_SPEC_BASELINE=1 cargo test -p arazzo-cli --test golden_spec_baseline
//! ```
//!
//! Review the resulting diff: a changed URL or diagnostic is a behavior change,
//! not a formality.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Map, Value};

/// Golden baseline, relative to this crate's manifest directory.
const GOLDEN_PATH: &str = "tests/golden/spec-baseline.json";
/// Fixed `$inputs` used for workflows whose URLs depend on them.
const INPUTS_PATH: &str = "tests/golden/spec-baseline-inputs.json";
/// Opt-in required to rewrite the golden. A plain `cargo test` never does.
const UPDATE_ENV: &str = "UPDATE_SPEC_BASELINE";

/// Directories swept, relative to the workspace root.
const SWEPT_DIRS: [&str; 2] = ["examples", "testdata"];

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
                    "CLI binary not found at {}; CARGO_BIN_EXE_arazzo-cli missing",
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
        Ok(path) => path,
        Err(err) => panic!("canonicalizing repo root {}: {err}", path.display()),
    }
}

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Every Arazzo document under the swept directories, recursively, as
/// forward-slash paths relative to the workspace root.
///
/// Both spec extensions count: `.yml` is supported by the CLI, and a
/// `.yaml`-only sweep would silently skip a file, which is the exact rot this
/// test exists to prevent.
fn discover_specs() -> Vec<String> {
    let root = repo_root();
    let mut specs = Vec::new();
    for dir in SWEPT_DIRS {
        collect_specs(&root.join(dir), &root, &mut specs);
    }
    specs.sort();
    assert!(
        !specs.is_empty(),
        "no Arazzo documents discovered under {SWEPT_DIRS:?}"
    );
    specs
}

fn collect_specs(dir: &Path, root: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => panic!("reading {}: {err}", dir.display()),
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(err) => panic!("reading entry under {}: {err}", dir.display()),
        };
        if path.is_dir() {
            collect_specs(&path, root, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Both directories also hold non-Arazzo files (README.md, OpenAPI
        // documents); only Arazzo specs belong in the sweep.
        if !(name.ends_with(".arazzo.yaml") || name.ends_with(".arazzo.yml")) {
            continue;
        }
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative,
            Err(err) => panic!("{} is not under {}: {err}", path.display(), root.display()),
        };
        out.push(
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
}

struct CliRun {
    success: bool,
    stdout: Option<Value>,
    stderr: String,
}

/// Runs the CLI from the workspace root so every path in captured output is
/// relative and therefore machine-independent.
fn run_cli(args: &[&str]) -> CliRun {
    let output = match Command::new(cli_bin())
        .current_dir(repo_root())
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(err) => panic!("running arazzo-cli {args:?}: {err}"),
    };
    CliRun {
        success: output.status.success(),
        stdout: serde_json::from_slice::<Value>(&output.stdout).ok(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn read_json(path: &Path, hint: &str) -> Value {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) => panic!("reading {}: {err}\n{hint}", path.display()),
    };
    match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(err) => panic!("parsing {}: {err}", path.display()),
    }
}

/// The fixed input set, keyed by spec path then workflow id.
fn fixed_inputs() -> Value {
    read_json(
        &crate_path(INPUTS_PATH),
        "the sweep needs its fixed input set to resolve URLs reproducibly",
    )
}

fn issues(body: Option<&Value>, field: &str, severity: &str) -> Vec<Value> {
    let Some(items) = body
        .and_then(|body| body.get(field))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            json!({
                "severity": severity,
                "kind": item.get("kind").and_then(Value::as_str).unwrap_or_default(),
                "path": item.get("path").and_then(Value::as_str).unwrap_or_default(),
                "message": item.get("message").and_then(Value::as_str).unwrap_or_default(),
            })
        })
        .collect()
}

fn validate_entry(spec: &str) -> (Value, bool) {
    let run = run_cli(&["--json", "validate", spec]);
    let body = run.stdout.as_ref();
    let mut diagnostics = issues(body, "errors", "error");
    diagnostics.extend(issues(body, "warnings", "warning"));

    let outcome = if run.success { "valid" } else { "invalid" };
    (
        json!({ "outcome": outcome, "diagnostics": diagnostics }),
        run.success,
    )
}

fn workflow_ids(spec: &str) -> Result<Vec<String>, String> {
    let run = run_cli(&["--json", "list", spec]);
    if !run.success {
        return Err(if run.stderr.is_empty() {
            "list failed".to_string()
        } else {
            run.stderr
        });
    }
    let Some(rows) = run.stdout.as_ref().and_then(Value::as_array) else {
        return Err("list did not return an array".to_string());
    };
    Ok(rows
        .iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect())
}

/// Renders the fixed inputs for one workflow as CLI flags, preserving JSON
/// types: strings go through `--input`, everything else through `--input-json`.
fn input_flags(inputs: Option<&Map<String, Value>>) -> Vec<String> {
    let Some(inputs) = inputs else {
        return Vec::new();
    };
    let mut flags = Vec::new();
    for (key, value) in inputs {
        match value {
            Value::String(text) => {
                flags.push("--input".to_string());
                flags.push(format!("{key}={text}"));
            }
            other => {
                flags.push("--input-json".to_string());
                flags.push(format!("{key}={other}"));
            }
        }
    }
    flags
}

fn workflow_entry(spec: &str, workflow: &str, inputs: Option<&Map<String, Value>>) -> Value {
    let flags = input_flags(inputs);
    let mut args = vec!["--json", "run", spec, workflow, "--dry-run"];
    args.extend(flags.iter().map(String::as_str));

    let run = run_cli(&args);
    let mut entry = Map::new();
    if let Some(inputs) = inputs {
        entry.insert("inputs".to_string(), Value::Object(inputs.clone()));
    }

    let kind = run
        .stdout
        .as_ref()
        .and_then(|body| body.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !run.success || kind != "dryRun" {
        // Never silently skipped: a workflow that cannot be dry-run is recorded
        // with the reason it could not be.
        let reason = run
            .stdout
            .as_ref()
            .and_then(|body| body.get("error"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| (!run.stderr.is_empty()).then(|| run.stderr.clone()))
            .unwrap_or_else(|| "dry-run produced no JSON envelope".to_string());
        entry.insert("outcome".to_string(), json!("notDryRunnable"));
        entry.insert("reason".to_string(), json!(reason));
        return Value::Object(entry);
    }

    let steps = run
        .stdout
        .as_ref()
        .and_then(|body| body.get("requests"))
        .and_then(Value::as_array)
        .map(|requests| {
            requests
                .iter()
                .map(|request| {
                    json!({
                        "stepId": request.get("stepId").and_then(Value::as_str).unwrap_or_default(),
                        "method": request.get("method").and_then(Value::as_str).unwrap_or_default(),
                        "url": request.get("url").and_then(Value::as_str).unwrap_or_default(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    entry.insert("outcome".to_string(), json!("dryRun"));
    entry.insert("steps".to_string(), Value::Array(steps));
    Value::Object(entry)
}

fn build_baseline() -> Value {
    let inputs = fixed_inputs();
    let mut files = Map::new();

    for spec in discover_specs() {
        let (validate, valid) = validate_entry(&spec);
        let mut entry = Map::new();
        entry.insert("validate".to_string(), validate);

        let mut workflows = Map::new();
        match workflow_ids(&spec) {
            Ok(ids) => {
                for id in ids {
                    let workflow_inputs = inputs
                        .get(&spec)
                        .and_then(|per_file| per_file.get(&id))
                        .and_then(Value::as_object);
                    workflows.insert(id.clone(), workflow_entry(&spec, &id, workflow_inputs));
                }
            }
            Err(reason) => {
                // A document that does not validate cannot have its workflows
                // enumerated; record why rather than dropping the file.
                assert!(
                    !valid,
                    "{spec} validates but its workflows could not be listed: {reason}"
                );
                entry.insert("workflowsUnavailable".to_string(), json!(reason));
            }
        }
        entry.insert("workflows".to_string(), Value::Object(workflows));
        files.insert(spec, Value::Object(entry));
    }

    json!({
        "about": [
            "Golden baseline for every Arazzo document in examples/ and testdata/.",
            "Records the validate outcome and diagnostics per file, and the resolved",
            "dry-run HTTP method and request URL per workflow step.",
            "Inputs shown here come from tests/golden/spec-baseline-inputs.json.",
            "Regenerate deliberately: UPDATE_SPEC_BASELINE=1 cargo test -p arazzo-cli --test golden_spec_baseline",
        ],
        "files": files,
    })
}

/// Recursively collects human-readable mismatches, naming the JSON path so a
/// failure points at the exact file, workflow, and step.
fn collect_diff(path: &str, golden: &Value, actual: &Value, out: &mut Vec<String>) {
    match (golden, actual) {
        (Value::Object(golden_map), Value::Object(actual_map)) => {
            for (key, golden_value) in golden_map {
                let child = format!("{path}.{key}");
                match actual_map.get(key) {
                    Some(actual_value) => collect_diff(&child, golden_value, actual_value, out),
                    None => out.push(format!("{child}: in golden, missing from this run")),
                }
            }
            for key in actual_map.keys() {
                if !golden_map.contains_key(key) {
                    out.push(format!(
                        "{path}.{key}: present in this run, missing from the golden \
                         (regenerate with {UPDATE_ENV}=1 once you have confirmed it is intended)"
                    ));
                }
            }
        }
        (Value::Array(golden_items), Value::Array(actual_items)) => {
            for (idx, golden_item) in golden_items.iter().enumerate() {
                let child = format!("{path}[{idx}]");
                match actual_items.get(idx) {
                    Some(actual_item) => collect_diff(&child, golden_item, actual_item, out),
                    None => out.push(format!("{child}: in golden, missing from this run")),
                }
            }
            for (idx, actual_item) in actual_items.iter().enumerate().skip(golden_items.len()) {
                out.push(format!(
                    "{path}[{idx}]: present in this run, missing from the golden: {actual_item}"
                ));
            }
        }
        (golden, actual) if golden != actual => {
            out.push(format!(
                "{path}:\n    golden: {golden}\n    actual: {actual}"
            ));
        }
        _ => {}
    }
}

fn write_golden(baseline: &Value) {
    let mut serialized = match serde_json::to_string_pretty(baseline) {
        Ok(serialized) => serialized,
        Err(err) => panic!("serializing baseline: {err}"),
    };
    serialized.push('\n');
    let path = crate_path(GOLDEN_PATH);
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            panic!("creating {}: {err}", parent.display());
        }
    }
    if let Err(err) = fs::write(&path, serialized) {
        panic!("writing {}: {err}", path.display());
    }
}

#[test]
fn spec_baseline_matches_golden() {
    let baseline = build_baseline();

    if std::env::var_os(UPDATE_ENV).is_some() {
        write_golden(&baseline);
        return;
    }

    let golden = read_json(
        &crate_path(GOLDEN_PATH),
        "regenerate with: UPDATE_SPEC_BASELINE=1 cargo test -p arazzo-cli --test golden_spec_baseline",
    );

    let mut mismatches = Vec::new();
    collect_diff("baseline", &golden, &baseline, &mut mismatches);

    assert!(
        mismatches.is_empty(),
        "spec baseline drift ({} mismatch(es)).\n\n{}\n\n\
         A changed URL or diagnostic is a behavior change. If it is intended, \
         regenerate with:\n  {UPDATE_ENV}=1 cargo test -p arazzo-cli --test golden_spec_baseline",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// The sweep is only a guard if it actually covers the corpus; an empty or
/// truncated golden would otherwise pass silently.
#[test]
fn golden_covers_every_discovered_document() {
    if std::env::var_os(UPDATE_ENV).is_some() {
        // The golden is being rewritten from this same discovery walk by the
        // sibling test; reading it mid-write proves nothing.
        return;
    }

    let golden = read_json(
        &crate_path(GOLDEN_PATH),
        "regenerate with: UPDATE_SPEC_BASELINE=1 cargo test -p arazzo-cli --test golden_spec_baseline",
    );
    let files = match golden.get("files").and_then(Value::as_object) {
        Some(files) => files,
        None => panic!("golden has no files map"),
    };

    let discovered = discover_specs();
    let missing = discovered
        .iter()
        .filter(|spec| !files.contains_key(*spec))
        .collect::<Vec<_>>();
    let stale = files
        .keys()
        .filter(|spec| !discovered.contains(spec))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "golden is out of step with the corpus\n  missing entries: {missing:?}\n  \
         entries for documents that no longer exist: {stale:?}\n  \
         regenerate with {UPDATE_ENV}=1 cargo test -p arazzo-cli --test golden_spec_baseline"
    );
}
