#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use arazzo_debug_protocol::{
    InitializeResult, PingResult, RequestEnvelope, ResponseEnvelope, ShutdownResult,
    INTERNAL_DEBUG_PROTOCOL_VERSION,
};
use serde_json::json;

#[test]
fn debug_stdio_smoke_round_trip() {
    let mut child = match Command::new(cli_bin())
        .arg("debug-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => panic!("spawning debug-stdio process: {err}"),
    };

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => panic!("child stdin not available"),
    };
    write_request(
        &mut stdin,
        RequestEnvelope {
            id: 1,
            method: "initialize".to_string(),
            params: json!({
                "clientName": "cli-debug-smoke",
                "clientVersion": "0.1.0",
                "protocolVersion": INTERNAL_DEBUG_PROTOCOL_VERSION
            }),
        },
    );
    write_request(
        &mut stdin,
        RequestEnvelope {
            id: 2,
            method: "ping".to_string(),
            params: serde_json::Value::Null,
        },
    );
    write_request(
        &mut stdin,
        RequestEnvelope {
            id: 3,
            method: "shutdown".to_string(),
            params: serde_json::Value::Null,
        },
    );
    drop(stdin);

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => panic!("waiting for debug-stdio output: {err}"),
    };
    assert!(
        output.status.success(),
        "debug-stdio exited with {:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = match String::from_utf8(output.stdout) {
        Ok(value) => value,
        Err(err) => panic!("decoding debug-stdio stdout: {err}"),
    };
    let responses = parse_responses(&stdout);
    assert_eq!(responses.len(), 3);
    assert!(responses.iter().all(|resp| resp.ok));

    let init = decode_result::<InitializeResult>(&responses[0]);
    assert_eq!(init.protocol_version, INTERNAL_DEBUG_PROTOCOL_VERSION);
    assert!(!init.capabilities.supports_breakpoints);

    let ping = decode_result::<PingResult>(&responses[1]);
    assert_eq!(ping.protocol_version, INTERNAL_DEBUG_PROTOCOL_VERSION);
    assert_eq!(ping.adapter_version, "v1");

    let shutdown = decode_result::<ShutdownResult>(&responses[2]);
    assert_eq!(shutdown.message, "session closed");
}

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => {
            let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            path.push("../..");
            let root = match fs::canonicalize(&path) {
                Ok(root) => root,
                Err(err) => panic!("canonicalizing repo root {}: {err}", path.display()),
            };
            let mut bin = root;
            bin.push("target/debug/arazzo-cli");
            if bin.exists() {
                bin
            } else {
                panic!(
                    "CLI binary path not found at {}; CARGO_BIN_EXE_arazzo-cli missing",
                    bin.display()
                );
            }
        }
    }
}

fn write_request<W>(writer: &mut W, request: RequestEnvelope)
where
    W: Write,
{
    let line = match serde_json::to_string(&request) {
        Ok(line) => line,
        Err(err) => panic!("encoding request json: {err}"),
    };
    if let Err(err) = writer.write_all(line.as_bytes()) {
        panic!("writing request json: {err}");
    }
    if let Err(err) = writer.write_all(b"\n") {
        panic!("writing request newline: {err}");
    }
    if let Err(err) = writer.flush() {
        panic!("flushing request stream: {err}");
    }
}

fn parse_responses(stdout: &str) -> Vec<ResponseEnvelope> {
    let mut responses = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<ResponseEnvelope>(line) {
            Ok(response) => response,
            Err(err) => panic!("decoding response line as json: {err}; line={line}"),
        };
        responses.push(response);
    }
    responses
}

fn decode_result<T>(response: &ResponseEnvelope) -> T
where
    T: serde::de::DeserializeOwned,
{
    let value = match &response.result {
        Some(value) => value.clone(),
        None => panic!("response missing result payload"),
    };
    match serde_json::from_value::<T>(value) {
        Ok(decoded) => decoded,
        Err(err) => panic!("decoding result payload: {err}"),
    }
}
