use std::io::Cursor;

use arazzo_debug_adapter::run_stdio;
use arazzo_debug_protocol::{
    InitializeResult, PingResult, RequestEnvelope, ResponseEnvelope, ShutdownResult,
    INTERNAL_DEBUG_PROTOCOL_VERSION,
};
use serde_json::json;

#[test]
fn session_smoke_initialize_ping_shutdown() {
    let mut input = String::new();
    input.push_str(&json_line(RequestEnvelope {
        id: 1,
        method: "initialize".to_string(),
        params: json!({
            "clientName": "smoke-test",
            "clientVersion": "0.1.0",
            "protocolVersion": INTERNAL_DEBUG_PROTOCOL_VERSION
        }),
    }));
    input.push('\n');
    input.push_str(&json_line(RequestEnvelope {
        id: 2,
        method: "ping".to_string(),
        params: serde_json::Value::Null,
    }));
    input.push('\n');
    input.push_str(&json_line(RequestEnvelope {
        id: 3,
        method: "shutdown".to_string(),
        params: serde_json::Value::Null,
    }));
    input.push('\n');

    let mut reader = Cursor::new(input.into_bytes());
    let mut output = Vec::<u8>::new();
    let run_result = run_stdio(&mut reader, &mut output);
    assert!(run_result.is_ok(), "expected run_stdio success");

    let out_text = match String::from_utf8(output) {
        Ok(value) => value,
        Err(err) => panic!("decoding output as utf8: {err}"),
    };
    let mut responses = Vec::<ResponseEnvelope>::new();
    for line in out_text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<ResponseEnvelope>(line) {
            Ok(value) => value,
            Err(err) => panic!("decoding response line: {err}"),
        };
        responses.push(response);
    }

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
        Err(err) => panic!("decoding response payload: {err}"),
    }
}

fn json_line<T>(value: T) -> String
where
    T: serde::Serialize,
{
    match serde_json::to_string(&value) {
        Ok(line) => line,
        Err(err) => panic!("encoding json line: {err}"),
    }
}
