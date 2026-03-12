use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    DebugController, DebugScopes, DebugStopEvent, DebugStopReason, EngineBuilder, RuntimeError,
    StepBreakpoint, StepCheckpoint,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use yaml_rust2::parser::{Event as YamlEvent, MarkedEventReceiver, Parser as YamlParser};
use yaml_rust2::scanner::Marker as YamlMarker;

#[path = "dap/events.rs"]
mod events;
#[path = "dap/requests.rs"]
mod requests;
#[path = "dap/responses.rs"]
mod responses;

use events::{initialized_event, output_event, stopped_event, terminated_event};
use requests::{DapBreakpoint, DapRequest};
use responses::{
    continue_body, empty_body, error_response, evaluate_body, initialize_capabilities,
    response_with_body, set_breakpoints_body, threads_body, ResolvedBreakpoint,
};

const MAIN_THREAD_ID: u64 = 1;
const FRAME_ID_BASE: u64 = 100;
const BREAKPOINT_NEAREST_LINE_THRESHOLD: u32 = 10;
const INLINE_EVENT_TIMEOUT: Duration = Duration::from_millis(100);
const ENGINE_MONITOR_POLL: Duration = Duration::from_millis(25);

enum DapCommand {
    Request(DapRequest),
    Eof,
    ReadError(String),
}

enum EngineEvent {
    Stopped(DebugStopEvent),
    Terminated,
    Panicked,
}

#[derive(Debug, Clone)]
struct LaunchConfig {
    spec: String,
    /// `None` means "use the first workflow in the spec".
    workflow_id: Option<String>,
    inputs: BTreeMap<String, Value>,
    dry_run: bool,
    stop_on_entry: bool,
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
    line_contexts: BTreeMap<u32, SourceLineContext>,
    output_expressions: BTreeMap<(String, String, String), String>,
}

#[derive(Debug, Clone)]
struct SourceLineContext {
    workflow_id: String,
    step_id: String,
    area: BreakpointArea,
    prefer_forward_snap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakpointArea {
    Step,
    SuccessCriteria,
    OnSuccess,
    OnFailure,
    Outputs,
}

impl BreakpointArea {
    fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::SuccessCriteria => "successCriteria",
            Self::OnSuccess => "onSuccess",
            Self::OnFailure => "onFailure",
            Self::Outputs => "outputs",
        }
    }
}

#[derive(Debug)]
struct RuntimeSession {
    controller: Arc<DebugController>,
    cancel_token: Option<CancellationToken>,
    monitor_handle: Option<thread::JoinHandle<()>>,
    last_stop: Option<DebugStopEvent>,
    terminated: bool,
    variable_store: VariableStore,
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
///
/// Decouples stdin reading, engine event monitoring, and command processing
/// across three threads to prevent deadlocks when HTTP requests exceed any
/// single polling timeout.
pub fn run_dap_stdio<R, W>(reader: R, writer: &mut W) -> Result<(), String>
where
    R: BufRead + Read + Send + 'static,
    W: Write,
{
    let mut state = SessionState::default();
    let mut outbound = OutboundSequence::new();
    let (cmd_tx, cmd_rx) = mpsc::channel::<DapCommand>();
    let (event_tx, event_rx) = mpsc::channel::<EngineEvent>();

    // Thread A: reads DAP commands from stdin and sends them to the coordinator.
    thread::spawn(move || {
        let mut reader = reader;
        loop {
            match read_dap_message(&mut reader) {
                Ok(Some(payload)) => match serde_json::from_str::<DapRequest>(&payload) {
                    Ok(request) => {
                        if cmd_tx.send(DapCommand::Request(request)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        // Intentional: reader thread is exiting; if main loop already
                        // dropped the receiver, this send failing is harmless.
                        if cmd_tx
                            .send(DapCommand::ReadError(format!(
                                "parsing DAP request JSON: {err}"
                            )))
                            .is_err()
                        {
                            // Coordinator is gone; nothing left to do in reader thread.
                        }
                        break;
                    }
                },
                Ok(None) => {
                    // Intentional: EOF on stdin; receiver may already be dropped.
                    if cmd_tx.send(DapCommand::Eof).is_err() {
                        // Coordinator already exited.
                    }
                    break;
                }
                Err(err) => {
                    // Intentional: reader thread is exiting; receiver may already be dropped.
                    if cmd_tx.send(DapCommand::ReadError(err)).is_err() {
                        // Coordinator already exited.
                    }
                    break;
                }
            }
        }
    });

    let mut stdin_closed = false;

    // Coordinator loop (Thread B / main thread): multiplexes commands and engine
    // events. Neither channel blocks the other—engine events arrive via Thread C
    // regardless of whether stdin is readable.
    loop {
        // Drain any pending engine events first.
        while let Ok(event) = event_rx.try_recv() {
            handle_engine_event(event, &mut state, writer, &mut outbound)?;
        }

        // Check for the next command.
        let cmd = if stdin_closed {
            None
        } else {
            match cmd_rx.try_recv() {
                Ok(cmd) => Some(cmd),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    stdin_closed = true;
                    None
                }
            }
        };

        let Some(cmd) = cmd else {
            // No command available — check exit conditions.
            if stdin_closed {
                let engine_done = state
                    .runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.terminated);
                if engine_done {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(5));
            continue;
        };

        match cmd {
            DapCommand::Eof => {
                stdin_closed = true;
                let engine_done = state
                    .runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.terminated);
                if engine_done {
                    break;
                }
            }
            DapCommand::ReadError(err) => {
                return Err(err);
            }
            DapCommand::Request(request) => {
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
                        state.source_index = try_build_source_index(&launch.spec);
                        rebuild_runtime_breakpoints(&mut state);
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
                            empty_body(),
                            request.seq,
                        );
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
                            state.source_index = try_build_source_index(&source_path);
                        }

                        let resolved =
                            resolve_source_breakpoints(&source_path, &breakpoints, &state).resolved;
                        rebuild_runtime_breakpoints(&mut state);
                        sync_runtime_breakpoints(&mut state)?;

                        let body = set_breakpoints_body(&resolved);
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "setExceptionBreakpoints" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
                            empty_body(),
                            request.seq,
                        );
                        write_dap_message(writer, &response)?;
                    }
                    "configurationDone" => {
                        if let Err(err) = ensure_runtime_started(&mut state, &event_tx) {
                            let msg = format!("Arazzo debug: {err}\n");
                            write_dap_message(
                                writer,
                                &output_event(outbound.alloc(), "console", &msg),
                            )?;
                            let response =
                                error_response(outbound.alloc(), &command, request.seq, err);
                            write_dap_message(writer, &response)?;
                            write_dap_message(writer, &terminated_event(outbound.alloc()))?;
                            continue;
                        }
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
                            empty_body(),
                            request.seq,
                        );
                        write_dap_message(writer, &response)?;
                        inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                    }
                    "threads" => {
                        let body = threads_body(MAIN_THREAD_ID, "main");
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "stackTrace" => {
                        let body = stack_trace_body(&state);
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "scopes" => {
                        let body = scopes_body(&mut state);
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "variables" => {
                        let reference =
                            parse_u64_argument(&request.arguments, "variablesReference")
                                .unwrap_or(0);
                        let body = variables_body(&mut state, reference);
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "evaluate" => {
                        let expression = parse_string_argument(&request.arguments, "expression")
                            .unwrap_or_default();
                        let body = evaluate_body_for_expression(&mut state, &expression);
                        let response =
                            response_with_body(outbound.alloc(), &command, body, request.seq);
                        write_dap_message(writer, &response)?;
                    }
                    "continue" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
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
                            inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                        }
                    }
                    "next" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
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
                            inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                        }
                    }
                    "stepIn" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
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
                            inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                        }
                    }
                    "stepOut" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
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
                            inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                        }
                    }
                    "pause" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
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
                            inline_event_check(&event_rx, &mut state, writer, &mut outbound)?;
                        }
                    }
                    "disconnect" => {
                        let response = response_with_body(
                            outbound.alloc(),
                            &command,
                            empty_body(),
                            request.seq,
                        );
                        write_dap_message(writer, &response)?;
                        write_dap_message(writer, &terminated_event(outbound.alloc()))?;
                        cleanup_runtime(&mut state);
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
        }
    }

    Ok(())
}

#[allow(deprecated)]
fn ensure_runtime_started(
    state: &mut SessionState,
    event_tx: &mpsc::Sender<EngineEvent>,
) -> Result<(), String> {
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

    let workflow_ids: Vec<String> = spec
        .workflows
        .iter()
        .map(|wf| wf.workflow_id.clone())
        .collect();
    let workflow_id = match launch.workflow_id.clone() {
        Some(id) => id,
        None => infer_workflow_id(&state.runtime_breakpoints, &workflow_ids)?,
    };

    let controller = Arc::new(DebugController::new());
    if !state.runtime_breakpoints.is_empty() {
        controller
            .set_breakpoints(state.runtime_breakpoints.clone())
            .map_err(|err| format!("applying breakpoints: {err}"))?;
    }
    if launch.stop_on_entry {
        controller
            .request_pause()
            .map_err(|err| format!("requesting initial pause: {err}"))?;
    }

    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<CancellationToken>();
    let engine = EngineBuilder::new(spec)
        .debug_controller(Arc::clone(&controller))
        .dry_run(launch.dry_run)
        .build()
        .map_err(|err| format!("creating runtime engine: {err}"))?;
    let inputs = launch.inputs.clone();
    let engine_done = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::clone(&engine_done);
    let engine_handle = thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().map_err(|err| {
            RuntimeError::new(
                arazzo_runtime::RuntimeErrorKind::InternalError,
                format!("creating tokio runtime: {err}"),
            )
        })?;
        let result = rt.block_on(async {
            let handle = engine.execute(&workflow_id, inputs);
            let _ = cancel_tx.send(handle.cancel_token().clone());
            handle.collect().await.outputs
        });
        // Signal completion BEFORE runtime shutdown so the monitor detects
        // it immediately via the flag, not via is_finished() (which waits
        // for Runtime::drop to complete).
        done_flag.store(true, Ordering::Release);
        rt.shutdown_timeout(Duration::from_millis(50));
        result
    });

    // Receive the CancellationToken from the engine thread (blocks briefly).
    let cancel_token = cancel_rx.recv().ok();

    // Thread C: monitors engine stop events and thread completion.
    let monitor_controller = Arc::clone(&controller);
    let monitor_cancel = cancel_token.clone();
    let monitor_event_tx = event_tx.clone();
    let monitor_handle = thread::spawn(move || {
        engine_event_monitor(
            monitor_controller,
            monitor_event_tx,
            monitor_cancel,
            engine_handle,
            engine_done,
        )
    });

    state.runtime = Some(RuntimeSession {
        controller,
        cancel_token,
        monitor_handle: Some(monitor_handle),
        last_stop: None,
        terminated: false,
        variable_store: VariableStore::default(),
    });
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

/// Thread C: monitors the engine's debug controller for stop events and thread
/// completion, forwarding them to the coordinator via the `event_tx` channel.
/// Owns the engine `JoinHandle` exclusively—joins it when the engine finishes
/// or when the cancel token is cancelled.
///
/// NOTE: The cancel token is also cancelled on *normal* completion —
/// `ExecutionHandle::drop` cancels it after `collect()` returns.  So we
/// must treat cancellation as "engine finished" rather than "abort", drain
/// any remaining stop events, and still emit the Terminated event.
fn engine_event_monitor(
    controller: Arc<DebugController>,
    event_tx: mpsc::Sender<EngineEvent>,
    cancel_token: Option<CancellationToken>,
    engine_handle: thread::JoinHandle<Result<BTreeMap<String, Value>, RuntimeError>>,
    engine_done: Arc<AtomicBool>,
) {
    let mut delivered = 0usize;
    let mut handle = Some(engine_handle);

    loop {
        // Detect completion: the done flag (set right after block_on returns)
        // or thread finished or cancellation token fired.  The cancel token
        // fires on BOTH external abort AND normal completion (ExecutionHandle
        // Drop cancels the token), so we treat all three as "engine done".
        let finished = engine_done.load(Ordering::Acquire)
            || cancel_token.as_ref().is_some_and(|t| t.is_cancelled())
            || handle.as_ref().is_some_and(|h| h.is_finished());

        // Drain any new stop events from the controller.
        if let Ok(stop_events) = controller.stop_events() {
            while delivered < stop_events.len() {
                let stop = stop_events[delivered].clone();
                delivered += 1;
                if event_tx.send(EngineEvent::Stopped(stop)).is_err() {
                    return;
                }
            }
        }

        if finished {
            let Some(h) = handle.take() else {
                return;
            };
            // join() may block briefly while the tokio runtime shuts down
            // (bounded by shutdown_timeout(50ms) in the engine thread).
            match h.join() {
                Ok(_) => {
                    if event_tx.send(EngineEvent::Terminated).is_err() {
                        // Coordinator already exited.
                    }
                }
                Err(_) => {
                    if event_tx.send(EngineEvent::Panicked).is_err() {
                        // Coordinator already exited.
                    }
                }
            }
            return;
        } else if handle.is_none() {
            return;
        }

        // Condvar-driven sleep—wakes instantly when a stop event is posted.
        // Intentional: timeout or lock failure just means we'll re-poll on next iteration.
        let expected = delivered.saturating_add(1);
        if controller
            .wait_for_stop_count(expected, ENGINE_MONITOR_POLL)
            .is_err()
        {
            // Debug controller became unavailable; continue polling until shutdown.
        }
    }
}

fn handle_engine_event<W>(
    event: EngineEvent,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String>
where
    W: Write,
{
    match event {
        EngineEvent::Stopped(stop) => {
            let reason = stop_reason_name(stop.reason.clone());
            if let Some(runtime) = state.runtime.as_mut() {
                runtime.last_stop = Some(stop);
                runtime.variable_store.reset();
            }
            write_dap_message(
                writer,
                &stopped_event(outbound.alloc(), MAIN_THREAD_ID, reason),
            )?;
        }
        EngineEvent::Terminated | EngineEvent::Panicked => {
            if let Some(runtime) = state.runtime.as_mut() {
                runtime.terminated = true;
            }
            write_dap_message(writer, &terminated_event(outbound.alloc()))?;
        }
    }
    Ok(())
}

fn inline_event_check<W>(
    event_rx: &mpsc::Receiver<EngineEvent>,
    state: &mut SessionState,
    writer: &mut W,
    outbound: &mut OutboundSequence,
) -> Result<(), String>
where
    W: Write,
{
    if let Ok(event) = event_rx.recv_timeout(INLINE_EVENT_TIMEOUT) {
        handle_engine_event(event, state, writer, outbound)?;
    }
    Ok(())
}

fn cleanup_runtime(state: &mut SessionState) {
    if let Some(runtime) = state.runtime.as_mut() {
        if let Some(token) = &runtime.cancel_token {
            token.cancel();
        }
        // force_resume still needed — unblocks spawn_blocking debug gates after cancel.
        if runtime.controller.force_resume().is_err() {
            // Controller unavailable during teardown; continue cleanup.
        }
        if let Some(monitor) = runtime.monitor_handle.take() {
            // Intentional: join can only fail if the monitor thread panicked;
            // we're tearing down regardless.
            if monitor.join().is_err() {
                // Monitor panicked; runtime is already shutting down.
            }
        }
        runtime.terminated = true;
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
    let http_scopes = http_scopes_from_locals(&locals);
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
    let mut scope_entries = vec![json!({
        "name": "Locals",
        "presentationHint": "locals",
        "variablesReference": locals_ref,
        "expensive": false
    })];

    if let Some(request_scope) = http_scopes.request {
        let request_ref = runtime.variable_store.insert_map(request_scope);
        scope_entries.push(json!({
            "name": "Request",
            "presentationHint": "registers",
            "variablesReference": request_ref,
            "expensive": false
        }));
    }
    if let Some(response_scope) = http_scopes.response {
        let response_ref = runtime.variable_store.insert_map(response_scope);
        scope_entries.push(json!({
            "name": "Response",
            "presentationHint": "registers",
            "variablesReference": response_ref,
            "expensive": false
        }));
    }

    let inputs_ref = runtime.variable_store.insert_map(scopes.inputs.clone());
    scope_entries.push(json!({
        "name": "Inputs",
        "presentationHint": "registers",
        "variablesReference": inputs_ref,
        "expensive": false
    }));

    let steps_ref = runtime
        .variable_store
        .insert_map(step_scopes_to_value_map(&scopes));
    scope_entries.push(json!({
        "name": "Steps",
        "presentationHint": "registers",
        "variablesReference": steps_ref,
        "expensive": false
    }));

    json!({ "scopes": scope_entries })
}

fn http_scopes_from_locals(locals: &BTreeMap<String, Value>) -> HttpScopeMaps {
    let mut request = BTreeMap::<String, Value>::new();
    insert_scope_value(&mut request, "method", locals, "requestMethod");
    insert_scope_value(&mut request, "url", locals, "requestUrl");
    insert_scope_value(&mut request, "headers", locals, "requestHeaders");
    insert_scope_value(&mut request, "body", locals, "requestBody");

    let mut response = BTreeMap::<String, Value>::new();
    insert_scope_value(&mut response, "statusCode", locals, "responseStatusCode");
    insert_scope_value(&mut response, "contentType", locals, "responseContentType");
    insert_scope_value(&mut response, "headers", locals, "responseHeaders");
    insert_scope_value(&mut response, "bodyPreview", locals, "responseBodyPreview");
    if locals.contains_key("responseBodyRaw") {
        response.insert("bodyRawAvailable".to_string(), Value::Bool(true));
    }

    HttpScopeMaps {
        request: (!request.is_empty()).then_some(request),
        response: (!response.is_empty()).then_some(response),
    }
}

fn insert_scope_value(
    target: &mut BTreeMap<String, Value>,
    target_key: &str,
    source: &BTreeMap<String, Value>,
    source_key: &str,
) {
    if let Some(value) = source.get(source_key) {
        target.insert(target_key.to_string(), value.clone());
    }
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
                return try_evaluate_watch_expression(runtime, mapped);
            }
        }
    }

    try_evaluate_watch_expression(runtime, trimmed)
}

#[derive(Debug, Default)]
struct ResolvedSourceBreakpoints {
    resolved: Vec<ResolvedBreakpoint>,
    runtime: Vec<StepBreakpoint>,
}

#[derive(Debug, Default)]
struct HttpScopeMaps {
    request: Option<BTreeMap<String, Value>>,
    response: Option<BTreeMap<String, Value>>,
}

fn resolve_source_breakpoints(
    source_path: &str,
    requested: &[DapBreakpoint],
    state: &SessionState,
) -> ResolvedSourceBreakpoints {
    let launch_workflow = state
        .launch
        .as_ref()
        .and_then(|launch| launch.workflow_id.as_deref());
    let mut index = state
        .source_index
        .clone()
        .filter(|idx| idx.path == source_path);
    if index.is_none() {
        index = try_build_source_index(source_path);
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
        let line_context = resolve_line_context(bp.line, &index, launch_workflow);
        let Some(checkpoint) = resolve_breakpoint_checkpoint(bp.line, &index, launch_workflow)
        else {
            let message = invalid_breakpoint_message(line_context.as_ref());
            resolved.push(ResolvedBreakpoint {
                line: bp.line,
                verified: false,
                message: Some(message),
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

        let mut parts = Vec::<String>::new();
        if checkpoint.line != bp.line {
            parts.push(format!(
                "mapped line {} to {} on line {}",
                bp.line,
                checkpoint_display_name(&checkpoint.checkpoint),
                checkpoint.line
            ));
        }
        if let Some(condition) = bp
            .condition
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            parts.push(format!(
                "condition on {}: {}",
                checkpoint_display_name(&checkpoint.checkpoint),
                condition
            ));
        }
        let message = (!parts.is_empty()).then(|| parts.join("; "));
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

    let line_context = resolve_line_context(line, index, workflow_filter);
    if let Some(ctx) = line_context.as_ref() {
        let same_step = candidates
            .iter()
            .filter(|candidate| {
                candidate.workflow_id == ctx.workflow_id && candidate.step_id == ctx.step_id
            })
            .cloned()
            .collect::<Vec<_>>();
        let same_area = same_step
            .iter()
            .filter(|candidate| checkpoint_area(&candidate.checkpoint) == ctx.area)
            .cloned()
            .collect::<Vec<_>>();
        if !same_area.is_empty() {
            candidates = same_area;
        } else if !same_step.is_empty() {
            candidates = same_step;
        }
    }

    let prefer_forward = line_context
        .as_ref()
        .map(|ctx| ctx.prefer_forward_snap)
        .unwrap_or(false);

    let mut best: Option<IndexedCheckpoint> = None;
    let mut best_distance = u32::MAX;
    for candidate in candidates {
        let distance = candidate.line.abs_diff(line);
        if distance < best_distance
            || (distance == best_distance
                && is_better_direction_tiebreak(best.as_ref(), &candidate, line, prefer_forward))
        {
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

fn resolve_line_context(
    line: u32,
    index: &SourceIndex,
    workflow_filter: Option<&str>,
) -> Option<SourceLineContext> {
    if let Some(exact) = index
        .line_contexts
        .get(&line)
        .filter(|ctx| workflow_filter.is_none_or(|workflow_id| ctx.workflow_id == workflow_id))
    {
        return Some(exact.clone());
    }

    let mut best: Option<&SourceLineContext> = None;
    let mut best_line = 0u32;
    let mut best_distance = u32::MAX;
    for (&ctx_line, ctx) in &index.line_contexts {
        if workflow_filter.is_some_and(|workflow_id| ctx.workflow_id != workflow_id) {
            continue;
        }
        let distance = ctx_line.abs_diff(line);
        if distance > BREAKPOINT_NEAREST_LINE_THRESHOLD {
            continue;
        }
        if distance < best_distance
            || (distance == best_distance
                && is_better_line_tiebreak(best_line, ctx_line, line, false))
        {
            best = Some(ctx);
            best_line = ctx_line;
            best_distance = distance;
        }
    }
    best.cloned()
}

fn checkpoint_area(checkpoint: &StepCheckpoint) -> BreakpointArea {
    match checkpoint {
        StepCheckpoint::Step => BreakpointArea::Step,
        StepCheckpoint::SuccessCriterion { .. } => BreakpointArea::SuccessCriteria,
        StepCheckpoint::OnSuccessAction { .. }
        | StepCheckpoint::OnSuccessCriterion { .. }
        | StepCheckpoint::OnSuccessRetrySelected { .. }
        | StepCheckpoint::OnSuccessRetryDelay { .. } => BreakpointArea::OnSuccess,
        StepCheckpoint::OnFailureAction { .. }
        | StepCheckpoint::OnFailureCriterion { .. }
        | StepCheckpoint::OnFailureRetrySelected { .. }
        | StepCheckpoint::OnFailureRetryDelay { .. } => BreakpointArea::OnFailure,
        StepCheckpoint::Output { .. } => BreakpointArea::Outputs,
        _ => BreakpointArea::Step,
    }
}

fn is_better_direction_tiebreak(
    current_best: Option<&IndexedCheckpoint>,
    candidate: &IndexedCheckpoint,
    line: u32,
    prefer_forward: bool,
) -> bool {
    let Some(best) = current_best else {
        return true;
    };
    is_better_line_tiebreak(best.line, candidate.line, line, prefer_forward)
}

fn is_better_line_tiebreak(
    current_best_line: u32,
    candidate_line: u32,
    target_line: u32,
    prefer_forward: bool,
) -> bool {
    let current_best_is_forward = current_best_line >= target_line;
    let candidate_is_forward = candidate_line >= target_line;
    if current_best_is_forward != candidate_is_forward {
        return candidate_is_forward == prefer_forward;
    }
    candidate_line < current_best_line
}

fn invalid_breakpoint_message(line_context: Option<&SourceLineContext>) -> String {
    if let Some(ctx) = line_context {
        return format!(
            "no executable checkpoint near this line in {} block; use step, criteria item, action item, or output entry lines",
            ctx.area.label()
        );
    }
    "breakpoint must be on or near step, successCriteria, onSuccess, onFailure, or outputs"
        .to_string()
}

fn build_source_index(path: &str) -> Result<SourceIndex, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("reading source index file: {err}"))?;
    let metadata = extract_source_metadata(&text);
    Ok(SourceIndex {
        path: path.to_string(),
        checkpoints: metadata.checkpoints,
        line_contexts: metadata.line_contexts,
        output_expressions: metadata.output_expressions,
    })
}

fn try_build_source_index(path: &str) -> Option<SourceIndex> {
    // Intentional: source index failures should not block launch or breakpoint setup.
    // The adapter returns verified placeholders and resolves at runtime instead.
    build_source_index(path).ok()
}

fn try_evaluate_watch_expression(runtime: &RuntimeSession, expression: &str) -> Option<Value> {
    // Intentional: watch/evaluate should degrade to "null" rather than hard-fail DAP.
    runtime
        .controller
        .evaluate_watch_expression(expression)
        .ok()
}

#[cfg(test)]
fn extract_checkpoints_from_text(text: &str) -> Vec<IndexedCheckpoint> {
    extract_source_metadata(text).checkpoints
}

#[derive(Debug, Default)]
struct SourceMetadata {
    checkpoints: Vec<IndexedCheckpoint>,
    line_contexts: BTreeMap<u32, SourceLineContext>,
    output_expressions: BTreeMap<(String, String, String), String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionSection {
    OnSuccess,
    OnFailure,
}

/// Position in the YAML tree during event-driven parsing.
enum PathSegment {
    Map {
        active_key: Option<String>,
        key_line: Option<u32>,
        expecting_value: bool,
    },
    Seq {
        index: usize,
    },
}

/// SAX-style receiver that builds [`SourceMetadata`] from yaml-rust2 parse events.
struct MetadataReceiver {
    path: Vec<PathSegment>,
    checkpoints: Vec<IndexedCheckpoint>,
    line_contexts: BTreeMap<u32, SourceLineContext>,
    output_expressions: BTreeMap<(String, String, String), String>,
    current_workflow_id: String,
    current_step_id: String,
    step_mapping_start_line: Option<u32>,
    criterion_index: usize,
    on_success_action_index: usize,
    on_failure_action_index: usize,
    current_action_section: Option<ActionSection>,
    current_action_index: Option<usize>,
    action_criteria_index: usize,
}

impl MetadataReceiver {
    fn new() -> Self {
        Self {
            path: Vec::new(),
            checkpoints: Vec::new(),
            line_contexts: BTreeMap::new(),
            output_expressions: BTreeMap::new(),
            current_workflow_id: String::new(),
            current_step_id: String::new(),
            step_mapping_start_line: None,
            criterion_index: 0,
            on_success_action_index: 0,
            on_failure_action_index: 0,
            current_action_section: None,
            current_action_index: None,
            action_criteria_index: 0,
        }
    }

    fn into_metadata(self) -> SourceMetadata {
        SourceMetadata {
            checkpoints: self.checkpoints,
            line_contexts: self.line_contexts,
            output_expressions: self.output_expressions,
        }
    }

    fn line_from_mark(mark: YamlMarker) -> u32 {
        // yaml-rust2 Marker::line() is 0-based but the mark passed to on_event
        // points to the scanner position after the token, effectively 1-based
        // for our purposes.
        u32::try_from(mark.line()).unwrap_or(u32::MAX)
    }

    fn record_context(&mut self, line: u32, area: BreakpointArea, prefer_forward_snap: bool) {
        if self.current_workflow_id.is_empty() || self.current_step_id.is_empty() {
            return;
        }
        self.line_contexts.insert(
            line,
            SourceLineContext {
                workflow_id: self.current_workflow_id.clone(),
                step_id: self.current_step_id.clone(),
                area,
                prefer_forward_snap,
            },
        );
    }

    fn push_checkpoint(&mut self, line: u32, checkpoint: StepCheckpoint) {
        self.checkpoints.push(IndexedCheckpoint {
            line,
            workflow_id: self.current_workflow_id.clone(),
            step_id: self.current_step_id.clone(),
            checkpoint,
        });
    }

    /// Returns true if we are inside `workflows` → seq → map → `steps` → seq.
    fn is_in_steps_seq(&self) -> bool {
        // Pattern: Map(workflows) / Seq / Map(active_key=steps) / Seq
        let len = self.path.len();
        if len < 4 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("steps")
    }

    /// Returns true if we are inside a step mapping (child of steps seq).
    fn is_in_step_mapping(&self) -> bool {
        // Pattern: Map(workflows) / Seq / Map(active_key=steps) / Seq / Map(step)
        let len = self.path.len();
        if len < 5 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("steps")
    }

    /// Returns the active key of the map at `path[index]`, if it is a map.
    fn parent_key_at(&self, index: usize) -> Option<&str> {
        match self.path.get(index) {
            Some(PathSegment::Map {
                active_key: Some(key),
                ..
            }) => Some(key.as_str()),
            _ => None,
        }
    }

    /// Returns the section key of the innermost step-level section we are inside.
    fn step_section_key(&self) -> Option<&str> {
        // Walk from the top of the stack looking for a map whose active key
        // is one of the recognized section headers.
        for segment in self.path.iter().rev() {
            if let PathSegment::Map {
                active_key: Some(key),
                ..
            } = segment
            {
                match key.as_str() {
                    "successCriteria" | "onSuccess" | "onFailure" | "criteria" | "outputs" => {
                        return Some(key.as_str());
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// The current breakpoint area derived from the path stack.
    fn current_area(&self) -> BreakpointArea {
        match self.step_section_key() {
            Some("outputs") => BreakpointArea::Outputs,
            Some("criteria") => match self.current_action_section {
                Some(ActionSection::OnSuccess) => BreakpointArea::OnSuccess,
                Some(ActionSection::OnFailure) => BreakpointArea::OnFailure,
                None => BreakpointArea::Step,
            },
            Some("onSuccess") => BreakpointArea::OnSuccess,
            Some("onFailure") => BreakpointArea::OnFailure,
            Some("successCriteria") => BreakpointArea::SuccessCriteria,
            _ => BreakpointArea::Step,
        }
    }

    /// Is the innermost seq a `successCriteria` list?
    fn is_in_success_criteria_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("successCriteria")
    }

    /// Is the innermost seq an `onSuccess` list?
    fn is_in_on_success_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("onSuccess")
    }

    /// Is the innermost seq an `onFailure` list?
    fn is_in_on_failure_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("onFailure")
    }

    /// Is the innermost seq a `criteria` list inside an action?
    fn is_in_action_criteria_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("criteria")
    }

    /// Is the innermost map an `outputs` map?
    fn is_in_outputs_map(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && self.parent_key_at(len - 2) == Some("outputs")
    }

    /// After a value is consumed (MappingEnd, SequenceEnd, Alias, or value scalar),
    /// reset the parent state so it's ready for the next key or seq item.
    fn consume_value_in_parent(&mut self) {
        let Some(parent) = self.path.last_mut() else {
            return;
        };
        match parent {
            PathSegment::Map {
                active_key,
                key_line,
                expecting_value,
            } => {
                *active_key = None;
                *key_line = None;
                *expecting_value = false;
            }
            PathSegment::Seq { index } => {
                *index = index.saturating_add(1);
            }
        }
    }

    fn handle_mapping_start(&mut self, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // If parent is the steps seq, this is a new step mapping.
        if self.is_in_steps_seq() {
            self.step_mapping_start_line = Some(line);
            self.current_step_id.clear();
            self.criterion_index = 0;
            self.on_success_action_index = 0;
            self.on_failure_action_index = 0;
            self.current_action_section = None;
            self.current_action_index = None;
            self.action_criteria_index = 0;
        }

        // If parent is an onSuccess or onFailure seq, emit the action checkpoint.
        if self.is_in_on_success_seq() {
            let action_index = self.on_success_action_index;
            self.on_success_action_index = self.on_success_action_index.saturating_add(1);
            self.current_action_section = Some(ActionSection::OnSuccess);
            self.current_action_index = Some(action_index);
            self.action_criteria_index = 0;
            self.push_checkpoint(
                line,
                StepCheckpoint::OnSuccessAction {
                    index: action_index,
                },
            );
        } else if self.is_in_on_failure_seq() {
            let action_index = self.on_failure_action_index;
            self.on_failure_action_index = self.on_failure_action_index.saturating_add(1);
            self.current_action_section = Some(ActionSection::OnFailure);
            self.current_action_index = Some(action_index);
            self.action_criteria_index = 0;
            self.push_checkpoint(
                line,
                StepCheckpoint::OnFailureAction {
                    index: action_index,
                },
            );
        } else if self.is_in_success_criteria_seq() {
            // Criterion item is a mapping (e.g. `- condition: ...`).
            self.push_checkpoint(
                line,
                StepCheckpoint::SuccessCriterion {
                    index: self.criterion_index,
                },
            );
            self.criterion_index = self.criterion_index.saturating_add(1);
        } else if self.is_in_action_criteria_seq() {
            let checkpoint = match self.current_action_section {
                Some(ActionSection::OnSuccess) => StepCheckpoint::OnSuccessCriterion {
                    action_index: self.current_action_index.unwrap_or(0),
                    criterion_index: self.action_criteria_index,
                },
                Some(ActionSection::OnFailure) => StepCheckpoint::OnFailureCriterion {
                    action_index: self.current_action_index.unwrap_or(0),
                    criterion_index: self.action_criteria_index,
                },
                None => StepCheckpoint::Step,
            };
            self.push_checkpoint(line, checkpoint);
            self.action_criteria_index = self.action_criteria_index.saturating_add(1);
        }

        self.path.push(PathSegment::Map {
            active_key: None,
            key_line: None,
            expecting_value: false,
        });
    }

    fn handle_mapping_end(&mut self) {
        // If leaving a step mapping, clear step context.
        if self.is_in_step_mapping() {
            self.current_step_id.clear();
            self.step_mapping_start_line = None;
        }

        // If leaving a workflow mapping, clear workflow context.
        // Pattern: workflows / seq / map(workflow) — path len would be the workflow map level.
        let len = self.path.len();
        if len >= 3
            && matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("workflows")
        {
            self.current_workflow_id.clear();
        }

        self.path.pop();
        self.consume_value_in_parent();
    }

    fn handle_sequence_start(&mut self, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // If parent key is a section header, record line_context with forward snap.
        if let Some(PathSegment::Map {
            active_key: Some(key),
            key_line,
            ..
        }) = self.path.last()
        {
            let ctx_line = key_line.unwrap_or(line);
            match key.as_str() {
                "successCriteria" => {
                    self.record_context(ctx_line, BreakpointArea::SuccessCriteria, true);
                    self.criterion_index = 0;
                }
                "onSuccess" => {
                    self.record_context(ctx_line, BreakpointArea::OnSuccess, true);
                    self.on_success_action_index = 0;
                }
                "onFailure" => {
                    self.record_context(ctx_line, BreakpointArea::OnFailure, true);
                    self.on_failure_action_index = 0;
                }
                "criteria" => {
                    let area = match self.current_action_section {
                        Some(ActionSection::OnSuccess) => BreakpointArea::OnSuccess,
                        Some(ActionSection::OnFailure) => BreakpointArea::OnFailure,
                        None => BreakpointArea::Step,
                    };
                    self.record_context(ctx_line, area, true);
                    self.action_criteria_index = 0;
                }
                _ => {}
            }
        }

        self.path.push(PathSegment::Seq { index: 0 });
    }

    fn handle_sequence_end(&mut self) {
        self.path.pop();
        self.consume_value_in_parent();
    }

    fn handle_scalar(&mut self, value: String, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // Snapshot the parent state to avoid holding a mutable borrow on self.path
        // while calling other &self / &mut self methods.
        enum ParentKind {
            MapKey,
            MapValue {
                key_name: String,
                key_mark_line: u32,
            },
            Seq,
        }

        let parent_kind = match self.path.last() {
            Some(PathSegment::Map {
                expecting_value,
                active_key,
                key_line,
                ..
            }) => {
                if *expecting_value {
                    ParentKind::MapValue {
                        key_name: active_key.clone().unwrap_or_default(),
                        key_mark_line: key_line.unwrap_or(line),
                    }
                } else {
                    ParentKind::MapKey
                }
            }
            Some(PathSegment::Seq { .. }) => ParentKind::Seq,
            None => return,
        };

        match parent_kind {
            ParentKind::MapValue {
                key_name,
                key_mark_line,
            } => {
                if self.is_in_outputs_map() {
                    self.push_checkpoint(
                        key_mark_line,
                        StepCheckpoint::Output {
                            name: key_name.clone(),
                        },
                    );
                    self.output_expressions.insert(
                        (
                            self.current_workflow_id.clone(),
                            self.current_step_id.clone(),
                            key_name,
                        ),
                        value,
                    );
                } else if key_name == "workflowId" && self.is_in_workflow_mapping() {
                    self.current_workflow_id = value;
                } else if key_name == "stepId" && self.is_in_step_mapping() {
                    self.current_step_id = value;
                    if let Some(step_line) = self.step_mapping_start_line {
                        self.push_checkpoint(step_line, StepCheckpoint::Step);
                    }
                } else {
                    let area = self.current_area();
                    self.record_context(line, area, false);
                }

                // Reset parent for next key.
                if let Some(PathSegment::Map {
                    active_key,
                    key_line,
                    expecting_value,
                }) = self.path.last_mut()
                {
                    *active_key = None;
                    *key_line = None;
                    *expecting_value = false;
                }
            }
            ParentKind::MapKey => {
                // Set key on parent.
                if let Some(PathSegment::Map {
                    active_key,
                    key_line,
                    expecting_value,
                }) = self.path.last_mut()
                {
                    *active_key = Some(value.clone());
                    *key_line = Some(line);
                    *expecting_value = true;
                }

                if value == "outputs" && self.is_in_step_mapping_via_parent() {
                    self.record_context(line, BreakpointArea::Outputs, true);
                }
            }
            ParentKind::Seq => {
                if self.is_in_success_criteria_seq() {
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::SuccessCriterion {
                            index: self.criterion_index,
                        },
                    );
                    self.criterion_index = self.criterion_index.saturating_add(1);
                } else if self.is_in_action_criteria_seq() {
                    let checkpoint = match self.current_action_section {
                        Some(ActionSection::OnSuccess) => StepCheckpoint::OnSuccessCriterion {
                            action_index: self.current_action_index.unwrap_or(0),
                            criterion_index: self.action_criteria_index,
                        },
                        Some(ActionSection::OnFailure) => StepCheckpoint::OnFailureCriterion {
                            action_index: self.current_action_index.unwrap_or(0),
                            criterion_index: self.action_criteria_index,
                        },
                        None => StepCheckpoint::Step,
                    };
                    self.push_checkpoint(line, checkpoint);
                    self.action_criteria_index = self.action_criteria_index.saturating_add(1);
                } else if self.is_in_on_success_seq() {
                    let action_index = self.on_success_action_index;
                    self.on_success_action_index = self.on_success_action_index.saturating_add(1);
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::OnSuccessAction {
                            index: action_index,
                        },
                    );
                } else if self.is_in_on_failure_seq() {
                    let action_index = self.on_failure_action_index;
                    self.on_failure_action_index = self.on_failure_action_index.saturating_add(1);
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::OnFailureAction {
                            index: action_index,
                        },
                    );
                }

                // Increment seq index.
                if let Some(PathSegment::Seq { index }) = self.path.last_mut() {
                    *index = index.saturating_add(1);
                }
            }
        }
    }

    /// True if current top-of-stack is inside a workflow mapping (not step).
    fn is_in_workflow_mapping(&self) -> bool {
        let len = self.path.len();
        if len < 3 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("workflows")
    }

    /// True if the parent of the current map is a step mapping.
    /// Used when we're inside a nested map (like outputs) but need to know we're
    /// within a step context.
    fn is_in_step_mapping_via_parent(&self) -> bool {
        // We need to find a step mapping ancestor.
        let len = self.path.len();
        if len < 5 {
            return false;
        }
        // Look for the steps/seq/map pattern in the path.
        for i in 0..len.saturating_sub(2) {
            if self.parent_key_at(i) == Some("steps")
                && matches!(self.path.get(i + 1), Some(PathSegment::Seq { .. }))
                && matches!(self.path.get(i + 2), Some(PathSegment::Map { .. }))
            {
                return true;
            }
        }
        false
    }

    fn handle_alias(&mut self) {
        self.consume_value_in_parent();
    }
}

impl MarkedEventReceiver for MetadataReceiver {
    fn on_event(&mut self, event: YamlEvent, mark: YamlMarker) {
        match event {
            YamlEvent::MappingStart(_, _) => self.handle_mapping_start(mark),
            YamlEvent::MappingEnd => self.handle_mapping_end(),
            YamlEvent::SequenceStart(_, _) => self.handle_sequence_start(mark),
            YamlEvent::SequenceEnd => self.handle_sequence_end(),
            YamlEvent::Scalar(value, _, _, _) => self.handle_scalar(value, mark),
            YamlEvent::Alias(_) => self.handle_alias(),
            _ => {}
        }
    }
}

fn extract_source_metadata(text: &str) -> SourceMetadata {
    let mut receiver = MetadataReceiver::new();
    let mut parser = YamlParser::new_from_str(text);
    // Parsing failures degrade to empty metadata rather than blocking DAP.
    let _ = parser.load(&mut receiver, false);
    receiver.into_metadata()
}

fn lookup_line_for_checkpoint(
    source_index: Option<&SourceIndex>,
    workflow_id: &str,
    step_id: &str,
    checkpoint: &StepCheckpoint,
) -> Option<u32> {
    let index = source_index?;
    let exact = index
        .checkpoints
        .iter()
        .find(|candidate| {
            candidate.workflow_id == workflow_id
                && candidate.step_id == step_id
                && candidate.checkpoint == *checkpoint
        })
        .or_else(|| {
            retry_lifecycle_action_checkpoint(checkpoint).and_then(|action_checkpoint| {
                index.checkpoints.iter().find(|candidate| {
                    candidate.workflow_id == workflow_id
                        && candidate.step_id == step_id
                        && candidate.checkpoint == action_checkpoint
                })
            })
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

/// When `workflowId` is omitted from the launch config, pick the workflow to run.
/// Preference order:
/// 1. The workflow that the first resolved breakpoint belongs to.
/// 2. The first workflow defined in the spec (by workflow_id list).
fn infer_workflow_id(
    runtime_breakpoints: &[StepBreakpoint],
    workflow_ids: &[String],
) -> Result<String, String> {
    if let Some(bp) = runtime_breakpoints.first() {
        return Ok(bp.workflow_id.clone());
    }
    workflow_ids
        .first()
        .cloned()
        .ok_or_else(|| "spec contains no workflows".to_string())
}

fn parse_launch_config(arguments: &Value) -> Result<LaunchConfig, String> {
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
        _ => "pause",
    }
}

fn checkpoint_display_name(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("successCriteria[{index}]"),
        StepCheckpoint::OnSuccessAction { index } => format!("onSuccess[{index}]"),
        StepCheckpoint::OnSuccessCriterion {
            action_index,
            criterion_index,
        } => format!("onSuccess[{action_index}].criteria[{criterion_index}]"),
        StepCheckpoint::OnFailureAction { index } => format!("onFailure[{index}]"),
        StepCheckpoint::OnFailureCriterion {
            action_index,
            criterion_index,
        } => format!("onFailure[{action_index}].criteria[{criterion_index}]"),
        StepCheckpoint::OnSuccessRetrySelected { action_index } => {
            format!("onSuccess[{action_index}].retrySelected")
        }
        StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            format!("onSuccess[{action_index}].retryDelay")
        }
        StepCheckpoint::OnFailureRetrySelected { action_index } => {
            format!("onFailure[{action_index}].retrySelected")
        }
        StepCheckpoint::OnFailureRetryDelay { action_index } => {
            format!("onFailure[{action_index}].retryDelay")
        }
        StepCheckpoint::Output { name } => format!("outputs.{name}"),
        _ => "step".to_string(),
    }
}

fn checkpoint_sort_key(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("criterion:{index:08}"),
        StepCheckpoint::OnSuccessAction { index } => format!("on-success:{index:08}"),
        StepCheckpoint::OnSuccessCriterion {
            action_index,
            criterion_index,
        } => format!("on-success-criterion:{action_index:08}:{criterion_index:08}"),
        StepCheckpoint::OnFailureAction { index } => format!("on-failure:{index:08}"),
        StepCheckpoint::OnFailureCriterion {
            action_index,
            criterion_index,
        } => format!("on-failure-criterion:{action_index:08}:{criterion_index:08}"),
        StepCheckpoint::OnSuccessRetrySelected { action_index } => {
            format!("on-success-retry-selected:{action_index:08}")
        }
        StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            format!("on-success-retry-delay:{action_index:08}")
        }
        StepCheckpoint::OnFailureRetrySelected { action_index } => {
            format!("on-failure-retry-selected:{action_index:08}")
        }
        StepCheckpoint::OnFailureRetryDelay { action_index } => {
            format!("on-failure-retry-delay:{action_index:08}")
        }
        StepCheckpoint::Output { name } => format!("output:{name}"),
        _ => "step".to_string(),
    }
}

fn retry_lifecycle_action_checkpoint(checkpoint: &StepCheckpoint) -> Option<StepCheckpoint> {
    match checkpoint {
        StepCheckpoint::OnSuccessRetrySelected { action_index }
        | StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            Some(StepCheckpoint::OnSuccessAction {
                index: *action_index,
            })
        }
        StepCheckpoint::OnFailureRetrySelected { action_index }
        | StepCheckpoint::OnFailureRetryDelay { action_index } => {
            Some(StepCheckpoint::OnFailureAction {
                index: *action_index,
            })
        }
        _ => None,
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

    #[test]
    fn extract_checkpoints_from_text_includes_action_and_output_lines() {
        let text = r#"
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: https://hnrss.org/frontpage
        successCriteria:
          - condition: $statusCode == 200
        onSuccess:
          - type: goto
            stepId: done
            criteria:
              - condition: $statusCode == 200
        onFailure:
          - type: retry
            criteria:
              - condition: $statusCode == 503
          - type: end
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
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnSuccessAction { index: 0 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnSuccessCriterion {
                    action_index: 0,
                    criterion_index: 0
                }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureAction { index: 0 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureCriterion {
                    action_index: 0,
                    criterion_index: 0
                }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureAction { index: 1 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 20
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::Output { ref name } if name == "title_1"
                )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 21
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

    #[test]
    fn resolve_breakpoint_checkpoint_snaps_on_failure_header_to_failure_action() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: fetch
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - type: retry
            criteria:
              - condition: $statusCode == 503
          - type: end
"#;
        let metadata = extract_source_metadata(text);
        let index = SourceIndex {
            path: "/tmp/workflow.arazzo.yaml".to_string(),
            checkpoints: metadata.checkpoints,
            line_contexts: metadata.line_contexts,
            output_expressions: metadata.output_expressions,
        };
        let on_failure_line = u32::try_from(
            text.lines()
                .position(|line| line.trim() == "onFailure:")
                .unwrap_or(0)
                .saturating_add(1),
        )
        .unwrap_or(0);
        let resolved = resolve_breakpoint_checkpoint(on_failure_line, &index, Some("wf"));
        let resolved = match resolved {
            Some(value) => value,
            None => panic!("expected onFailure header to resolve to failure action"),
        };
        assert!(resolved.line > on_failure_line);
        assert!(matches!(
            resolved.checkpoint,
            StepCheckpoint::OnFailureAction { index: 0 }
        ));
    }

    #[test]
    fn resolve_source_breakpoints_reports_mapped_checkpoint_name() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: fetch
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - type: end
"#;
        let metadata = extract_source_metadata(text);
        let source_path = "/tmp/workflow.arazzo.yaml".to_string();
        let state = SessionState {
            launch: Some(LaunchConfig {
                spec: source_path.clone(),
                workflow_id: Some("wf".to_string()),
                inputs: BTreeMap::new(),
                dry_run: false,
                stop_on_entry: false,
            }),
            source_index: Some(SourceIndex {
                path: source_path.clone(),
                checkpoints: metadata.checkpoints,
                line_contexts: metadata.line_contexts,
                output_expressions: metadata.output_expressions,
            }),
            ..SessionState::default()
        };

        let on_failure_line = u32::try_from(
            text.lines()
                .position(|line| line.trim() == "onFailure:")
                .unwrap_or(0)
                .saturating_add(1),
        )
        .unwrap_or(0);

        let resolved = resolve_source_breakpoints(
            &source_path,
            &[DapBreakpoint {
                line: on_failure_line,
                condition: None,
            }],
            &state,
        );
        assert_eq!(resolved.resolved.len(), 1);
        let mapped = &resolved.resolved[0];
        assert!(mapped.verified);
        let message = mapped.message.as_deref().unwrap_or("");
        assert!(message.contains("onFailure[0]"));
        assert!(message.contains("mapped line"));
    }

    #[test]
    fn extract_source_metadata_handles_flow_style_outputs() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        outputs: {title: "$response.body.title", count: "$response.body.count"}
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            matches!(
                &entry.checkpoint,
                StepCheckpoint::Output { name } if name == "title"
            )
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            matches!(
                &entry.checkpoint,
                StepCheckpoint::Output { name } if name == "count"
            )
        }));
        let key = ("wf".to_string(), "s1".to_string(), "title".to_string());
        assert_eq!(
            metadata.output_expressions.get(&key).map(String::as_str),
            Some("$response.body.title")
        );
    }

    #[test]
    fn extract_source_metadata_handles_block_scalar() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        description: |
          This step fetches data
          from the API endpoint.
        successCriteria:
          - condition: $statusCode == 200
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1" && matches!(entry.checkpoint, StepCheckpoint::Step)
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1"
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
    }

    #[test]
    fn extract_source_metadata_handles_flow_sequence_criteria() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        successCriteria: [{condition: "$statusCode == 200"}]
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1" && matches!(entry.checkpoint, StepCheckpoint::Step)
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1"
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
    }
}
