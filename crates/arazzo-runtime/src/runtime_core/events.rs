use super::*;

// ── Streamed event types ────────────────────────────────────────────

/// Event streamed from the engine during execution via the mpsc channel.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum EngineEvent {
    TraceStep(TraceStepRecord),
    DryRunRequest(DryRunRequest),
    Execution(ExecutionEvent),
    Observer(ObserverEvent),
}

/// Handle returned by [`Engine::execute`] for streaming execution results.
///
/// The spawned task drops the event sender before sending the final result
/// via the oneshot channel, guaranteeing that `collect()` can drain all
/// events before awaiting the result.
///
/// Dropping the handle cancels the running task via the `CancellationToken`.
pub struct ExecutionHandle {
    events: Option<mpsc::Receiver<EngineEvent>>,
    result: Option<oneshot::Receiver<Result<BTreeMap<String, Value>, RuntimeError>>>,
    cancel: CancellationToken,
    is_timeout: Arc<AtomicBool>,
}

impl Drop for ExecutionHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl ExecutionHandle {
    pub(crate) fn new(
        events: mpsc::Receiver<EngineEvent>,
        result: oneshot::Receiver<Result<BTreeMap<String, Value>, RuntimeError>>,
        cancel: CancellationToken,
        is_timeout: Arc<AtomicBool>,
    ) -> Self {
        Self {
            events: Some(events),
            result: Some(result),
            cancel,
            is_timeout,
        }
    }

    /// Access the cancellation token to cancel the running task.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Access the timeout flag (set by `execute_with_timeout` watchdog).
    pub fn timeout_flag(&self) -> &Arc<AtomicBool> {
        &self.is_timeout
    }

    /// Drain all events and await the final result.
    #[allow(clippy::missing_panics_doc)]
    pub async fn collect(mut self) -> ExecutionResult {
        let mut events_rx = self
            .events
            .take()
            .unwrap_or_else(|| panic!("events already consumed"));
        let result_rx = self
            .result
            .take()
            .unwrap_or_else(|| panic!("result already consumed"));
        let mut events = Vec::new();
        while let Some(event) = events_rx.recv().await {
            events.push(event);
        }
        let outputs = result_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "execution task completed without sending result",
            ))
        });
        ExecutionResult { outputs, events }
        // Drop runs here, calling cancel() — task already finished, no-op
    }

    /// Discard events and return only the workflow result.
    #[allow(clippy::missing_panics_doc)]
    pub async fn result_only(mut self) -> Result<BTreeMap<String, Value>, RuntimeError> {
        drop(self.events.take()); // unblock event sends immediately
        let result_rx = self
            .result
            .take()
            .unwrap_or_else(|| panic!("result already consumed"));
        result_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "execution task completed without sending result",
            ))
        })
    }
}

/// Collected execution output from a completed workflow.
pub struct ExecutionResult {
    pub outputs: Result<BTreeMap<String, Value>, RuntimeError>,
    pub events: Vec<EngineEvent>,
}

impl ExecutionResult {
    /// Filter trace step records from the event stream.
    pub fn trace_steps(&self) -> Vec<&TraceStepRecord> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::TraceStep(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Filter dry-run requests from the event stream.
    pub fn dry_run_requests(&self) -> Vec<&DryRunRequest> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::DryRunRequest(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Filter execution lifecycle events from the event stream.
    pub fn execution_events(&self) -> Vec<&ExecutionEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Execution(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}

/// Captured request emitted during dry-run mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DryRunRequest {
    pub step_id: String,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Trace path chosen after a step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum TraceDecisionPath {
    #[default]
    Next,
    Done,
    GotoStep,
    GotoWorkflow,
    Retry,
    Error,
}

/// Trace decision metadata for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceDecision {
    pub path: TraceDecisionPath,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_limit: Option<u64>,
}

impl TraceDecision {
    /// Creates a `TraceDecision` with the given `path` and all other fields defaulted.
    pub fn with_path(path: TraceDecisionPath) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }
}

/// Trace request payload for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Trace response payload for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceResponse {
    pub status_code: i64,
    pub content_type: ContentType,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub body_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// True when the body was converted via lossy UTF-8 (non-UTF-8 bytes replaced
    /// with U+FFFD). Replay consumers should treat the body as approximate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_lossy: bool,
}

/// Trace result of evaluating one success criterion.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceCriterionResult {
    pub index: usize,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub type_: String,
    pub condition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    pub result: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Runtime trace record for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceStepRecord {
    pub seq: u64,
    pub workflow_id: String,
    pub step_id: String,
    pub attempt: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id_ref: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<TraceRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<TraceResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<TraceCriterionResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub decision: TraceDecision,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Trace payload for step lifecycle events.
#[derive(Debug, Clone, Default)]
pub struct StepEvent {
    pub workflow_id: String,
    pub step_id: String,
    pub operation_path: String,
    pub workflow_id_ref: String,
    pub status_code: i64,
    pub outputs: BTreeMap<String, Value>,
    pub err: Option<String>,
    pub duration: Duration,
}

/// Canonical runtime execution event kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ExecutionEventKind {
    BeforeStep,
    AfterStep,
}

/// Canonical runtime execution event emitted for every step lifecycle transition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvent {
    pub seq: u64,
    pub kind: ExecutionEventKind,
    pub workflow_id: String,
    pub step_id: String,
    pub operation_path: String,
    pub workflow_id_ref: String,
    pub status_code: i64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
    pub duration_ns: u64,
}

/// Hook for step-level tracing.
pub trait TraceHook: Send + Sync {
    fn before_step(&self, event: &StepEvent);
    fn after_step(&self, event: &StepEvent);
}

/// Rich execution event for TUI/observer integration.
///
/// Each variant captures a specific lifecycle moment during workflow execution,
/// carrying the relevant data for that moment. Observers receive these events
/// via [`ExecutionObserver::on_event`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ObserverEvent {
    /// Step is about to begin execution.
    StepStarted {
        workflow_id: String,
        step_id: String,
        operation_path: String,
        workflow_id_ref: String,
    },

    /// HTTP request has been resolved and is about to be sent.
    RequestPrepared {
        workflow_id: String,
        step_id: String,
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        has_body: bool,
    },

    /// HTTP request has been dispatched, awaiting response.
    RequestSent {
        workflow_id: String,
        step_id: String,
        method: String,
        url: String,
    },

    /// A single success criterion has been evaluated.
    CriterionEvaluated {
        workflow_id: String,
        step_id: String,
        index: usize,
        condition: String,
        passed: bool,
    },

    /// A retry action has been selected; about to wait.
    RetryScheduled {
        workflow_id: String,
        step_id: String,
        attempt: usize,
        max_attempts: usize,
        delay_seconds: u64,
    },

    /// Step completed (success or failure).
    /// Fires BEFORE the action handler decides retry/goto/end.
    StepCompleted {
        workflow_id: String,
        step_id: String,
        status_code: i64,
        duration: Duration,
        outputs: BTreeMap<String, Value>,
        error: Option<String>,
        criteria_passed: bool,
    },

    /// Sub-workflow invocation starting.
    SubWorkflowStarted {
        parent_workflow_id: String,
        parent_step_id: String,
        child_workflow_id: String,
        depth: usize,
    },

    /// Workflow execution finished.
    WorkflowCompleted {
        workflow_id: String,
        outputs: BTreeMap<String, Value>,
        duration: Duration,
        error: Option<String>,
    },
}

/// Observer trait for rich execution event streaming.
///
/// Unlike [`TraceHook`] (which provides only before/after step),
/// `ExecutionObserver` receives fine-grained events including
/// request preparation, HTTP dispatch, criterion evaluation,
/// retry scheduling, and sub-workflow invocation.
///
/// Implementations must be `Send + Sync` (called from async tasks).
/// Callbacks should be non-blocking — do not perform I/O or heavy
/// computation. Send events to a channel and process on another task.
pub trait ExecutionObserver: Send + Sync {
    fn on_event(&self, event: &ObserverEvent);
}
