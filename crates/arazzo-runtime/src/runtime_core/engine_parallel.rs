use super::*;

impl Engine {
    pub(super) fn execute_parallel(
        &self,
        workflow_id: &str,
        workflow: &Workflow,
        vars: &mut VarStore,
        options: &ExecutionOptions,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let workflow_start = Instant::now();
        let levels = build_levels(workflow)?;
        for mut level in levels {
            options.check()?;
            level.sort_unstable();
            let level_vars = vars.clone();
            let mut level_results =
                Vec::<(usize, Step, Result<ParallelStepExecution, RuntimeError>)>::new();

            for idx in level.iter().copied() {
                let step = workflow.steps.get(idx).cloned().ok_or_else(|| {
                    RuntimeError::new(RuntimeErrorKind::StepNotFound, "invalid step index")
                })?;
                self.emit_before_step_event(workflow_id, &step);
            }

            std::thread::scope(|scope| -> Result<(), RuntimeError> {
                let mut handles = Vec::new();

                for idx in level.iter().copied() {
                    let step = {
                        let mut s = workflow.steps.get(idx).cloned().ok_or_else(|| {
                            RuntimeError::new(RuntimeErrorKind::StepNotFound, "invalid step index")
                        })?;
                        merge_workflow_params(&workflow.parameters, &mut s);
                        s
                    };
                    let step_vars = level_vars.clone();
                    let opts = options.clone();
                    handles.push(scope.spawn(move || {
                        let result =
                            self.execute_parallel_step(workflow_id, &step, &step_vars, &opts);
                        (idx, step, result)
                    }));
                }

                for handle in handles {
                    match handle.join() {
                        Ok(value) => level_results.push(value),
                        Err(_) => {
                            return Err(RuntimeError::new(
                                RuntimeErrorKind::ParallelThreadPanic,
                                "parallel step thread panicked",
                            ));
                        }
                    }
                }
                Ok(())
            })?;

            level_results.sort_by_key(|(idx, _, _)| *idx);
            for (_idx, step, execution_result) in level_results {
                let attempt = if self.trace_enabled {
                    self.next_attempt(workflow_id, &step.step_id)
                } else {
                    0
                };

                let execution = match execution_result {
                    Ok(execution) => execution,
                    Err(err) => {
                        if self.trace_enabled {
                            let record = self.build_step_trace_record(
                                workflow_id,
                                &step,
                                attempt,
                                Duration::ZERO,
                                &StepTraceData::default(),
                                TraceDecision::with_path(TraceDecisionPath::Error),
                                BTreeMap::new(),
                                Some(err.message.clone()),
                            );
                            self.push_trace_record(record);
                        }
                        self.emit_observer_event(ObserverEvent::WorkflowCompleted {
                            workflow_id: workflow_id.to_string(),
                            outputs: BTreeMap::new(),
                            duration: workflow_start.elapsed(),
                            error: Some(err.message.clone()),
                        });
                        return Err(err);
                    }
                };
                let duration = execution.duration;
                let execution = execution.execution;

                let outputs_for_trace = execution.outputs.clone();
                let par_status_code = execution
                    .result
                    .response
                    .as_ref()
                    .map(|r| r.status_code)
                    .unwrap_or(0);

                self.emit_after_step_event(
                    workflow_id,
                    &step,
                    par_status_code,
                    outputs_for_trace.clone(),
                    execution.result.err.clone(),
                    duration,
                );

                self.emit_step_completed_event(
                    workflow_id,
                    &step,
                    par_status_code,
                    duration,
                    outputs_for_trace.clone(),
                    execution.result.err.clone(),
                    execution.result.success,
                );

                if !execution.result.success {
                    let err = step_result_error(&step.step_id, &execution.result);
                    if self.trace_enabled {
                        let record = self.build_step_trace_record(
                            workflow_id,
                            &step,
                            attempt,
                            duration,
                            &execution.trace,
                            TraceDecision::with_path(TraceDecisionPath::Error),
                            outputs_for_trace,
                            Some(err.message.clone()),
                        );
                        self.push_trace_record(record);
                    }
                    self.emit_observer_event(ObserverEvent::WorkflowCompleted {
                        workflow_id: workflow_id.to_string(),
                        outputs: BTreeMap::new(),
                        duration: workflow_start.elapsed(),
                        error: Some(err.message.clone()),
                    });
                    return Err(err);
                }
                if let Some(req) = execution.dry_run_request.clone() {
                    if let Ok(mut guard) = self.dry_run_reqs.lock() {
                        guard.push(req);
                    }
                }
                for (name, value) in &execution.outputs {
                    vars.set_step_output(&step.step_id, name, value.clone());
                }
                if self.trace_enabled {
                    let record = self.build_step_trace_record(
                        workflow_id,
                        &step,
                        attempt,
                        duration,
                        &execution.trace,
                        TraceDecision::with_path(TraceDecisionPath::Next),
                        outputs_for_trace,
                        execution.result.err.clone(),
                    );
                    self.push_trace_record(record);
                }
            }
        }
        let workflow_outputs = self.build_outputs(workflow, vars);
        self.emit_observer_event(ObserverEvent::WorkflowCompleted {
            workflow_id: workflow_id.to_string(),
            outputs: workflow_outputs.clone(),
            duration: workflow_start.elapsed(),
            error: None,
        });
        Ok(workflow_outputs)
    }

    fn execute_parallel_step(
        &self,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        options: &ExecutionOptions,
    ) -> Result<ParallelStepExecution, RuntimeError> {
        options.check()?;

        let start = Instant::now();
        let execution = self.execute_http_step(workflow_id, step, vars, 0, options)?;
        let duration = start.elapsed();

        Ok(ParallelStepExecution {
            execution,
            duration,
        })
    }
}

#[derive(Debug, Clone)]
struct ParallelStepExecution {
    execution: StepExecution,
    duration: Duration,
}
