use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Read, Write};

use serde_json::{json, Value};

#[path = "dap/events.rs"]
mod events;
#[path = "dap/requests.rs"]
mod requests;
#[path = "dap/responses.rs"]
mod responses;

use events::{initialized_event, stopped_event, terminated_event};
use requests::{DapBreakpoint, DapRequest};
use responses::{
    continue_body, empty_body, error_response, evaluate_body, initialize_capabilities,
    response_with_body, scopes_body, set_breakpoints_body, stack_trace_body, threads_body,
    variables_body, ResolvedBreakpoint,
};

const MAIN_THREAD_ID: u64 = 1;
const STACK_FRAME_ID: u64 = 1;
const LOCALS_SCOPE_ID: u64 = 1;
const WATCH_SCOPE_ID: u64 = 2;

#[derive(Debug)]
struct SessionState {
    launched_spec: Option<String>,
    workflow_id: Option<String>,
    breakpoints_by_source: HashMap<String, Vec<u32>>,
    step_lines_by_source: HashMap<String, Vec<u32>>,
    current_source: Option<String>,
    current_line: u32,
    terminated: bool,
}

impl SessionState {
    fn new() -> Self {
        Self {
            launched_spec: None,
            workflow_id: None,
            breakpoints_by_source: HashMap::new(),
            step_lines_by_source: HashMap::new(),
            current_source: None,
            current_line: 1,
            terminated: false,
        }
    }

    fn selected_source(&self) -> Option<&str> {
        if let Some(source) = self.current_source.as_deref() {
            return Some(source);
        }
        if let Some(spec) = self.launched_spec.as_deref() {
            return Some(spec);
        }
        self.breakpoints_by_source.keys().next().map(String::as_str)
    }

    fn selected_line(&self) -> u32 {
        if self.current_line == 0 {
            1
        } else {
            self.current_line
        }
    }

    fn frame_name(&self) -> &str {
        self.workflow_id.as_deref().unwrap_or("arazzo-workflow")
    }

    fn ensure_step_lines_loaded(&mut self, source_path: &str) {
        if self.step_lines_by_source.contains_key(source_path) {
            return;
        }

        let step_lines = self
            .workflow_id
            .as_deref()
            .and_then(|workflow_id| extract_step_lines_for_workflow(source_path, workflow_id).ok())
            .unwrap_or_default();
        self.step_lines_by_source
            .insert(source_path.to_string(), step_lines);
    }

    fn set_entry_line_from_steps(&mut self) {
        let Some(source_path) = self.selected_source().map(ToString::to_string) else {
            return;
        };
        self.ensure_step_lines_loaded(&source_path);
        let Some(lines) = self.step_lines_by_source.get(&source_path) else {
            return;
        };
        if let Some(first_line) = lines.first().copied() {
            self.current_line = first_line;
        }
    }

    fn next_step_line(&self) -> Option<u32> {
        let source = self.selected_source()?;
        let step_lines = self.step_lines_by_source.get(source)?;
        step_lines
            .iter()
            .copied()
            .find(|line| *line > self.current_line)
    }

    fn advance_step(&mut self) -> bool {
        let Some(next_line) = self.next_step_line() else {
            return false;
        };
        self.current_line = next_line;
        true
    }

    fn next_breakpoint_line(&self) -> Option<u32> {
        let source = self.selected_source()?;
        let breakpoints = self.breakpoints_by_source.get(source)?;
        breakpoints
            .iter()
            .copied()
            .find(|line| *line > self.current_line)
    }

    fn locals_entries(&self) -> Vec<(&str, String)> {
        let mut entries = Vec::<(&str, String)>::new();
        if let Some(spec) = self.launched_spec.as_ref() {
            entries.push(("spec", spec.clone()));
        }
        if let Some(workflow) = self.workflow_id.as_ref() {
            entries.push(("workflowId", workflow.clone()));
        }
        entries.push(("line", self.selected_line().to_string()));
        entries
    }
}

#[derive(Debug)]
struct OutboundSequence {
    next: u64,
}

impl OutboundSequence {
    fn new() -> Self {
        Self { next: 1 }
    }

    fn alloc(&mut self) -> u64 {
        let seq = self.next;
        self.next = self.next.saturating_add(1);
        seq
    }
}

/// Runs a minimal DAP loop over stdio using Content-Length framing.
pub fn run_dap_stdio<R, W>(reader: &mut R, writer: &mut W) -> Result<(), String>
where
    R: BufRead + Read,
    W: Write,
{
    let mut state = SessionState::new();
    let mut outbound = OutboundSequence::new();

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
                    outbound.alloc(),
                    &command,
                    initialize_capabilities(),
                    request.seq,
                );
                write_dap_message(writer, &response)?;
                write_dap_message(writer, &initialized_event(outbound.alloc()))?;
            }
            "launch" => {
                state.launched_spec = parse_string_argument(&request.arguments, "spec");
                state.workflow_id = parse_string_argument(&request.arguments, "workflowId");
                if let Some(spec) = state.launched_spec.as_ref() {
                    state.current_source = Some(spec.clone());
                }
                state.current_line = 1;
                state.terminated = false;
                state.set_entry_line_from_steps();

                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "setBreakpoints" => {
                let (source_path, breakpoints) = parse_breakpoints(&request.arguments);
                let source_path =
                    source_path.or_else(|| state.selected_source().map(str::to_string));
                if let Some(source_path) = source_path {
                    state.current_source = Some(source_path.clone());
                    state.ensure_step_lines_loaded(&source_path);
                    let step_lines = state
                        .step_lines_by_source
                        .get(&source_path)
                        .cloned()
                        .unwrap_or_default();

                    let mut resolved = Vec::<ResolvedBreakpoint>::new();
                    let mut active_lines = Vec::<u32>::new();
                    for bp in breakpoints {
                        if let Some(mapped) = resolve_breakpoint_line(bp.line, &step_lines) {
                            let message = if mapped != bp.line {
                                Some(format!("mapped to step line {mapped}"))
                            } else {
                                bp.condition
                                    .map(|condition| format!("condition: {condition}"))
                            };
                            resolved.push(ResolvedBreakpoint {
                                line: mapped,
                                verified: true,
                                message,
                            });
                            active_lines.push(mapped);
                        } else {
                            resolved.push(ResolvedBreakpoint {
                                line: bp.line,
                                verified: false,
                                message: Some(
                                    "breakpoint must be on or near a workflow step".to_string(),
                                ),
                            });
                        }
                    }
                    active_lines.sort_unstable();
                    active_lines.dedup();
                    if let Some(first_line) = active_lines.first().copied() {
                        state.current_line = first_line;
                    }
                    state
                        .breakpoints_by_source
                        .insert(source_path, active_lines);

                    let body = set_breakpoints_body(&resolved);
                    let response =
                        response_with_body(outbound.alloc(), &command, body, request.seq);
                    write_dap_message(writer, &response)?;
                } else {
                    let empty = Vec::<ResolvedBreakpoint>::new();
                    let body = set_breakpoints_body(&empty);
                    let response =
                        response_with_body(outbound.alloc(), &command, body, request.seq);
                    write_dap_message(writer, &response)?;
                }
            }
            "setExceptionBreakpoints" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "configurationDone" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                write_dap_message(
                    writer,
                    &stopped_event(outbound.alloc(), MAIN_THREAD_ID, "entry"),
                )?;
            }
            "threads" => {
                let body = threads_body(MAIN_THREAD_ID, "main");
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "stackTrace" => {
                let body = if let Some(source_path) = state.selected_source() {
                    stack_trace_body(
                        STACK_FRAME_ID,
                        state.frame_name(),
                        source_path,
                        state.selected_line(),
                    )
                } else {
                    json!({
                        "stackFrames": [],
                        "totalFrames": 0
                    })
                };
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "scopes" => {
                let body = scopes_body(LOCALS_SCOPE_ID, WATCH_SCOPE_ID);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "variables" => {
                let reference =
                    parse_u64_argument(&request.arguments, "variablesReference").unwrap_or(0);
                let body = if reference == LOCALS_SCOPE_ID {
                    let locals = state.locals_entries();
                    variables_body(&locals)
                } else if reference == WATCH_SCOPE_ID {
                    let watches = [(
                        "watch",
                        "watch expressions are returned via evaluate".to_string(),
                    )];
                    variables_body(&watches)
                } else {
                    let empty: [(&str, String); 0] = [];
                    variables_body(&empty)
                };
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "continue" => {
                let response =
                    response_with_body(outbound.alloc(), &command, continue_body(), request.seq);
                write_dap_message(writer, &response)?;

                if let Some(next_breakpoint) = state.next_breakpoint_line() {
                    state.current_line = next_breakpoint;
                    write_dap_message(
                        writer,
                        &stopped_event(outbound.alloc(), MAIN_THREAD_ID, "breakpoint"),
                    )?;
                } else if state.advance_step() || state.selected_source().is_none() {
                    let reason = if state.selected_source().is_none() {
                        "pause"
                    } else {
                        "step"
                    };
                    write_dap_message(
                        writer,
                        &stopped_event(outbound.alloc(), MAIN_THREAD_ID, reason),
                    )?;
                } else if !state.terminated {
                    write_dap_message(writer, &terminated_event(outbound.alloc()))?;
                    state.terminated = true;
                }
            }
            "evaluate" => {
                let expression =
                    parse_string_argument(&request.arguments, "expression").unwrap_or_default();
                let body = evaluate_body(format!("evaluation not connected yet: {expression}"));
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "next" | "stepIn" | "stepOut" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                let _ = state.advance_step();
                write_dap_message(
                    writer,
                    &stopped_event(outbound.alloc(), MAIN_THREAD_ID, "step"),
                )?;
            }
            "pause" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                write_dap_message(
                    writer,
                    &stopped_event(outbound.alloc(), MAIN_THREAD_ID, "pause"),
                )?;
            }
            "disconnect" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                if !state.terminated {
                    write_dap_message(writer, &terminated_event(outbound.alloc()))?;
                    state.terminated = true;
                }
                break;
            }
            _ => {
                let response = error_response(
                    outbound.alloc(),
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

fn parse_breakpoints(arguments: &Value) -> (Option<String>, Vec<DapBreakpoint>) {
    let source_path = arguments
        .get("source")
        .and_then(|source| source.get("path"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let mut lines = Vec::new();
    let Some(array) = arguments.get("breakpoints").and_then(Value::as_array) else {
        return (source_path, lines);
    };

    for item in array {
        let Some(line_value) = item.get("line").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(line) = u32::try_from(line_value) else {
            continue;
        };
        let condition = item
            .get("condition")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        lines.push(DapBreakpoint { line, condition });
    }
    (source_path, lines)
}

fn parse_string_argument(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_u64_argument(arguments: &Value, key: &str) -> Option<u64> {
    arguments.get(key).and_then(Value::as_u64)
}

fn resolve_breakpoint_line(requested: u32, step_lines: &[u32]) -> Option<u32> {
    if step_lines.is_empty() {
        return Some(requested);
    }
    if step_lines.contains(&requested) {
        return Some(requested);
    }

    let nearest = step_lines
        .iter()
        .copied()
        .take_while(|line| *line <= requested)
        .last()?;

    // Let users click nearby executable lines (operationPath, successCriteria, etc.)
    // and snap the breakpoint back to the owning step line.
    if requested.saturating_sub(nearest) <= 8 {
        Some(nearest)
    } else {
        None
    }
}

fn extract_step_lines_for_workflow(path: &str, workflow_id: &str) -> Result<Vec<u32>, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("reading spec file {path}: {err}"))?;
    Ok(extract_step_lines_from_text(&text, workflow_id))
}

fn extract_step_lines_from_text(text: &str, workflow_id: &str) -> Vec<u32> {
    let mut in_workflows = false;
    let mut in_target_workflow = false;
    let mut workflow_indent = 0usize;
    let mut in_steps = false;
    let mut steps_indent = 0usize;
    let mut step_lines = Vec::<u32>::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_no = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let trimmed_start = raw_line.trim_start();
        let trimmed = trimmed_start.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw_line.len().saturating_sub(trimmed_start.len());

        if !in_workflows {
            if trimmed == "workflows:" {
                in_workflows = true;
            }
            continue;
        }

        if indent == 0 && trimmed != "workflows:" {
            in_workflows = false;
            in_target_workflow = false;
            in_steps = false;
            continue;
        }

        if let Some(found_workflow) = parse_yaml_inline_value(trimmed, "- workflowId:") {
            workflow_indent = indent;
            in_target_workflow = found_workflow == workflow_id;
            in_steps = false;
            continue;
        }

        if in_target_workflow {
            if indent <= workflow_indent && trimmed.starts_with("- ") {
                in_target_workflow = false;
                in_steps = false;
                continue;
            }

            if trimmed == "steps:" {
                in_steps = true;
                steps_indent = indent;
                continue;
            }

            if in_steps && indent <= steps_indent && !trimmed.starts_with("- ") {
                in_steps = false;
            }

            if in_steps && trimmed.starts_with("- stepId:") {
                step_lines.push(line_no);
            }
        }
    }

    step_lines
}

fn parse_yaml_inline_value(line: &str, prefix: &str) -> Option<String> {
    let value = line.strip_prefix(prefix)?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let value = value.split(" #").next().unwrap_or(value).trim();
    let value = value.trim_matches('"').trim_matches('\'').trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_breakpoints_extracts_lines() {
        let args = json!({
            "source": { "path": "/tmp/workflow.arazzo.yaml" },
            "breakpoints": [
                { "line": 4, "condition": "$statusCode == 429" },
                { "line": 10 }
            ]
        });
        let (source_path, breakpoints) = parse_breakpoints(&args);
        assert_eq!(source_path.as_deref(), Some("/tmp/workflow.arazzo.yaml"));
        assert_eq!(breakpoints.len(), 2);
        assert_eq!(breakpoints[0].line, 4);
        assert_eq!(
            breakpoints[0].condition.as_deref(),
            Some("$statusCode == 429")
        );
        assert_eq!(breakpoints[1].line, 10);
        assert_eq!(breakpoints[1].condition.as_deref(), None);
    }

    #[test]
    fn extract_step_lines_from_text_finds_target_workflow_steps() {
        let text = r#"
info:
  title: Test
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: https://example.com/rss
      - stepId: select-top
        operationId: select
  - workflowId: other
    steps:
      - stepId: x
"#;
        let lines = extract_step_lines_from_text(text, "get-hackernews");
        assert_eq!(lines, vec![7, 9]);
    }

    #[test]
    fn resolve_breakpoint_line_maps_nearby_lines() {
        let step_lines = vec![31, 47];
        assert_eq!(resolve_breakpoint_line(31, &step_lines), Some(31));
        assert_eq!(resolve_breakpoint_line(34, &step_lines), Some(31));
        assert_eq!(resolve_breakpoint_line(55, &step_lines), Some(47));
        assert_eq!(resolve_breakpoint_line(80, &step_lines), None);
    }
}
