use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use arazzo_runtime::{
    DebugController, DebugScopes, DebugStopEvent, DebugStopReason, Engine, RuntimeError,
    StepBreakpoint, StepCheckpoint,
};
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
    response_with_body, set_breakpoints_body, threads_body, ResolvedBreakpoint,
};

const MAIN_THREAD_ID: u64 = 1;
const FRAME_ID_BASE: u64 = 100;
const BREAKPOINT_NEAREST_LINE_THRESHOLD: u32 = 10;
const STOP_WAIT_SLICE: Duration = Duration::from_millis(40);
const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
struct LaunchConfig {
    spec: String,
    workflow_id: String,
    inputs: BTreeMap<String, Value>,
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct IndexedCheckpoint {
    line: u32,
    workflow_id: String,
    step_id: String,
    checkpoint: StepCheckpoint,
}

#[derive(Debug, Clone)]
struct SourceIndex {
    path: String,
    checkpoints: Vec<IndexedCheckpoint>,
    output_expressions: BTreeMap<(String, String, String), String>,
}

#[derive(Debug)]
struct RuntimeSession {
    controller: Arc<DebugController>,
    handle: Option<thread::JoinHandle<Result<BTreeMap<String, Value>, RuntimeError>>>,
    delivered_stop_events: usize,
    last_stop: Option<DebugStopEvent>,
    terminated: bool,
    variable_store: VariableStore,
}

impl RuntimeSession {
    fn new(
        controller: Arc<DebugController>,
        handle: thread::JoinHandle<Result<BTreeMap<String, Value>, RuntimeError>>,
    ) -> Self {
        Self {
            controller,
            handle: Some(handle),
            delivered_stop_events: 0,
            last_stop: None,
            terminated: false,
            variable_store: VariableStore::default(),
        }
    }
}

#[derive(Debug, Default)]
struct VariableStore {
    next_ref: u64,
    entries: HashMap<u64, BTreeMap<String, Value>>,
}

impl VariableStore {
    fn reset(&mut self) {
        self.next_ref = 1;
        self.entries.clear();
    }

    fn insert_map(&mut self, map: BTreeMap<String, Value>) -> u64 {
        let reference = self.next_ref.max(1);
        self.next_ref = reference.saturating_add(1);
        self.entries.insert(reference, map);
        reference
    }

    fn variables_for_reference(&mut self, reference: u64) -> Vec<Value> {
        let Some(entries) = self.entries.get(&reference).cloned() else {
            return Vec::new();
        };
        let mut variables = Vec::<Value>::new();
        for (name, value) in entries {
            let child_reference = map_from_value(&value)
                .map(|map| self.insert_map(map))
                .unwrap_or(0);
            variables.push(json!({
                "name": name,
                "value": display_value(&value),
                "variablesReference": child_reference
            }));
        }
        variables
    }
}

#[derive(Debug, Default)]
struct SessionState {
    launch: Option<LaunchConfig>,
    source_index: Option<SourceIndex>,
    pending_breakpoints: HashMap<String, Vec<DapBreakpoint>>,
    runtime_breakpoints: Vec<StepBreakpoint>,
    runtime: Option<RuntimeSession>,
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

/// Runs a runtime-backed DAP loop over stdio using Content-Length framing.
pub fn run_dap_stdio<R, W>(reader: &mut R, writer: &mut W) -> Result<(), String>
where
    R: BufRead + Read,
    W: Write,
{
    let mut state = SessionState::default();
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
                let launch = parse_launch_config(&request.arguments)?;
                state.launch = Some(launch.clone());
                state.source_index = build_source_index(&launch.spec).ok();
                rebuild_runtime_breakpoints(&mut state);
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "setBreakpoints" => {
                let (source_path, breakpoints) = parse_breakpoints(&request.arguments);
                let source_path =
                    source_path.or_else(|| state.launch.as_ref().map(|l| l.spec.clone()));
                let Some(source_path) = source_path else {
                    let response = error_response(
                        outbound.alloc(),
                        &command,
                        request.seq,
                        "setBreakpoints requires source.path".to_string(),
                    );
                    write_dap_message(writer, &response)?;
                    continue;
                };

                state
                    .pending_breakpoints
                    .insert(source_path.clone(), breakpoints.clone());
                if state
                    .source_index
                    .as_ref()
                    .is_none_or(|index| index.path != source_path)
                {
                    state.source_index = build_source_index(&source_path).ok();
                }

                let resolved =
                    resolve_source_breakpoints(&source_path, &breakpoints, &state).resolved;
                rebuild_runtime_breakpoints(&mut state);
                sync_runtime_breakpoints(&mut state)?;

                let body = set_breakpoints_body(&resolved);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "setExceptionBreakpoints" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
            }
            "configurationDone" => {
                if let Err(err) = ensure_runtime_started(&mut state) {
                    let response = error_response(outbound.alloc(), &command, request.seq, err);
                    write_dap_message(writer, &response)?;
                    continue;
                }
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
            }
            "threads" => {
                let body = threads_body(MAIN_THREAD_ID, "main");
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "stackTrace" => {
                let body = stack_trace_body(&state);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "scopes" => {
                let body = scopes_body(&mut state);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "variables" => {
                let reference =
                    parse_u64_argument(&request.arguments, "variablesReference").unwrap_or(0);
                let body = variables_body(&mut state, reference);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "evaluate" => {
                let expression =
                    parse_string_argument(&request.arguments, "expression").unwrap_or_default();
                let body = evaluate_body_for_expression(&mut state, &expression);
                let response = response_with_body(outbound.alloc(), &command, body, request.seq);
                write_dap_message(writer, &response)?;
            }
            "continue" => {
                let response =
                    response_with_body(outbound.alloc(), &command, continue_body(), request.seq);
                write_dap_message(writer, &response)?;
                if let Some(runtime) = state.runtime.as_ref() {
                    runtime
                        .controller
                        .continue_execution()
                        .map_err(|err| format!("continuing runtime: {err}"))?;
                    emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
                }
            }
            "next" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                if let Some(runtime) = state.runtime.as_ref() {
                    runtime
                        .controller
                        .step_over()
                        .map_err(|err| format!("step over: {err}"))?;
                    emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
                }
            }
            "stepIn" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                if let Some(runtime) = state.runtime.as_ref() {
                    runtime
                        .controller
                        .step_in()
                        .map_err(|err| format!("step in: {err}"))?;
                    emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
                }
            }
            "stepOut" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                if let Some(runtime) = state.runtime.as_ref() {
                    runtime
                        .controller
                        .step_out()
                        .map_err(|err| format!("step out: {err}"))?;
                    emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
                }
            }
            "pause" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                if let Some(runtime) = state.runtime.as_ref() {
                    runtime
                        .controller
                        .request_pause()
                        .map_err(|err| format!("request pause: {err}"))?;
                    emit_next_runtime_event(&mut state, writer, &mut outbound, STOP_WAIT_TIMEOUT)?;
                }
            }
            "disconnect" => {
                let response =
                    response_with_body(outbound.alloc(), &command, empty_body(), request.seq);
                write_dap_message(writer, &response)?;
                write_dap_message(writer, &terminated_event(outbound.alloc()))?;
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

fn ensure_runtime_started(state: &mut SessionState) -> Result<(), String> {
    if state.runtime.is_some() {
        return Ok(());
    }

    let launch = state
        .launch
        .as_ref()
        .cloned()
        .ok_or_else(|| "launch must be sent before configurationDone".to_string())?;
    let spec = arazzo_validate::parse(&launch.spec)
        .map_err(|err| format!("loading arazzo spec for debug: {err}"))?;
    let controller = Arc::new(DebugController::new());
    if !state.runtime_breakpoints.is_empty() {
        controller
            .set_breakpoints(state.runtime_breakpoints.clone())
            .map_err(|err| format!("applying breakpoints: {err}"))?;
    }
    controller
        .request_pause()
        .map_err(|err| format!("requesting initial pause: {err}"))?;

    let mut engine = Engine::new(spec).map_err(|err| format!("creating runtime engine: {err}"))?;
    engine.set_debug_controller(Arc::clone(&controller));
    engine.set_dry_run_mode(launch.dry_run);
    let workflow_id = launch.workflow_id.clone();
    let inputs = launch.inputs.clone();
    let handle = thread::spawn(move || engine.execute(&workflow_id, inputs));
    state.runtime = Some(RuntimeSession::new(controller, handle));
    Ok(())
}

fn sync_runtime_breakpoints(state: &mut SessionState) -> Result<(), String> {
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .set_breakpoints(state.runtime_breakpoints.clone())
            .map_err(|err| format!("updating runtime breakpoints: {err}"))?;
    }
    Ok(())
}

fn rebuild_runtime_breakpoints(state: &mut SessionState) {
    let mut runtime_breakpoints = Vec::<StepBreakpoint>::new();
    for (source_path, requested) in &state.pending_breakpoints {
        let resolved = resolve_source_breakpoints(source_path, requested, state);
        runtime_breakpoints.extend(resolved.runtime);
    }
    runtime_breakpoints.sort_by(|left, right| {
        (
            left.workflow_id.as_str(),
            left.step_id.as_str(),
            checkpoint_sort_key(&left.checkpoint),
            left.condition.as_deref().unwrap_or(""),
        )
            .cmp(&(
                right.workflow_id.as_str(),
                right.step_id.as_str(),
                checkpoint_sort_key(&right.checkpoint),
                right.condition.as_deref().unwrap_or(""),
            ))
    });
    runtime_breakpoints.dedup();
    state.runtime_breakpoints = runtime_breakpoints;
}

fn emit_next_runtime_event<W>(
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
    timeout: Duration,
) -> Result<(), String>
where
    W: Write,
{
    let Some(runtime) = state.runtime.as_mut() else {
        return Ok(());
    };

    let deadline = Instant::now() + timeout;
    loop {
        let stop_events = runtime
            .controller
            .stop_events()
            .map_err(|err| format!("reading stop events: {err}"))?;
        if stop_events.len() > runtime.delivered_stop_events {
            let stop = stop_events[runtime.delivered_stop_events].clone();
            runtime.delivered_stop_events = runtime.delivered_stop_events.saturating_add(1);
            runtime.last_stop = Some(stop.clone());
            runtime.variable_store.reset();
            write_dap_message(
                writer,
                &stopped_event(
                    outbound.alloc(),
                    MAIN_THREAD_ID,
                    stop_reason_name(stop.reason.clone()),
                ),
            )?;
            return Ok(());
        }

        if !runtime.terminated {
            let finished = runtime
                .handle
                .as_ref()
                .map(|handle| handle.is_finished())
                .unwrap_or(true);
            if finished {
                let Some(handle) = runtime.handle.take() else {
                    runtime.terminated = true;
                    return Ok(());
                };
                let _ = handle
                    .join()
                    .map_err(|_| "runtime execution thread panicked".to_string())?;
                runtime.terminated = true;
                write_dap_message(writer, &terminated_event(outbound.alloc()))?;
                return Ok(());
            }
        }

        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }
        let wait = deadline.saturating_duration_since(now).min(STOP_WAIT_SLICE);
        let expected = runtime.delivered_stop_events.saturating_add(1);
        let _ = runtime
            .controller
            .wait_for_stop_count(expected, wait)
            .map_err(|err| format!("waiting for stop events: {err}"))?;
    }
}

fn stack_trace_body(state: &SessionState) -> Value {
    let Some(runtime) = state.runtime.as_ref() else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let Some(stop) = runtime.last_stop.as_ref() else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let source_path = state
        .launch
        .as_ref()
        .map(|launch| launch.spec.clone())
        .unwrap_or_default();

    let stack = runtime.controller.current_stack().unwrap_or_default();
    let mut frames = Vec::<Value>::new();
    if stack.is_empty() {
        let line = lookup_line_for_checkpoint(
            state.source_index.as_ref(),
            &stop.workflow_id,
            &stop.step_id,
            &stop.checkpoint,
        )
        .unwrap_or(1);
        frames.push(json!({
            "id": FRAME_ID_BASE,
            "name": format!("{}::{}", stop.workflow_id, stop.step_id),
            "line": line,
            "column": 1,
            "source": {
                "name": source_name(&source_path),
                "path": source_path
            }
        }));
    } else {
        for frame in stack.iter().rev() {
            let checkpoint = if frame.depth == stop.depth {
                stop.checkpoint.clone()
            } else {
                StepCheckpoint::Step
            };
            let line = lookup_line_for_checkpoint(
                state.source_index.as_ref(),
                &frame.workflow_id,
                &frame.step_id,
                &checkpoint,
            )
            .unwrap_or(1);
            let frame_id = FRAME_ID_BASE.saturating_add(u64::try_from(frame.depth).unwrap_or(0));
            frames.push(json!({
                "id": frame_id,
                "name": format!("{}::{}", frame.workflow_id, frame.step_id),
                "line": line,
                "column": 1,
                "source": {
                    "name": source_name(&source_path),
                    "path": source_path
                }
            }));
        }
    }

    json!({
        "stackFrames": frames,
        "totalFrames": frames.len()
    })
}

fn scopes_body(state: &mut SessionState) -> Value {
    let Some(runtime) = state.runtime.as_mut() else {
        return json!({ "scopes": [] });
    };
    let Some(stop) = runtime.last_stop.as_ref() else {
        return json!({ "scopes": [] });
    };
    let scopes = runtime.controller.current_scopes().unwrap_or_default();
    runtime.variable_store.reset();

    let mut locals = scopes.locals.clone();
    locals
        .entry("workflowId".to_string())
        .or_insert(Value::String(stop.workflow_id.clone()));
    locals
        .entry("stepId".to_string())
        .or_insert(Value::String(stop.step_id.clone()));
    locals
        .entry("checkpoint".to_string())
        .or_insert(Value::String(checkpoint_display_name(&stop.checkpoint)));

    let locals_ref = runtime.variable_store.insert_map(locals);
    let inputs_ref = runtime.variable_store.insert_map(scopes.inputs.clone());
    let steps_ref = runtime
        .variable_store
        .insert_map(step_scopes_to_value_map(&scopes));

    json!({
        "scopes": [
            {
                "name": "Locals",
                "presentationHint": "locals",
                "variablesReference": locals_ref,
                "expensive": false
            },
            {
                "name": "Inputs",
                "presentationHint": "registers",
                "variablesReference": inputs_ref,
                "expensive": false
            },
            {
                "name": "Steps",
                "presentationHint": "registers",
                "variablesReference": steps_ref,
                "expensive": false
            }
        ]
    })
}

fn variables_body(state: &mut SessionState, reference: u64) -> Value {
    let Some(runtime) = state.runtime.as_mut() else {
        return json!({ "variables": [] });
    };
    let variables = runtime.variable_store.variables_for_reference(reference);
    json!({ "variables": variables })
}

fn evaluate_body_for_expression(state: &mut SessionState, expression: &str) -> Value {
    let source_index = state.source_index.clone();
    let Some(runtime) = state.runtime.as_mut() else {
        return evaluate_body("runtime not started".to_string());
    };

    let value = evaluate_expression_with_fallback(runtime, source_index.as_ref(), expression)
        .unwrap_or_else(|| Value::String("null".to_string()));
    let child_ref = map_from_value(&value)
        .map(|map| runtime.variable_store.insert_map(map))
        .unwrap_or(0);
    json!({
        "result": display_value(&value),
        "variablesReference": child_ref
    })
}

fn evaluate_expression_with_fallback(
    runtime: &RuntimeSession,
    source_index: Option<&SourceIndex>,
    expression: &str,
) -> Option<Value> {
    let trimmed = expression.trim();
    if !trimmed.is_empty() && !trimmed.starts_with('$') && !trimmed.starts_with('/') {
        if let Some(stop) = runtime.last_stop.as_ref() {
            if let Some(mapped) =
                lookup_output_expression(source_index, &stop.workflow_id, &stop.step_id, trimmed)
            {
                return runtime.controller.evaluate_watch_expression(mapped).ok();
            }
        }
    }

    runtime.controller.evaluate_watch_expression(trimmed).ok()
}

#[derive(Debug, Default)]
struct ResolvedSourceBreakpoints {
    resolved: Vec<ResolvedBreakpoint>,
    runtime: Vec<StepBreakpoint>,
}

fn resolve_source_breakpoints(
    source_path: &str,
    requested: &[DapBreakpoint],
    state: &SessionState,
) -> ResolvedSourceBreakpoints {
    let launch_workflow = state
        .launch
        .as_ref()
        .map(|launch| launch.workflow_id.as_str());
    let mut index = state
        .source_index
        .clone()
        .filter(|idx| idx.path == source_path);
    if index.is_none() {
        index = build_source_index(source_path).ok();
    }

    let Some(index) = index else {
        let resolved = requested
            .iter()
            .map(|bp| ResolvedBreakpoint {
                line: bp.line,
                verified: true,
                message: Some("source index unavailable; deferred mapping".to_string()),
            })
            .collect::<Vec<_>>();
        return ResolvedSourceBreakpoints {
            resolved,
            runtime: Vec::new(),
        };
    };

    let mut resolved = Vec::<ResolvedBreakpoint>::new();
    let mut runtime_breakpoints = Vec::<StepBreakpoint>::new();
    for bp in requested {
        let Some(checkpoint) = resolve_breakpoint_checkpoint(bp.line, &index, launch_workflow)
        else {
            resolved.push(ResolvedBreakpoint {
                line: bp.line,
                verified: false,
                message: Some(
                    "breakpoint must be on or near step, successCriteria, or outputs".to_string(),
                ),
            });
            continue;
        };

        let mut runtime_bp =
            StepBreakpoint::new(checkpoint.workflow_id.clone(), checkpoint.step_id.clone());
        runtime_bp.checkpoint = checkpoint.checkpoint.clone();
        if let Some(condition) = bp
            .condition
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            runtime_bp = runtime_bp.with_condition(condition.clone());
        }
        runtime_breakpoints.push(runtime_bp);

        let mut message = if checkpoint.line != bp.line {
            Some(format!("mapped to executable line {}", checkpoint.line))
        } else {
            None
        };
        if let Some(condition) = bp
            .condition
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            message = Some(format!("condition: {condition}"));
        }
        resolved.push(ResolvedBreakpoint {
            line: checkpoint.line,
            verified: true,
            message,
        });
    }

    ResolvedSourceBreakpoints {
        resolved,
        runtime: runtime_breakpoints,
    }
}

fn resolve_breakpoint_checkpoint(
    line: u32,
    index: &SourceIndex,
    workflow_filter: Option<&str>,
) -> Option<IndexedCheckpoint> {
    let mut candidates = index
        .checkpoints
        .iter()
        .filter(|candidate| {
            workflow_filter.is_none_or(|workflow_id| candidate.workflow_id == workflow_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.line);

    if let Some(exact) = candidates.iter().find(|candidate| candidate.line == line) {
        return Some(exact.clone());
    }

    let mut best: Option<IndexedCheckpoint> = None;
    let mut best_distance = u32::MAX;
    for candidate in candidates {
        if candidate.line > line {
            continue;
        }
        let distance = line.saturating_sub(candidate.line);
        if distance < best_distance {
            best = Some(candidate);
            best_distance = distance;
        }
    }
    if best_distance <= BREAKPOINT_NEAREST_LINE_THRESHOLD {
        best
    } else {
        None
    }
}

fn build_source_index(path: &str) -> Result<SourceIndex, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("reading source index file: {err}"))?;
    let metadata = extract_source_metadata(&text);
    Ok(SourceIndex {
        path: path.to_string(),
        checkpoints: metadata.checkpoints,
        output_expressions: metadata.output_expressions,
    })
}

#[cfg(test)]
fn extract_checkpoints_from_text(text: &str) -> Vec<IndexedCheckpoint> {
    extract_source_metadata(text).checkpoints
}

#[derive(Debug, Default)]
struct SourceMetadata {
    checkpoints: Vec<IndexedCheckpoint>,
    output_expressions: BTreeMap<(String, String, String), String>,
}

fn extract_source_metadata(text: &str) -> SourceMetadata {
    let mut checkpoints = Vec::<IndexedCheckpoint>::new();
    let mut output_expressions = BTreeMap::<(String, String, String), String>::new();

    let mut in_workflows = false;
    let mut workflows_indent = 0usize;

    let mut current_workflow_id = String::new();
    let mut workflow_indent = 0usize;

    let mut in_steps = false;
    let mut steps_indent = 0usize;

    let mut current_step_id = String::new();
    let mut step_indent = 0usize;

    let mut in_success_criteria = false;
    let mut success_criteria_indent = 0usize;
    let mut criterion_index = 0usize;

    let mut in_outputs = false;
    let mut outputs_indent = 0usize;

    for (idx, raw_line) in text.lines().enumerate() {
        let line = u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX);
        let trimmed_start = raw_line.trim_start();
        let trimmed = trimmed_start.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw_line.len().saturating_sub(trimmed_start.len());

        if !in_workflows {
            if trimmed == "workflows:" {
                in_workflows = true;
                workflows_indent = indent;
            }
            continue;
        }

        if indent <= workflows_indent && trimmed != "workflows:" {
            in_workflows = false;
            in_steps = false;
            current_workflow_id.clear();
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
            continue;
        }

        if let Some(workflow_id) = parse_yaml_inline_value(trimmed, "- workflowId:")
            .or_else(|| parse_yaml_inline_value(trimmed, "workflowId:"))
        {
            current_workflow_id = workflow_id;
            workflow_indent = indent;
            in_steps = false;
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
            continue;
        }

        if current_workflow_id.is_empty() {
            continue;
        }

        if trimmed == "steps:" {
            in_steps = true;
            steps_indent = indent;
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
            continue;
        }

        if in_steps && indent <= steps_indent && trimmed != "steps:" {
            in_steps = false;
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
        }

        if !in_steps {
            continue;
        }

        if let Some(step_id) = parse_yaml_inline_value(trimmed, "- stepId:")
            .or_else(|| parse_yaml_inline_value(trimmed, "stepId:"))
        {
            current_step_id = step_id;
            step_indent = indent;
            in_success_criteria = false;
            in_outputs = false;
            criterion_index = 0;
            checkpoints.push(IndexedCheckpoint {
                line,
                workflow_id: current_workflow_id.clone(),
                step_id: current_step_id.clone(),
                checkpoint: StepCheckpoint::Step,
            });
            continue;
        }

        if current_step_id.is_empty() {
            continue;
        }

        if indent <= step_indent && trimmed.starts_with("- ") {
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
            continue;
        }

        if trimmed == "successCriteria:" {
            in_success_criteria = true;
            success_criteria_indent = indent;
            criterion_index = 0;
            continue;
        }

        if in_success_criteria {
            if indent <= success_criteria_indent {
                in_success_criteria = false;
            } else if trimmed.starts_with("- ") {
                checkpoints.push(IndexedCheckpoint {
                    line,
                    workflow_id: current_workflow_id.clone(),
                    step_id: current_step_id.clone(),
                    checkpoint: StepCheckpoint::SuccessCriterion {
                        index: criterion_index,
                    },
                });
                criterion_index = criterion_index.saturating_add(1);
                continue;
            }
        }

        if trimmed == "outputs:" {
            in_outputs = true;
            outputs_indent = indent;
            continue;
        }

        if in_outputs {
            if indent <= outputs_indent {
                in_outputs = false;
            } else if let Some((name, expression)) = parse_output_entry(trimmed) {
                checkpoints.push(IndexedCheckpoint {
                    line,
                    workflow_id: current_workflow_id.clone(),
                    step_id: current_step_id.clone(),
                    checkpoint: StepCheckpoint::Output { name: name.clone() },
                });
                output_expressions.insert(
                    (current_workflow_id.clone(), current_step_id.clone(), name),
                    expression,
                );
                continue;
            }
        }

        if indent <= workflow_indent && trimmed.starts_with("- ") {
            current_step_id.clear();
            in_success_criteria = false;
            in_outputs = false;
        }
    }

    SourceMetadata {
        checkpoints,
        output_expressions,
    }
}

fn parse_output_entry(line: &str) -> Option<(String, String)> {
    if line.starts_with('-') {
        return None;
    }
    let mut split = line.splitn(2, ':');
    let key = split.next()?.trim();
    let raw_value = split.next()?.trim();
    if key.is_empty() {
        return None;
    }
    let raw_value = raw_value.split(" #").next().unwrap_or(raw_value).trim();
    if raw_value.is_empty() {
        return None;
    }
    Some((trim_yaml_scalar(key), trim_yaml_scalar(raw_value)))
}

fn parse_yaml_inline_value(line: &str, prefix: &str) -> Option<String> {
    let raw = line.strip_prefix(prefix)?.trim();
    if raw.is_empty() {
        return None;
    }
    let raw = raw.split(" #").next().unwrap_or(raw).trim();
    if raw.is_empty() {
        return None;
    }
    Some(trim_yaml_scalar(raw))
}

fn trim_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn lookup_line_for_checkpoint(
    source_index: Option<&SourceIndex>,
    workflow_id: &str,
    step_id: &str,
    checkpoint: &StepCheckpoint,
) -> Option<u32> {
    let index = source_index?;
    let exact = index.checkpoints.iter().find(|candidate| {
        candidate.workflow_id == workflow_id
            && candidate.step_id == step_id
            && candidate.checkpoint == *checkpoint
    });
    if let Some(value) = exact {
        return Some(value.line);
    }
    let fallback = index.checkpoints.iter().find(|candidate| {
        candidate.workflow_id == workflow_id
            && candidate.step_id == step_id
            && matches!(candidate.checkpoint, StepCheckpoint::Step)
    });
    fallback.map(|value| value.line)
}

fn lookup_output_expression<'a>(
    source_index: Option<&'a SourceIndex>,
    workflow_id: &str,
    step_id: &str,
    output_name: &str,
) -> Option<&'a str> {
    let index = source_index?;
    index
        .output_expressions
        .get(&(
            workflow_id.to_string(),
            step_id.to_string(),
            output_name.to_string(),
        ))
        .map(String::as_str)
}

fn parse_launch_config(arguments: &Value) -> Result<LaunchConfig, String> {
    let spec = parse_string_argument(arguments, "spec")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "launch requires non-empty 'spec'".to_string())?;
    let workflow_id = parse_string_argument(arguments, "workflowId")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "launch requires non-empty 'workflowId'".to_string())?;

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

    Ok(LaunchConfig {
        spec,
        workflow_id,
        inputs,
        dry_run,
    })
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

fn source_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workflow")
        .to_string()
}

fn stop_reason_name(reason: DebugStopReason) -> &'static str {
    match reason {
        DebugStopReason::Breakpoint => "breakpoint",
        DebugStopReason::Pause => "pause",
        DebugStopReason::Step => "step",
    }
}

fn checkpoint_display_name(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("successCriteria[{index}]"),
        StepCheckpoint::Output { name } => format!("outputs.{name}"),
    }
}

fn checkpoint_sort_key(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("criterion:{index:08}"),
        StepCheckpoint::Output { name } => format!("output:{name}"),
    }
}

fn map_from_value(value: &Value) -> Option<BTreeMap<String, Value>> {
    match value {
        Value::Object(object) => {
            let mut map = BTreeMap::new();
            for (key, value) in object {
                map.insert(key.clone(), value.clone());
            }
            Some(map)
        }
        Value::Array(array) => {
            let mut map = BTreeMap::new();
            for (index, value) in array.iter().enumerate() {
                map.insert(format!("[{index}]"), value.clone());
            }
            Some(map)
        }
        _ => None,
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => match serde_json::to_string(value) {
            Ok(serialized) => serialized,
            Err(_) => "<unprintable>".to_string(),
        },
    }
}

fn step_scopes_to_value_map(scopes: &DebugScopes) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for (step_id, outputs) in &scopes.steps {
        let mut object = serde_json::Map::new();
        for (name, value) in outputs {
            object.insert(name.clone(), value.clone());
        }
        map.insert(step_id.clone(), Value::Object(object));
    }
    map
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
    fn extract_checkpoints_from_text_includes_step_criterion_and_output_lines() {
        let text = r#"
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: https://hnrss.org/frontpage
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          title_1: //item[1]/title
          link_1: //item[1]/link
"#;
        let checkpoints = extract_checkpoints_from_text(text);
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 5
                && matches!(entry.checkpoint, StepCheckpoint::Step)
                && entry.workflow_id == "get-hackernews"
                && entry.step_id == "fetch-rss"
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 8
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 10
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::Output { ref name } if name == "title_1"
                )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 11
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::Output { ref name } if name == "link_1"
                )
        }));
    }

    #[test]
    fn extract_source_metadata_tracks_output_expressions() {
        let text = r#"
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        outputs:
          title_1: //item[1]/title
"#;
        let metadata = extract_source_metadata(text);
        let key = (
            "get-hackernews".to_string(),
            "fetch-rss".to_string(),
            "title_1".to_string(),
        );
        assert_eq!(
            metadata.output_expressions.get(&key).map(String::as_str),
            Some("//item[1]/title")
        );
    }
}
