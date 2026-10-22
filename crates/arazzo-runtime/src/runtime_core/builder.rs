use super::*;

use std::path::{Path, PathBuf};

use document_set::{DocumentSet, ProvidedDocument};
use state::{OperationIndex, OperationOrigin};

pub struct EngineBuilder {
    spec: ArazzoSpec,
    client_config: Option<ClientConfig>,
    parallel: bool,
    dry_run: bool,
    trace: bool,
    capture_run: bool,
    replay_trace_steps: Option<Vec<TraceStepRecord>>,
    strict_inputs: bool,
    channel_capacity: usize,
    max_response_bytes: usize,
    trace_hook: Option<Arc<dyn TraceHook>>,
    observer: Option<Arc<dyn ExecutionObserver>>,
    debug_controller: Option<Arc<DebugController>>,
    openapi_specs: Vec<ProvidedDocument>,
    source_base_dir: Option<PathBuf>,
}

impl EngineBuilder {
    /// Creates a new builder with the given Arazzo spec. All optional settings
    /// default to their inactive/off state.
    pub fn new(spec: ArazzoSpec) -> Self {
        Self {
            spec,
            client_config: None,
            parallel: false,
            dry_run: false,
            trace: false,
            capture_run: false,
            replay_trace_steps: None,
            strict_inputs: false,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            trace_hook: None,
            observer: None,
            debug_controller: None,
            openapi_specs: Vec::new(),
            source_base_dir: None,
        }
    }

    /// Sets the directory the Arazzo document was read from. It supplies the
    /// retrieval URI that relative `sourceDescriptions[].url` references
    /// resolve against when the document declares no absolute `$self`
    /// (Arazzo 1.1 §5.6.1). Required whenever a `type: openapi` source uses a
    /// relative url and no absolute `$self` establishes a base URI; building
    /// without either fails for such sources.
    pub fn source_base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.source_base_dir = Some(dir.into());
        self
    }

    /// Sets custom HTTP client configuration. When omitted, `ClientConfig::default()` is used.
    pub fn client_config(mut self, config: ClientConfig) -> Self {
        self.client_config = Some(config);
        self
    }

    /// Enables or disables parallel execution of independent steps within a workflow.
    pub fn parallel(mut self, enabled: bool) -> Self {
        self.parallel = enabled;
        self
    }

    /// Enables or disables dry-run mode, which resolves requests without sending them.
    pub fn dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// Enables or disables detailed per-step trace recording during execution.
    pub fn trace(mut self, enabled: bool) -> Self {
        self.trace = enabled;
        self
    }

    /// Enables body-free records for each settled step attempt, independently of tracing.
    pub fn capture_run(mut self, enabled: bool) -> Self {
        self.capture_run = enabled;
        self
    }

    /// Enables replay mode by serving recorded step responses from trace records
    /// instead of issuing live network requests.
    pub fn replay_trace_steps(mut self, steps: Vec<TraceStepRecord>) -> Self {
        self.replay_trace_steps = Some(steps);
        self
    }

    /// Enables or disables strict input validation. When enabled, missing required
    /// inputs, type mismatches, and property enum assertion failures cause a fatal
    /// `InputValidation` error. When disabled (default), validation issues are
    /// printed as warnings to stderr.
    pub fn strict_inputs(mut self, enabled: bool) -> Self {
        self.strict_inputs = enabled;
        self
    }

    /// Sets the bounded channel capacity for event streaming. Default: 1024.
    pub fn channel_capacity(mut self, cap: usize) -> Self {
        self.channel_capacity = cap;
        self
    }

    /// Registers a trace hook that receives step lifecycle events during execution.
    pub fn trace_hook(mut self, hook: Arc<dyn TraceHook>) -> Self {
        self.trace_hook = Some(hook);
        self
    }

    /// Registers an execution observer for rich event streaming during execution.
    pub fn observer(mut self, observer: Arc<dyn ExecutionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Attaches a debug controller for breakpoint-driven step-through execution.
    pub fn debug_controller(mut self, controller: Arc<DebugController>) -> Self {
        self.debug_controller = Some(controller);
        self
    }

    /// Sets the maximum allowed response body size in bytes. Responses exceeding
    /// this limit will produce a `ResponseTooLarge` error. Default: 10 MiB.
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }

    /// Adds an OpenAPI document to the set provided to this engine. Call
    /// multiple times for multiple documents. Replaces
    /// `Engine::load_openapi_spec`.
    ///
    /// `retrieval_path` is where the document was read from, when the caller
    /// knows. Together with an absolute `$self` inside the document it gives
    /// the document an identity, so a `sourceDescriptions[].url` resolving to
    /// that identity binds to these bytes instead of reading the filesystem
    /// (Arazzo 1.1 §5.5.2). A document with neither cannot bind to a source and
    /// its operations are unreachable. Provided documents are binding inputs,
    /// not another operation namespace.
    pub fn openapi_spec(mut self, data: Vec<u8>, retrieval_path: Option<PathBuf>) -> Self {
        self.openapi_specs
            .push(ProvidedDocument::new(data, retrieval_path));
        self
    }

    /// Consumes the builder and creates a fully configured [`Engine`].
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. invalid
    /// TLS settings), or if a `type: openapi` source description cannot bind
    /// to a provided/local document, be parsed, or yield a usable server base.
    pub fn build(self) -> Result<Engine, RuntimeError> {
        let config = self.client_config.unwrap_or_default();
        let client = HttpClient::new(&config, self.max_response_bytes, self.replay_trace_steps)?;

        // Resolution runs before any source is bound: every provided document
        // is parsed for its identity first, because Arazzo 1.1 §5.5 forbids
        // calling a reference unresolvable "before completely parsing all
        // documents provided to the implementation".
        let mut documents = DocumentSet::new(
            &self.spec,
            self.source_base_dir.as_deref(),
            self.openapi_specs,
        )?;

        let mut source_bases = BTreeMap::new();
        let mut source_ops = OperationIndex::default();
        let mut source_documents = BTreeMap::new();
        for sd in &self.spec.source_descriptions {
            if let Some(bound) = documents.bind(sd)? {
                let root: serde_yaml_ng::Value =
                    serde_yaml_ng::from_slice(&bound.data).map_err(|err| {
                        RuntimeError::new(
                            RuntimeErrorKind::SourceDescriptionParse,
                            format!(
                                "sourceDescription \"{}\": parsing OpenAPI document \"{}\": {err}",
                                sd.name, bound.label
                            ),
                        )
                    })?;
                let base = operation_target::derive_servers_base(&root, sd, &bound.label)?;
                let origin = OperationOrigin::Source {
                    name: sd.name.clone(),
                    path: bound.label.clone(),
                };
                operation_target::index_operations(&root, &origin, &mut source_ops);
                source_bases.insert(sd.name.clone(), base);
                source_documents.insert(
                    sd.name.clone(),
                    operation_target::BoundOpenApi {
                        root,
                        label: bound.label,
                    },
                );
            }
        }

        let mut source_descriptions_map = BTreeMap::new();
        for sd in &self.spec.source_descriptions {
            source_descriptions_map.insert(
                sd.name.clone(),
                arazzo_expr::SourceDescriptionContext {
                    url: sd.url.clone(),
                    type_: sd.type_.as_str().to_string(),
                },
            );
        }

        let mut workflow_index = BTreeMap::new();
        let mut step_indexes = BTreeMap::new();
        for (wf_idx, wf) in self.spec.workflows.iter().enumerate() {
            workflow_index.insert(wf.workflow_id.clone(), wf_idx);
            let mut step_idx_map = BTreeMap::new();
            for (step_idx, step) in wf.steps.iter().enumerate() {
                step_idx_map.insert(step.step_id.clone(), step_idx);
            }
            step_indexes.insert(wf.workflow_id.clone(), step_idx_map);
        }

        Ok(Engine {
            inner: Arc::new(EngineInner {
                #[cfg(test)]
                controlled_exchange: None,
                #[cfg(test)]
                nats_trial: None,
                index: WorkflowIndex {
                    spec: self.spec,
                    source_descriptions_map,
                    source_bases,
                    source_ops,
                    workflow_index,
                    step_indexes,
                    source_documents,
                },
                client,
                parallel_mode: self.parallel,
                dry_run_mode: self.dry_run,
                trace_enabled: self.trace,
                capture_run: self.capture_run,
                strict_inputs: self.strict_inputs,
                channel_capacity: self.channel_capacity,
                trace_hook: self.trace_hook.map(|h| h as Arc<dyn TraceHook>),
                observer: self.observer,
                regex_cache: helpers::RegexCache::new(),
                debug_controller: self.debug_controller,
            }),
        })
    }
}

/// Local files a build will read for `type: openapi` source descriptions,
/// paired with their source names. Callers that gate filesystem access (e.g.
/// the MCP server) can vet these paths before building an engine with
/// [`EngineBuilder::source_base_dir`].
///
/// Each url is resolved the way the build resolves it — against the Arazzo
/// document's `$self` when it declares an absolute one, otherwise against
/// `base_dir` — and only a reference that lands on a `file://` URI appears
/// here. A reference that lands anywhere else is refused at build rather than
/// read, so this list still covers every disk read the engine can perform.
pub fn relative_openapi_source_paths(spec: &ArazzoSpec, base_dir: &Path) -> Vec<(String, PathBuf)> {
    document_set::local_source_paths(spec, base_dir)
}
