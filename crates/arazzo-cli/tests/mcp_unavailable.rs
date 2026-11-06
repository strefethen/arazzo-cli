use std::process::Command;

#[test]
fn release_cli_cannot_start_or_describe_mcp() {
    for args in [
        vec!["serve"],
        vec!["serve", "--help"],
        vec!["serve", "missing.yaml"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("MCP exclusion check: {err}"));
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'serve'"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(["--json", "serve"])
        .output()
        .unwrap_or_else(|err| panic!("MCP exclusion check: {err}"));
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("MCP exclusion check: {err}"));
    assert_eq!(error["code"], "CLI_USAGE");
    let help = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .arg("--help")
        .output()
        .unwrap_or_else(|err| panic!("MCP exclusion check: {err}"));
    assert!(help.status.success());
    let text =
        String::from_utf8(help.stdout).unwrap_or_else(|err| panic!("MCP exclusion check: {err}"));
    assert!(!text.contains("serve"));
    assert!(!text.contains("MCP"));
}
