#![forbid(unsafe_code)]

//! `run --verbose` prints each step as `[status] METHOD url`. The URL gets the
//! same query redaction the trace file applies, so a credential in a query
//! parameter never reaches the terminal or CI logs (ac-f2305).

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

const SECRET: &str = "SECRET-QUERY-VALUE";

fn spec(base_url: &str) -> String {
    let base_url = common::fixture_url(base_url);
    format!(
        r#"arazzo: 1.0.1
info:
  title: Verbose redaction
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {base_url}
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: list
        operationId: fixtureGetItems
        parameters:
          - name: access_token
            in: query
            value: {SECRET}
          - name: page
            in: query
            value: 2
        successCriteria:
          - condition: $statusCode == 200
"#
    )
}

#[test]
fn verbose_step_line_redacts_sensitive_query_values() {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("binding test server: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let handle = thread::spawn(move || {
        while let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(1)) {
            request
                .respond(tiny_http::Response::from_string("{}"))
                .unwrap_or_else(|err| panic!("responding: {err}"));
        }
    });

    let dir = std::env::temp_dir().join(format!("arazzo-verbose-redaction-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
    let spec_path: PathBuf = dir.join("spec.arazzo.yaml");
    fs::write(&spec_path, spec(&base_url))
        .unwrap_or_else(|err| panic!("writing {}: {err}", spec_path.display()));

    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .arg("run")
        .arg(&spec_path)
        .args(["wf", "--verbose"])
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"));
    handle
        .join()
        .unwrap_or_else(|_| panic!("server thread panicked"));
    let _ = fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "run failed: stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stdout.contains(SECRET) && !stderr.contains(SECRET),
        "secret leaked: stdout={stdout} stderr={stderr}"
    );
    let line = stdout
        .lines()
        .find(|line| line.contains("GET "))
        .unwrap_or_else(|| panic!("no step line in stdout={stdout}"));
    assert!(
        line.starts_with("  [200] GET http://127.0.0.1:")
            && line.contains("/items?access_token=[REDACTED]&page=2  ("),
        "unexpected step line: {line}"
    );
}

#[test]
fn dry_run_redacts_userinfo_with_multiple_at_signs() {
    const SUFFIX: &str = "LEAKED-SECRET";
    let dir = std::env::temp_dir().join(format!(
        "arazzo-multiple-at-redaction-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating fixture: {err}"));
    let spec_path = dir.join("spec.arazzo.yaml");
    fs::write(
        &spec_path,
        spec(&format!("https://user:prefix@{SUFFIX}@example.com")),
    )
    .unwrap_or_else(|err| panic!("writing spec: {err}"));
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["--json", "run"])
        .arg(&spec_path)
        .args(["wf", "--dry-run"])
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("running CLI: {err}"));
    assert!(output.status.success(), "dry-run failed: {output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SUFFIX));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SUFFIX));
    let body: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("parsing dry-run response: {err}"));
    assert_eq!(
        body["requests"][0]["url"],
        "https://[REDACTED]@example.com/items?access_token=[REDACTED]&page=2"
    );
    fs::remove_dir_all(&dir).unwrap_or_else(|err| panic!("removing fixture: {err}"));
}

#[test]
fn credentialed_request_trace_and_verbose_output_redact_userinfo() {
    const PASSWORD: &str = "URL-USERINFO-PASSWORD";
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("binding test server: {err:?}"));
    let base_url = format!("http://user:{PASSWORD}@{}", server.server_addr());
    let destination = format!("{base_url}/final");
    let handle = thread::spawn(move || {
        for index in 0..2 {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|err| panic!("receiving: {err:?}"))
                .unwrap_or_else(|| panic!("request"));
            let response = if index == 0 {
                tiny_http::Response::from_string("")
                    .with_status_code(302)
                    .with_header(
                        tiny_http::Header::from_bytes("Location", destination.as_str())
                            .unwrap_or_else(|err| panic!("location header: {err:?}")),
                    )
            } else {
                tiny_http::Response::from_string("{}")
            };
            request
                .respond(response)
                .unwrap_or_else(|err| panic!("responding: {err:?}"));
        }
    });
    let dir =
        std::env::temp_dir().join(format!("arazzo-userinfo-redaction-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("creating fixture directory: {err:?}"));
    let spec_path = dir.join("spec.arazzo.yaml");
    let trace_path = dir.join("trace.json");
    fs::write(&spec_path, spec(&base_url)).unwrap_or_else(|err| panic!("writing spec: {err:?}"));
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .arg("run")
        .arg(&spec_path)
        .args(["wf", "--verbose", "--trace"])
        .arg(&trace_path)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("running CLI: {err:?}"));
    handle
        .join()
        .unwrap_or_else(|err| panic!("server thread: {err:?}"));
    assert!(output.status.success(), "run failed: {output:?}");
    let bytes = fs::read(&trace_path).unwrap_or_else(|err| panic!("reading trace bytes: {err:?}"));
    assert!(
        !bytes
            .windows(PASSWORD.len())
            .any(|part| part == PASSWORD.as_bytes()),
        "password leaked in trace bytes"
    );
    let trace: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("trace JSON: {err:?}"));
    let request = &trace["steps"][0]["request"];
    assert!(request["url"]
        .as_str()
        .unwrap_or_else(|| panic!("request URL"))
        .contains("[REDACTED]@"));
    let hops = request["redirects"]
        .as_array()
        .unwrap_or_else(|| panic!("redirects"));
    assert_eq!(hops.len(), 1);
    for field in ["from", "to"] {
        assert!(hops[0][field]
            .as_str()
            .unwrap_or_else(|| panic!("hop URL"))
            .contains("[REDACTED]@"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains(PASSWORD) && !stderr.contains(PASSWORD));
    assert!(stdout.contains("[REDACTED]@"));
    fs::remove_dir_all(&dir).unwrap_or_else(|err| panic!("removing fixture directory: {err:?}"));
}
