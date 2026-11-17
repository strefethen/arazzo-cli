//! Content-Length framed JSON-RPC 2.0 transport and MCP server loop.
//!
//! Adapted from the DAP adapter (`arazzo-debug-adapter/src/dap.rs`).

use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::thread;

use arazzo_runtime::ExecutionCancellation;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::{JoinError, JoinHandle};

use crate::framing;
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

enum ReaderMsg {
    Request(JsonRpcRequest),
    Rejected(&'static str),
    Eof,
}

struct ActiveCall {
    id: Value,
    cancellation: ExecutionCancellation,
    task: JoinHandle<Value>,
}

impl ActiveCall {
    fn cancel(self, runtime: &Runtime) {
        self.cancellation.cancel();
        let _settled = runtime.block_on(self.task);
    }

    fn response(&self, outcome: Result<Value, JoinError>, state: &ServerState) -> Value {
        let content = outcome.unwrap_or_else(|err| {
            handlers::capped_error(state, &format!("workflow task failed: {err}"))
        });
        jsonrpc_result(&self.id, content)
    }

    fn is_cancelled_by(&self, req: &JsonRpcRequest) -> bool {
        req.id.is_none()
            && req.method == "notifications/cancelled"
            && req
                .params
                .as_ref()
                .and_then(|params| params.get("requestId"))
                == Some(&self.id)
    }
}

enum Wake {
    Finished(Result<Value, JoinError>),
    Message(ReaderMsg),
}

async fn next_wake(call: &mut ActiveCall, rx: &mut mpsc::Receiver<ReaderMsg>) -> Wake {
    tokio::select! {
        biased;
        outcome = &mut call.task => Wake::Finished(outcome),
        message = rx.recv() => Wake::Message(message.unwrap_or(ReaderMsg::Eof)),
    }
}

enum Dispatched {
    Reply(Value),
    Silent,
    Started(ActiveCall),
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
    let limits = state.limits();
    let (tx, mut rx) = mpsc::channel::<ReaderMsg>(limits.max_pending_requests);

    thread::spawn(move || {
        let mut reader = reader;
        loop {
            let message = match framing::read_frame(&mut reader, limits.max_request_bytes) {
                Ok(Some(payload)) => match serde_json::from_slice::<JsonRpcRequest>(&payload) {
                    Ok(request) => ReaderMsg::Request(request),
                    Err(err) => {
                        eprintln!("mcp: ignoring malformed JSON-RPC request: {err}");
                        continue;
                    }
                },
                Ok(None) => ReaderMsg::Eof,
                Err(error) => error
                    .rejection()
                    .map_or(ReaderMsg::Eof, ReaderMsg::Rejected),
            };
            let last = !matches!(message, ReaderMsg::Request(_));
            if tx.blocking_send(message).is_err() || last {
                break;
            }
        }
    });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("creating tokio runtime: {err}"))?;

    let mut initialized = false;
    let mut active: Option<ActiveCall> = None;
    let mut queued: VecDeque<JsonRpcRequest> = VecDeque::new();

    loop {
        let message = match active.take() {
            None => match queued.pop_front() {
                Some(request) => ReaderMsg::Request(request),
                None => runtime.block_on(rx.recv()).unwrap_or(ReaderMsg::Eof),
            },
            Some(mut call) => {
                let wake = if queued.len() < limits.max_pending_requests {
                    runtime.block_on(next_wake(&mut call, &mut rx))
                } else {
                    Wake::Finished(runtime.block_on(&mut call.task))
                };
                match wake {
                    Wake::Finished(outcome) => {
                        framing::write_frame(writer, &call.response(outcome, state))?;
                        continue;
                    }
                    Wake::Message(message) => {
                        active = Some(call);
                        message
                    }
                }
            }
        };

        let req = match message {
            ReaderMsg::Request(req) => req,
            ReaderMsg::Eof => {
                if let Some(call) = active.take() {
                    call.cancel(&runtime);
                }
                break;
            }
            ReaderMsg::Rejected(message) => {
                if let Some(call) = active.take() {
                    call.cancel(&runtime);
                }
                framing::write_frame(writer, &jsonrpc_error(&Value::Null, -32600, message))?;
                break;
            }
        };

        if let Some(call) = active.take() {
            if call.is_cancelled_by(&req) {
                call.cancel(&runtime);
                continue;
            }
            active = Some(call);
            if req.id.is_some() && req.method == "tools/call" {
                queued.push_back(req);
                continue;
            }
        }

        match dispatch(&req, state, &runtime, &mut initialized) {
            Dispatched::Reply(response) => framing::write_frame(writer, &response)?,
            Dispatched::Silent => {}
            Dispatched::Started(call) => active = Some(call),
        }
    }

    Ok(())
}

fn dispatch(
    req: &JsonRpcRequest,
    state: &ServerState,
    runtime: &Runtime,
    initialized: &mut bool,
) -> Dispatched {
    let method = req.method.as_str();

    // Notifications (no id) get no response.
    let Some(id) = &req.id else {
        // Handle client notifications.
        if method == "notifications/initialized" {
            *initialized = true;
        }
        return Dispatched::Silent;
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
            Dispatched::Reply(jsonrpc_result(id, result))
        }

        "tools/list" => {
            let tools = tools::definitions(state);
            Dispatched::Reply(jsonrpc_result(id, json!({ "tools": tools })))
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

            let content = match tool_name {
                "list_workflows" => handlers::list_workflows(state),
                "describe_workflow" => handlers::describe_workflow(state, &arguments),
                "run_workflow" => match handlers::start_workflow(state, &arguments, runtime) {
                    Ok(started) => {
                        return Dispatched::Started(ActiveCall {
                            id: id.clone(),
                            cancellation: started.cancellation,
                            task: started.task,
                        })
                    }
                    Err(result) => Ok(result),
                },
                "validate_spec" => handlers::validate_spec(state, &arguments),
                "generate_workflow" => handlers::generate_workflow(state, &arguments),
                "describe_openapi" => handlers::describe_openapi(state, &arguments),
                "generate_example" => handlers::generate_example(state, &arguments),
                _ => Err(format!("unknown tool: {tool_name}")),
            }
            .unwrap_or_else(|err| handlers::capped_error(state, &err));

            Dispatched::Reply(jsonrpc_result(id, content))
        }

        "ping" => Dispatched::Reply(jsonrpc_result(id, json!({}))),

        _ => Dispatched::Reply(jsonrpc_error(
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
