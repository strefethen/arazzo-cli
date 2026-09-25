//! The step-attempt protocol, owned once for every scheduler.
//!
//! An attempt at a step is announced, numbered, settled, completed, routed,
//! and traced, and a retry it schedules is counted and reported. Sequential
//! execution, single-step execution (`arazzo run --step`), and parallel
//! levels all record their attempts here, so the execution events, observer
//! callbacks, trace-hook calls, and trace records of the three modes cannot
//! drift apart.
//!
//! The schedulers keep only scheduling: which step runs next and when its
//! attempt starts, event buffering and replay order, retry references, and
//! workflow-terminal events. This module calls the emission primitives in
//! `engine_trace`, routing in `engine_actions`, and the cancellation error in
//! `control`, and it never calls back into a scheduler. That is why the
//! sequential composition is a pair with the run between its two calls:
//!
//! - [`Engine::begin_attempt`] announces an attempt, numbers it, and starts
//!   its clock. The scheduler runs the step and passes the run's result to
//!   [`Engine::finish_attempt`], which settles, completes, routes, and traces
//!   the attempt, in that order.
//! - A parallel level runs each attempt in its step's task against a private
//!   event buffer, then settles and routes it there ([`settle`],
//!   [`Engine::route_attempt`], [`SettledAttempt::new`], [`count_retry`]).
//!   When the level replays the buffers in step order, the attempts are
//!   numbered, completed, and traced ([`Engine::record_settled`]), and their
//!   retries reported ([`Engine::emit_retry_scheduled`]).
//!
//! A sequential attempt is completed before it is routed because routing
//! waits out a retry's delay and runs the debugger's action checkpoints:
//! observers and trace hooks see a failed attempt before its backoff. Its
//! trace record follows routing because the record carries the decision.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use arazzo_spec::{Step, Workflow};
use tokio_util::sync::CancellationToken;

use super::{
    control::cancellation_error,
    engine_actions::{FlowDecision, RetrySite, RoutedDecision, StepDecisionContext},
    error::RuntimeError,
    events::{ObserverEvent, TraceDecision},
    state::{Engine, ExecutionContext, StepExecution, StepResult, StepTraceData, VarStore},
};

/// The routing inputs of one attempt: a [`StepDecisionContext`] without the
/// step's result, which routing takes from the settled execution.
#[derive(Debug)]
pub(super) struct RouteScope<'a> {
    pub(super) workflow_id: &'a str,
    pub(super) workflow: &'a Workflow,
    pub(super) step_idx: usize,
    /// Variables that routing criteria read.
    pub(super) vars: &'a VarStore,
    /// The invocation's call depth, which debug checkpoints report.
    pub(super) depth: usize,
    /// Retries already counted at each site, which bound the next one.
    pub(super) retry_count: &'a BTreeMap<RetrySite, u64>,
    pub(super) cancel: &'a CancellationToken,
    pub(super) is_timeout: &'a AtomicBool,
}

/// An announced attempt that has not been recorded yet.
///
/// [`Engine::begin_attempt`] returns one and only [`Engine::finish_attempt`]
/// consumes it, so a scheduler that announces an attempt and then drops it
/// unrecorded leaves an unused value the compiler reports.
#[must_use = "an announced attempt is recorded only by `Engine::finish_attempt`"]
#[derive(Debug)]
pub(super) struct BegunAttempt {
    /// Trace attempt number; 0 when tracing is off.
    attempt: u32,
    started: Instant,
}

/// One attempt's settled execution and the routing outcome its trace record
/// carries.
#[derive(Debug)]
pub(super) struct SettledAttempt {
    /// What the attempt produced. A runtime error is already settled into a
    /// failed result.
    pub(super) execution: StepExecution,
    duration: Duration,
    /// The routing decision, as traced.
    decision: TraceDecision,
    /// The error the trace record names: routing's own when routing ends the
    /// workflow, otherwise the attempt's.
    trace_error: Option<String>,
}

/// A retry counted against its site's budget, reported by
/// [`Engine::emit_retry_scheduled`] when it is about to run.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScheduledRetry {
    /// This retry's 1-based count at its site.
    attempt: u64,
    max_attempts: u64,
    delay_seconds: f64,
}

impl Engine {
    /// Announces an attempt at `step`, numbers it, and starts its clock.
    ///
    /// The attempt is numbered before it runs, so the attempts of a
    /// sub-workflow it calls are numbered after it.
    pub(super) async fn begin_attempt(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
    ) -> BegunAttempt {
        self.emit_before_step_event(ctx, workflow_id, step).await;
        let attempt = self.number_attempt(ctx, workflow_id, step);
        BegunAttempt {
            attempt,
            started: Instant::now(),
        }
    }

    /// Records the attempt `begun` announced, given the result of running it:
    /// settles, completes, routes, and traces it, in that order.
    ///
    /// Returns the attempt as recorded, and the decision the scheduler acts
    /// on.
    pub(super) async fn finish_attempt(
        &self,
        ctx: &ExecutionContext,
        step: &Step,
        begun: BegunAttempt,
        run: Result<StepExecution, RuntimeError>,
        scope: RouteScope<'_>,
    ) -> (SettledAttempt, FlowDecision) {
        let BegunAttempt { attempt, started } = begun;
        let execution = settle(run);
        let duration = started.elapsed();
        self.complete_attempt(ctx, scope.workflow_id, step, &execution, duration)
            .await;
        let routed = self.route_attempt(&scope, &execution).await;
        let (settled, flow) = SettledAttempt::new(execution, duration, routed);
        self.trace_attempt(ctx, scope.workflow_id, step, attempt, &settled)
            .await;
        (settled, flow)
    }

    /// Routes a settled attempt through the success or failure actions that
    /// apply to its step. A cancelled invocation's attempt ends in the
    /// cancellation, whatever routing chose before it noticed.
    pub(super) async fn route_attempt(
        &self,
        scope: &RouteScope<'_>,
        execution: &StepExecution,
    ) -> RoutedDecision {
        let routed = self
            .handle_step_result(StepDecisionContext {
                workflow_id: scope.workflow_id,
                workflow: scope.workflow,
                step_idx: scope.step_idx,
                result: &execution.result,
                vars: scope.vars,
                depth: scope.depth,
                retry_count: scope.retry_count,
                cancel: scope.cancel,
                is_timeout: scope.is_timeout,
            })
            .await;
        if scope.cancel.is_cancelled() {
            return RoutedDecision::error(cancellation_error(scope.is_timeout));
        }
        routed
    }

    /// Records a parallel attempt as its level replays it: numbers,
    /// completes, and traces it. Replay runs in step order, so a level
    /// numbers its attempts as sequential execution would.
    pub(super) async fn record_settled(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        attempt: &SettledAttempt,
    ) {
        let number = self.number_attempt(ctx, workflow_id, step);
        self.complete_attempt(ctx, workflow_id, step, &attempt.execution, attempt.duration)
            .await;
        self.trace_attempt(ctx, workflow_id, step, number, attempt)
            .await;
    }

    /// Reports a counted retry that will run: the scheduler calls this after
    /// the retry's delay and any recovery reference, immediately before the
    /// retried attempt.
    pub(super) async fn emit_retry_scheduled(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step_id: &str,
        retry: ScheduledRetry,
    ) {
        self.emit_observer_event(
            ctx,
            ObserverEvent::RetryScheduled {
                workflow_id: workflow_id.to_string(),
                step_id: step_id.to_string(),
                attempt: retry.attempt,
                max_attempts: retry.max_attempts,
                delay_seconds: retry.delay_seconds,
            },
        )
        .await;
    }

    /// The next attempt number of `step` in this invocation, or 0 when
    /// tracing is off.
    fn number_attempt(&self, ctx: &ExecutionContext, workflow_id: &str, step: &Step) -> u32 {
        if self.inner.trace_enabled {
            Engine::next_attempt(ctx, workflow_id, &step.step_id)
        } else {
            0
        }
    }

    /// Emits the attempt's AfterStep execution event and `StepCompleted`
    /// observer event.
    ///
    /// Both carry the attempt's own outputs. The var store keeps the last
    /// successful run's outputs for `$steps` expressions, and a failed re-run
    /// must not be reported with them.
    async fn complete_attempt(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        execution: &StepExecution,
        duration: Duration,
    ) {
        let status_code = execution
            .result
            .response
            .as_ref()
            .map(|r| r.status_code)
            .unwrap_or(0);
        self.emit_after_step_event(
            ctx,
            workflow_id,
            step,
            status_code,
            execution.outputs.clone(),
            execution.result.err.clone(),
            duration,
        )
        .await;
        self.emit_step_completed_event(
            ctx,
            workflow_id,
            step,
            status_code,
            duration,
            execution.outputs.clone(),
            execution.result.err.clone(),
            execution.result.success,
        )
        .await;
    }

    /// Streams the attempt's trace record, which carries the attempt's own
    /// outputs, when tracing is on.
    async fn trace_attempt(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        number: u32,
        attempt: &SettledAttempt,
    ) {
        if !self.inner.trace_enabled {
            return;
        }
        let record = Engine::build_step_trace_record(
            ctx,
            workflow_id,
            step,
            number,
            attempt.duration,
            &attempt.execution.trace,
            attempt.decision.clone(),
            attempt.execution.outputs.clone(),
            attempt.trace_error.clone(),
        );
        Engine::push_trace_record(ctx, record).await;
    }
}

/// Settles a run into the attempt it records.
///
/// A runtime error, such as an HTTP timeout or a refused connection, becomes
/// a failed result that onFailure routes like any other step failure: the
/// specification's onFailure handles every form of step failure.
pub(super) fn settle(run: Result<StepExecution, RuntimeError>) -> StepExecution {
    run.unwrap_or_else(|err| StepExecution {
        result: StepResult {
            success: false,
            response: None,
            err: Some(err.message),
            err_kind: Some(err.kind),
        },
        outputs: BTreeMap::new(),
        dry_run_request: None,
        trace: StepTraceData::default(),
    })
}

impl SettledAttempt {
    /// Pairs a settled execution with how it was routed, and returns the
    /// flow decision separately for the scheduler.
    ///
    /// The trace names routing's own error when routing ends the workflow,
    /// and the attempt's error otherwise.
    pub(super) fn new(
        execution: StepExecution,
        duration: Duration,
        routed: RoutedDecision,
    ) -> (Self, FlowDecision) {
        let RoutedDecision { flow, trace } = routed;
        let trace_error = match &flow {
            FlowDecision::Error(err) => Some(err.message.clone()),
            _ => execution.result.err.clone(),
        };
        let attempt = Self {
            execution,
            duration,
            decision: trace,
            trace_error,
        };
        (attempt, flow)
    }
}

/// Counts one more retry at `site` and returns it for reporting.
///
/// Each retry site (a step and the index of its failure action) keeps its own
/// count, so a later failure action's retries count from one.
pub(super) fn count_retry(
    counts: &mut BTreeMap<RetrySite, u64>,
    site: RetrySite,
    max_attempts: u64,
    delay_seconds: f64,
) -> ScheduledRetry {
    let count = counts.entry(site).or_insert(0);
    *count += 1;
    ScheduledRetry {
        attempt: *count,
        max_attempts,
        delay_seconds,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::{RuntimeErrorKind, TraceDecisionPath};

    fn execution(
        success: bool,
        err: Option<&str>,
        outputs: BTreeMap<String, Value>,
    ) -> StepExecution {
        StepExecution {
            result: StepResult {
                success,
                response: None,
                err: err.map(str::to_string),
                err_kind: None,
            },
            outputs,
            dry_run_request: None,
            trace: StepTraceData::default(),
        }
    }

    #[test]
    fn settle_keeps_a_run_that_returned_an_execution() {
        let outputs = BTreeMap::from([("ok".to_string(), json!(true))]);
        let settled = settle(Ok(execution(true, None, outputs.clone())));

        assert!(settled.result.success);
        assert_eq!(settled.result.err, None);
        assert_eq!(settled.outputs, outputs);
    }

    #[test]
    fn settle_turns_a_runtime_error_into_a_failed_attempt() {
        let settled = settle(Err(RuntimeError::new(
            RuntimeErrorKind::HttpRequest,
            "connection refused",
        )));

        assert!(!settled.result.success);
        assert!(settled.result.response.is_none());
        assert_eq!(settled.result.err.as_deref(), Some("connection refused"));
        assert_eq!(settled.result.err_kind, Some(RuntimeErrorKind::HttpRequest));
        assert!(settled.outputs.is_empty());
        assert!(settled.dry_run_request.is_none());
        assert!(settled.trace.request.is_none());
        assert!(settled.trace.response.is_none());
        assert!(settled.trace.criteria.is_empty());
        assert!(settled.trace.warnings.is_empty());
    }

    #[test]
    fn a_settled_attempt_traces_the_error_that_ends_the_workflow() {
        let routed = RoutedDecision::error(RuntimeError::new(
            RuntimeErrorKind::RetryLimitExceeded,
            "step s: max retries (2) exceeded",
        ));
        let duration = Duration::from_millis(5);

        let (attempt, flow) = SettledAttempt::new(
            execution(false, Some("connection refused"), BTreeMap::new()),
            duration,
            routed,
        );

        assert!(matches!(
            flow,
            FlowDecision::Error(ref err) if err.kind == RuntimeErrorKind::RetryLimitExceeded
        ));
        assert_eq!(
            attempt.trace_error.as_deref(),
            Some("step s: max retries (2) exceeded")
        );
        assert_eq!(attempt.decision.path, TraceDecisionPath::Error);
        assert_eq!(attempt.duration, duration);
    }

    #[test]
    fn a_settled_attempt_traces_its_own_error_when_routing_continues() {
        for err in [None, Some("connection refused")] {
            let decision = TraceDecision::with_path(TraceDecisionPath::Next);
            let routed = RoutedDecision {
                flow: FlowDecision::Next(1),
                trace: decision.clone(),
            };

            let (attempt, flow) = SettledAttempt::new(
                execution(err.is_none(), err, BTreeMap::new()),
                Duration::ZERO,
                routed,
            );

            assert!(matches!(flow, FlowDecision::Next(1)));
            assert_eq!(attempt.trace_error.as_deref(), err);
            assert_eq!(attempt.decision, decision);
        }
    }

    #[test]
    fn count_retry_counts_each_site_from_one() {
        let first_action = RetrySite::new(0, 0);
        let later_action = RetrySite::new(0, 1);
        let mut counts = BTreeMap::new();

        let reported: Vec<(u64, u64, f64)> = [
            count_retry(&mut counts, first_action, 3, 0.5),
            count_retry(&mut counts, first_action, 3, 0.5),
            count_retry(&mut counts, later_action, 1, 2.0),
        ]
        .iter()
        .map(|retry| (retry.attempt, retry.max_attempts, retry.delay_seconds))
        .collect();

        assert_eq!(reported, [(1, 3, 0.5), (2, 3, 0.5), (1, 1, 2.0)]);
        assert_eq!(
            counts,
            BTreeMap::from([(first_action, 2), (later_action, 1)])
        );
    }
}
