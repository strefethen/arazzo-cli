use super::*;

use std::path::{Path, PathBuf};

use arazzo_spec::SourceDescription;

use document_set::{BoundDocument, DocumentSet, ProvidedDocument};
use state::{OperationIndex, OperationOrigin};

pub struct EngineBuilder {
    spec: ArazzoSpec,
    client_config: Option<ClientConfig>,
    parallel: bool,
    dry_run: bool,
    trace: bool,
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
    /// (Arazzo 1.1 §5.5.2). A document with neither is still indexed; it is
    /// simply not reachable by reference.
    pub fn openapi_spec(mut self, data: Vec<u8>, retrieval_path: Option<PathBuf>) -> Self {
        self.openapi_specs
            .push(ProvidedDocument::new(data, retrieval_path));
        self
    }

    /// Consumes the builder and creates a fully configured [`Engine`].
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. invalid
    /// TLS settings), or if a `type: openapi` source description with a
    /// relative url cannot be loaded, parsed, or yields no usable server base.
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

        // Each source gets an effective request base: document sources (bound
        // to provided bytes or read from a resolved `file://` url, eagerly
        // here) derive it from the document's `servers`; legacy sources
        // (absolute non-`file` url no provided document answers to) keep the
        // literal url, exactly as before.
        let mut source_bases = BTreeMap::new();
        let mut source_ops = OperationIndex::default();
        let mut base_url = String::new();
        for (idx, sd) in self.spec.source_descriptions.iter().enumerate() {
            let effective_base = match documents.bind(sd)? {
                Some(bound) => index_bound_document(sd, &bound, &mut source_ops)?,
                None => sd.url.clone(),
            };
            if idx == 0 {
                base_url = effective_base.clone();
            }
            source_bases.insert(sd.name.clone(), effective_base);
        }
        let openapi_specs_raw = documents.unclaimed();

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
                index: WorkflowIndex {
                    spec: self.spec,
                    base_url,
                    source_descriptions_map,
                    source_bases,
                    source_ops,
                    workflow_index,
                    step_indexes,
                    openapi_specs_raw,
                    op_index: OnceLock::new(),
                },
                client,
                parallel_mode: self.parallel,
                dry_run_mode: self.dry_run,
                trace_enabled: self.trace,
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

/// Indexes a bound document's operations and derives its request base from
/// `servers[0].url`. Every failure is a build error naming the source and the
/// document it was bound to — never a silent fallback.
fn index_bound_document(
    sd: &SourceDescription,
    bound: &BoundDocument,
    source_ops: &mut OperationIndex,
) -> Result<String, RuntimeError> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bound.data).map_err(|err| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!(
                "sourceDescription \"{}\": parsing OpenAPI document \"{}\": {err}",
                sd.name, bound.label
            ),
        )
    })?;
    let origin = OperationOrigin::Source {
        name: sd.name.clone(),
        path: bound.label.clone(),
    };
    index_operations(&root, &origin, source_ops);
    derive_servers_base(&root, sd, &bound.label)
}

/// Derives the request base URL from `servers[0].url`, substituting server
/// variable defaults (mirrors the typed logic used by `generate`).
fn derive_servers_base(
    root: &serde_yaml_ng::Value,
    sd: &SourceDescription,
    document: &str,
) -> Result<String, RuntimeError> {
    let no_servers = || {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!(
                "sourceDescription \"{}\": OpenAPI document \"{document}\" declares no \
                 servers[0].url; an absolute server URL is required to derive the request base",
                sd.name,
            ),
        )
    };
    let first_server = root
        .get("servers")
        .and_then(serde_yaml_ng::Value::as_sequence)
        .and_then(|servers| servers.first())
        .ok_or_else(no_servers)?;
    let mut url = first_server
        .get("url")
        .and_then(serde_yaml_ng::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if url.is_empty() {
        return Err(no_servers());
    }
    if let Some(vars) = first_server
        .get("variables")
        .and_then(serde_yaml_ng::Value::as_mapping)
    {
        for (name, var) in vars {
            let (Some(name), Some(default)) = (
                name.as_str(),
                var.get("default").and_then(serde_yaml_ng::Value::as_str),
            ) else {
                continue;
            };
            url = url.replace(&format!("{{{name}}}"), default);
        }
    }
    if url.starts_with('/') {
        return Err(RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!(
                "sourceDescription \"{}\": server URL \"{url}\" in OpenAPI document \
                 \"{document}\" is relative; an absolute URL is required to derive the request \
                 base",
                sd.name,
            ),
        ));
    }
    Ok(url.trim_end_matches('/').to_string())
}

/// Parses an OpenAPI spec and populates the operation index.
pub(super) fn parse_openapi_into_index(
    data: &[u8],
    origin: &OperationOrigin,
    op_index: &mut OperationIndex,
) -> Result<(), RuntimeError> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_slice(data).map_err(|err| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!("parsing OpenAPI spec: {err}"),
        )
    })?;
    index_operations(&root, origin, op_index);
    Ok(())
}

/// Walks an already-parsed OpenAPI document and indexes its operations.
fn index_operations(
    root: &serde_yaml_ng::Value,
    origin: &OperationOrigin,
    op_index: &mut OperationIndex,
) {
    let Some(paths) = root.get("paths") else {
        return;
    };
    let Some(paths_map) = paths.as_mapping() else {
        return;
    };

    let http_methods: BTreeSet<&str> = BTreeSet::from([
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ]);

    for (path_key, methods_value) in paths_map {
        let Some(path) = path_key.as_str() else {
            continue;
        };
        let Some(methods_map) = methods_value.as_mapping() else {
            continue;
        };

        for (method_key, operation_value) in methods_map {
            let Some(method) = method_key.as_str() else {
                continue;
            };
            let method_l = method.to_lowercase();
            if !http_methods.contains(method_l.as_str()) {
                continue;
            }
            let Some(operation_map) = operation_value.as_mapping() else {
                continue;
            };
            let op_id = operation_map
                .get(serde_yaml_ng::Value::String("operationId".to_string()))
                .and_then(serde_yaml_ng::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if op_id.is_empty() {
                continue;
            }
            let entry = OperationEntry {
                method: method.to_uppercase(),
                path: path.to_string(),
                origin: origin.clone(),
            };
            // Every definition is retained, so an operationId defined by two
            // documents is a lookup-time ambiguity rather than whichever one
            // this loop reached last.
            if let Some(previous) = op_index.last_origin(&op_id) {
                warn_on_source_override(&op_id, previous, origin);
            }
            op_index.push(op_id, entry);
        }
    }
}

/// Emits a stderr warning when an explicitly provided OpenAPI spec overrides
/// an operationId that a loaded source description document already defined.
fn warn_on_source_override(op_id: &str, previous: &OperationOrigin, new: &OperationOrigin) {
    if let (OperationOrigin::Source { name, path }, OperationOrigin::ExplicitSpec { ordinal }) =
        (previous, new)
    {
        eprintln!(
            "warning: operationId \"{op_id}\" from sourceDescription \"{name}\" ({path}) is \
             overridden by explicitly provided OpenAPI spec #{ordinal}"
        );
    }
}
