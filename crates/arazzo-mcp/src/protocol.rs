//! Content-Length framed JSON-RPC 2.0 transport and MCP server loop.
//!
//! Adapted from the DAP adapter (`arazzo-debug-adapter/src/dap.rs`).

use std::io::{BufRead, Read, Write};
use std::sync::mpsc;
use std::thread;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::handlers;
use crate::state::ServerState;
use crate::tools;

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

// ---------------------------------------------------------------------------
// Reader thread message
// ---------------------------------------------------------------------------

enum ReaderMsg {
    Request(JsonRpcRequest),
    Eof,
    ReadError(()),
}

// ---------------------------------------------------------------------------
// Content-Length framing
// ---------------------------------------------------------------------------

/// Read a JSON-RPC message.
///
/// Supports two framing modes:
/// - **Newline-delimited JSON** (MCP 2025-11-25 stdio): one JSON object per line.
/// - **Content-Length framed** (MCP 2024-11-05 / DAP): `Content-Length: N\r\n\r\n{...}`.
///
/// Auto-detects based on whether the first non-empty line starts with `{`.
fn read_message<R: BufRead + Read>(reader: &mut R) -> Result<Option<String>, String> {
    let mut line = String::new();

    // Skip blank lines, then peek at the first non-empty line.
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|err| format!("reading line: {err}"))?;
        if bytes == 0 {
            return Ok(None); // EOF
        }
        if !line.trim().is_empty() {
            break;
        }
    }

    let trimmed = line.trim();

    // Newline-delimited JSON: line starts with `{`.
    if trimmed.starts_with('{') {
        return Ok(Some(trimmed.to_string()));
    }

    // Content-Length framed: parse headers until empty line.
    let mut content_length: Option<usize> = None;
    if let Some(raw) = trimmed.strip_prefix("Content-Length:") {
        content_length = Some(
            raw.trim()
                .parse::<usize>()
                .map_err(|err| format!("parsing Content-Length: {err}"))?,
        );
    }

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|err| format!("reading header line: {err}"))?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(raw) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                raw.trim()
                    .parse::<usize>()
                    .map_err(|err| format!("parsing Content-Length: {err}"))?,
            );
        }
    }

    let Some(content_length) = content_length else {
        return Err("missing Content-Length header".to_string());
    };
    let mut buf = vec![0u8; content_length];
    reader
        .read_exact(&mut buf)
        .map_err(|err| format!("reading payload: {err}"))?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| format!("decoding payload utf8: {err}"))
}

/// Write a JSON-RPC message as newline-delimited JSON (MCP 2025-11-25 stdio).
fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<(), String> {
    let payload = serde_json::to_vec(value).map_err(|err| format!("serializing JSON: {err}"))?;
    writer
        .write_all(&payload)
        .map_err(|err| format!("writing payload: {err}"))?;
    writer
        .write_all(b"\n")
        .map_err(|err| format!("writing newline: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flushing output: {err}"))
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn jsonrpc_result(id: &Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

fn tool_result(content_text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": content_text }],
        "isError": is_error,
    })
}

// ---------------------------------------------------------------------------
// Main server loop
// ---------------------------------------------------------------------------

/// Runs the MCP server over stdio using Content-Length framing.
///
/// The `reader` is moved onto a dedicated blocking thread (stdin cannot be
/// read asynchronously on most platforms). The main thread processes requests
/// and writes responses to `writer`.
pub fn serve<R, W>(reader: R, writer: &mut W, state: &ServerState) -> Result<(), String>
where
    R: BufRead + Read + Send + 'static,
    W: Write,
{
    let (tx, rx) = mpsc::channel::<ReaderMsg>();

    // Reader thread: reads Content-Length framed messages from stdin.
    thread::spawn(move || {
        let mut reader = reader;
        loop {
            match read_message(&mut reader) {
                Ok(Some(payload)) => match serde_json::from_str::<JsonRpcRequest>(&payload) {
                    Ok(request) => {
                        if tx.send(ReaderMsg::Request(request)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        // Log parse error but keep reading. A single malformed
                        // message should not terminate the server.
                        eprintln!("mcp: ignoring malformed JSON-RPC request: {err}");
                        continue;
                    }
                },
                Ok(None) => {
                    let _ = tx.send(ReaderMsg::Eof);
                    break;
                }
                Err(_) => {
                    let _ = tx.send(ReaderMsg::ReadError(()));
                    break;
                }
            }
        }
    });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("creating tokio runtime: {err}"))?;

    let mut initialized = false;

    while let Ok(msg) = rx.recv() {
        match msg {
            ReaderMsg::Eof => break,
            ReaderMsg::ReadError(_) => break,
            ReaderMsg::Request(req) => {
                let response = dispatch(&req, state, &runtime, &mut initialized);
                if let Some(resp) = response {
                    write_message(writer, &resp)?;
                }
            }
        }
    }

    Ok(())
}

/// Dispatch a single JSON-RPC request and return the response (if any).
///
/// Notifications (requests without an `id`) return `None`.
fn dispatch(
    req: &JsonRpcRequest,
    state: &ServerState,
    runtime: &tokio::runtime::Runtime,
    initialized: &mut bool,
) -> Option<Value> {
    let method = req.method.as_str();

    // Notifications (no id) get no response.
    let Some(id) = &req.id else {
        // Handle client notifications.
        if method == "notifications/initialized" {
            *initialized = true;
        }
        return None;
    };

    match method {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "tools": { "listChanged": false },
                },
                "serverInfo": {
                    "name": "arazzo-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            });
            Some(jsonrpc_result(id, result))
        }

        "tools/list" => {
            let tools = tools::definitions(state);
            Some(jsonrpc_result(id, json!({ "tools": tools })))
        }

        "tools/call" => {
            let params = req.params.as_ref();
            let tool_name = params
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let result = match tool_name {
                "list_workflows" => handlers::list_workflows(state),
                "describe_workflow" => handlers::describe_workflow(state, &arguments),
                "run_workflow" => handlers::run_workflow(state, &arguments, runtime),
                "validate_spec" => handlers::validate_spec(state, &arguments),
                "generate_workflow" => handlers::generate_workflow(state, &arguments),
                "describe_openapi" => handlers::describe_openapi(state, &arguments),
                "generate_example" => handlers::generate_example(&arguments),
                _ => {
                    let msg = format!("unknown tool: {tool_name}");
                    Ok(tool_result(&msg, true))
                }
            };

            match result {
                Ok(content) => Some(jsonrpc_result(id, content)),
                Err(err) => Some(jsonrpc_result(id, tool_result(&err, true))),
            }
        }

        "ping" => Some(jsonrpc_result(id, json!({}))),

        _ => Some(jsonrpc_error(
            id,
            -32601,
            &format!("method not found: {method}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(payload: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload).into_bytes()
    }

    #[test]
    fn read_write_round_trip() {
        let original = json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}});
        let payload = serde_json::to_string(&original).ok();
        let payload = payload.as_deref().unwrap_or("{}");
        let framed = frame(payload);
        let mut cursor = Cursor::new(framed);
        let read_back = read_message(&mut cursor).ok().flatten();
        assert!(read_back.is_some());
        let parsed: Value =
            serde_json::from_str(&read_back.unwrap_or_default()).unwrap_or(Value::Null);
        assert_eq!(parsed["method"], "ping");
    }

    #[test]
    fn read_eof_returns_none() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let result = read_message(&mut cursor);
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn write_produces_newline_delimited_json() {
        let msg = json!({"jsonrpc":"2.0","id":1,"result":{}});
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).ok();
        let output = String::from_utf8(buf).unwrap_or_default();
        assert!(output.ends_with('\n'));
        assert!(output.trim().starts_with('{'));
        let parsed: Value = serde_json::from_str(output.trim()).unwrap_or(Value::Null);
        assert_eq!(parsed["id"], 1);
    }

    #[test]
    fn read_newline_delimited_json() {
        let msg = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut cursor = Cursor::new(msg.as_bytes().to_vec());
        let result = read_message(&mut cursor).ok().flatten();
        assert!(result.is_some());
        let parsed: Value =
            serde_json::from_str(&result.unwrap_or_default()).unwrap_or(Value::Null);
        assert_eq!(parsed["method"], "ping");
    }

    #[test]
    fn initialize_smoke() {
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0.1" }
            }
        });

        let input = frame(&serde_json::to_string(&init_req).unwrap_or_default());

        // Append EOF so the reader thread exits.
        // (No more data after the initialize request.)

        let state = ServerState::empty();
        let mut output = Vec::new();
        let reader = Cursor::new(input);

        serve(reader, &mut output, &state).ok();

        let output_str = String::from_utf8(output).unwrap_or_default();
        assert!(output_str.contains("arazzo-mcp"));
        assert!(output_str.contains("protocolVersion"));
    }
}
