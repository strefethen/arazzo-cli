#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

const SPEC: &str = "arazzo: 1.1.0\ninfo: {title: Startup, version: '1'}\nsourceDescriptions: [{name: api, url: https://example.com, type: openapi}]\nworkflows: [{workflowId: probe, steps: [{stepId: call, operationId: probe}]}]\n";

#[test]
fn serve_tool_reads_follow_roots_and_ordinary_cli_reads_are_unrestricted() {
    let root = std::env::temp_dir().join(format!("arazzo-cli-files-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let startup = root.join("startup.arazzo.yaml");
    let hidden = root.join(".CANARY_452ce.arazzo.yaml");
    fs::write(&startup, SPEC).unwrap();
    fs::write(&hidden, SPEC).unwrap();
    for path in [&startup, &hidden] {
        let ordinary = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(["validate", path.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(
            ordinary.status.success(),
            "{}",
            String::from_utf8_lossy(&ordinary.stderr)
        );
    }
    for explicit in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
        command.args(["serve", startup.to_str().unwrap()]);
        if explicit {
            command.args(["--allowed-dir", root.to_str().unwrap()]);
        }
        let mut child = command
            .current_dir(&root)
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
        assert_ne!(responses[0]["result"]["isError"], true, "{}", responses[0]);
        assert_eq!(responses[1]["result"]["isError"], true);
        let denial: Value = serde_json::from_str(
            responses[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            denial,
            json!({"code":"MCP_FILE_DENIED","message":"File access denied by server policy"})
        );
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
