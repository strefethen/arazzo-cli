use super::*;

impl Engine {
    pub(super) async fn execute_parallel(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        workflow: &Workflow,
        vars: &mut VarStore,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let workflow_start = Instant::now();
        let levels = build_levels(workflow)?;
        for mut level in levels {
            exec_ctx.check_cancelled()?;
            level.sort_unstable();
            let level_vars = vars.clone();
            let mut level_results =
                Vec::<(usize, Step, Result<ParallelStepExecution, RuntimeError>)>::new();

            for idx in level.iter().copied() {
                let step = workflow.steps.get(idx).cloned().ok_or_else(|| {
                    RuntimeError::new(RuntimeErrorKind::StepNotFound, "invalid step index")
                })?;
                self.emit_before_step_event(exec_ctx, workflow_id, &step)
                    .await;
            }

            // Spawn parallel steps via JoinSet
            let mut join_set = tokio::task::JoinSet::new();
            for idx in level.iter().copied() {
                let step = {
                    let mut s = workflow.steps.get(idx).cloned().ok_or_else(|| {
                        RuntimeError::new(RuntimeErrorKind::StepNotFound, "invalid step index")
                    })?;
                    merge_workflow_params(&workflow.parameters, &mut s);
                    s
                };
                let engine = self.clone();
                let step_vars = level_vars.clone();
                let wf_id = workflow_id.to_string();
                let cancel = exec_ctx.cancel.clone();
                let is_timeout = Arc::clone(&exec_ctx.is_timeout);
                join_set.spawn(async move {
                    let result = engine
                        .execute_parallel_step(&wf_id, &step, &step_vars, &cancel, &is_timeout)
                        .await;
                    (idx, step, result)
                });
            }

            while let Some(join_result) = join_set.join_next().await {
                match join_result {
                    Ok(value) => level_results.push(value),
                    Err(_) => {
                        return Err(RuntimeError::new(
                            RuntimeErrorKind::ParallelThreadPanic,
                            "parallel step task panicked",
                        ));
                    }
                }
            }

            level_results.sort_by_key(|(idx, _, _)| *idx);
            // Parallel levels never retry or goto, so use an empty retry map.
            let retry_count = BTreeMap::<usize, usize>::new();
            for (idx, step, execution_result) in level_results {
                let attempt = if self.inner.trace_enabled {
                    Engine::next_attempt(exec_ctx, workflow_id, &step.step_id)
                } else {
                    0
                };

                let (execution, duration) = match execution_result {
                    Ok(mut par_exec) => {
                        // Replay intra-step events through the parent context
                        for event in std::mem::take(&mut par_exec.events) {
                            let _ = exec_ctx.event_tx.send(event).await;
                        }
                        (par_exec.execution, par_exec.duration)
                    }
                    Err(err) => {
                        // Wrap runtime errors into a failed StepExecution so they
                        // flow through handle_step_result (matching execute_inner).
                        let execution = StepExecution {
                            result: StepResult {
                                success: false,
                                response: None,
                                err: Some(err.message.clone()),
                                err_kind: Some(err.kind),
                            },
                            outputs: BTreeMap::new(),
                            dry_run_request: None,
                            trace: StepTraceData::default(),
                        };
                        (execution, Duration::ZERO)
                    }
                };

                let outputs_for_trace = execution.outputs.clone();
                let par_status_code = execution
                    .result
                    .response
                    .as_ref()
                    .map(|r| r.status_code)
                    .unwrap_or(0);

                self.emit_after_step_event(
                    exec_ctx,
                    workflow_id,
                    &step,
                    par_status_code,
                    outputs_for_trace.clone(),
                    execution.result.err.clone(),
                    duration,
                )
                .await;

                self.emit_step_completed_event(
                    exec_ctx,
                    workflow_id,
                    &step,
                    par_status_code,
                    duration,
                    outputs_for_trace.clone(),
                    execution.result.err.clone(),
                    execution.result.success,
                )
                .await;

                // Route through action handlers for consistent behavior with
                // sequential mode. The can_execute_parallel guard currently blocks
                // workflows with actions, so Retry/GotoWorkflow are unreachable.
                let action = self
                    .handle_step_result(StepDecisionContext {
                        workflow_id,
                        workflow,
                        step_idx: idx,
                        result: &execution.result,
                        vars,
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
                        outputs_for_trace,
                        trace_err,
                    );
                    Engine::push_trace_record(exec_ctx, record).await;
                }

                match action.flow {
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
                    FlowDecision::Done => {
                        let workflow_outputs = self.build_outputs(workflow, vars);
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
                        return Ok(workflow_outputs);
                    }
                    FlowDecision::Next(_) => {
                        // Expected path — continue to next step in level.
                    }
                    FlowDecision::Retry(_) | FlowDecision::GotoWorkflow(_) => {
                        // Unreachable: can_execute_parallel blocks workflows with
                        // retry/goto actions. Defensive error if guard is relaxed.
                        return Err(RuntimeError::new(
                            RuntimeErrorKind::InternalError,
                            format!(
                                "parallel execution does not support {:?} flow decisions",
                                "Retry/GotoWorkflow"
                            ),
                        ));
                    }
                }

                if let Some(req) = execution.dry_run_request.clone() {
                    let _ = exec_ctx
                        .event_tx
                        .send(EngineEvent::DryRunRequest(req))
                        .await;
                }
                for (name, value) in &execution.outputs {
                    vars.set_step_output(&step.step_id, name, value.clone());
                }
            }
        }
        let workflow_outputs = self.build_outputs(workflow, vars);
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
    }

    async fn execute_parallel_step(
        &self,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        cancel: &CancellationToken,
        is_timeout: &Arc<AtomicBool>,
    ) -> Result<ParallelStepExecution, RuntimeError> {
        if cancel.is_cancelled() {
            return Err(RuntimeError::new(
                RuntimeErrorKind::ExecutionCancelled,
                "execution cancelled",
            ));
        }

        // Parallel steps don't get a full ExecutionContext with event_tx because
        // they run independently. Create a minimal context for the HTTP call.
        // Events from parallel steps are collected after join and emitted by the
        // caller (execute_parallel) which has access to the real exec_ctx.
        let (tx, mut rx) = mpsc::channel(64);
        let minimal_ctx = ExecutionContext {
            event_tx: tx,
            trace_seq: AtomicU64::new(0),
            execution_event_seq: AtomicU64::new(0),
            step_attempts: Mutex::new(BTreeMap::new()),
            cancel: cancel.clone(),
            is_timeout: Arc::clone(is_timeout),
        };

        let start = Instant::now();
        let execution = self
            .execute_http_step(&minimal_ctx, workflow_id, step, vars, 0)
            .await?;
        let duration = start.elapsed();

        // Drain intra-step events so they can be replayed through the parent context.
        let mut events = Vec::new();
        rx.close();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        Ok(ParallelStepExecution {
            execution,
            duration,
            events,
        })
    }
}

#[derive(Debug, Clone)]
struct ParallelStepExecution {
    execution: StepExecution,
    duration: Duration,
    events: Vec<EngineEvent>,
}
