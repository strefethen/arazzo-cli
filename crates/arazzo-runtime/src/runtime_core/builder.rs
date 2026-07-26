use super::*;

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
    openapi_specs: Vec<Vec<u8>>,
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
        }
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
    /// inputs and type mismatches cause a fatal `InputValidation` error. When
    /// disabled (default), validation issues are printed as warnings to stderr.
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

    /// Adds an OpenAPI spec to be parsed and indexed during build.
    /// Call multiple times for multiple specs. Replaces `Engine::load_openapi_spec`.
    pub fn openapi_spec(mut self, data: Vec<u8>) -> Self {
        self.openapi_specs.push(data);
        self
    }

    /// Consumes the builder and creates a fully configured [`Engine`].
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. invalid TLS settings).
    pub fn build(self) -> Result<Engine, RuntimeError> {
        let config = self.client_config.unwrap_or_default();
        let client = HttpClient::new(&config, self.max_response_bytes, self.replay_trace_steps)?;

        let base_url = self
            .spec
            .source_descriptions
            .first()
            .map(|s| s.url.clone())
            .unwrap_or_default();

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
                    workflow_index,
                    step_indexes,
                    openapi_specs_raw: self.openapi_specs,
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

/// Parses an OpenAPI spec and populates the operation index.
pub(super) fn parse_openapi_into_index(
    data: &[u8],
    op_index: &mut BTreeMap<String, OperationEntry>,
) -> Result<(), RuntimeError> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_slice(data).map_err(|err| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!("parsing OpenAPI spec: {err}"),
        )
    })?;
    let Some(paths) = root.get("paths") else {
        return Ok(());
    };
    let Some(paths_map) = paths.as_mapping() else {
        return Ok(());
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
            op_index.insert(
                op_id,
                OperationEntry {
                    method: method.to_uppercase(),
                    path: path.to_string(),
                },
            );
        }
    }
    Ok(())
}
