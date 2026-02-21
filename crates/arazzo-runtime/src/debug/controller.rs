use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use arazzo_expr::EvalContext;
use serde::{Deserialize, Serialize};

use super::breakpoints::{first_matching_breakpoint, StepBreakpoint};
use super::{DebugScopes, DebugStackFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RunMode {
    #[default]
    Continue,
    StepIn {
        origin_seq: u64,
    },
    StepOver {
        origin_seq: u64,
        origin_depth: usize,
    },
    StepOut {
        origin_seq: u64,
        target_depth: usize,
    },
}

/// Reason the runtime stopped at a debug gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DebugStopReason {
    Breakpoint,
    Pause,
    Step,
}

/// One deterministic debug stop event emitted at a step boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DebugStopEvent {
    pub seq: u64,
    pub workflow_id: String,
    pub step_id: String,
    pub depth: usize,
    pub reason: DebugStopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint_condition: Option<String>,
}

#[derive(Debug, Default)]
struct ControllerState {
    breakpoints: Vec<StepBreakpoint>,
    pause_requested: bool,
    run_mode: RunMode,
    waiting: bool,
    continue_permit: bool,
    stop_events: Vec<DebugStopEvent>,
    next_seq: u64,
    current_stack: Vec<DebugStackFrame>,
    current_scopes: DebugScopes,
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

    pub fn continue_execution(&self) -> Result<(), String> {
        self.arm_run_mode(RunMode::Continue)
    }

    pub fn resume(&self) -> Result<(), String> {
        self.continue_execution()
    }

    pub fn step_in(&self) -> Result<(), String> {
        self.arm_run_mode_from_stop(|stop| RunMode::StepIn {
            origin_seq: stop.seq,
        })
    }

    pub fn step_over(&self) -> Result<(), String> {
        self.arm_run_mode_from_stop(|stop| RunMode::StepOver {
            origin_seq: stop.seq,
            origin_depth: stop.depth,
        })
    }

    pub fn step_out(&self) -> Result<(), String> {
        self.arm_run_mode_from_stop(|stop| RunMode::StepOut {
            origin_seq: stop.seq,
            target_depth: stop.depth.saturating_sub(1),
        })
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

    pub fn current_stack(&self) -> Result<Vec<DebugStackFrame>, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(guard.current_stack.clone())
    }

    pub fn current_scopes(&self) -> Result<DebugScopes, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(guard.current_scopes.clone())
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
        depth: usize,
        eval_ctx: &EvalContext,
        scopes: DebugScopes,
    ) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        guard.current_scopes = scopes;
        upsert_stack_frame(&mut guard.current_stack, workflow_id, step_id, depth);

        let matched_breakpoint =
            first_matching_breakpoint(&guard.breakpoints, workflow_id, step_id, eval_ctx);
        let candidate_seq = guard.next_seq.saturating_add(1);
        let step_mode_stop = should_stop_for_step_mode(guard.run_mode, candidate_seq, depth);

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
        } else if guard.pause_requested {
            Some((DebugStopReason::Pause, None))
        } else if step_mode_stop {
            Some((DebugStopReason::Step, None))
        } else {
            None
        };

        let Some((reason, breakpoint_condition)) = stop_info else {
            return Ok(());
        };

        guard.next_seq = candidate_seq;
        guard.stop_events.push(DebugStopEvent {
            seq: candidate_seq,
            workflow_id: workflow_id.to_string(),
            step_id: step_id.to_string(),
            depth,
            reason,
            breakpoint_condition,
        });
        guard.pause_requested = false;
        guard.run_mode = RunMode::Continue;
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

    fn arm_run_mode_from_stop<F>(&self, f: F) -> Result<(), String>
    where
        F: FnOnce(&DebugStopEvent) -> RunMode,
    {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        if !guard.waiting {
            return Err("debug controller is not paused".to_string());
        }
        let Some(stop) = guard.stop_events.last() else {
            return Err("debug controller has no stop event".to_string());
        };
        guard.run_mode = f(stop);
        guard.pause_requested = false;
        guard.continue_permit = true;
        self.condvar.notify_all();
        Ok(())
    }

    fn arm_run_mode(&self, mode: RunMode) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        if !guard.waiting {
            return Err("debug controller is not paused".to_string());
        }
        guard.run_mode = mode;
        guard.pause_requested = false;
        guard.continue_permit = true;
        self.condvar.notify_all();
        Ok(())
    }
}

fn should_stop_for_step_mode(run_mode: RunMode, candidate_seq: u64, depth: usize) -> bool {
    match run_mode {
        RunMode::Continue => false,
        RunMode::StepIn { origin_seq } => candidate_seq > origin_seq,
        RunMode::StepOver {
            origin_seq,
            origin_depth,
        } => candidate_seq > origin_seq && depth <= origin_depth,
        RunMode::StepOut {
            origin_seq,
            target_depth,
        } => candidate_seq > origin_seq && depth <= target_depth,
    }
}

fn upsert_stack_frame(
    stack: &mut Vec<DebugStackFrame>,
    workflow_id: &str,
    step_id: &str,
    depth: usize,
) {
    if stack.len() > depth {
        stack.truncate(depth + 1);
    } else {
        while stack.len() <= depth {
            let fill_depth = stack.len();
            stack.push(DebugStackFrame {
                depth: fill_depth,
                workflow_id: String::new(),
                step_id: String::new(),
            });
        }
    }

    if let Some(frame) = stack.get_mut(depth) {
        frame.workflow_id = workflow_id.to_string();
        frame.step_id = step_id.to_string();
    }
}
