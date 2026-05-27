use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;

use serde_json::Value;

use super::events::{initialized_event, output_event, terminated_event};
use super::requests::{DapBreakpoint, DapRequest};
use super::responses::{
    continue_body, empty_body, error_response, initialize_capabilities, response_with_body,
    set_breakpoints_body, threads_body,
};
use super::session::{
    cleanup_runtime, ensure_runtime_started, inline_event_check, rebuild_runtime_breakpoints,
    sync_runtime_breakpoints, EngineEvent, LaunchConfig, SessionState, MAIN_THREAD_ID,
};
use super::source_index::{resolve_source_breakpoints, try_build_source_index};
use super::transport::{write_dap_message, OutboundSequence};
use super::variables::{
    evaluate_body_for_expression, scopes_body, stack_trace_body, variables_body,
};

pub(super) enum DispatchOutcome {
    Continue,
    Break,
}

pub(super) fn dispatch_request<W>(
    request: DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_tx: &mpsc::Sender<EngineEvent>,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<DispatchOutcome, String>
where
    W: Write,
{
    let command = request.command.clone();
    match command.as_str() {
        "initialize" => handle_initialize(&request, writer, outbound)?,
        "launch" => handle_launch(&request, state, writer, outbound)?,
        "setBreakpoints" => handle_set_breakpoints(&request, state, writer, outbound)?,
        "setExceptionBreakpoints" => handle_set_exception_breakpoints(&request, writer, outbound)?,
        "configurationDone" => {
            handle_configuration_done(&request, state, writer, outbound, event_tx, event_rx)?
        }
        "threads" => handle_threads(&request, writer, outbound)?,
        "stackTrace" => handle_stack_trace(&request, state, writer, outbound)?,
        "scopes" => handle_scopes(&request, state, writer, outbound)?,
        "variables" => handle_variables(&request, state, writer, outbound)?,
        "evaluate" => handle_evaluate(&request, state, writer, outbound)?,
        "continue" => handle_continue(&request, state, writer, outbound, event_rx)?,
        "next" => handle_next(&request, state, writer, outbound, event_rx)?,
        "stepIn" => handle_step_in(&request, state, writer, outbound, event_rx)?,
        "stepOut" => handle_step_out(&request, state, writer, outbound, event_rx)?,
        "pause" => handle_pause(&request, state, writer, outbound, event_rx)?,
        "disconnect" => {
            handle_disconnect(&request, state, writer, outbound)?;
            return Ok(DispatchOutcome::Break);
        }
        _ => handle_unsupported(&request, writer, outbound)?,
    }
    Ok(DispatchOutcome::Continue)
}

fn handle_initialize<W: Write>(
    request: &DapRequest,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        initialize_capabilities(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    write_dap_message(writer, &initialized_event(outbound.alloc()))
}

fn handle_launch<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let launch = parse_launch_config(&request.arguments)?;
    state.launch = Some(launch.clone());
    state.source_index = try_build_source_index(&launch.spec);
    rebuild_runtime_breakpoints(state);
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)
}

fn handle_set_breakpoints<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let (source_path, breakpoints) = parse_breakpoints(&request.arguments);
    let source_path = source_path.or_else(|| state.launch.as_ref().map(|l| l.spec.clone()));
    let Some(source_path) = source_path else {
        let response = error_response(
            outbound.alloc(),
            &request.command,
            request.seq,
            "setBreakpoints requires source.path".to_string(),
        );
        return write_dap_message(writer, &response);
    };

    state
        .pending_breakpoints
        .insert(source_path.clone(), breakpoints.clone());
    if state
        .source_index
        .as_ref()
        .is_none_or(|index| index.path != source_path)
    {
        state.source_index = try_build_source_index(&source_path);
    }

    let launch_workflow = state
        .launch
        .as_ref()
        .and_then(|launch| launch.workflow_id.as_deref());
    let resolved = resolve_source_breakpoints(
        &source_path,
        &breakpoints,
        launch_workflow,
        state.source_index.as_ref(),
    )
    .resolved;
    rebuild_runtime_breakpoints(state);
    sync_runtime_breakpoints(state)?;

    let body = set_breakpoints_body(&resolved);
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_set_exception_breakpoints<W: Write>(
    request: &DapRequest,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)
}

fn handle_configuration_done<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_tx: &mpsc::Sender<EngineEvent>,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    if let Err(err) = ensure_runtime_started(state, event_tx) {
        let msg = format!("Arazzo debug: {err}\n");
        write_dap_message(writer, &output_event(outbound.alloc(), "console", &msg))?;
        let response = error_response(outbound.alloc(), &request.command, request.seq, err);
        write_dap_message(writer, &response)?;
        write_dap_message(writer, &terminated_event(outbound.alloc()))?;
        return Ok(());
    }
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    inline_event_check(event_rx, state, writer, outbound)
}

fn handle_threads<W: Write>(
    request: &DapRequest,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let body = threads_body(MAIN_THREAD_ID, "main");
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_stack_trace<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let body = stack_trace_body(state);
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_scopes<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let body = scopes_body(state);
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_variables<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let reference = parse_u64_argument(&request.arguments, "variablesReference").unwrap_or(0);
    let body = variables_body(state, reference);
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_evaluate<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let expression = parse_string_argument(&request.arguments, "expression").unwrap_or_default();
    let body = evaluate_body_for_expression(state, &expression);
    let response = response_with_body(outbound.alloc(), &request.command, body, request.seq);
    write_dap_message(writer, &response)
}

fn handle_continue<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        continue_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .continue_execution()
            .map_err(|err| format!("continuing runtime: {err}"))?;
    }
    if state.runtime.is_some() {
        inline_event_check(event_rx, state, writer, outbound)?;
    }
    Ok(())
}

fn handle_next<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .step_over()
            .map_err(|err| format!("step over: {err}"))?;
    }
    if state.runtime.is_some() {
        inline_event_check(event_rx, state, writer, outbound)?;
    }
    Ok(())
}

fn handle_step_in<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .step_in()
            .map_err(|err| format!("step in: {err}"))?;
    }
    if state.runtime.is_some() {
        inline_event_check(event_rx, state, writer, outbound)?;
    }
    Ok(())
}

fn handle_step_out<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .step_out()
            .map_err(|err| format!("step out: {err}"))?;
    }
    if state.runtime.is_some() {
        inline_event_check(event_rx, state, writer, outbound)?;
    }
    Ok(())
}

fn handle_pause<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    event_rx: &mpsc::Receiver<EngineEvent>,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .request_pause()
            .map_err(|err| format!("request pause: {err}"))?;
    }
    if state.runtime.is_some() {
        inline_event_check(event_rx, state, writer, outbound)?;
    }
    Ok(())
}

fn handle_disconnect<W: Write>(
    request: &DapRequest,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let response = response_with_body(
        outbound.alloc(),
        &request.command,
        empty_body(),
        request.seq,
    );
    write_dap_message(writer, &response)?;
    write_dap_message(writer, &terminated_event(outbound.alloc()))?;
    cleanup_runtime(state);
    Ok(())
}

fn handle_unsupported<W: Write>(
    request: &DapRequest,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String> {
    let response = error_response(
        outbound.alloc(),
        &request.command,
        request.seq,
        format!("unsupported DAP command: {}", request.command),
    );
    write_dap_message(writer, &response)
}

pub(super) fn parse_launch_config(arguments: &Value) -> Result<LaunchConfig, String> {
    let spec = parse_string_argument(arguments, "spec")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "launch requires non-empty 'spec'".to_string())?;
    let workflow_id =
        parse_string_argument(arguments, "workflowId").filter(|value| !value.trim().is_empty());

    let inputs = arguments
        .get("inputs")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let dry_run = arguments
        .get("dryRun")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stop_on_entry = arguments
        .get("stopOnEntry")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(LaunchConfig {
        spec,
        workflow_id,
        inputs,
        dry_run,
        stop_on_entry,
    })
}

pub(super) fn parse_breakpoints(arguments: &Value) -> (Option<String>, Vec<DapBreakpoint>) {
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
    fn parse_launch_config_defaults_stop_on_entry_to_false() {
        let args = json!({
            "spec": "/tmp/workflow.arazzo.yaml",
            "workflowId": "wf",
            "inputs": {"code": 429}
        });
        let launch = match parse_launch_config(&args) {
            Ok(launch) => launch,
            Err(err) => panic!("valid launch config expected, got: {err}"),
        };
        assert!(!launch.stop_on_entry);
    }

    #[test]
    fn parse_launch_config_reads_stop_on_entry() {
        let args = json!({
            "spec": "/tmp/workflow.arazzo.yaml",
            "workflowId": "wf",
            "stopOnEntry": true
        });
        let launch = match parse_launch_config(&args) {
            Ok(launch) => launch,
            Err(err) => panic!("valid launch config expected, got: {err}"),
        };
        assert!(launch.stop_on_entry);
    }

    #[test]
    fn parse_launch_config_allows_missing_workflow_id() {
        let args = json!({
            "spec": "/tmp/workflow.arazzo.yaml",
            "inputs": {"code": 429}
        });
        let launch = match parse_launch_config(&args) {
            Ok(launch) => launch,
            Err(err) => panic!("valid launch config expected, got: {err}"),
        };
        assert!(launch.workflow_id.is_none());
    }
}
