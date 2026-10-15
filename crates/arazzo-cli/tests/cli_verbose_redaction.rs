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
