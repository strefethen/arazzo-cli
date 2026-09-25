use std::future::Future;
use std::pin::Pin;

use super::step_attempt::{count_retry, RouteScope};
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
        self.execute_with_completed_workflows(workflow_id, inputs, &BTreeSet::new())
    }

    /// Spawns execution with caller-provided completion evidence for local
    /// workflow dependencies. The set is copied into this invocation and is
    /// not retained by the engine after the handle is dropped.
    pub fn execute_with_completed_workflows(
        &self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        completed_workflows: &BTreeSet<String>,
    ) -> ExecutionHandle {
        let (event_tx, event_rx) = mpsc::channel(self.inner.channel_capacity);
        let (result_tx, result_rx) = oneshot::channel();
        let cancel = CancellationToken::new();
        let is_timeout = Arc::new(AtomicBool::new(false));

        let engine = self.clone();
        let wf_id = workflow_id.to_string();
        let cancel_clone = cancel.clone();
        let timeout_clone = Arc::clone(&is_timeout);
        let completed_workflows = completed_workflows.clone();

        tokio::spawn(async move {
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                role: ContextRole::Invocation,
                trace_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
                completed_workflows: Mutex::new(completed_workflows),
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
        self.execute_with_timeout_and_completed_workflows(
            workflow_id,
            inputs,
            timeout,
            &BTreeSet::new(),
        )
    }

    /// Spawns execution with a timeout and caller-provided completion
    /// evidence for local workflow dependencies.
    pub fn execute_with_timeout_and_completed_workflows(
        &self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        timeout: Duration,
        completed_workflows: &BTreeSet<String>,
    ) -> ExecutionHandle {
        let handle =
            self.execute_with_completed_workflows(workflow_id, inputs, completed_workflows);
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
        self.execute_step_with_completed_workflows(
            workflow_id,
            step_id,
            inputs,
            no_deps,
            &BTreeSet::new(),
        )
    }

    /// Executes a step while enforcing workflow-level completion evidence.
    pub fn execute_step_with_completed_workflows(
        &self,
        workflow_id: &str,
        step_id: &str,
        inputs: BTreeMap<String, Value>,
        no_deps: bool,
        completed_workflows: &BTreeSet<String>,
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
        let completed_workflows = completed_workflows.clone();

        tokio::spawn(async move {
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                role: ContextRole::Invocation,
                trace_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
                completed_workflows: Mutex::new(completed_workflows),
            });

            let result = engine
                .execute_step_inner(&ctx, &wf_id, &s_id, inputs, no_deps)
                .await;

            drop(ctx);
            let _ = result_tx.send(result);
        });

        ExecutionHandle::new(event_rx, result_rx, cancel, is_timeout)
    }

    #[allow(clippy::type_complexity)]
    fn execute_step_inner<'a>(
        &'a self,
        exec_ctx: &'a ExecutionContext,
        workflow_id: &'a str,
        step_id: &'a str,
        inputs: BTreeMap<String, Value>,
        no_deps: bool,
    ) -> Pin<Box<dyn Future<Output = Result<BTreeMap<String, Value>, RuntimeError>> + Send + 'a>>
    {
        Box::pin(async move {
            exec_ctx.check_cancelled()?;
            let workflow = self.get_workflow(workflow_id).cloned().ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::WorkflowNotFound,
                    format!("workflow \"{workflow_id}\" not found"),
                )
            })?;

            self.require_workflow_dependencies(exec_ctx, &workflow)?;

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

            if self.inner.parallel_mode {
                let fallback = SequentialFallback::new(
                    workflow_id,
                    SequentialFallbackReason::SingleStep,
                    step_id,
                    &format!(
                        "step \"{step_id}\" was selected for single-step execution, which runs one step at a time"
                    ),
                );
                Engine::push_sequential_fallback(exec_ctx, fallback).await;
            }

            // Shared retry counts across all iterations. Keys identify retry
            // execution sites, so later failure-action retries are independent.
            let mut retry_count = BTreeMap::<RetrySite, u64>::new();
            let max_iterations = compute_max_iterations(
                steps_to_run.iter().map(|&i| &workflow.steps[i]),
                &workflow.failure_actions,
            );
            let mut run_cursor: usize = 0;

            for _ in 0..max_iterations {
                exec_ctx.check_cancelled()?;
                if run_cursor >= steps_to_run.len() {
                    break;
                }
                let idx = steps_to_run[run_cursor];
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
                        // Route runtime errors through onFailure handlers instead of
                        // failing immediately (same as execute_inner — Bug #10).
                        StepExecution {
                            result: StepResult {
                                success: false,
                                response: None,
                                err: Some(err.message.clone()),
                                err_kind: Some(err.kind),
                            },
                            outputs: BTreeMap::new(),
                            dry_run_request: None,
                            trace: StepTraceData::default(),
                        }
                    }
                };
                let duration = start.elapsed();

                let mut action = self
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
                if exec_ctx.cancel.is_cancelled() {
                    action = engine_actions::RoutedDecision::error(exec_ctx.cancelled_error());
                }

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
                        // This attempt's own outputs. `vars` keeps the last
                        // successful run's for `$steps` expressions, and a
                        // failed re-run must not be recorded with them.
                        execution.outputs,
                        trace_err,
                    );
                    Engine::push_trace_record(exec_ctx, record).await;
                }
                exec_ctx.check_cancelled()?;
                match action.flow {
                    FlowDecision::Done => {
                        break;
                    }
                    FlowDecision::Next(next_idx) => {
                        // Find the position of next_idx in our filtered steps_to_run set.
                        if let Some(pos) = steps_to_run.iter().position(|&i| i == next_idx) {
                            run_cursor = pos;
                        } else if let Some(pos) = steps_to_run.iter().position(|&i| i >= next_idx) {
                            // Target is not in the filtered set (steps_to_run is
                            // sorted ascending) — resume at the first in-scope
                            // step at or after it, never before it.
                            run_cursor = pos;
                        } else {
                            // Target is past the filtered tail — nothing left in scope.
                            break;
                        }
                    }
                    FlowDecision::Retry {
                        step_idx: retry_idx,
                        retry_site,
                        retry_limit,
                        delay_seconds,
                        reference,
                    } => {
                        // Retry targets the current step; find it in our filtered set.
                        if let Some(pos) = steps_to_run.iter().position(|&i| i == retry_idx) {
                            exec_ctx.check_cancelled()?;
                            if let Some(reference) = reference {
                                self.execute_retry_reference(
                                    exec_ctx,
                                    RetryReferenceContext {
                                        workflow_id,
                                        workflow: &workflow,
                                        retried_step_id: &workflow.steps[retry_idx].step_id,
                                        depth: 0,
                                    },
                                    reference,
                                    &mut vars,
                                )
                                .await?;
                                exec_ctx.check_cancelled()?;
                            }
                            let value = retry_count.entry(retry_site).or_insert(0);
                            *value += 1;
                            let retry_step = &workflow.steps[retry_idx];
                            self.emit_observer_event(
                                exec_ctx,
                                ObserverEvent::RetryScheduled {
                                    workflow_id: workflow_id.to_string(),
                                    step_id: retry_step.step_id.clone(),
                                    attempt: *value,
                                    max_attempts: retry_limit,
                                    delay_seconds,
                                },
                            )
                            .await;
                            exec_ctx.check_cancelled()?;
                            run_cursor = pos;
                        } else {
                            return Err(RuntimeError::new(
                                RuntimeErrorKind::GotoTargetNotFound,
                                format!(
                                    "retry target step index {retry_idx} not in execute_step scope"
                                ),
                            ));
                        }
                    }
                    FlowDecision::GotoWorkflow {
                        workflow_id,
                        inputs,
                    } => {
                        exec_ctx.check_cancelled()?;
                        let inputs = inputs.unwrap_or_else(|| vars.inputs.clone());
                        return self.execute_inner(exec_ctx, &workflow_id, inputs, 1).await;
                    }
                    FlowDecision::Error(err) => {
                        return Err(err);
                    }
                }
            }

            exec_ctx.check_cancelled()?;
            Ok(vars.step_outputs(step_id))
        })
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

            self.require_workflow_dependencies(exec_ctx, &workflow)?;

            let mut vars = self.validate_and_populate_inputs(&workflow, inputs)?;

            if self.inner.parallel_mode {
                if let Some(fallback) = self.sequential_fallback(&workflow) {
                    Engine::push_sequential_fallback(exec_ctx, fallback).await;
                } else {
                    let result = self
                        .execute_parallel(exec_ctx, workflow_id, &workflow, &mut vars, depth)
                        .await;
                    // execute_parallel settles cancellation before it reports a
                    // terminal outcome, so its result decides: a cancelled
                    // invocation did not complete and is not recorded. The
                    // dependency guard ran before entering parallel execution, so
                    // record terminal failures too: this invocation was admitted
                    // and therefore completed, even when one of its steps failed.
                    if !result.as_ref().is_err_and(|err| err.kind.is_cancellation()) {
                        exec_ctx.mark_workflow_completed(workflow_id);
                    }
                    return result;
                }
            }

            let workflow_start = Instant::now();
            let mut step_index: usize = 0;
            let mut retry_count = BTreeMap::<RetrySite, u64>::new();
            let max_iterations =
                compute_max_iterations(workflow.steps.iter(), &workflow.failure_actions);
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
                exec_ctx.check_cancelled()?;

                let begun = self.begin_attempt(exec_ctx, workflow_id, &step).await;
                exec_ctx.check_cancelled()?;

                let run = self
                    .execute_step_with_result(exec_ctx, workflow_id, &step, &mut vars, depth)
                    .await;
                let scope = RouteScope {
                    workflow_id,
                    workflow: &workflow,
                    step_idx: step_index,
                    vars: &vars,
                    depth,
                    retry_count: &retry_count,
                    cancel: &exec_ctx.cancel,
                    is_timeout: &exec_ctx.is_timeout,
                };
                let (_settled, flow) = self
                    .finish_attempt(exec_ctx, &step, begun, run, scope)
                    .await;
                exec_ctx.check_cancelled()?;
                match flow {
                    FlowDecision::Done => {
                        completed = true;
                        break;
                    }
                    FlowDecision::Next(idx) => {
                        step_index = idx;
                    }
                    FlowDecision::Retry {
                        step_idx: idx,
                        retry_site,
                        retry_limit,
                        delay_seconds,
                        reference,
                    } => {
                        exec_ctx.check_cancelled()?;
                        if let Some(reference) = reference {
                            if let Err(err) = self
                                .execute_retry_reference(
                                    exec_ctx,
                                    RetryReferenceContext {
                                        workflow_id,
                                        workflow: &workflow,
                                        retried_step_id: &workflow.steps[idx].step_id,
                                        depth,
                                    },
                                    reference,
                                    &mut vars,
                                )
                                .await
                            {
                                // A cancelled invocation did not complete, so it
                                // is neither recorded nor reported as completed.
                                exec_ctx.check_cancelled()?;
                                exec_ctx.mark_workflow_completed(workflow_id);
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
                            exec_ctx.check_cancelled()?;
                        }
                        let retry =
                            count_retry(&mut retry_count, retry_site, retry_limit, delay_seconds);
                        self.emit_retry_scheduled(
                            exec_ctx,
                            workflow_id,
                            &workflow.steps[idx].step_id,
                            retry,
                        )
                        .await;
                        exec_ctx.check_cancelled()?;
                        step_index = idx;
                    }
                    FlowDecision::GotoWorkflow {
                        workflow_id: target_workflow_id,
                        inputs,
                    } => {
                        exec_ctx.check_cancelled()?;
                        let inputs = inputs.unwrap_or_else(|| vars.inputs.clone());
                        let result = self
                            .execute_inner(exec_ctx, &target_workflow_id, inputs, depth + 1)
                            .await;
                        // The target settled its own outcome, cancellation
                        // included, so its result decides: a cancelled caller
                        // did not complete and is not recorded. Otherwise the
                        // caller workflow was admitted and ran far enough to
                        // invoke the target, and its terminal outcome is
                        // completion evidence even when the target fails.
                        if !result.as_ref().is_err_and(|err| err.kind.is_cancellation()) {
                            exec_ctx.mark_workflow_completed(workflow_id);
                        }
                        return result;
                    }
                    FlowDecision::Error(err) => {
                        exec_ctx.mark_workflow_completed(workflow_id);
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
                exec_ctx.mark_workflow_completed(workflow_id);
                return Err(RuntimeError::new(
                    RuntimeErrorKind::IterationLimitExceeded,
                    format!(
                        "workflow \"{workflow_id}\" exceeded iteration limit ({max_iterations}) — possible infinite retry/goto loop"
                    ),
                ));
            }

            exec_ctx.check_cancelled()?;
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
            exec_ctx.mark_workflow_completed(workflow_id);
            Ok(workflow_outputs)
        })
    }

    fn require_workflow_dependencies(
        &self,
        exec_ctx: &ExecutionContext,
        workflow: &Workflow,
    ) -> Result<(), RuntimeError> {
        for dependency in &workflow.depends_on {
            let satisfied = match arazzo_spec::classify_workflow_dependency(dependency) {
                arazzo_spec::WorkflowDependency::Local(workflow_id) => {
                    exec_ctx.workflow_is_completed(workflow_id)
                }
                arazzo_spec::WorkflowDependency::External { .. }
                | arazzo_spec::WorkflowDependency::Invalid => false,
            };
            if !satisfied {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::WorkflowDependencyUnsatisfied,
                    format!(
                        "workflow \"{}\" depends on workflow \"{dependency}\", which has not completed in this execution context",
                        workflow.workflow_id
                    ),
                ));
            }
        }
        Ok(())
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

    /// Executes a retry action's `stepId`/`workflowId` recovery reference,
    /// call-and-return, before the failed step is retried.
    ///
    /// Failure Action Object: "If a stepId or workflowId are specified, then
    /// the reference is executed and the context is returned, after which the
    /// current step is retried." A workflow reference runs like a sub-workflow
    /// call (`$workflows.<id>.*` resolves afterward); a step reference runs
    /// the referenced step of the current workflow once, persisting its
    /// outputs, without following that step's own onSuccess/onFailure routing.
    /// Any failure aborts the workflow — a broken recovery reference must not
    /// degrade into a plain retry.
    async fn execute_retry_reference(
        &self,
        exec_ctx: &ExecutionContext,
        ctx: RetryReferenceContext<'_>,
        reference: RetryReference,
        vars: &mut VarStore,
    ) -> Result<(), RuntimeError> {
        let RetryReferenceContext {
            workflow_id,
            workflow,
            retried_step_id,
            depth,
        } = ctx;
        match reference {
            RetryReference::Workflow {
                workflow_id: target,
                inputs,
            } => {
                exec_ctx.check_cancelled()?;
                let sub_inputs = inputs.unwrap_or_else(|| vars.inputs.clone());
                self.emit_observer_event(
                    exec_ctx,
                    ObserverEvent::SubWorkflowStarted {
                        parent_workflow_id: workflow_id.to_string(),
                        parent_step_id: retried_step_id.to_string(),
                        child_workflow_id: target.clone(),
                        depth: depth + 1,
                    },
                )
                .await;
                exec_ctx.check_cancelled()?;
                let output_result = self
                    .execute_inner(exec_ctx, &target, sub_inputs.clone(), depth + 1)
                    .await;
                if exec_ctx.cancel.is_cancelled() {
                    return Err(exec_ctx.cancelled_error());
                }
                let outputs = output_result.map_err(|err| {
                    if err.kind == RuntimeErrorKind::WorkflowDependencyUnsatisfied {
                        err
                    } else {
                        let msg = format!(
                            "step {retried_step_id}: retry reference workflow \"{target}\": {}",
                            err.message
                        );
                        RuntimeError::with_source(RuntimeErrorKind::RetryReferenceFailed, msg, err)
                    }
                })?;
                // Register completed reference state for $workflows.<id>.* —
                // the "context is returned" half of the spec sentence. The
                // retried step's own outputs are not touched.
                vars.register_workflow_state(&target, sub_inputs, outputs);
                Ok(())
            }
            RetryReference::Step { step_id } => {
                exec_ctx.check_cancelled()?;
                let Some(idx) = self.find_step_index(workflow, &step_id) else {
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::RetryReferenceFailed,
                        format!(
                            "step {retried_step_id}: retry reference step \"{step_id}\" not found in workflow \"{workflow_id}\""
                        ),
                    ));
                };
                let step = {
                    let mut s = workflow.steps[idx].clone();
                    merge_workflow_params(&workflow.parameters, &mut s);
                    s
                };
                let execution_result = self
                    .execute_step_with_result(exec_ctx, workflow_id, &step, vars, depth)
                    .await;
                if exec_ctx.cancel.is_cancelled() {
                    return Err(exec_ctx.cancelled_error());
                }
                let execution = execution_result.map_err(|err| {
                    let msg = format!(
                        "step {retried_step_id}: retry reference step \"{step_id}\": {}",
                        err.message
                    );
                    RuntimeError::with_source(RuntimeErrorKind::RetryReferenceFailed, msg, err)
                })?;
                if !execution.result.success {
                    let detail = execution
                        .result
                        .err
                        .as_deref()
                        .unwrap_or("step did not succeed");
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::RetryReferenceFailed,
                        format!(
                            "step {retried_step_id}: retry reference step \"{step_id}\": {detail}"
                        ),
                    ));
                }
                Ok(())
            }
        }
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
            let (value, warnings) = match &param.value {
                ValueSource::Selector(_) => resolve_value_source(&param.value, &eval),
                ValueSource::Literal(_) => {
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
                    (value, Vec::new())
                }
            };
            for warning in warnings {
                eprintln!(
                    "warning: sub-workflow parameter {:?}: {warning}",
                    param.name
                );
            }
            sub_inputs.insert(param.name.clone(), value);
        }

        let wf_id = match &step.target {
            Some(StepTarget::WorkflowId(id)) => id.as_str(),
            _ => "",
        };
        let output_result = self
            .execute_inner(exec_ctx, wf_id, sub_inputs.clone(), depth + 1)
            .await;
        if exec_ctx.cancel.is_cancelled() {
            return Err(exec_ctx.cancelled_error());
        }
        let outputs = output_result.map_err(|err| {
            if err.kind == RuntimeErrorKind::WorkflowDependencyUnsatisfied {
                err
            } else {
                let msg = format!("sub-workflow {wf_id}: {}", err.message);
                RuntimeError::with_source(RuntimeErrorKind::SubWorkflowFailed, msg, err)
            }
        })?;

        // Register completed sub-workflow state for $workflows.<id>.* expressions.
        vars.register_workflow_state(wf_id, sub_inputs, outputs.clone());

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
                    err_kind: None,
                });
            }
        }

        Ok(StepResult {
            success: true,
            response: None,
            err: None,
            err_kind: None,
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
        ctx.self_uri = self.inner.index.spec.self_uri.clone();
        ctx.source_descriptions = self.inner.index.source_descriptions_map.clone();
        ctx
    }

    pub(crate) fn build_outputs(
        &self,
        workflow: &Workflow,
        vars: &VarStore,
    ) -> BTreeMap<String, Value> {
        let mut ctx = self.make_eval_context(vars, None);
        for (name, output) in &workflow.outputs {
            let eval = ExpressionEvaluator::new(ctx.clone());
            let (value, warnings) = match output {
                OutputValue::RuntimeExpression(expression) => {
                    eval.resolve_value_with_diagnostics(expression)
                }
                OutputValue::Selector(selector) => resolve_selector(selector, &eval),
            };
            for warning in warnings {
                eprintln!("warning: workflow output {name:?}: {warning}");
            }
            ctx.outputs.insert(name.clone(), value);
        }
        ctx.outputs
    }
}

/// Call-site context for [`Engine::execute_retry_reference`].
#[derive(Debug)]
struct RetryReferenceContext<'a> {
    workflow_id: &'a str,
    workflow: &'a Workflow,
    /// Step being retried — names the reference in errors and observer events.
    retried_step_id: &'a str,
    depth: usize,
}

pub(super) fn merge_workflow_params(workflow_params: &[Parameter], step: &mut Step) {
    if !matches!(&step.target, Some(StepTarget::WorkflowId(_))) && !workflow_params.is_empty() {
        // Build a set of (name, in_) keys from step params for O(1) lookup.
        let step_keys: std::collections::HashSet<(&str, Option<&ParamLocation>)> = step
            .parameters
            .iter()
            .map(|p| (p.name.as_str(), p.in_.as_ref()))
            .collect();
        // Prepend workflow params that aren't overridden by step params.
        let mut merged: Vec<Parameter> = workflow_params
            .iter()
            .filter(|wp| !step_keys.contains(&(wp.name.as_str(), wp.in_.as_ref())))
            .cloned()
            .collect();
        merged.append(&mut step.parameters);
        step.parameters = merged;
    }
}

/// Computes a safe iteration limit for workflow execution that accounts for
/// per-step retry budgets. Each step needs its initial attempt plus the sum of
/// its applicable effective failure-action retry budgets in the worst case.
/// A goto multiplier (×2) provides headroom for goto cycles. The result is
/// floored at `step_count × 10` for backwards compatibility with goto-heavy
/// workflows. The `u128` budget keeps one initial attempt plus `u64::MAX`
/// retries representable.
fn compute_max_iterations<'a>(
    steps: impl Iterator<Item = &'a Step>,
    workflow_actions: &[OnAction],
) -> u128 {
    let workflow_retry = retry_budget_from_actions(workflow_actions);
    let mut step_count = 0u128;
    let mut total_budget = 0u128;
    for step in steps {
        step_count = step_count.saturating_add(1);
        // Failure routing uses the step list when present, otherwise the
        // workflow list. An exhausted retry can fall through to each later
        // retry action, so their budgets add rather than take a maximum.
        let retry_budget = if step.on_failure.is_empty() {
            workflow_retry
        } else {
            retry_budget_from_actions(&step.on_failure)
        };
        total_budget = total_budget.saturating_add(1u128.saturating_add(retry_budget));
    }
    // Goto multiplier ×2, floored at the legacy heuristic (step_count × 10).
    let retry_aware = total_budget.saturating_mul(2);
    retry_aware.max(step_count.saturating_mul(10))
}

fn retry_budget_from_actions(actions: &[OnAction]) -> u128 {
    actions
        .iter()
        .filter(|a| a.action_type() == ActionType::Retry)
        .fold(0u128, |budget, action| {
            budget.saturating_add(u128::from(effective_retry_limit(action.retry_limit)))
        })
}

#[cfg(test)]
mod retry_iteration_tests {
    use super::*;

    #[test]
    fn maximum_iterations_sums_effective_failure_retry_budgets_only() {
        let steps = [Step {
            on_failure: vec![
                OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(10),
                    ..OnAction::default()
                },
                OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(20),
                    ..OnAction::default()
                },
                OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(30),
                    ..OnAction::default()
                },
            ],
            // Retry is not legal in a Success Action Object. The runtime's
            // iteration protection must not give this model deviation budget.
            on_success: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(99),
                ..OnAction::default()
            }],
            ..Step::default()
        }];

        assert_eq!(compute_max_iterations(steps.iter(), &[]), 122);
    }

    #[test]
    fn maximum_iterations_preserves_u64_max_retry_and_initial_attempt() {
        let steps = [Step {
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(u64::MAX),
                ..OnAction::default()
            }],
            ..Step::default()
        }];

        assert_eq!(
            compute_max_iterations(steps.iter(), &[]),
            (u128::from(u64::MAX) + 1) * 2
        );
    }
}
