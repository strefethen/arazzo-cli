use std::io::{BufRead, Read, Write};

use serde_json::Value;

#[path = "dap/events.rs"]
mod events;
#[path = "dap/requests.rs"]
mod requests;
#[path = "dap/responses.rs"]
mod responses;

use events::initialized_event;
use requests::{DapBreakpoint, DapRequest};
use responses::{
    continue_body, empty_body, error_response, evaluate_body, initialize_capabilities,
    response_with_body, set_breakpoints_body,
};

/// Runs a minimal DAP loop over stdio using Content-Length framing.
pub fn run_dap_stdio<R, W>(reader: &mut R, writer: &mut W) -> Result<(), String>
where
    R: BufRead + Read,
    W: Write,
{
    loop {
        let Some(payload) = read_dap_message(reader)? else {
            break;
        };
        let request: DapRequest = serde_json::from_str(&payload)
            .map_err(|err| format!("parsing DAP request JSON: {err}"))?;

        let command = request.command.clone();
        match command.as_str() {
            "initialize" => {
                let response = response_with_body(
                    request.seq,
                    &command,
                    initialize_capabilities(),
                    request.seq,
                );
                write_dap_message(writer, &response)?;
                write_dap_message(writer, &initialized_event(request.seq + 1))?;
            }
            "setBreakpoints" => {
                let breakpoints = parse_breakpoints(&request.arguments);
                let body = set_breakpoints_body(&breakpoints);
                let response = response_with_body(request.seq, &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "continue" => {
                let response =
                    response_with_body(request.seq, &command, continue_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "evaluate" => {
                let expression =
                    parse_string_argument(&request.arguments, "expression").unwrap_or_default();
                let body = evaluate_body(format!("evaluation not connected yet: {expression}"));
                let response = response_with_body(request.seq, &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "next" | "stepIn" | "stepOut" | "pause" => {
                let response = response_with_body(request.seq, &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "disconnect" => {
                let response = response_with_body(request.seq, &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                break;
            }
            _ => {
                let response = error_response(
                    request.seq,
                    &command,
                    request.seq,
                    format!("unsupported DAP command: {command}"),
                );
                write_dap_message(writer, &response)?;
            }
        }
    }

    Ok(())
}

fn read_dap_message<R>(reader: &mut R) -> Result<Option<String>, String>
where
    R: BufRead + Read,
{
    let mut line = String::new();
    let mut content_length: Option<usize> = None;

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|err| format!("reading DAP header line: {err}"))?;
        if bytes == 0 {
            return Ok(None);
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(raw) = trimmed.strip_prefix("Content-Length:") {
            let parsed = raw
                .trim()
                .parse::<usize>()
                .map_err(|err| format!("parsing DAP Content-Length: {err}"))?;
            content_length = Some(parsed);
        }
    }

    let Some(content_length) = content_length else {
        return Err("missing DAP Content-Length header".to_string());
    };
    let mut buf = vec![0u8; content_length];
    reader
        .read_exact(&mut buf)
        .map_err(|err| format!("reading DAP payload: {err}"))?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| format!("decoding DAP payload utf8: {err}"))
}

fn write_dap_message<W>(writer: &mut W, value: &Value) -> Result<(), String>
where
    W: Write,
{
    let payload =
        serde_json::to_vec(value).map_err(|err| format!("serializing DAP JSON: {err}"))?;
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    writer
        .write_all(header.as_bytes())
        .map_err(|err| format!("writing DAP header: {err}"))?;
    writer
        .write_all(&payload)
        .map_err(|err| format!("writing DAP payload: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flushing DAP output: {err}"))
}

fn parse_breakpoints(arguments: &Value) -> Vec<DapBreakpoint> {
    let mut lines = Vec::new();
    let Some(array) = arguments.get("breakpoints").and_then(Value::as_array) else {
        return lines;
    };

    for item in array {
        let Some(line_value) = item.get("line").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(line) = u32::try_from(line_value) else {
            continue;
        };
        lines.push(DapBreakpoint { line });
    }
    lines
}

fn parse_string_argument(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_breakpoints_extracts_lines() {
        let args = json!({
            "breakpoints": [
                { "line": 4 },
                { "line": 10 }
            ]
        });
        let breakpoints = parse_breakpoints(&args);
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 4);
        assert_eq!(breakpoints[1].line, 10);
    }
}
