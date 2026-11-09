#![forbid(unsafe_code)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

const PASSWORD: &str = "SYNTHETIC-form-password";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1In0.U1lOVEhFVElD";
const VERBATIM: &str = "<order><item id=\"1\">book</item><note>a &amp; b</note></order>";

struct Fixture {
    dir: PathBuf,
    spec_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-string-body-redaction-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        let source_url = common::fixture_url("https://api.example.com/v1");
        let spec_path = dir.join("spec.arazzo.yaml");
        fs::write(
            &spec_path,
            format!(
                r#"arazzo: 1.1.0
info:
  title: String body redaction
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
        pw:
          type: string
          format: password
    steps:
      - stepId: form
        operationId: fixturePostItems
        requestBody:
          contentType: application/x-www-form-urlencoded
          payload: "grant_type=password&username=u&password={{$inputs.pw}}"
      - stepId: text
        operationId: fixturePostItems
        requestBody:
          contentType: text/plain
          payload: "Authorization: Bearer {JWT}"
      - stepId: xml
        operationId: fixturePostItems
        requestBody:
          contentType: application/xml
          payload: '{VERBATIM}'
"#
            ),
        )
        .unwrap_or_else(|err| panic!("writing {}: {err}", spec_path.display()));
        Self { dir, spec_path }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn string_request_bodies_are_redacted_in_dry_run_json_and_trace() {
    let fixture = Fixture::new();
    let trace_path = fixture.dir.join("trace.json");
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["--json", "run"])
        .arg(&fixture.spec_path)
        .args(["wf", "--dry-run", "--trace"])
        .arg(&trace_path)
        .args(["--input", &format!("pw={PASSWORD}")])
        .current_dir(&fixture.dir)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"));
    assert!(
        output.status.success(),
        "run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let raw_trace = fs::read_to_string(&trace_path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", trace_path.display()));
    let raw_dry_run = String::from_utf8_lossy(&output.stdout);
    for raw in [raw_trace.as_str(), raw_dry_run.as_ref()] {
        assert!(!raw.contains(PASSWORD), "password leaked: {raw}");
        assert!(!raw.contains(JWT), "bearer token leaked: {raw}");
    }

    let dry_run: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("parsing dry-run JSON: {err}"));
    let trace: Value = serde_json::from_str(&raw_trace)
        .unwrap_or_else(|err| panic!("parsing {}: {err}", trace_path.display()));
    for (index, expected) in [
        "grant_type=password&username=u&password=[REDACTED]",
        "Authorization: [REDACTED] [REDACTED]",
        VERBATIM,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(dry_run["requests"][index]["body"], expected, "{dry_run}");
        assert_eq!(
            trace["steps"][index]["request"]["body"], expected,
            "{trace}"
        );
    }
}
