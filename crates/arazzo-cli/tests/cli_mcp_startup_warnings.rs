#![forbid(unsafe_code)]

//! `serve` prints the fixed operator-trust notice on stderr exactly once at
//! startup, before discovery, loading, or any request, with or without a TTY
//! and with `--json`. stdout stays MCP-only, and no other command prints it.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// The notice, written out independently of the implementation.
const NOTICE_LINES: [&str; 3] = [
    "notice: the connected MCP client can execute every loaded workflow, including its API side effects, using this process's credentials and environment. No approval is requested per operation.",
    "notice: file tools and referenced documents are limited to the configured roots; this is a path restriction, not a sandbox, and does not isolate the process from other local software.",
    "notice: review README.md (MCP Server > Security) and SECURITY.md before connecting an agent.",
];

/// A distinctive value placed in paths and the environment; the notice must
/// never echo it.
const SENTINEL: &str = "sentinel-abdc8-c0ffee-must-not-leak";

const SPEC: &str = "\
arazzo: 1.0.0
info:
  title: mcp startup warning
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: ./fixture.openapi.json
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: s
        operationId: fixtureGetGet
        successCriteria:
          - condition: $statusCode == 200
";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        // Unroutable server: nothing in these tests sends a request.
        common::write_in(&path, "http://127.0.0.1:9");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.path.join(name), contents)
            .unwrap_or_else(|err| panic!("writing {name}: {err}"));
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Runs the CLI in `dir`, writes `stdin` then closes it, and waits at most
/// 20 seconds, killing and reaping the child on timeout.
fn run_cli(dir: &Path, args: &[&str], stdin: &[u8], env: &[(&str, &str)]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|err| panic!("spawning arazzo-cli: {err}"));

    let mut stdout_pipe = child
        .stdout
        .take()
        .unwrap_or_else(|| panic!("piped stdout"));
    let mut stderr_pipe = child
        .stderr
        .take()
        .unwrap_or_else(|| panic!("piped stderr"));
    let stdout_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    {
        let mut pipe = child.stdin.take().unwrap_or_else(|| panic!("piped stdin"));
        // The child may exit before reading (startup failure); a broken pipe
        // is not a test failure.
        let _ = pipe.write_all(stdin);
    } // stdin closed here.

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child
            .try_wait()
            .unwrap_or_else(|err| panic!("polling arazzo-cli: {err}"))
        {
            Some(status) => break status,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("arazzo-cli {args:?} did not exit within 20s");
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };

    let stdout = stdout_reader
        .join()
        .unwrap_or_else(|_| panic!("stdout reader panicked"));
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| panic!("stderr reader panicked"));
    Run {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

fn frame(msg: &Value) -> Vec<u8> {
    let payload =
        serde_json::to_string(msg).unwrap_or_else(|err| panic!("serializing request: {err}"));
    format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload).into_bytes()
}

fn initialize_request() -> Vec<u8> {
    frame(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1"}
        }
    }))
}

/// Asserts stderr opens with the full notice and holds each line exactly once.
fn assert_notice_once_first(stderr: &str) {
    let expected_prefix = NOTICE_LINES.join("\n") + "\n";
    assert!(
        stderr.starts_with(&expected_prefix),
        "stderr must open with the notice:\n{stderr}"
    );
    for line in NOTICE_LINES {
        assert_eq!(
            stderr.lines().filter(|l| *l == line).count(),
            1,
            "notice line must appear exactly once: {line}\nstderr:\n{stderr}"
        );
    }
}

fn assert_no_notice(text: &str) {
    for line in NOTICE_LINES {
        assert!(!text.contains(line), "unexpected MCP notice in:\n{text}");
    }
    assert!(
        !text.contains("the connected MCP client can execute every loaded workflow"),
        "unexpected MCP notice fragment in:\n{text}"
    );
}

/// Parses every non-empty stdout line as a JSON-RPC message; anything else
/// on stdout fails the test.
fn parse_mcp_stdout(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|err| panic!("non-JSON stdout line {line:?}: {err}"))
        })
        .collect()
}

fn assert_initialize_exchange(run: &Run) {
    assert!(
        run.status.success(),
        "status {:?}\n{}",
        run.status,
        run.stderr
    );
    assert_notice_once_first(&run.stderr);
    assert_no_notice(&run.stdout);
    let messages = parse_mcp_stdout(&run.stdout);
    assert_eq!(messages.len(), 1, "stdout: {}", run.stdout);
    let response = &messages[0];
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["serverInfo"]["name"], "arazzo-mcp");
    assert!(
        response["result"]["protocolVersion"].is_string(),
        "{response}"
    );
}

#[test]
fn serve_prints_notice_once_and_keeps_stdout_mcp_only() {
    let temp = TempDir::new("arazzo-cli-mcp-notice");
    temp.write("wf.arazzo.yaml", SPEC);

    let run = run_cli(
        temp.path(),
        &["serve", "wf.arazzo.yaml"],
        &initialize_request(),
        &[],
    );

    assert_initialize_exchange(&run);
}

#[test]
fn json_serve_prints_notice_once_on_stderr_and_keeps_stdout_mcp_only() {
    let temp = TempDir::new("arazzo-cli-mcp-notice-json");
    temp.write("wf.arazzo.yaml", SPEC);

    let run = run_cli(
        temp.path(),
        &["--json", "serve", "wf.arazzo.yaml"],
        &initialize_request(),
        &[],
    );

    assert_initialize_exchange(&run);
}

#[test]
fn serve_with_immediate_stdin_eof_never_prompts_and_exits_cleanly() {
    let temp = TempDir::new("arazzo-cli-mcp-notice-eof");
    temp.write("wf.arazzo.yaml", SPEC);

    for args in [
        &["serve", "wf.arazzo.yaml"][..],
        &["--json", "serve", "wf.arazzo.yaml"][..],
    ] {
        let run = run_cli(temp.path(), args, b"", &[]);
        assert!(run.status.success(), "{args:?}: {}", run.stderr);
        assert_notice_once_first(&run.stderr);
        assert_eq!(run.stdout, "", "{args:?}: no request, no stdout");
    }
}

#[test]
fn notice_precedes_startup_failures_and_exit_behavior_is_unchanged() {
    let temp = TempDir::new("arazzo-cli-mcp-notice-fail");

    // (args, code, error) — discovery, empty list, and load failures.
    let cases: [(&[&str], &str, &str); 3] = [
        (
            &["serve", "--dir", "missing-dir"],
            "SERVE_SPEC_DISCOVERY",
            "discovering specs: ",
        ),
        (&["serve"], "SERVE_NO_SPECS", "no spec files provided"),
        (&["serve", "missing.arazzo.yaml"], "SERVE_MCP", "loading "),
    ];
    for (args, code, error) in cases {
        let plain = run_cli(temp.path(), args, b"", &[]);
        assert_eq!(plain.status.code(), Some(1), "{args:?}: {}", plain.stderr);
        assert_notice_once_first(&plain.stderr);
        assert!(
            plain.stderr[NOTICE_LINES.join("\n").len()..].contains(error),
            "{args:?}: error after notice: {}",
            plain.stderr
        );
        assert_eq!(plain.stdout, "", "{args:?}");

        let json_args: Vec<&str> = std::iter::once("--json")
            .chain(args.iter().copied())
            .collect();
        let json_run = run_cli(temp.path(), &json_args, b"", &[]);
        assert_eq!(json_run.status.code(), Some(1), "{json_args:?}");
        assert_notice_once_first(&json_run.stderr);
        assert_no_notice(&json_run.stdout);
        let failure: Value = serde_json::from_str(&json_run.stdout)
            .unwrap_or_else(|err| panic!("{json_args:?}: stdout not one JSON object: {err}"));
        assert_eq!(failure["kind"], "error", "{failure}");
        assert_eq!(failure["code"], code, "{failure}");
        assert!(
            failure["error"]
                .as_str()
                .unwrap_or_default()
                .contains(error),
            "{failure}"
        );
    }
}

#[test]
fn notice_is_constant_and_never_echoes_dynamic_values() {
    let temp = TempDir::new("arazzo-cli-mcp-notice-sentinel");
    let allowed = format!("allowed-{SENTINEL}");
    fs::create_dir(temp.path().join(&allowed))
        .unwrap_or_else(|err| panic!("creating allowed dir: {err}"));
    let spec = format!("{SENTINEL}.arazzo.yaml");
    temp.write(&spec, SPEC);

    let run = run_cli(
        temp.path(),
        &["serve", "--allowed-dir", &allowed, &spec],
        &initialize_request(),
        &[("ARAZZO_T_MCP_NOTICE_SENTINEL", SENTINEL)],
    );

    assert_initialize_exchange(&run);
    let notice_len = NOTICE_LINES.join("\n").len() + 1;
    let notice = &run.stderr[..notice_len];
    assert_eq!(notice, NOTICE_LINES.join("\n") + "\n");
    assert!(!notice.contains(SENTINEL));
    assert!(!notice.contains(&temp.path().display().to_string()));
    // The loader's own diagnostic is separate and is not the notice.
    let rest = &run.stderr[notice_len..];
    assert_no_notice(rest);
    assert!(!run.stdout.contains(SENTINEL), "{}", run.stdout);
}

#[test]
fn version_help_and_other_commands_print_no_notice() {
    let temp = TempDir::new("arazzo-cli-mcp-notice-negative");
    temp.write("wf.arazzo.yaml", SPEC);

    for args in [
        &["--version"][..],
        &["serve", "--help"][..],
        &["validate", "wf.arazzo.yaml"][..],
        &["--json", "validate", "wf.arazzo.yaml"][..],
    ] {
        let run = run_cli(temp.path(), args, b"", &[]);
        assert!(run.status.success(), "{args:?}: {}", run.stderr);
        assert_no_notice(&run.stderr);
        assert_no_notice(&run.stdout);
    }
}

#[test]
fn limit_flags_reach_the_server() {
    let temp = TempDir::new("arazzo-cli-mcp-limits");
    temp.write("wf.arazzo.yaml", SPEC);
    let describe = frame(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "describe_workflow", "arguments": {"workflow_id": "wf"}}
    }));

    let capped = run_cli(
        temp.path(),
        &[
            "serve",
            "wf.arazzo.yaml",
            "--max-result-bytes",
            "64",
            "--max-http-timeout",
            "45s",
            "--max-execution-timeout",
            "2m",
        ],
        &describe,
        &[],
    );
    assert!(capped.status.success(), "{}", capped.stderr);
    assert!(
        capped.stderr.contains(
            "max_result_bytes=64 max_response_body_bytes=4194304 max_document_bytes=4194304 max_http_timeout=45s max_execution_timeout=120s"
        ),
        "{}",
        capped.stderr
    );
    let messages = parse_mcp_stdout(&capped.stdout);
    assert_eq!(messages.len(), 1, "stdout: {}", capped.stdout);
    assert_eq!(messages[0]["result"]["isError"], true, "{}", messages[0]);
    let denial: Value = serde_json::from_str(
        messages[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default(),
    )
    .unwrap_or_else(|err| panic!("limit result text: {err}"));
    assert_eq!(denial["code"], "MCP_LIMIT_EXCEEDED", "{denial}");

    let uncapped = run_cli(temp.path(), &["serve", "wf.arazzo.yaml"], &describe, &[]);
    assert!(uncapped.status.success(), "{}", uncapped.stderr);
    let messages = parse_mcp_stdout(&uncapped.stdout);
    assert_eq!(messages.len(), 1, "stdout: {}", uncapped.stdout);
    assert_ne!(messages[0]["result"]["isError"], true, "{}", messages[0]);
}
