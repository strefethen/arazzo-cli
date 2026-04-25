use super::*;

// ── Per-execution context ───────────────────────────────────────────

/// Per-execution mutable state, shared across tasks via `Arc`.
pub(super) struct ExecutionContext {
    pub event_tx: mpsc::Sender<EngineEvent>,
    pub trace_seq: AtomicU64,
    pub execution_event_seq: AtomicU64,
    pub step_attempts: Mutex<BTreeMap<(String, String), u32>>,
    pub cancel: CancellationToken,
    pub is_timeout: Arc<AtomicBool>,
}

impl ExecutionContext {
    pub(super) fn check_cancelled(&self) -> Result<(), RuntimeError> {
        if self.cancel.is_cancelled() {
            Err(self.cancelled_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn cancelled_error(&self) -> RuntimeError {
        if self.is_timeout.load(Ordering::Acquire) {
            RuntimeError::new(
                RuntimeErrorKind::ExecutionTimeout,
                "execution timeout exceeded",
            )
        } else {
            RuntimeError::new(RuntimeErrorKind::ExecutionCancelled, "execution cancelled")
        }
    }
}

pub(crate) struct OperationEntry {
    pub(super) method: String,
    pub(super) path: String,
}

#[derive(Debug, Clone)]
pub(super) struct StepResult {
    pub(super) success: bool,
    pub(super) response: Option<Arc<Response>>,
    pub(super) err: Option<String>,
    /// Original error kind from a runtime error (e.g. HttpRequest, ExecutionTimeout).
    /// Preserved so that onFailure `end` actions can report the true cause
    /// instead of a generic `SuccessCriteriaFailed`.
    pub(super) err_kind: Option<RuntimeErrorKind>,
}

#[derive(Debug, Clone)]
pub(super) struct StepExecution {
    pub(super) result: StepResult,
    pub(super) outputs: BTreeMap<String, Value>,
    pub(super) dry_run_request: Option<DryRunRequest>,
    pub(super) trace: StepTraceData,
}

#[derive(Debug, Clone, Default)]
pub(super) struct StepTraceData {
    pub(super) request: Option<TraceRequest>,
    pub(super) response: Option<TraceResponse>,
    pub(super) criteria: Vec<TraceCriterionResult>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VarStore {
    pub(super) inputs: BTreeMap<String, Value>,
    steps: Arc<BTreeMap<String, BTreeMap<String, Value>>>,
    workflow_states: BTreeMap<String, arazzo_expr::WorkflowEvalState>,
}

impl VarStore {
    pub(crate) fn set_input(&mut self, name: &str, value: Value) {
        self.inputs.insert(name.to_string(), value);
    }

    pub(crate) fn set_step_output(&mut self, step_id: &str, name: &str, value: Value) {
        Arc::make_mut(&mut self.steps)
            .entry(step_id.to_string())
            .or_default()
            .insert(name.to_string(), value);
    }

    pub(crate) fn step_outputs(&self, step_id: &str) -> BTreeMap<String, Value> {
        self.steps.get(step_id).cloned().unwrap_or_default()
    }

    pub(crate) fn debug_scopes(&self) -> DebugScopes {
        DebugScopes {
            locals: BTreeMap::new(),
            inputs: self.inputs.clone(),
            steps: (*self.steps).clone(),
        }
    }

    pub(crate) fn register_workflow_state(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        outputs: BTreeMap<String, Value>,
    ) {
        self.workflow_states.insert(
            workflow_id.to_string(),
            arazzo_expr::WorkflowEvalState { inputs, outputs },
        );
    }

    pub(crate) fn eval_context(&self, response: Option<&Response>) -> EvalContext {
        let mut ctx = EvalContext {
            inputs: self.inputs.clone(),
            steps: Arc::clone(&self.steps),
            workflows: self.workflow_states.clone(),
            ..EvalContext::default()
        };
        if let Some(resp) = response {
            ctx.status_code = Some(resp.status_code);
            ctx.response_headers = resp.headers.clone();
            // Prefer parsed JSON; fall back to the raw body as a string so that
            // $response.body returns the text content for XML / plain-text
            // responses instead of silently resolving to Null.
            ctx.response_body = resp
                .body_json
                .clone()
                .or_else(|| String::from_utf8(resp.body.clone()).ok().map(Value::String));
        }
        ctx
    }
}

/// Immutable index built once from the parsed spec.
pub(crate) struct WorkflowIndex {
    pub spec: ArazzoSpec,
    pub base_url: String,
    pub source_descriptions_map: BTreeMap<String, String>,
    pub workflow_index: BTreeMap<String, usize>,
    pub step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    pub(super) openapi_specs_raw: Vec<Vec<u8>>,
    pub(super) op_index: OnceLock<BTreeMap<String, OperationEntry>>,
}

/// Shared immutable core of the engine, wrapped in `Arc`.
pub(super) struct EngineInner {
    pub(super) index: WorkflowIndex,
    pub(super) client: HttpClient,
    pub(super) parallel_mode: bool,
    pub(super) dry_run_mode: bool,
    pub(super) trace_enabled: bool,
    pub(super) strict_inputs: bool,
    pub(super) channel_capacity: usize,
    pub(super) trace_hook: Option<Arc<dyn TraceHook>>,
    pub(super) observer: Option<Arc<dyn ExecutionObserver>>,
    pub(super) debug_controller: Option<Arc<DebugController>>,
    pub(super) regex_cache: RegexCache,
}

/// Runtime engine for executing Arazzo workflows.
///
/// `Engine` is cheaply cloneable (wraps `Arc<EngineInner>`) and can be
/// shared across tasks for concurrent workflow execution.
#[derive(Clone)]
pub struct Engine {
    pub(super) inner: Arc<EngineInner>,
}
