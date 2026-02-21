use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use arazzo_expr::EvalContext;
use serde::{Deserialize, Serialize};

use super::breakpoints::{first_matching_breakpoint, StepBreakpoint};

/// Reason the runtime stopped at a debug gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DebugStopReason {
    Breakpoint,
    Pause,
}

/// One deterministic debug stop event emitted at a step boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugStopEvent {
    pub seq: u64,
    pub workflow_id: String,
    pub step_id: String,
    pub reason: DebugStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint_condition: Option<String>,
}

#[derive(Debug, Default)]
struct ControllerState {
    breakpoints: Vec<StepBreakpoint>,
    pause_requested: bool,
    waiting: bool,
    continue_permit: bool,
    stop_events: Vec<DebugStopEvent>,
    next_seq: u64,
}

/// Thread-safe runtime debug gate controller for pause/resume and breakpoints.
#[derive(Debug, Default)]
pub struct DebugController {
    state: Mutex<ControllerState>,
    condvar: Condvar,
}

impl DebugController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_breakpoints(&self, breakpoints: Vec<StepBreakpoint>) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.breakpoints = breakpoints;
        Ok(())
    }

    pub fn request_pause(&self) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.pause_requested = true;
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.pause_requested = false;
        if guard.waiting {
            guard.continue_permit = true;
            self.condvar.notify_all();
        }
        Ok(())
    }

    pub fn stop_events(&self) -> Result<Vec<DebugStopEvent>, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(guard.stop_events.clone())
    }

    pub fn clear_stop_events(&self) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.stop_events.clear();
        Ok(())
    }

    pub fn wait_for_stop_count(&self, expected: usize, timeout: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;

        loop {
            if guard.stop_events.len() >= expected {
                return Ok(true);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next_guard, result) = self
                .condvar
                .wait_timeout(guard, remaining)
                .map_err(|_| "debug controller lock poisoned".to_string())?;
            guard = next_guard;
            if result.timed_out() && guard.stop_events.len() < expected {
                return Ok(false);
            }
        }
    }

    pub(crate) fn gate_step(
        &self,
        workflow_id: &str,
        step_id: &str,
        eval_ctx: &EvalContext,
    ) -> Result<(), String> {
        let (pause_requested, breakpoints) = {
            let guard = self
                .state
                .lock()
                .map_err(|_| "debug controller lock poisoned".to_string())?;
            (guard.pause_requested, guard.breakpoints.clone())
        };

        let matched_breakpoint =
            first_matching_breakpoint(&breakpoints, workflow_id, step_id, eval_ctx);
        let stop_info = if let Some(breakpoint) = matched_breakpoint {
            Some((
                DebugStopReason::Breakpoint,
                breakpoint.condition.and_then(|value| {
                    if value.trim().is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                }),
            ))
        } else if pause_requested {
            Some((DebugStopReason::Pause, None))
        } else {
            None
        };

        let Some((reason, breakpoint_condition)) = stop_info else {
            return Ok(());
        };

        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.next_seq = guard.next_seq.saturating_add(1);
        let seq = guard.next_seq;
        guard.stop_events.push(DebugStopEvent {
            seq,
            workflow_id: workflow_id.to_string(),
            step_id: step_id.to_string(),
            reason,
            breakpoint_condition,
        });
        guard.waiting = true;
        self.condvar.notify_all();

        while !guard.continue_permit {
            guard = self
                .condvar
                .wait(guard)
                .map_err(|_| "debug controller lock poisoned".to_string())?;
        }
        guard.continue_permit = false;
        guard.waiting = false;
        self.condvar.notify_all();
        Ok(())
    }
}
