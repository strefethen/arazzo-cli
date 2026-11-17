use super::*;

impl Engine {
    /// Spawns an async task that executes the workflow and returns a handle
    /// for streaming events and awaiting the final result.
    ///
    /// A Tokio runtime must be active when calling this method.
    pub fn execute(&self, workflow_id: &str, inputs: impl Into<WorkflowInputs>) -> ExecutionHandle {
        self.execute_with_completed_workflows(workflow_id, inputs, &BTreeSet::new())
    }

    /// Spawns execution with caller-provided completion evidence for local
    /// workflow dependencies. The set is copied into this invocation and is
    /// not retained by the engine after the handle is dropped.
    pub fn execute_with_completed_workflows(
        &self,
        workflow_id: &str,
        inputs: impl Into<WorkflowInputs>,
        completed_workflows: &BTreeSet<String>,
    ) -> ExecutionHandle {
        let inputs = ProvenancedValues::from(inputs.into());
        let (event_tx, event_rx) = mpsc::channel(self.inner.channel_capacity);
        let (result_tx, result_rx) = oneshot::channel();
        let cancellation = ExecutionCancellation::default();
        let completion_guard = cancellation.completion_guard();
        let cancel = cancellation.worker();
        let is_timeout = cancellation.timeout_flag();

        let engine = self.clone();
        let wf_id = workflow_id.to_string();
        let cancel_clone = cancel.clone();
        let timeout_clone = Arc::clone(&is_timeout);
        let completed_workflows = completed_workflows.clone();

        tokio::spawn(async move {
            let _completion_guard = completion_guard;
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                role: ContextRole::Invocation,
                trace_seq: AtomicU64::new(0),
                run_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
                completed_workflows: Mutex::new(completed_workflows),
                message_receivers: Arc::default(),
            });

            let result = engine
                .execute_inner(
                    &ctx,
                    &wf_id,
                    inputs,
                    0,
                    Some(_completion_guard.cancellation()),
                )
                .await
                .map(WorkflowOutputs::from);

            drop(ctx);
            let result = _completion_guard.finish(result);
            let _ = result_tx.send(result);
        });

        ExecutionHandle::new(event_rx, result_rx, cancellation)
    }

    /// Spawns execution with a timeout watchdog that cancels after `timeout`.
    pub fn execute_with_timeout(
        &self,
        workflow_id: &str,
        inputs: impl Into<WorkflowInputs>,
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
        inputs: impl Into<WorkflowInputs>,
        timeout: Duration,
        completed_workflows: &BTreeSet<String>,
    ) -> ExecutionHandle {
        let mut handle =
            self.execute_with_completed_workflows(workflow_id, inputs, completed_workflows);
        handle.start_watchdog(timeout);
        handle
    }

    /// Convenience: collects all events and the final result.
    pub async fn execute_collect(
        &self,
        workflow_id: &str,
        inputs: impl Into<WorkflowInputs>,
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
        inputs: impl Into<WorkflowInputs>,
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
        inputs: impl Into<WorkflowInputs>,
        no_deps: bool,
        completed_workflows: &BTreeSet<String>,
    ) -> ExecutionHandle {
        let inputs = ProvenancedValues::from(inputs.into());
        let (event_tx, event_rx) = mpsc::channel(self.inner.channel_capacity);
        let (result_tx, result_rx) = oneshot::channel();
        let cancellation = ExecutionCancellation::default();
        let completion_guard = cancellation.completion_guard();
        let cancel = cancellation.worker();
        let is_timeout = cancellation.timeout_flag();

        let engine = self.clone();
        let wf_id = workflow_id.to_string();
        let s_id = step_id.to_string();
        let cancel_clone = cancel.clone();
        let timeout_clone = Arc::clone(&is_timeout);
        let completed_workflows = completed_workflows.clone();

        tokio::spawn(async move {
            let _completion_guard = completion_guard;
            let ctx = Arc::new(ExecutionContext {
                event_tx,
                role: ContextRole::Invocation,
                trace_seq: AtomicU64::new(0),
                run_seq: AtomicU64::new(0),
                execution_event_seq: AtomicU64::new(0),
                step_attempts: Mutex::new(BTreeMap::new()),
                cancel: cancel_clone,
                is_timeout: timeout_clone,
                completed_workflows: Mutex::new(completed_workflows),
                message_receivers: Arc::default(),
            });

            let result = engine
                .execute_step_inner(
                    &ctx,
                    &wf_id,
                    &s_id,
                    inputs,
                    no_deps,
                    Some(_completion_guard.cancellation()),
                )
                .await
                .map(WorkflowOutputs::from);

            drop(ctx);
            let result = _completion_guard.finish(result);
            let _ = result_tx.send(result);
        });

        ExecutionHandle::new(event_rx, result_rx, cancellation)
    }
}
