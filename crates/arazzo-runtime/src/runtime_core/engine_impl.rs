use std::future::Future;
use std::pin::Pin;

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

    /// Returns a reference to the underlying Arazzo spec.
    pub fn spec(&self) -> &ArazzoSpec {
        &self.inner.index.spec
    }

    /// Returns the workflow IDs defined in the spec.
    pub fn workflows(&self) -> Vec<String> {
        self.inner
            .index
            .spec
            .workflows
            .iter()
            .map(|wf| wf.workflow_id.clone())
            .collect()
    }

    /// Spawns an async task that executes the workflow and returns a handle
    /// for streaming events and awaiting the final result.
    ///
    /// A Tokio runtime must be active when calling this method.
    pub fn execute(&self, workflow_id: &str, inputs: BTreeMap<String, Value>) -> ExecutionHandle {
        let (event_tx, event_rx) = mpsc::channel(self.inner.channel_capacity);
        let (result_tx, result_rx) = oneshot::channel();
        let cancel = CancellationToken::new();
        let is_timeout = Arc::new(AtomicBool::new(false));

        let engine = self.clone();
        let wf_id = workflow_id.to_string();
        let cancel_clone = cancel.clone();
        let timeout_clone = Arc::clone(&is_timeout);

        tokio::spawn(async move {
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                trace_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
            });

            let result = engine.execute_inner(&ctx, &wf_id, inputs, 0).await;

            // Drop event_tx (held inside ctx) BEFORE sending result.
            // This guarantees collect() drains all events before getting the result.
            drop(ctx);
            let _ = result_tx.send(result);
        });

        ExecutionHandle::new(event_rx, result_rx, cancel, is_timeout)
    }

    /// Spawns execution with a timeout watchdog that cancels after `timeout`.
    pub fn execute_with_timeout(
        &self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        timeout: Duration,
    ) -> ExecutionHandle {
        let handle = self.execute(workflow_id, inputs);
        let cancel = handle.cancel_token().clone();
        let timeout_flag = handle.timeout_flag().clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            timeout_flag.store(true, Ordering::Release);
            cancel.cancel();
        });
        handle
    }

    /// Convenience: collects all events and the final result.
    pub async fn execute_collect(
        &self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
    ) -> ExecutionResult {
        self.execute(workflow_id, inputs).collect().await
    }

    /// Execute a single step (and optionally its transitive dependencies) within a workflow.
    ///
    /// When `no_deps` is false (default), computes transitive step dependencies via
    /// `$steps.*` references and executes them in workflow order before the target.
    /// When `no_deps` is true, executes only the target step — failing early if it
    /// references outputs from steps that have not been executed.
    pub fn execute_step(
        &self,
        workflow_id: &str,
        step_id: &str,
        inputs: BTreeMap<String, Value>,
        no_deps: bool,
    ) -> ExecutionHandle {
        let (event_tx, event_rx) = mpsc::channel(self.inner.channel_capacity);
        let (result_tx, result_rx) = oneshot::channel();
        let cancel = CancellationToken::new();
        let is_timeout = Arc::new(AtomicBool::new(false));

        let engine = self.clone();
        let wf_id = workflow_id.to_string();
        let s_id = step_id.to_string();
        let cancel_clone = cancel.clone();
        let timeout_clone = Arc::clone(&is_timeout);

        tokio::spawn(async move {
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                trace_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
            });

            let result = engine
                .execute_step_inner(&ctx, &wf_id, &s_id, inputs, no_deps)
                .await;

            drop(ctx);
            let _ = result_tx.send(result);
        });

        ExecutionHandle::new(event_rx, result_rx, cancel, is_timeout)
    }

    async fn execute_step_inner(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step_id: &str,
        inputs: BTreeMap<String, Value>,
        no_deps: bool,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let workflow = self.get_workflow(workflow_id).cloned().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::WorkflowNotFound,
                format!("workflow \"{workflow_id}\" not found"),
            )
        })?;

        let target_idx = workflow
            .steps
            .iter()
            .position(|s| s.step_id == step_id)
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::StepNotFound,
                    format!("step \"{step_id}\" not found in workflow \"{workflow_id}\""),
                )
            })?;

        let steps_to_run: Vec<usize> = if no_deps {
            let direct_refs: Vec<String> = extract_step_refs(&workflow.steps[target_idx])
                .into_iter()
                .filter(|r| r != step_id)
                .collect();
            if !direct_refs.is_empty() {
                let dep_names = direct_refs.join(", ");
                return Err(RuntimeError::new(
                    RuntimeErrorKind::StepMissingDependency,
                    format!(
                        "step \"{step_id}\" references outputs from step(s) [{dep_names}] which were not executed (use without --no-deps to auto-resolve)"
                    ),
                ));
            }
            vec![target_idx]
        } else {
            let mut deps = compute_transitive_deps(&workflow, step_id)?;
            deps.insert(target_idx);
            deps.into_iter().collect()
        };

        let mut vars = self.validate_and_populate_inputs(&workflow, inputs)?;

        for &idx in &steps_to_run {
            exec_ctx.check_cancelled()?;
            let step = {
                let mut s = workflow.steps[idx].clone();
                merge_workflow_params(&workflow.parameters, &mut s);
                s
            };

            let start = std::time::Instant::now();
            let attempt = if self.inner.trace_enabled {
                Engine::next_attempt(exec_ctx, workflow_id, &step.step_id)
            } else {
                0
            };

            let execution = match self
                .execute_step_with_result(exec_ctx, workflow_id, &step, &mut vars, 0)
                .await
            {
                Ok(exec) => exec,
                Err(err) => {
                    let duration = start.elapsed();
                    if self.inner.trace_enabled {
                        let record = Engine::build_step_trace_record(
                            exec_ctx,
                            workflow_id,
                            &step,
                            attempt,
                            duration,
                            &StepTraceData::default(),
                            TraceDecision::with_path(TraceDecisionPath::Error),
                            BTreeMap::new(),
                            Some(err.message.clone()),
                        );
                        Engine::push_trace_record(exec_ctx, record).await;
                    }
                    return Err(err);
                }
            };
            let duration = start.elapsed();
            let step_outputs = vars.step_outputs(&step.step_id);

            // Evaluate onSuccess/onFailure actions even in single-step mode,
            // so that action-based error handling is honored.
            let retry_count = BTreeMap::new();
            let action = self
                .handle_step_result(StepDecisionContext {
                    workflow_id,
                    workflow: &workflow,
                    step_idx: idx,
                    result: &execution.result,
                    vars: &vars,
                    depth: 0,
                    retry_count: &retry_count,
                    cancel: &exec_ctx.cancel,
                    is_timeout: &exec_ctx.is_timeout,
                })
                .await;

            let trace_err = match &action.flow {
                FlowDecision::Error(err) => Some(err.message.clone()),
                _ => execution.result.err.clone(),
            };
            if self.inner.trace_enabled {
                let record = Engine::build_step_trace_record(
                    exec_ctx,
                    workflow_id,
                    &step,
                    attempt,
                    duration,
                    &execution.trace,
                    action.trace,
                    step_outputs,
                    trace_err,
                );
                Engine::push_trace_record(exec_ctx, record).await;
            }

            if let FlowDecision::Error(err) = action.flow {
                return Err(err);
            }
            if !execution.result.success {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::SuccessCriteriaFailed,
                    format!("step \"{}\" failed success criteria", step.step_id),
                ));
            }
        }

        Ok(vars.step_outputs(step_id))
    }

    /// Core recursive execution loop. Uses `Box::pin` for async recursion
    /// (GotoWorkflow and sub-workflow calls recurse back into this method).
    #[allow(clippy::type_complexity)]
    fn execute_inner<'a>(
        &'a self,
        exec_ctx: &'a ExecutionContext,
        workflow_id: &'a str,
        inputs: BTreeMap<String, Value>,
        depth: usize,
    ) -> Pin<Box<dyn Future<Output = Result<BTreeMap<String, Value>, RuntimeError>> + Send + 'a>>
    {
        Box::pin(async move {
            exec_ctx.check_cancelled()?;
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

            let mut vars = self.validate_and_populate_inputs(&workflow, inputs)?;

            if self.inner.parallel_mode
                && self.inner.debug_controller.is_none()
                && can_execute_parallel(&workflow)
            {
                return self
                    .execute_parallel(exec_ctx, workflow_id, &workflow, &mut vars)
                    .await;
            }

            let workflow_start = Instant::now();
            let mut step_index: usize = 0;
            let mut retry_count = BTreeMap::<usize, usize>::new();
            let max_iterations = workflow.steps.len().saturating_mul(10);
            let mut completed = false;

            for _ in 0..max_iterations {
                exec_ctx.check_cancelled()?;
                if step_index >= workflow.steps.len() {
                    completed = true;
                    break;
                }

                let step = {
                    let mut s = workflow.steps[step_index].clone();
                    merge_workflow_params(&workflow.parameters, &mut s);
                    s
                };
                self.debug_gate_step(exec_ctx, workflow_id, &step, &vars, depth)
                    .await?;

                self.emit_before_step_event(exec_ctx, workflow_id, &step)
                    .await;

                let attempt = if self.inner.trace_enabled {
                    Engine::next_attempt(exec_ctx, workflow_id, &step.step_id)
                } else {
                    0
                };

                let start = Instant::now();
                let execution = match self
                    .execute_step_with_result(exec_ctx, workflow_id, &step, &mut vars, depth)
                    .await
                {
                    Ok(execution) => execution,
                    Err(err) => {
                        let duration = start.elapsed();
                        if self.inner.trace_enabled {
                            let record = Engine::build_step_trace_record(
                                exec_ctx,
                                workflow_id,
                                &step,
                                attempt,
                                duration,
                                &StepTraceData::default(),
                                TraceDecision::with_path(TraceDecisionPath::Error),
                                BTreeMap::new(),
                                Some(err.message.clone()),
                            );
                            Engine::push_trace_record(exec_ctx, record).await;
                        }
                        self.emit_observer_event(
                            exec_ctx,
                            ObserverEvent::WorkflowCompleted {
                                workflow_id: workflow_id.to_string(),
                                outputs: BTreeMap::new(),
                                duration: workflow_start.elapsed(),
                                error: Some(err.message.clone()),
                            },
                        )
                        .await;
                        return Err(err);
                    }
                };
                let duration = start.elapsed();
                let step_outputs = vars.step_outputs(&step.step_id);

                let step_status_code = execution
                    .result
                    .response
                    .as_ref()
                    .map(|r| r.status_code)
                    .unwrap_or(0);

                self.emit_after_step_event(
                    exec_ctx,
                    workflow_id,
                    &step,
                    step_status_code,
                    step_outputs.clone(),
                    execution.result.err.clone(),
                    duration,
                )
                .await;

                self.emit_step_completed_event(
                    exec_ctx,
                    workflow_id,
                    &step,
                    step_status_code,
                    duration,
                    step_outputs.clone(),
                    execution.result.err.clone(),
                    execution.result.success,
                )
                .await;

                let action = self
                    .handle_step_result(StepDecisionContext {
                        workflow_id,
                        workflow: &workflow,
                        step_idx: step_index,
                        result: &execution.result,
                        vars: &vars,
                        depth,
                        retry_count: &retry_count,
                        cancel: &exec_ctx.cancel,
                        is_timeout: &exec_ctx.is_timeout,
                    })
                    .await;

                let trace_err = match &action.flow {
                    FlowDecision::Error(err) => Some(err.message.clone()),
                    _ => execution.result.err.clone(),
                };
                if self.inner.trace_enabled {
                    let record = Engine::build_step_trace_record(
                        exec_ctx,
                        workflow_id,
                        &step,
                        attempt,
                        duration,
                        &execution.trace,
                        action.trace.clone(),
                        step_outputs,
                        trace_err,
                    );
                    Engine::push_trace_record(exec_ctx, record).await;
                }

                match action.flow {
                    FlowDecision::Done => {
                        completed = true;
                        break;
                    }
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
                        // Emit retry event for observers
                        let retry_step = &workflow.steps[idx];
                        let retry_trace = action.trace.clone();
                        self.emit_observer_event(
                            exec_ctx,
                            ObserverEvent::RetryScheduled {
                                workflow_id: workflow_id.to_string(),
                                step_id: retry_step.step_id.clone(),
                                attempt: *value,
                                max_attempts: retry_trace
                                    .retry_limit
                                    .map(|v| v as usize)
                                    .unwrap_or(MAX_RETRIES_PER_STEP),
                                delay_seconds: retry_trace.retry_after_seconds.unwrap_or(0),
                            },
                        )
                        .await;
                        step_index = idx;
                    }
                    FlowDecision::GotoWorkflow(next_wf) => {
                        return self
                            .execute_inner(exec_ctx, &next_wf, vars.inputs.clone(), depth + 1)
                            .await;
                    }
                    FlowDecision::Error(err) => {
                        self.emit_observer_event(
                            exec_ctx,
                            ObserverEvent::WorkflowCompleted {
                                workflow_id: workflow_id.to_string(),
                                outputs: BTreeMap::new(),
                                duration: workflow_start.elapsed(),
                                error: Some(err.message.clone()),
                            },
                        )
                        .await;
                        return Err(err);
                    }
                }
            }

            if !completed {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::IterationLimitExceeded,
                    format!(
                        "workflow \"{workflow_id}\" exceeded iteration limit ({max_iterations}) — possible infinite retry/goto loop"
                    ),
                ));
            }

            let workflow_outputs = self.build_outputs(&workflow, &vars);
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::WorkflowCompleted {
                    workflow_id: workflow_id.to_string(),
                    outputs: workflow_outputs.clone(),
                    duration: workflow_start.elapsed(),
                    error: None,
                },
            )
            .await;
            Ok(workflow_outputs)
        })
    }

    fn get_workflow(&self, workflow_id: &str) -> Option<&Workflow> {
        self.inner
            .index
            .workflow_index
            .get(workflow_id)
            .and_then(|idx| self.inner.index.spec.workflows.get(*idx))
    }

    async fn execute_step_with_result(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
    ) -> Result<StepExecution, RuntimeError> {
        if matches!(&step.target, Some(StepTarget::WorkflowId(_))) {
            let child_wf_id = match &step.target {
                Some(StepTarget::WorkflowId(id)) => id.clone(),
                _ => String::new(),
            };
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::SubWorkflowStarted {
                    parent_workflow_id: workflow_id.to_string(),
                    parent_step_id: step.step_id.clone(),
                    child_workflow_id: child_wf_id,
                    depth: depth + 1,
                },
            )
            .await;
            let result = self
                .execute_subworkflow_step(exec_ctx, step, vars, depth)
                .await?;
            return Ok(StepExecution {
                result,
                outputs: vars.step_outputs(&step.step_id),
                dry_run_request: None,
                trace: StepTraceData::default(),
            });
        }

        let execution = self
            .execute_http_step(exec_ctx, workflow_id, step, vars, depth)
            .await?;
        if let Some(req) = execution.dry_run_request.clone() {
            let _ = exec_ctx
                .event_tx
                .send(EngineEvent::DryRunRequest(req))
                .await;
        }
        for (name, value) in &execution.outputs {
            vars.set_step_output(&step.step_id, name, value.clone());
        }
        Ok(execution)
    }

    async fn execute_subworkflow_step(
        &self,
        exec_ctx: &ExecutionContext,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
    ) -> Result<StepResult, RuntimeError> {
        exec_ctx.check_cancelled()?;
        let eval = ExpressionEvaluator::new(self.make_eval_context(vars, None));
        let mut sub_inputs = BTreeMap::new();
        for param in &step.parameters {
            let value_str = param.value_as_str();
            let value = if let Some(inner) = value_str
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .filter(|s| s.starts_with('$') && !s.contains('{'))
            {
                // Single expression like {$inputs.count} — preserve type
                eval.evaluate(inner)
            } else if value_str.contains("{$") {
                // Mixed text + expressions — must stringify
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
            .execute_inner(exec_ctx, wf_id, sub_inputs, depth + 1)
            .await
            .map_err(|err| {
                let msg = format!("sub-workflow {wf_id}: {}", err.message);
                RuntimeError::with_source(RuntimeErrorKind::SubWorkflowFailed, msg, err)
            })?;

        for (name, value) in outputs {
            vars.set_step_output(&step.step_id, &name, value);
        }

        let eval_post = ExpressionEvaluator::new(self.make_eval_context(vars, None));
        for criterion in &step.success_criteria {
            if !evaluate_criterion(criterion, &eval_post, None, &self.inner.regex_cache) {
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

    fn validate_and_populate_inputs(
        &self,
        workflow: &Workflow,
        mut inputs: BTreeMap<String, Value>,
    ) -> Result<VarStore, RuntimeError> {
        if let Some(schema) = &workflow.inputs {
            let issues = validate_inputs(schema, &mut inputs);
            if !issues.is_empty() {
                let has_errors = issues
                    .iter()
                    .any(|i| i.severity == InputIssueSeverity::Error);

                if self.inner.strict_inputs && has_errors {
                    let msgs: Vec<String> = issues.iter().map(ToString::to_string).collect();
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::InputValidation,
                        format!(
                            "input validation failed for workflow \"{}\": {}",
                            workflow.workflow_id,
                            msgs.join("; ")
                        ),
                    ));
                }

                for issue in &issues {
                    eprintln!("warning: {issue}");
                }
            }
        }

        let mut vars = VarStore::default();
        for (k, v) in inputs {
            vars.set_input(&k, v);
        }
        Ok(vars)
    }

    pub(super) fn make_eval_context(
        &self,
        vars: &VarStore,
        response: Option<&Response>,
    ) -> EvalContext {
        let mut ctx = vars.eval_context(response);
        ctx.source_descriptions = self.inner.index.source_descriptions_map.clone();
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
            let value = eval.resolve_value(expr);
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
