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
    /// Workflow IDs completed before or during this invocation. The set is
    /// scoped to the execution context and is never stored on [`Engine`].
    pub completed_workflows: Mutex<BTreeSet<String>>,
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
        control::cancellation_error(&self.is_timeout)
    }

    pub(super) fn workflow_is_completed(&self, workflow_id: &str) -> bool {
        self.completed_workflows
            .lock()
            .map(|completed| completed.contains(workflow_id))
            .unwrap_or(false)
    }

    pub(super) fn mark_workflow_completed(&self, workflow_id: &str) {
        if let Ok(mut completed) = self.completed_workflows.lock() {
            completed.insert(workflow_id.to_string());
        }
    }
}

/// Where an indexed operation came from, used for duplicate-operationId
/// diagnostics and per-source attribution.
#[derive(Debug, Clone)]
pub(crate) enum OperationOrigin {
    /// Loaded from a `sourceDescriptions[]` OpenAPI document at engine build.
    Source { name: String, path: String },
    /// Explicitly provided via `EngineBuilder::openapi_spec` (1-based order).
    ExplicitSpec { ordinal: usize },
}

impl OperationOrigin {
    /// The Source Description that owns the operation. `None` for an
    /// explicitly provided spec, which belongs to no source description and
    /// therefore has no server base of its own.
    pub(super) fn source_name(&self) -> Option<&str> {
        match self {
            Self::Source { name, .. } => Some(name),
            Self::ExplicitSpec { .. } => None,
        }
    }

    /// Phrase naming where the operation came from, for ambiguity messages.
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Source { name, path } => format!("sourceDescription \"{name}\" ({path})"),
            Self::ExplicitSpec { ordinal } => {
                format!("explicitly provided OpenAPI spec #{ordinal}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OperationEntry {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) origin: OperationOrigin,
    /// The server the operation declares for itself, recorded when its
    /// document is indexed.
    pub(super) server: OperationServer,
}

/// Where an operation's request goes when its own OpenAPI objects say so.
///
/// OpenAPI 3.2 lets a Path Item Object (§4.9.1) and an Operation Object
/// (§4.10.1) each declare an alternative `servers` array, and the lowest
/// declaring level overrides every one above it.
#[derive(Debug, Clone)]
pub(crate) enum OperationServer {
    /// Neither level declares a server, so the document's request base
    /// applies. Every operation of an explicitly provided spec is recorded
    /// this way: such a spec belongs to no source description, and its
    /// `servers` are consulted at no level.
    Inherited,
    /// The request base the lowest declaring level yields.
    Declared(String),
    /// The lowest declaring level yields no usable server. Resolving the
    /// operation is refused; a declared level never falls back to one above
    /// it.
    Unusable(UnusableServer),
}

impl OperationServer {
    /// The request base this operation is sent to, given its document's.
    pub(super) fn base(&self, document_base: &str) -> Result<String, &UnusableServer> {
        match self {
            Self::Inherited => Ok(document_base.to_string()),
            Self::Declared(base) => Ok(base.clone()),
            Self::Unusable(unusable) => Err(unusable),
        }
    }
}

/// A declared `servers` field that yields no usable server, and the level
/// that declared it.
#[derive(Debug, Clone)]
pub(crate) struct UnusableServer {
    pub(super) level: ServerLevel,
    pub(super) issue: ServerIssue,
}

/// The OpenAPI object below the document root that declares `servers`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ServerLevel {
    PathItem,
    Operation,
}

impl ServerLevel {
    /// The specification's name for the object, as messages quote it.
    pub(super) fn object_name(self) -> &'static str {
        match self {
            Self::PathItem => "Path Item Object",
            Self::Operation => "Operation Object",
        }
    }
}

/// Why a present `servers` field yields no usable server. Absence and an
/// empty array are not issues: they declare nothing.
#[derive(Debug, Clone)]
pub(crate) enum ServerIssue {
    /// The field is not an array; `null` included.
    NotAnArray,
    /// The first Server Object has no `url`, or an empty one. The Server
    /// Object's `url` is REQUIRED (OpenAPI 3.2 §4.5.1).
    MissingUrl,
    /// The first Server Object's `url`, its variables substituted, is a
    /// relative reference. OpenAPI allows one — it resolves against the
    /// location the document is served from (§4.5.2.1) — but a document this
    /// runtime reads from disk has no HTTP location, so no request base
    /// follows from it.
    Relative(String),
}

/// Every indexed operation, keyed by `operationId`, retaining every definition
/// rather than the last one indexed.
///
/// The plain `BTreeMap<String, OperationEntry>` this replaces could not express
/// what the specification assumes: two Source Descriptions may each define
/// `getPet`, and under last-insert-wins a step naming it was routed to
/// whichever document happened to be indexed last — silently, and to the
/// engine-wide base rather than that document's server.
#[derive(Debug, Clone, Default)]
pub(crate) struct OperationIndex {
    by_id: BTreeMap<String, Vec<OperationEntry>>,
}

/// What an `operationId` lookup matched.
pub(crate) enum OperationMatch<'a> {
    /// Nothing defines it.
    Missing,
    One(&'a OperationEntry),
    /// Two or more definitions of equal precedence, in index order. This
    /// runtime refuses to pick between them instead of resolving by insertion
    /// order.
    Ambiguous(Vec<&'a OperationEntry>),
}

impl OperationIndex {
    pub(super) fn push(&mut self, operation_id: String, entry: OperationEntry) {
        self.by_id.entry(operation_id).or_default().push(entry);
    }

    /// The origin most recently indexed under `operation_id`, which is the
    /// entry a last-insert-wins map would have replaced. Used only to keep the
    /// build-time override warning firing exactly when it used to.
    pub(super) fn last_origin(&self, operation_id: &str) -> Option<&OperationOrigin> {
        self.by_id
            .get(operation_id)
            .and_then(|entries| entries.last())
            .map(|entry| &entry.origin)
    }

    /// The operation the named Source Description defines under this id.
    ///
    /// Explicit specs are never consulted: they belong to no source, so a
    /// source-qualified target neither resolves from one nor is overridden
    /// by one.
    pub(super) fn in_source(&self, source_name: &str, operation_id: &str) -> OperationMatch<'_> {
        self.select(operation_id, |entry| {
            entry.origin.source_name() == Some(source_name)
        })
    }

    /// The operation an unqualified `operation_id` names.
    ///
    /// An explicitly provided spec outranks a Source Description document,
    /// which is the direction the build-time override warning already
    /// announces; source documents are consulted only when no explicit spec
    /// defines the id.
    pub(super) fn bare(&self, operation_id: &str) -> OperationMatch<'_> {
        match self.select(operation_id, |entry| entry.origin.source_name().is_none()) {
            OperationMatch::Missing => self.select(operation_id, |_| true),
            matched => matched,
        }
    }

    fn select<'a>(
        &'a self,
        operation_id: &str,
        keep: impl Fn(&OperationEntry) -> bool,
    ) -> OperationMatch<'a> {
        let Some(entries) = self.by_id.get(operation_id) else {
            return OperationMatch::Missing;
        };
        let mut matched = entries.iter().filter(|entry| keep(entry));
        let Some(first) = matched.next() else {
            return OperationMatch::Missing;
        };
        let rest: Vec<&OperationEntry> = matched.collect();
        if rest.is_empty() {
            return OperationMatch::One(first);
        }
        let mut all = Vec::with_capacity(rest.len() + 1);
        all.push(first);
        all.extend(rest);
        OperationMatch::Ambiguous(all)
    }
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
    /// Effective request base of the first source description. For document
    /// sources this is derived from the loaded OpenAPI `servers`; for legacy
    /// sources it is the literal `url` value.
    pub base_url: String,
    pub source_descriptions_map: BTreeMap<String, arazzo_expr::SourceDescriptionContext>,
    /// Effective request base per source name (`{name}.` operationPath routing).
    pub(super) source_bases: BTreeMap<String, String>,
    /// Operations indexed from `sourceDescriptions[]` documents at build time.
    pub(super) source_ops: OperationIndex,
    pub workflow_index: BTreeMap<String, usize>,
    pub step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    /// Provided OpenAPI documents no Source Description claimed by identity,
    /// each with the 1-based position it was supplied in so diagnostics keep
    /// naming the same document however many earlier ones were claimed.
    pub(super) openapi_specs_raw: Vec<(usize, Vec<u8>)>,
    pub(super) op_index: OnceLock<OperationIndex>,
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
