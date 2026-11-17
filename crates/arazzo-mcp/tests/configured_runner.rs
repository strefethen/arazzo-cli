#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use arazzo_mcp::{run_mcp_stdio, run_mcp_stdio_configured, McpServerConfig};
use serde_json::{json, Value};

const SPEC: &str = "arazzo: 1.1.0\ninfo: {title: Startup, version: '1'}\nsourceDescriptions: [{name: api, url: https://example.com, type: openapi}]\nworkflows: [{workflowId: probe, steps: [{stepId: call, operationId: probe}]}]\n";

fn fixture() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "arazzo-roots-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn input(path: &str) -> Cursor<Vec<u8>> {
    Cursor::new(format!("{}\n", json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"validate_spec", "arguments":{"file_path":path}}})).into_bytes())
}

#[test]
fn legacy_and_configured_runners_share_default_explicit_empty_semantics() {
    let root = fixture();
    let startup = root.join("startup.arazzo.yaml");
    fs::write(&startup, SPEC).unwrap();
    let other = fixture();
    fs::write(other.join("other.arazzo.yaml"), SPEC).unwrap();
    let paths = vec![startup.to_str().unwrap().to_string()];
    for configured in [false, true] {
        for roots in [
            None,
            Some(vec![]),
            Some(vec![root.to_str().unwrap().to_string()]),
            Some(vec![other.to_str().unwrap().to_string()]),
        ] {
            let expected = roots.is_none()
                || roots
                    .as_ref()
                    .is_some_and(|roots| roots.contains(&root.to_str().unwrap().to_string()));
            let mut output = Vec::new();
            if configured {
                let config = McpServerConfig {
                    allowed_dirs: roots,
                    ..McpServerConfig::default()
                };
                run_mcp_stdio_configured(
                    input(startup.to_str().unwrap()),
                    &mut output,
                    &paths,
                    config,
                )
                .unwrap();
            } else {
                run_mcp_stdio(input(startup.to_str().unwrap()), &mut output, &paths, roots)
                    .unwrap();
            }
            let response: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(response["result"]["isError"], !expected, "{response}");
            let text: Value =
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            if expected {
                assert_eq!(text["valid"], true);
            } else {
                assert_eq!(text["code"], "MCP_FILE_DENIED");
            }
        }
        let mut output = Vec::new();
        run_mcp_stdio(input(startup.to_str().unwrap()), &mut output, &[], None).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["result"]["isError"], true);
    }
}

#[test]
fn invalid_roots_are_fixed_startup_failures_before_dispatch() {
    let root = fixture();
    let file = root.join("private-canary-file");
    fs::write(&file, SPEC).unwrap();
    for invalid in [file, root.join("private-canary-missing")] {
        let mut output = Vec::new();
        let error = run_mcp_stdio(
            Cursor::new(Vec::new()),
            &mut output,
            &[],
            Some(vec![invalid.to_str().unwrap().into()]),
        )
        .unwrap_err();
        assert_eq!(error, "Invalid server file access configuration");
        assert!(output.is_empty());
    }
}

#[test]
fn duplicated_roots_use_one_pinned_identity() {
    let root = fixture();
    let state = arazzo_mcp::state::ServerState::load(
        &[],
        Some(vec![
            root.to_str().unwrap().into(),
            root.join(".").to_str().unwrap().into(),
        ]),
    )
    .unwrap();
    let selectors = state.allowed_dirs.as_ref().unwrap();
    let distinct: std::collections::HashSet<_> = selectors.iter().collect();
    assert_eq!(distinct.len(), selectors.len(), "{selectors:?}");
    assert!(selectors
        .iter()
        .all(|selector| fs::canonicalize(selector).unwrap() == root));
}

#[test]
fn standalone_binary_preserves_startup_and_denies_safe_failure_details() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let root = fixture();
    let startup = root.join("startup.arazzo.yaml");
    fs::write(&startup, SPEC).unwrap();
    let hidden = root.join(".CANARY_452ce");
    fs::write(&hidden, "CANARY source").unwrap();
    for explicit in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-mcp"));
        command.arg(&startup).current_dir(&root);
        if explicit {
            command.arg("--allowed-dir").arg(&root);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for (id, path) in [(1, &startup), (2, &hidden)] {
            writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"validate_spec","arguments":{"file_path":path}}})).unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let responses: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["isError"], false);
        assert_eq!(responses[1]["result"]["isError"], true);
        assert!(!responses[1].to_string().contains("CANARY"));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("CANARY"));
        assert!(!stderr.contains(root.to_str().unwrap()));
        assert!(stderr.contains(if explicit {
            "mode=explicit"
        } else {
            "mode=startup-defaults"
        }));
    }
}

#[test]
fn relative_paths_use_captured_cwd_after_process_cwd_changes() {
    if std::env::var_os("ARAZZO_CWD_CHILD_452CE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "relative_paths_use_captured_cwd_after_process_cwd_changes",
                "--nocapture",
            ])
            .env("ARAZZO_CWD_CHILD_452CE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = fixture();
    let replacement = fixture();
    fs::write(
        root.join("api.yaml"),
        "openapi: 3.0.3\ninfo: {title: Original, version: '1'}\npaths: {}\n",
    )
    .unwrap();
    fs::write(
        replacement.join("api.yaml"),
        "CANARY replacement parser input",
    )
    .unwrap();
    std::env::set_current_dir(&root).unwrap();
    let state = arazzo_mcp::state::ServerState::load(&[], Some(vec![".".into()])).unwrap();
    std::env::set_current_dir(&replacement).unwrap();
    let result =
        arazzo_mcp::handlers::describe_openapi(&state, &json!({"file_path":"./api.yaml"})).unwrap();
    assert_eq!(result["isError"], false);
    assert!(!result.to_string().contains("CANARY"));
}
