use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    DebugController, DebugStopEvent, DebugStopReason, EngineBuilder, RuntimeError, StepBreakpoint,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::events::{stopped_event, terminated_event};
use super::requests::DapBreakpoint;
use super::source_index::{checkpoint_sort_key, resolve_source_breakpoints, SourceIndex};
use super::transport::{write_dap_message, OutboundSequence};
use super::variables::VariableStore;

pub(super) const MAIN_THREAD_ID: u64 = 1;
const INLINE_EVENT_TIMEOUT: Duration = Duration::from_millis(100);
const ENGINE_MONITOR_POLL: Duration = Duration::from_millis(25);

pub(super) enum EngineEvent {
    Stopped(DebugStopEvent),
    Terminated,
    Panicked,
}

#[derive(Debug, Clone)]
pub(super) struct LaunchConfig {
    pub(super) spec: String,
    /// `None` means "use the first workflow in the spec".
    pub(super) workflow_id: Option<String>,
    pub(super) inputs: BTreeMap<String, Value>,
    pub(super) dry_run: bool,
    pub(super) stop_on_entry: bool,
}

#[derive(Debug)]
pub(super) struct RuntimeSession {
    pub(super) controller: Arc<DebugController>,
    pub(super) cancel_token: Option<CancellationToken>,
    pub(super) monitor_handle: Option<thread::JoinHandle<()>>,
    pub(super) last_stop: Option<DebugStopEvent>,
    pub(super) terminated: bool,
    pub(super) variable_store: VariableStore,
}

#[derive(Debug, Default)]
pub(super) struct SessionState {
    pub(super) launch: Option<LaunchConfig>,
    pub(super) source_index: Option<SourceIndex>,
    pub(super) pending_breakpoints: HashMap<String, Vec<DapBreakpoint>>,
    pub(super) runtime_breakpoints: Vec<StepBreakpoint>,
    pub(super) runtime: Option<RuntimeSession>,
}

#[allow(deprecated)]
pub(super) fn ensure_runtime_started(
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

pub(super) fn sync_runtime_breakpoints(state: &mut SessionState) -> Result<(), String> {
    if let Some(runtime) = state.runtime.as_ref() {
        runtime
            .controller
            .set_breakpoints(state.runtime_breakpoints.clone())
            .map_err(|err| format!("updating runtime breakpoints: {err}"))?;
    }
    Ok(())
}

pub(super) fn rebuild_runtime_breakpoints(state: &mut SessionState) {
    let launch_workflow = state
        .launch
        .as_ref()
        .and_then(|launch| launch.workflow_id.as_deref());
    let mut runtime_breakpoints = Vec::<StepBreakpoint>::new();
    for (source_path, requested) in &state.pending_breakpoints {
        let resolved = resolve_source_breakpoints(
            source_path,
            requested,
            launch_workflow,
            state.source_index.as_ref(),
        );
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

pub(super) fn handle_engine_event<W>(
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

pub(super) fn inline_event_check<W>(
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

pub(super) fn cleanup_runtime(state: &mut SessionState) {
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

fn stop_reason_name(reason: DebugStopReason) -> &'static str {
    match reason {
        DebugStopReason::Breakpoint => "breakpoint",
        DebugStopReason::Pause => "pause",
        DebugStopReason::Step => "step",
        _ => "pause",
    }
}
