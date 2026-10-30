use super::catalog::{CaseSpec, ControlKind, HarnessError, Stall};
use super::execution_oracle::{terminal, OracleReport};
use super::protocol::{ControlClient, Identity, ReadyReceipt, Snapshot};
use super::worker::{
    complete, CleanupBudget, ObservedExecution, TimedExecution, CLEANUP_BUDGET, SETUP_BUDGET,
};
use arazzo_runtime::{Engine, ExecutionHandle};
use serde_json::json;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(crate) const API_BUDGET: Duration = Duration::from_millis(900);

struct ReadinessState {
    started: Instant,
    stopped: Arc<AtomicBool>,
}

fn barrier_work(
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
    deadline: Instant,
    stopped: Arc<AtomicBool>,
) -> JoinHandle<Result<ReadyReceipt, HarnessError>> {
    let control = control.clone();
    let identity = identity.clone();
    let mut expected = case.expected.required_paths.clone();
    expected.sort();
    tokio::task::spawn_blocking(move || loop {
        if stopped.load(Ordering::Acquire) {
            return Err(HarnessError::new(
                "readiness",
                "barrier_stopped",
                "readiness phase ended",
            ));
        }
        if Instant::now() >= deadline {
            return Err(HarnessError::new(
                "readiness",
                "barrier_deadline",
                "stall barrier exceeded five seconds",
            ));
        }
        let receipt = control.readiness(&identity)?;
        if Instant::now() >= deadline {
            return Err(HarnessError::new(
                "readiness",
                "barrier_deadline",
                "late stall receipt",
            ));
        }
        if receipt.ready {
            if receipt.arrived_paths != expected || receipt.stalled_paths != expected {
                return Err(HarnessError::new(
                    "readiness",
                    "barrier_identity",
                    json!(receipt),
                ));
            }
            return Ok(receipt);
        }
        if receipt
            .arrived_paths
            .iter()
            .chain(&receipt.stalled_paths)
            .any(|p| !expected.contains(p))
        {
            return Err(HarnessError::new(
                "readiness",
                "unexpected_path",
                json!(receipt),
            ));
        }
        std::thread::park_timeout(Duration::from_millis(1));
    })
}

async fn failed_readiness(
    error: HarnessError,
    completion: &mut Pin<Box<impl Future<Output = ObservedExecution>>>,
    completed: Option<ObservedExecution>,
    work: Option<JoinHandle<Result<ReadyReceipt, HarnessError>>>,
    state: &ReadinessState,
    cancel: CancellationToken,
    cleanup_budget: &mut CleanupBudget,
) -> HarnessError {
    let cleanup_start = Instant::now();
    let cleanup_deadline = cleanup_budget.begin();
    state.stopped.store(true, Ordering::Release);
    cancel.cancel();
    let readiness_elapsed = state.started.elapsed();
    let mut work = work;
    let drain = async {
        match completed {
            Some(actual) => actual,
            None => completion.await,
        }
    };
    let join = async {
        match work.as_mut() {
            Some(work) => work
                .await
                .map_err(|e| HarnessError::new("cleanup", "readiness_join", e))
                .and_then(|result| result.map(Some)),
            None => Ok(None),
        }
    };
    let cleanup =
        tokio::time::timeout_at(cleanup_deadline.into(), async { tokio::join!(drain, join) }).await;
    let (actual, joined) = match cleanup {
        Ok(result) => result,
        Err(timeout) => {
            eprintln!(
                "{}",
                json!({"error":HarnessError::new("cleanup", "cleanup_deadline", json!({"cause":error,"timeout":timeout.to_string(),"readiness_elapsed_ns":readiness_elapsed.as_nanos(),"cleanup_budget_ms":CLEANUP_BUDGET.as_millis()}))})
            );
            std::process::exit(124);
        }
    };
    let stage = error.stage.clone();
    let code = error.code.clone();
    HarnessError::new(
        &stage,
        &code,
        json!({
            "cause":error,
            "readiness_elapsed_ns":readiness_elapsed.as_nanos(),
            "cleanup_elapsed_ns":cleanup_start.elapsed().as_nanos(),
            "barrier_cleanup":joined,
            "completion_terminal":terminal(&actual),
            "completion_error":actual.outputs.as_ref().err().map(|error|json!({"code":error.code(),"message":error.message})),
            "completion_events":actual.events.len()
        }),
    )
}

async fn execute(
    handle: ExecutionHandle,
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
    watchdog_start: Option<Instant>,
    cleanup: &mut CleanupBudget,
) -> Result<TimedExecution, HarnessError> {
    let cancel = handle.cancel_token().clone();
    let mut completion = Box::pin(complete(handle, case.settings.handle_mode));
    let started = Instant::now();
    let deadline = started + SETUP_BUDGET;
    let stopped = Arc::new(AtomicBool::new(false));
    let mut barrier = barrier_work(case, control, identity, deadline, Arc::clone(&stopped));
    let state = ReadinessState { started, stopped };
    let mut completed = None;
    let mut pending = true;
    let readiness = tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline.into()) => {
            Err(HarnessError::new("readiness", "barrier_deadline", "absolute five-second stall deadline expired"))
        }
        actual = &mut completion => {
            completed = Some(actual);
            Err(HarnessError::new("readiness", "early_completion", "selected completion finished before stall barrier"))
        }
        joined = &mut barrier => {
            pending = false;
            joined.map_err(|e| HarnessError::new("readiness", "join", e)).and_then(|result|result)
        }
    };
    if let Err(error) = readiness {
        return Err(failed_readiness(
            error,
            &mut completion,
            completed,
            if pending { Some(barrier) } else { None },
            &state,
            cancel,
            cleanup,
        )
        .await);
    }
    let start = watchdog_start.unwrap_or_else(Instant::now);
    if watchdog_start.is_none() {
        cancel.cancel();
    }
    let actual = tokio::time::timeout(API_BUDGET, &mut completion)
        .await
        .map_err(|e| HarnessError::new("execution", "api_deadline", e))?;
    let elapsed_ns = u64::try_from(start.elapsed().as_nanos())
        .map_err(|e| HarnessError::new("timing", "range", e))?;
    if elapsed_ns > API_BUDGET.as_nanos() as u64 {
        return Err(HarnessError::new("execution", "api_deadline", elapsed_ns));
    }
    Ok(TimedExecution { elapsed_ns, actual })
}

#[allow(dead_code)]
pub(crate) async fn execute_cancelled(
    engine: &Engine,
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
) -> Result<TimedExecution, HarnessError> {
    let mut cleanup = CleanupBudget::default();
    execute_cancelled_with_cleanup(engine, case, control, identity, &mut cleanup).await
}

pub(crate) async fn execute_cancelled_with_cleanup(
    engine: &Engine,
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
    cleanup: &mut CleanupBudget,
) -> Result<TimedExecution, HarnessError> {
    let handle = engine.execute(&case.workflow_id, BTreeMap::new());
    execute(handle, case, control, identity, None, cleanup).await
}

#[allow(dead_code)]
pub(crate) async fn execute_watchdog(
    engine: &Engine,
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
) -> Result<TimedExecution, HarnessError> {
    let mut cleanup = CleanupBudget::default();
    execute_watchdog_with_cleanup(engine, case, control, identity, &mut cleanup).await
}

pub(crate) async fn execute_watchdog_with_cleanup(
    engine: &Engine,
    case: &CaseSpec,
    control: &ControlClient,
    identity: &Identity,
    cleanup: &mut CleanupBudget,
) -> Result<TimedExecution, HarnessError> {
    let start = Instant::now();
    let handle = engine.execute_with_timeout(
        &case.workflow_id,
        BTreeMap::new(),
        Duration::from_millis(case.dimensions.watchdog_timeout_ms),
    );
    execute(handle, case, control, identity, Some(start), cleanup).await
}

pub(crate) fn check_cancelled(
    case: &CaseSpec,
    timed: &TimedExecution,
    snapshot: &Snapshot,
) -> OracleReport {
    let mut report = OracleReport::default();
    report.compare(
        "terminal",
        json!(case.expected.terminal_kind),
        json!(terminal(&timed.actual)),
    );
    report.compare(
        "api_bound_ns",
        json!(true),
        json!(timed.elapsed_ns <= API_BUDGET.as_nanos() as u64),
    );
    if case.control == Some(ControlKind::Watchdog) {
        report.compare(
            "watchdog_minimum_ns",
            json!(true),
            json!(timed.elapsed_ns >= case.dimensions.watchdog_timeout_ms * 1_000_000),
        );
    }
    report.compare(
        "service/rejected",
        json!(0),
        json!(snapshot.rejected_requests),
    );
    let mut actual: Vec<_> = snapshot
        .requests
        .iter()
        .map(|r| {
            json!([
                r.path,
                r.attempt,
                r.status,
                r.body_bytes_written,
                r.complete,
                r.final_response_release_ns
            ])
        })
        .collect();
    actual.sort_by_key(serde_json::Value::to_string);
    let written = if case.response_policy.stall == Stall::BetweenChunks {
        case.dimensions.serialized_body_bytes.div_ceil(2)
    } else {
        0
    };
    let mut expected: Vec<_> = case
        .expected
        .required_paths
        .iter()
        .map(|p| json!([p, 1, 200, written, false, null]))
        .collect();
    expected.sort_by_key(serde_json::Value::to_string);
    report.compare("service/stalled_requests", json!(expected), json!(actual));
    report.compare(
        "service/held_until_completion",
        json!(case.dimensions.width),
        json!(snapshot.active_connections),
    );
    report.compare(
        "capture_and_trace_absent",
        json!([]),
        json!(timed
            .actual
            .events
            .iter()
            .filter(|e| !matches!(
                e,
                arazzo_runtime::EngineEvent::Execution(_)
                    | arazzo_runtime::EngineEvent::Observer(_)
            ))
            .map(super::execution_oracle::event_projection)
            .collect::<Vec<_>>()),
    );
    report
}
