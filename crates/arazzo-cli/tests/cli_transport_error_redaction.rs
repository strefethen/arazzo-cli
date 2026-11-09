#![forbid(unsafe_code)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

const REDACTED_URL: &str = "https://[REDACTED]@127.0.0.1:1/a?api_key=[REDACTED]";

struct Fixture {
    dir: PathBuf,
    spec_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arazzo-transport-error-redaction-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        let source_url = common::fixture_url("https://user:secret@127.0.0.1:1");
        let spec_path = dir.join("spec.arazzo.yaml");
        fs::write(
            &spec_path,
            format!(
                r#"arazzo: 1.1.0
info:
  title: Transport error redaction
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {source_url}
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: unreachable
        operationId: fixtureGetA
        parameters:
          - name: api_key
            in: query
            value: abc
"#
            ),
        )
        .unwrap_or_else(|err| panic!("writing {}: {err}", spec_path.display()));
        Self { dir, spec_path }
    }

    fn run(&self, json: bool, trace_path: &PathBuf) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        if json {
            command.arg("--json");
        }
        command
            .arg("run")
            .arg(&self.spec_path)
            .args(["wf", "--trace"])
            .arg(trace_path)
            .current_dir(&self.dir)
            .output()
            .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn assert_redacted(channel: &str, text: &str) {
    assert!(
        !text.contains("secret"),
        "{channel} leaked userinfo: {text}"
    );
    assert!(!text.contains("abc"), "{channel} leaked api_key: {text}");
}

#[test]
fn connection_failure_redacts_url_in_json_stderr_and_trace() {
    let fixture = Fixture::new();
    let json_trace = fixture.dir.join("json-trace.json");
    let output = fixture.run(true, &json_trace);
    assert!(!output.status.success(), "unreachable host must fail");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_redacted("--json stdout", &stdout);
    assert_redacted("--json stderr", &stderr);
    let document: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("parsing --json output: {err}: {stdout}"));
    let run_error = document["error"]
        .as_str()
        .unwrap_or_else(|| panic!("error missing: {document}"));
    assert!(
        run_error.contains("error sending request") && run_error.contains(REDACTED_URL),
        "{run_error}"
    );
    assert_eq!(document["kind"], "error", "{document}");
    assert_eq!(document["code"], "RUNTIME_HTTP_REQUEST", "{document}");

    let raw_trace = fs::read_to_string(&json_trace)
        .unwrap_or_else(|err| panic!("reading {}: {err}", json_trace.display()));
    assert_redacted("--trace", &raw_trace);
    let trace: Value = serde_json::from_str(&raw_trace)
        .unwrap_or_else(|err| panic!("parsing trace: {err}: {raw_trace}"));
    let trace_error = trace["steps"][0]["error"]
        .as_str()
        .unwrap_or_else(|| panic!("trace step error missing: {trace}"));
    assert!(trace_error.contains(REDACTED_URL), "{trace_error}");

    let human_trace = fixture.dir.join("human-trace.json");
    let output = fixture.run(false, &human_trace);
    assert!(!output.status.success(), "unreachable host must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_redacted("stderr", &stderr);
    assert_redacted("stdout", &String::from_utf8_lossy(&output.stdout));
    assert!(stderr.contains(REDACTED_URL), "{stderr}");
}
