use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use arazzo_expr::{
    parse_simple_condition, ConditionError, ConditionErrorKind, EvalContext, ExpressionEvaluator,
};
use arazzo_spec::OutputValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime_core::{evaluate_output_value_detailed, select_xpath};

use super::breakpoints::{first_matching_breakpoint, StepBreakpoint, StepCheckpoint};
use super::{DebugScopes, DebugStackFrame, WatchEvaluation};

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
#[non_exhaustive]
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
    #[serde(default)]
    pub checkpoint: StepCheckpoint,
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
    current_eval_ctx: EvalContext,
    output_diagnostics: Vec<String>,
    frames: Vec<(DebugScopes, EvalContext)>,
}

/// Thread-safe runtime debug gate controller for pause/resume and breakpoints.
#[derive(Debug, Default)]
pub struct DebugController {
    state: Mutex<ControllerState>,
    condvar: Condvar,
}

impl DebugController {
    /// Drain unresolved-output diagnostics, including outputs evaluated without a pause.
    pub fn take_output_diagnostics(&self) -> Result<Vec<String>, String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(std::mem::take(&mut guard.output_diagnostics))
    }

    /// Distinguish an unresolved declared output from a resolved JSON null.
    pub fn output_is_unresolved(&self, output: &OutputValue) -> Result<bool, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(output_is_unresolved(
            output,
            &ExpressionEvaluator::new(guard.current_eval_ctx.clone()),
        ))
    }

    /// Snapshot inspection state for a selected paused stack frame.
    pub fn inspect_frame(&self, depth: usize) -> Result<Self, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        if !guard.waiting || guard.continue_permit {
            return Err(
                "cannot inspect values while the workflow is running; pause it first".into(),
            );
        }
        let (scopes, context) = guard
            .frames
            .get(depth)
            .ok_or_else(|| format!("stack frame at depth {depth} is no longer available"))?;
        Ok(Self {
            state: Mutex::new(ControllerState {
                current_scopes: scopes.clone(),
                current_eval_ctx: context.clone(),
                current_stack: guard.current_stack[..=depth].to_vec(),
                ..ControllerState::default()
            }),
            condvar: Condvar::new(),
        })
    }

    /// Report a workflow output without changing runtime output values.
    pub(crate) fn report_workflow_output(
        &self,
        workflow: &str,
        name: &str,
        output: &OutputValue,
        context: &EvalContext,
    ) -> Result<(), String> {
        let evaluator = ExpressionEvaluator::new(context.clone());
        let unresolved = match output {
            OutputValue::RuntimeExpression(expression) => {
                let (_, warnings) = evaluator.resolve_value_with_diagnostics(expression);
                !warnings.is_empty()
                    || (expression.starts_with('$')
                        && evaluator.evaluate_resolved(expression).is_none())
            }
            _ => output_is_unresolved(output, &evaluator),
        };
        if unresolved {
            let expression = serde_json::to_string(output)
                .map_err(|err| format!("formatting workflow output {name:?}: {err}"))?;
            let mut guard = self
                .state
                .lock()
                .map_err(|_| "debug controller lock poisoned".to_string())?;
            guard.output_diagnostics.push(format!("unresolved output value: {name:?} expression: {expression} in workflow: {workflow:?} (workflow output)."));
            self.condvar.notify_all();
        }
        Ok(())
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Validate breakpoint condition syntax before installing it.
    pub fn validate_breakpoint_condition(condition: &str) -> Result<(), String> {
        parse_simple_condition(condition)
            .map(|_| ())
            .map_err(|error| format_condition_error(&error))
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

    /// Drain and return all accumulated stop events, preventing unbounded growth.
    pub fn take_stop_events(&self) -> Result<Vec<DebugStopEvent>, String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(std::mem::take(&mut guard.stop_events))
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

    pub fn evaluate_expression(&self, expression: &str) -> Result<Value, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        Ok(ExpressionEvaluator::new(guard.current_eval_ctx.clone()).evaluate(expression))
    }

    pub fn evaluate_watch_expression(&self, expression: &str) -> Result<Value, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        evaluate_watch_expression_from_state(&guard, expression)
    }

    /// Preview a declared output using the current paused evaluation context.
    pub fn evaluate_output_value(&self, output: &OutputValue) -> Result<Value, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        let evaluator = ExpressionEvaluator::new(guard.current_eval_ctx.clone());
        if let OutputValue::RuntimeExpression(expression) = output {
            return evaluate_watch_expression_from_state(&guard, expression);
        }
        let (value, warnings) = evaluate_output_value_detailed(output, &evaluator, None);
        if !warnings.is_empty() {
            return Err(warnings
                .into_iter()
                .map(|warning| warning.message)
                .collect::<Vec<_>>()
                .join("; "));
        }
        Ok(value)
    }

    pub fn evaluate_condition(&self, condition: &str) -> Result<bool, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        let evaluation = ExpressionEvaluator::new(guard.current_eval_ctx.clone())
            .evaluate_condition_detailed(condition);
        match evaluation.error {
            Some(error) => Err(format_condition_error(&error)),
            None => Ok(evaluation.result),
        }
    }

    /// Evaluate only complete boolean condition expressions.
    ///
    /// Debug frontends also send standalone watch expressions through DAP
    /// `evaluate`; those should stay on the watch path even though they may be
    /// valid condition operands.
    pub fn try_evaluate_condition_expression(
        &self,
        expression: &str,
    ) -> Result<Option<bool>, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        let trimmed = expression.trim();
        let parsed = match parse_simple_condition(trimmed) {
            Ok(parsed) => parsed,
            Err(error) if error.kind == ConditionErrorKind::Syntax => return Ok(None),
            Err(error) => return Err(format_condition_error(&error)),
        };
        if !parsed.is_condition_decision() {
            return Ok(None);
        }

        let evaluation = ExpressionEvaluator::new(guard.current_eval_ctx.clone())
            .evaluate_condition_detailed(trimmed);
        match evaluation.error {
            Some(error) => Err(format_condition_error(&error)),
            None => Ok(Some(evaluation.result)),
        }
    }

    pub fn evaluate_watches(&self, expressions: &[String]) -> Result<Vec<WatchEvaluation>, String> {
        let guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        expressions
            .iter()
            .map(|expression| {
                Ok(WatchEvaluation {
                    expression: expression.clone(),
                    value: evaluate_watch_expression_from_state(&guard, expression)?,
                })
            })
            .collect()
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
        checkpoint: StepCheckpoint,
        depth: usize,
        eval_ctx: &EvalContext,
        scopes: DebugScopes,
    ) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        if let StepCheckpoint::Output { name } = &checkpoint {
            if let Some(expression) = scopes.locals.get("outputExpression") {
                let output: OutputValue = serde_json::from_value(expression.clone())
                    .map_err(|err| format!("reading debugger output expression: {err}"))?;
                if output_is_unresolved(&output, &ExpressionEvaluator::new(eval_ctx.clone())) {
                    guard.output_diagnostics.push(format!(
                        "unresolved output value: {name:?} expression: {expression} in workflow: {workflow_id:?}, step: {step_id:?}."
                    ));
                    self.condvar.notify_all();
                }
            }
        }
        guard.frames.truncate(depth);
        guard.frames.push((scopes.clone(), eval_ctx.clone()));
        guard.current_scopes = scopes;
        guard.current_eval_ctx = eval_ctx.clone();
        upsert_stack_frame(&mut guard.current_stack, workflow_id, step_id, depth);

        let matched_breakpoint = first_matching_breakpoint(
            &guard.breakpoints,
            workflow_id,
            step_id,
            &checkpoint,
            eval_ctx,
        )?;
        let candidate_seq = guard.next_seq.saturating_add(1);
        let step_mode_stop = should_stop_for_step_mode(guard.run_mode, candidate_seq, depth);

        let stop_info = if let Some(breakpoint) = matched_breakpoint {
            Some((
                DebugStopReason::Breakpoint,
                breakpoint
                    .condition
                    .filter(|value| !value.trim().is_empty()),
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
            checkpoint,
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

    pub fn force_resume(&self) -> Result<(), String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "debug controller lock poisoned".to_string())?;
        if !guard.waiting {
            return Ok(());
        }
        guard.run_mode = RunMode::Continue;
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

fn output_is_unresolved(output: &OutputValue, evaluator: &ExpressionEvaluator) -> bool {
    match output {
        OutputValue::RuntimeExpression(expression) if expression.starts_with('$') => {
            evaluator.evaluate_resolved(expression).is_none()
        }
        OutputValue::Selector(_) => !evaluate_output_value_detailed(output, evaluator, None)
            .1
            .is_empty(),
        // Legacy non-runtime forms retain their existing evaluation behavior.
        _ => false,
    }
}

fn evaluate_watch_expression_from_state(
    guard: &ControllerState,
    expression: &str,
) -> Result<Value, String> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return Err("watch expression is empty".into());
    }
    if trimmed.starts_with('/') {
        let body = guard
            .current_scopes
            .locals
            .get("responseBodyRaw")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("cannot evaluate XPath {trimmed:?}: this frame has no response body")
            })?;
        let selection = select_xpath(body.as_bytes(), trimmed, Some("xpath-10"))
            .map_err(|err| format!("cannot evaluate XPath {trimmed:?}: {err}"))?;
        if selection.match_count == 0 {
            return Err(format!(
                "unresolved XPath expression: {trimmed:?}; no matching nodes"
            ));
        }
        return selection.value;
    }
    if !trimmed.starts_with('$') {
        if let Some(value) = guard.current_scopes.locals.get(trimmed) {
            return Ok(value.clone());
        }
        if let Some(value) = guard
            .current_stack
            .last()
            .and_then(|frame| guard.current_scopes.steps.get(&frame.step_id))
            .and_then(|outputs| outputs.get(trimmed))
        {
            return Ok(value.clone());
        }
        return Err(format!(
            "unknown watch name: {trimmed:?}; no local or output has that name in this frame"
        ));
    }
    let evaluator = ExpressionEvaluator::new(guard.current_eval_ctx.clone());
    if let Some(value) = evaluator.evaluate_resolved(trimmed) {
        return Ok(value);
    }
    let (_, warnings) = evaluator.evaluate_with_diagnostics(trimmed);
    let detail = warnings
        .into_iter()
        .map(|warning| warning.message)
        .collect::<Vec<_>>()
        .join("; ");
    if detail.is_empty() {
        Err(format!("unresolved expression: {trimmed:?}"))
    } else {
        Err(format!("cannot evaluate expression: {trimmed:?}: {detail}"))
    }
}

fn format_condition_error(error: &ConditionError) -> String {
    format!(
        "invalid simple condition at byte {}: {}",
        error.byte_offset, error.message
    )
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

#[cfg(test)]
mod output_diagnostic_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diagnostic_is_available_while_paused_at_output() {
        let controller = std::sync::Arc::new(DebugController::new());
        controller
            .request_pause()
            .unwrap_or_else(|err| panic!("pause failed: {err}"));
        let worker_controller = std::sync::Arc::clone(&controller);
        let worker = std::thread::spawn(move || {
            let expression = "$response.body#/features/0/properties/event";
            let mut scopes = DebugScopes::default();
            scopes
                .locals
                .insert("outputExpression".into(), json!(expression));
            let context = EvalContext {
                response_body: Some(json!({"features": []})),
                ..Default::default()
            };
            worker_controller.gate_step(
                "forecast",
                "alerts",
                StepCheckpoint::Output {
                    name: "event".into(),
                },
                0,
                &context,
                scopes,
            )
        });
        let stopped = controller
            .wait_for_stop_count(1, Duration::from_secs(2))
            .unwrap_or_else(|err| panic!("waiting failed: {err}"));
        let diagnostics = controller
            .take_output_diagnostics()
            .unwrap_or_else(|err| panic!("diagnostics failed: {err}"));
        controller
            .force_resume()
            .unwrap_or_else(|err| panic!("resume failed: {err}"));
        worker
            .join()
            .unwrap_or_else(|_| panic!("output thread panicked"))
            .unwrap_or_else(|err| panic!("output gate failed: {err}"));
        assert!(stopped);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn missing_output_is_reported_without_pausing_and_null_is_not_missing() {
        let controller = DebugController::new();
        let expression = "$response.body#/features/0/properties/event";
        let checkpoint = StepCheckpoint::Output {
            name: "first_alert_event".to_owned(),
        };
        let mut scopes = DebugScopes::default();
        scopes
            .locals
            .insert("outputExpression".to_owned(), json!(expression));
        let mut context = EvalContext {
            response_body: Some(json!({"features": []})),
            ..Default::default()
        };
        controller
            .gate_step(
                "forecast",
                "check-active-alerts",
                checkpoint.clone(),
                0,
                &context,
                scopes.clone(),
            )
            .unwrap_or_else(|err| panic!("output gate failed: {err}"));
        let diagnostics = controller
            .take_output_diagnostics()
            .unwrap_or_else(|err| panic!("reading diagnostics: {err}"));
        assert_eq!(diagnostics.len(), 1);
        for expected in [
            "forecast",
            "check-active-alerts",
            "first_alert_event",
            expression,
        ] {
            assert!(diagnostics[0].contains(expected), "{diagnostics:?}");
        }
        assert!(controller.stop_events().unwrap_or_default().is_empty());
        assert!(controller
            .take_output_diagnostics()
            .unwrap_or_default()
            .is_empty());
        assert!(controller
            .output_is_unresolved(&OutputValue::RuntimeExpression(expression.into()))
            .unwrap_or_else(|err| panic!("preview failed: {err}")));

        for value in [Value::Null, json!("Flood Warning")] {
            context.response_body = Some(json!({"features": [{"properties": {"event": value}}]}));
            controller
                .gate_step(
                    "forecast",
                    "check-active-alerts",
                    checkpoint.clone(),
                    0,
                    &context,
                    scopes.clone(),
                )
                .unwrap_or_else(|err| panic!("output gate failed: {err}"));
            assert!(controller
                .take_output_diagnostics()
                .unwrap_or_default()
                .is_empty());
            assert!(!controller
                .output_is_unresolved(&OutputValue::RuntimeExpression(expression.into()))
                .unwrap_or_else(|err| panic!("preview failed: {err}")));
        }
    }
}
