#![forbid(unsafe_code)]

//! A secret passed as a workflow input never reaches a trace file or the
//! `--verbose` `Inputs:` echo in clear (ac-b4a51). An input is secret when its
//! name matches the credential vocabulary regardless of `-`/`_`/space
//! separators, or when its top-level `inputs` schema property declares
//! `writeOnly: true` or `format: password`. Nested input schemas are not
//! consulted, and that non-goal is pinned below.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

const REDACTED: &str = "[REDACTED]";

/// Inputs passed as `--input name=value`; each value is unique so a leak of
/// any one is attributable.
const NAME_MATCHED: [(&str, &str); 5] = [
    ("apiKey", "CLEAR-camel"),
    ("api_key", "CLEAR-snake"),
    ("api-key", "CLEAR-kebab"),
    ("API_KEY", "CLEAR-upper"),
    ("x_api_key", "CLEAR-x-snake"),
];
const SCHEMA_DECLARED: [(&str, &str); 2] =
    [("handle", "CLEAR-writeonly"), ("pin", "CLEAR-password")];
const PLAIN: (&str, &str) = ("plain", "visible-plain");
const NESTED_VALUE: &str = "visible-nested-writeonly";
/// A nested field whose key name is sensitive: redacted by name in both sinks.
const NESTED_SECRET: &str = "CLEAR-nested-password";

fn spec(source_url: &str) -> String {
    format!(
        r#"arazzo: 1.1.0
info:
  title: Secret input redaction
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {source_url}
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        apiKey:
          type: string
        api_key:
          type: string
        api-key:
          type: string
        API_KEY:
          type: string
        x_api_key:
          type: string
        handle:
          type: string
          writeOnly: true
        pin:
          type: string
          format: password
        plain:
          type: string
        config:
          type: object
          properties:
            inner:
              type: string
              writeOnly: true
    steps:
      - stepId: list
        operationId: fixtureGetItems
        parameters:
          - name: api_key
            in: query
            value: $inputs.api_key
          - name: page
            in: query
            value: 2
"#
    )
}

struct Fixture {
    dir: PathBuf,
    spec_path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-secret-input-redaction-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        let source_url = common::fixture_url("https://api.example.com/v1");
        let spec_path = dir.join("spec.arazzo.yaml");
        fs::write(&spec_path, spec(&source_url))
            .unwrap_or_else(|err| panic!("writing {}: {err}", spec_path.display()));
        Self { dir, spec_path }
    }

    fn run(&self, extra: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        command
            .arg("run")
            .arg(&self.spec_path)
            .args(["wf", "--dry-run"])
            .args(extra)
            .current_dir(&self.dir);
        for (name, value) in NAME_MATCHED
            .iter()
            .chain(SCHEMA_DECLARED.iter())
            .chain(std::iter::once(&PLAIN))
        {
            command.arg("--input").arg(format!("{name}={value}"));
        }
        command.arg("--input-json").arg(format!(
            "config={}",
            json!({ "inner": NESTED_VALUE, "password": NESTED_SECRET })
        ));
        command
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

fn secret_values() -> impl Iterator<Item = &'static str> {
    NAME_MATCHED
        .iter()
        .chain(SCHEMA_DECLARED.iter())
        .map(|(_, value)| *value)
        .chain(std::iter::once(NESTED_SECRET))
}

#[test]
fn dry_run_trace_redacts_name_matched_and_schema_declared_inputs() {
    let fixture = Fixture::new("trace");
    let trace_path = fixture.dir.join("trace.json");
    let output = fixture.run(&["--trace", trace_path.to_str().unwrap_or_default()]);
    assert_success(&output);

    let raw = fs::read_to_string(&trace_path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", trace_path.display()));
    for secret in secret_values() {
        assert!(!raw.contains(secret), "{secret} leaked into trace: {raw}");
    }

    let trace = read_json(&trace_path);
    let inputs = &trace["inputs"];
    for (name, _) in NAME_MATCHED.iter().chain(SCHEMA_DECLARED.iter()) {
        assert_eq!(inputs[*name], json!(REDACTED), "input {name}: {inputs}");
    }
    assert_eq!(inputs[PLAIN.0], json!(PLAIN.1), "{inputs}");
    // Non-goal pinned: a `writeOnly` sub-property of a nested object input is
    // not reachable from the typed model, so it is not redacted by schema.
    // A nested field is still redacted by key name.
    assert_eq!(
        inputs["config"],
        json!({ "inner": NESTED_VALUE, "password": REDACTED }),
        "{inputs}"
    );

    let url = trace["steps"][0]["request"]["url"]
        .as_str()
        .unwrap_or_else(|| panic!("no request url in trace: {trace}"));
    assert!(
        url.ends_with("/items?api_key=[REDACTED]&page=2"),
        "query api_key not redacted: {url}"
    );
}

#[test]
fn verbose_inputs_echo_redacts_the_same_names_as_the_trace() {
    let fixture = Fixture::new("verbose");
    let trace_path = fixture.dir.join("trace.json");
    let output = fixture.run(&[
        "--verbose",
        "--trace",
        trace_path.to_str().unwrap_or_default(),
    ]);
    assert_success(&output);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for secret in secret_values() {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "{secret} leaked: stdout={stdout} stderr={stderr}"
        );
    }
    let line = stderr
        .lines()
        .find(|line| line.starts_with("Inputs: "))
        .unwrap_or_else(|| panic!("no Inputs: line in stderr={stderr}"));

    let trace = read_json(&trace_path);
    let inputs = trace["inputs"]
        .as_object()
        .unwrap_or_else(|| panic!("trace inputs not an object: {trace}"));
    for (name, value) in inputs {
        let redacted_in_trace = value == &json!(REDACTED);
        let redacted_in_echo = line.contains(&format!("{name:?}: String({REDACTED:?})"));
        assert_eq!(
            redacted_in_echo, redacted_in_trace,
            "input {name} disagrees between trace ({value}) and echo ({line})"
        );
    }
    assert!(
        line.contains(PLAIN.1),
        "plain input missing from echo: {line}"
    );
    assert!(
        line.contains(NESTED_VALUE),
        "nested input missing from echo: {line}"
    );
    assert!(
        line.contains(&format!("\"password\": String({REDACTED:?})")),
        "nested password not redacted in echo: {line}"
    );
}
