use super::step_attempt::{count_retry, settle, RouteScope, ScheduledRetry, SettledAttempt};
use super::*;

impl Engine {
    /// Why parallel mode runs this invocation of `workflow` sequentially, or
    /// `None` when its steps run in dependency levels.
    pub(super) fn sequential_fallback(&self, workflow: &Workflow) -> Option<SequentialFallback> {
        if self.inner.debug_controller.is_some() {
            return Some(SequentialFallback::new(
                &workflow.workflow_id,
                SequentialFallbackReason::Debugger,
                "",
                "a debug controller is attached",
            ));
        }
        let blocker = parallel_blocker(workflow)?;
        Some(SequentialFallback::new(
            &workflow.workflow_id,
            blocker.reason,
            &blocker.step_id,
            &blocker.detail,
        ))
    }

    pub(super) async fn push_sequential_fallback(
        ctx: &ExecutionContext,
        fallback: SequentialFallback,
    ) {
        let _ = ctx
            .event_tx
            .send(EngineEvent::SequentialFallback(fallback))
            .await;
    }

    pub(super) async fn execute_parallel(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        workflow: &Workflow,
        vars: &mut VarStore,
        depth: usize,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let workflow_start = Instant::now();
        let levels = build_levels(workflow)?;
        // Each step's task routes its own failures through the workflow's actions.
        let shared_workflow = Arc::new(workflow.clone());
        for mut level in levels {
            exec_ctx.check_cancelled()?;
            level.sort_unstable();
            let level_vars = vars.clone();

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
                let workflow = Arc::clone(&shared_workflow);
                let step_vars = level_vars.clone();
                let wf_id = workflow_id.to_string();
                let cancel = exec_ctx.cancel.clone();
                let is_timeout = Arc::clone(&exec_ctx.is_timeout);
                join_set.spawn(async move {
                    let run = engine
                        .execute_parallel_step(ParallelStepContext {
                            workflow_id: &wf_id,
                            workflow: &workflow,
                            step_idx: idx,
                            step: &step,
                            vars: &step_vars,
                            depth,
                            cancel: &cancel,
                            is_timeout: &is_timeout,
                        })
                        .await;
                    (idx, step, run)
                });
            }

            let mut level_results = Vec::<(usize, Step, ParallelStepRun)>::new();
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
            // Every step in the level has already run and sent its requests, so
            // each one is recorded even after an earlier step ends the workflow;
            // the lowest-index outcome decides it once the level is recorded.
            let mut terminal = None;
            for (_, step, run) in level_results {
                let (mut execution, flow) = self
                    .replay_parallel_step(exec_ctx, workflow_id, &step, run)
                    .await;
                if let Some(req) = execution.dry_run_request.take() {
                    let _ = exec_ctx
                        .event_tx
                        .send(EngineEvent::DryRunRequest(req))
                        .await;
                }
                if terminal.is_some() {
                    continue;
                }
                match flow {
                    FlowDecision::Next(_) | FlowDecision::Done => {
                        for (name, value) in execution.outputs {
                            vars.set_step_output(&step.step_id, &name, value);
                        }
                        if matches!(flow, FlowDecision::Done) {
                            terminal = Some(Ok(self.build_outputs(workflow, vars)));
                        }
                    }
                    FlowDecision::Error(err) => terminal = Some(Err(err)),
                    FlowDecision::Retry { .. } | FlowDecision::GotoWorkflow { .. } => {
                        // execute_parallel_step consumes plain retries and turns
                        // anything else into an error before it returns.
                        terminal = Some(Err(RuntimeError::new(
                            RuntimeErrorKind::InternalError,
                            "parallel execution does not support Retry/GotoWorkflow flow decisions",
                        )));
                    }
                }
            }
            if let Some(outcome) = terminal {
                // A cancelled invocation did not complete, whatever its
                // level's steps settled.
                exec_ctx.check_cancelled()?;
                let (outputs, error) = match &outcome {
                    Ok(outputs) => (outputs.clone(), None),
                    Err(err) => (BTreeMap::new(), Some(err.message.clone())),
                };
                self.emit_observer_event(
                    exec_ctx,
                    ObserverEvent::WorkflowCompleted {
                        workflow_id: workflow_id.to_string(),
                        outputs,
                        duration: workflow_start.elapsed(),
                        error,
                    },
                )
                .await;
                return outcome;
            }
        }
        exec_ctx.check_cancelled()?;
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

    /// Replays one step's attempts into the invocation's streams in the order
    /// sequential execution produces them, then returns the settling attempt's
    /// execution and the step's routing outcome.
    async fn replay_parallel_step(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        run: ParallelStepRun,
    ) -> (StepExecution, FlowDecision) {
        let ParallelStepRun {
            retried,
            last,
            flow,
        } = run;
        // The level announced each step's first attempt before its task started.
        let mut announce = false;
        for (attempt, retry) in retried {
            self.replay_parallel_attempt(exec_ctx, workflow_id, step, attempt, announce)
                .await;
            self.emit_retry_scheduled(exec_ctx, workflow_id, &step.step_id, retry)
                .await;
            announce = true;
        }
        let last = self
            .replay_parallel_attempt(exec_ctx, workflow_id, step, last, announce)
            .await;
        (last.execution, flow)
    }

    /// Replays one attempt: announces it when it retries the step, forwards
    /// the events it buffered, and records it.
    async fn replay_parallel_attempt(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        attempt: ParallelAttempt,
        announce: bool,
    ) -> SettledAttempt {
        if announce {
            self.emit_before_step_event(exec_ctx, workflow_id, step)
                .await;
        }
        let ParallelAttempt { events, settled } = attempt;

        // Replay intra-step events through the parent context. Each observer
        // event reaches the observer as it enters the invocation's stream.
        for event in events {
            match event {
                EngineEvent::Observer(event) => self.emit_observer_event(exec_ctx, event).await,
                other => {
                    let _ = exec_ctx.event_tx.send(other).await;
                }
            }
        }

        self.record_settled(exec_ctx, workflow_id, step, &settled)
            .await;
        settled
    }

    /// Runs one step of a parallel level to its routing outcome.
    ///
    /// A failure routed to a plain `retry` action is retried here, after the
    /// action's delay, so one step's retries never hold up its siblings.
    /// Routing reads the level's snapshot of `vars`; `build_levels` puts every
    /// step whose outputs a routing criterion reads in an earlier level.
    async fn execute_parallel_step(&self, ctx: ParallelStepContext<'_>) -> ParallelStepRun {
        let mut retried = Vec::new();
        // Retry budgets are keyed by (step, action index), so this step's own
        // map counts exactly what the sequential engine's shared map would.
        let mut retry_count = BTreeMap::<RetrySite, u64>::new();
        loop {
            let (execution, duration, events) = self.execute_parallel_attempt(&ctx).await;
            let scope = RouteScope {
                workflow_id: ctx.workflow_id,
                workflow: ctx.workflow,
                step_idx: ctx.step_idx,
                vars: ctx.vars,
                depth: ctx.depth,
                retry_count: &retry_count,
                cancel: ctx.cancel,
                is_timeout: ctx.is_timeout,
            };
            let mut routed = self.route_attempt(&scope, &execution).await;
            if matches!(
                routed.flow,
                FlowDecision::Retry {
                    reference: Some(_),
                    ..
                } | FlowDecision::GotoWorkflow { .. }
            ) {
                // parallel_blocker keeps both out of parallel levels; fail
                // loudly if that guard is ever relaxed rather than skip one.
                routed = engine_actions::RoutedDecision::error(RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    "parallel execution does not support Retry/GotoWorkflow flow decisions",
                ));
            }
            let (settled, flow) = SettledAttempt::new(execution, duration, routed);
            let attempt = ParallelAttempt { events, settled };
            match flow {
                FlowDecision::Retry {
                    retry_site,
                    retry_limit,
                    delay_seconds,
                    ..
                } => {
                    let retry =
                        count_retry(&mut retry_count, retry_site, retry_limit, delay_seconds);
                    retried.push((attempt, retry));
                }
                flow => {
                    return ParallelStepRun {
                        retried,
                        last: attempt,
                        flow,
                    };
                }
            }
        }
    }

    /// Runs one attempt against a private event buffer and settles it: the
    /// invocation's stream, and the observer with it, receives the attempt's
    /// events in step order once the level ends.
    async fn execute_parallel_attempt(
        &self,
        ctx: &ParallelStepContext<'_>,
    ) -> (StepExecution, Duration, Vec<EngineEvent>) {
        // Parallel steps run independently, so the attempt sends its events to
        // a buffer of its own rather than the invocation's stream.
        let (tx, mut rx) = mpsc::channel(64);
        let minimal_ctx = ExecutionContext {
            event_tx: tx,
            role: ContextRole::AttemptBuffer,
            trace_seq: AtomicU64::new(0),
            execution_event_seq: AtomicU64::new(0),
            step_attempts: Mutex::new(BTreeMap::new()),
            cancel: ctx.cancel.clone(),
            is_timeout: Arc::clone(ctx.is_timeout),
            completed_workflows: Mutex::new(BTreeSet::new()),
        };

        let start = Instant::now();
        let execution =
            self.execute_http_step(&minimal_ctx, ctx.workflow_id, ctx.step, ctx.vars, ctx.depth);
        tokio::pin!(execution);
        let mut events = Vec::new();
        let execution = loop {
            tokio::select! {
                biased;
                result = &mut execution => break result,
                Some(event) = rx.recv() => events.push(event),
            }
        };
        let duration = start.elapsed();

        // Stop accepting new events after the producer finishes, then retain every
        // buffered event for source-index-stable replay by the parent context.
        rx.close();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (settle(execution), duration, events)
    }
}

/// Borrowed inputs of one step's task in a parallel level.
struct ParallelStepContext<'a> {
    workflow_id: &'a str,
    workflow: &'a Workflow,
    step_idx: usize,
    /// The step with workflow parameters merged in.
    step: &'a Step,
    /// Outputs of every earlier level.
    vars: &'a VarStore,
    /// The invocation's call depth.
    depth: usize,
    cancel: &'a CancellationToken,
    is_timeout: &'a Arc<AtomicBool>,
}

/// Everything one step did inside its task, kept for in-order replay.
#[derive(Debug)]
struct ParallelStepRun {
    /// Attempts whose failure scheduled a retry, oldest first.
    retried: Vec<(ParallelAttempt, ScheduledRetry)>,
    /// The attempt that settled the step.
    last: ParallelAttempt,
    /// `last`'s routing outcome; never a retry.
    flow: FlowDecision,
}

/// One attempt as its task left it: the events it buffered, beside how it
/// settled and was routed.
#[derive(Debug)]
struct ParallelAttempt {
    /// Events the attempt emitted while it ran.
    events: Vec<EngineEvent>,
    settled: SettledAttempt,
}
