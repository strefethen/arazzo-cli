use super::*;

impl Engine {
    /// Creates a new engine from an Arazzo spec using default HTTP client settings.
    ///
    /// This is a convenience constructor equivalent to `EngineBuilder::new(spec).build()`.
    pub fn new(spec: ArazzoSpec) -> Result<Self, RuntimeError> {
        EngineBuilder::new(spec).build()
    }

    /// Creates a new engine from an Arazzo spec with custom HTTP client settings.
    ///
    /// This is a convenience constructor equivalent to
    /// `EngineBuilder::new(spec).client_config(config).build()`.
    pub fn with_client_config(
        spec: ArazzoSpec,
        config: ClientConfig,
    ) -> Result<Self, RuntimeError> {
        EngineBuilder::new(spec).client_config(config).build()
    }

    /// Registers a trace hook that receives step lifecycle events during execution.
    #[deprecated(note = "Use EngineBuilder::trace_hook() instead")]
    pub fn set_trace_hook(&mut self, hook: Arc<dyn TraceHook>) {
        self.trace_hook = Some(hook);
    }

    /// Attaches a debug controller for breakpoint-driven step-through execution.
    #[deprecated(note = "Use EngineBuilder::debug_controller() instead")]
    pub fn set_debug_controller(&mut self, controller: Arc<DebugController>) {
        self.debug_controller = Some(controller);
    }

    /// Enables or disables parallel execution of independent steps within a workflow.
    #[deprecated(note = "Use EngineBuilder::parallel() instead")]
    pub fn set_parallel_mode(&mut self, enabled: bool) {
        self.parallel_mode = enabled;
    }

    /// Enables or disables dry-run mode, which resolves requests without sending them.
    #[deprecated(note = "Use EngineBuilder::dry_run() instead")]
    pub fn set_dry_run_mode(&mut self, enabled: bool) {
        self.dry_run_mode = enabled;
    }

    /// Enables or disables detailed per-step trace recording during execution.
    #[deprecated(note = "Use EngineBuilder::trace() instead")]
    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.trace_enabled = enabled;
    }

    /// Returns the captured requests from the most recent dry-run execution.
    pub fn dry_run_requests(&self) -> Vec<DryRunRequest> {
        match self.dry_run_reqs.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                // Intentional: recover partial data from poisoned mutex (thread panicked)
                eprintln!("WARNING: dry_run_reqs mutex poisoned, returning partial data");
                poisoned.into_inner().clone()
            }
        }
    }

    /// Returns the per-step trace records from the most recent execution.
    /// Only populated when trace is enabled via [`Engine::set_trace_enabled`].
    pub fn trace_steps(&self) -> Vec<TraceStepRecord> {
        match self.trace_steps.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                // Intentional: recover partial data from poisoned mutex (thread panicked)
                eprintln!("WARNING: trace_steps mutex poisoned, returning partial data");
                poisoned.into_inner().clone()
            }
        }
    }

    /// Returns all execution lifecycle events from the most recent execution.
    pub fn execution_events(&self) -> Vec<ExecutionEvent> {
        match self.execution_events.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                // Intentional: recover partial data from poisoned mutex (thread panicked)
                eprintln!("WARNING: execution_events mutex poisoned, returning partial data");
                poisoned.into_inner().clone()
            }
        }
    }

    /// Returns a reference to the underlying Arazzo spec.
    pub fn spec(&self) -> &ArazzoSpec {
        &self.index.spec
    }

    /// Returns the workflow IDs defined in the spec.
    pub fn workflows(&self) -> Vec<String> {
        self.index
            .spec
            .workflows
            .iter()
            .map(|wf| wf.workflow_id.clone())
            .collect()
    }

    /// Parses an OpenAPI spec and indexes its operations by `operationId`.
    ///
    /// Must be called before [`Engine::execute`] if any workflow steps reference operations by ID.
    pub fn load_openapi_spec(&mut self, data: &[u8]) -> Result<(), RuntimeError> {
        let root: serde_yml::Value = serde_yml::from_slice(data).map_err(|err| {
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
                    .get(serde_yml::Value::String("operationId".to_string()))
                    .and_then(serde_yml::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if op_id.is_empty() {
                    continue;
                }
                self.index.op_index.insert(
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

    /// Executes a workflow by ID with the given inputs, returning its outputs.
    pub fn execute(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        self.execute_with_options(workflow_id, inputs, ExecutionOptions::default())
    }

    /// Executes a workflow by ID with the given inputs and execution options (deadline, cancellation).
    pub fn execute_with_options(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        options: ExecutionOptions,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        if self.dry_run_mode {
            if let Ok(mut guard) = self.dry_run_reqs.lock() {
                guard.clear();
            }
        }
        self.clear_trace_state();
        self.execute_inner(workflow_id, inputs, 0, &options)
    }

    fn execute_inner(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        options.check()?;
        if depth >= MAX_CALL_DEPTH {
            return Err(RuntimeError::new(
                RuntimeErrorKind::MaxCallDepthExceeded,
                format!(
                    "max call depth ({MAX_CALL_DEPTH}) exceeded calling workflow \"{workflow_id}\""
                ),
            ));
        }

        let workflow = self.get_workflow(workflow_id).cloned().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::WorkflowNotFound,
                format!("workflow \"{workflow_id}\" not found"),
            )
        })?;

        let mut vars = VarStore::default();
        for (k, v) in inputs {
            vars.set_input(&k, v);
        }

        if self.parallel_mode && self.debug_controller.is_none() && can_execute_parallel(&workflow)
        {
            return self.execute_parallel(workflow_id, &workflow, &mut vars, options);
        }

        let mut step_index: usize = 0;
        let mut retry_count = BTreeMap::<usize, usize>::new();
        let max_iterations = workflow.steps.len().saturating_mul(10);

        for _ in 0..max_iterations {
            options.check()?;
            if step_index >= workflow.steps.len() {
                break;
            }

            let step = {
                let mut s = workflow.steps[step_index].clone();
                merge_workflow_params(&workflow.parameters, &mut s);
                s
            };
            self.debug_gate_step(workflow_id, &step, &vars, depth)?;

            self.emit_before_step_event(workflow_id, &step);

            let attempt = if self.trace_enabled {
                self.next_attempt(workflow_id, &step.step_id)
            } else {
                0
            };

            let start = Instant::now();
            let execution = match self.execute_step_with_result(
                workflow_id,
                &step,
                &mut vars,
                depth,
                options,
            ) {
                Ok(execution) => execution,
                Err(err) => {
                    let duration = start.elapsed();
                    if self.trace_enabled {
                        let record = self.build_step_trace_record(
                            workflow_id,
                            &step,
                            attempt,
                            duration,
                            &StepTraceData::default(),
                            TraceDecision::with_path(TraceDecisionPath::Error),
                            BTreeMap::new(),
                            Some(err.message.clone()),
                        );
                        self.push_trace_record(record);
                    }
                    return Err(err);
                }
            };
            let duration = start.elapsed();
            let step_outputs = vars.step_outputs(&step.step_id);

            self.emit_after_step_event(
                workflow_id,
                &step,
                execution
                    .result
                    .response
                    .as_ref()
                    .map(|r| r.status_code)
                    .unwrap_or(0),
                step_outputs.clone(),
                execution.result.err.clone(),
                duration,
            );

            let action = self.handle_step_result(StepDecisionContext {
                workflow_id,
                workflow: &workflow,
                step_idx: step_index,
                result: &execution.result,
                vars: &vars,
                depth,
                retry_count: &retry_count,
                options,
            });

            let trace_err = match &action.flow {
                FlowDecision::Error(err) => Some(err.message.clone()),
                _ => execution.result.err.clone(),
            };
            if self.trace_enabled {
                let record = self.build_step_trace_record(
                    workflow_id,
                    &step,
                    attempt,
                    duration,
                    &execution.trace,
                    action.trace.clone(),
                    step_outputs,
                    trace_err,
                );
                self.push_trace_record(record);
            }

            match action.flow {
                FlowDecision::Done => break,
                FlowDecision::Next(idx) => {
                    if idx == step_index {
                        let value = retry_count.entry(step_index).or_insert(0);
                        *value += 1;
                    } else {
                        retry_count.remove(&step_index);
                    }
                    step_index = idx;
                }
                FlowDecision::Retry(idx) => {
                    let value = retry_count.entry(idx).or_insert(0);
                    *value += 1;
                    step_index = idx;
                }
                FlowDecision::GotoWorkflow(next_wf) => {
                    return self.execute_inner(&next_wf, vars.inputs.clone(), depth + 1, options);
                }
                FlowDecision::Error(err) => return Err(err),
            }
        }

        Ok(self.build_outputs(&workflow, &vars))
    }

    fn get_workflow(&self, workflow_id: &str) -> Option<&Workflow> {
        self.index
            .workflow_index
            .get(workflow_id)
            .and_then(|idx| self.index.spec.workflows.get(*idx))
    }

    fn execute_step_with_result(
        &mut self,
        workflow_id: &str,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<StepExecution, RuntimeError> {
        if matches!(&step.target, Some(StepTarget::WorkflowId(_))) {
            let result = self.execute_subworkflow_step(step, vars, depth, options)?;
            return Ok(StepExecution {
                result,
                outputs: vars.step_outputs(&step.step_id),
                dry_run_request: None,
                trace: StepTraceData::default(),
            });
        }

        let execution = self.execute_http_step(workflow_id, step, vars, depth, options)?;
        if let Some(req) = execution.dry_run_request.clone() {
            if let Ok(mut guard) = self.dry_run_reqs.lock() {
                guard.push(req);
            }
        }
        for (name, value) in &execution.outputs {
            vars.set_step_output(&step.step_id, name, value.clone());
        }
        Ok(execution)
    }

    fn execute_subworkflow_step(
        &mut self,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<StepResult, RuntimeError> {
        options.check()?;
        let eval = ExpressionEvaluator::new(self.make_eval_context(vars, None));
        let mut sub_inputs = BTreeMap::new();
        for param in &step.parameters {
            let value_str = param.value_as_str();
            let value = if value_str.contains("{$") {
                Value::String(eval.interpolate_string(&value_str))
            } else {
                eval.evaluate(&value_str)
            };
            sub_inputs.insert(param.name.clone(), value);
        }

        let wf_id = match &step.target {
            Some(StepTarget::WorkflowId(id)) => id.as_str(),
            _ => "",
        };
        let outputs = self
            .execute_inner(wf_id, sub_inputs, depth + 1, options)
            .map_err(|err| {
                let msg = format!("sub-workflow {wf_id}: {}", err.message);
                RuntimeError::with_source(RuntimeErrorKind::SubWorkflowFailed, msg, err)
            })?;

        for (name, value) in outputs {
            vars.set_step_output(&step.step_id, &name, value);
        }

        let eval_post = ExpressionEvaluator::new(self.make_eval_context(vars, None));
        for criterion in &step.success_criteria {
            if !evaluate_criterion(criterion, &eval_post, None) {
                return Ok(StepResult {
                    success: false,
                    response: None,
                    err: None,
                });
            }
        }

        Ok(StepResult {
            success: true,
            response: None,
            err: None,
        })
    }

    pub(super) fn make_eval_context(
        &self,
        vars: &VarStore,
        response: Option<&Response>,
    ) -> EvalContext {
        let mut ctx = vars.eval_context(response);
        ctx.source_descriptions = self.index.source_descriptions_map.clone();
        ctx
    }

    pub(crate) fn build_outputs(
        &self,
        workflow: &Workflow,
        vars: &VarStore,
    ) -> BTreeMap<String, Value> {
        let mut ctx = self.make_eval_context(vars, None);
        let mut computed_outputs = BTreeMap::new();
        for (name, expr) in &workflow.outputs {
            let eval = ExpressionEvaluator::new(ctx.clone());
            let value = if expr.starts_with('$') {
                eval.evaluate(expr)
            } else if expr.contains("{$") {
                Value::String(eval.interpolate_string(expr))
            } else {
                Value::String(expr.clone())
            };
            computed_outputs.insert(name.clone(), value);
            ctx.outputs = computed_outputs.clone();
        }
        computed_outputs
    }
}

pub(super) fn merge_workflow_params(workflow_params: &[Parameter], step: &mut Step) {
    if !matches!(&step.target, Some(StepTarget::WorkflowId(_))) && !workflow_params.is_empty() {
        let mut merged = workflow_params.to_vec();
        for sp in &step.parameters {
            merged.retain(|wp| !(wp.name == sp.name && wp.in_ == sp.in_));
            merged.push(sp.clone());
        }
        step.parameters = merged;
    }
}
