use super::*;

impl Engine {
    pub(super) async fn emit_before_step_event(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
    ) {
        self.emit_execution_event(
            ctx,
            ExecutionEvent {
                seq: ctx
                    .execution_event_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
                kind: ExecutionEventKind::BeforeStep,
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                operation_path: step_operation_path(step),
                workflow_id_ref: step_workflow_id_ref(step),
                status_code: 0,
                outputs: BTreeMap::new(),
                err: None,
                duration_ns: 0,
            },
        )
        .await;

        self.emit_observer_event(
            ctx,
            ObserverEvent::StepStarted {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                operation_path: step_operation_path(step),
                workflow_id_ref: step_workflow_id_ref(step),
            },
        )
        .await;
    }

    pub(super) async fn debug_gate_step(
        &self,
        _ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
    ) -> Result<(), RuntimeError> {
        let empty_outputs = BTreeMap::new();
        let gate = DebugGateContext {
            workflow_id,
            step_id: &step.step_id,
            vars,
            response: None,
            request: None,
            current_outputs: &empty_outputs,
            depth,
        };
        self.debug_gate_checkpoint(&gate, StepCheckpoint::Step, BTreeMap::new())
            .await
    }

    pub(super) async fn debug_gate_success_criterion(
        &self,
        gate: &DebugGateContext<'_>,
        index: usize,
        evaluation: &CriterionEvaluation,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = gate
            .response
            .map(|response| response.status_code)
            .unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        locals.insert("criterionIndex".to_string(), json!(index));
        insert_criterion_locals(&mut locals, evaluation);
        if let Some(request) = gate.request {
            insert_request_locals(&mut locals, request);
        }
        if let Some(response) = gate.response {
            insert_response_locals(&mut locals, response);
        }

        self.debug_gate_checkpoint(gate, StepCheckpoint::SuccessCriterion { index }, locals)
            .await
    }

    pub(super) async fn debug_gate_output(
        &self,
        gate: &DebugGateContext<'_>,
        output_name: &str,
        output_expr: &str,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = gate
            .response
            .map(|response| response.status_code)
            .unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        locals.insert(
            "outputName".to_string(),
            Value::String(output_name.to_string()),
        );
        locals.insert(
            "outputExpression".to_string(),
            Value::String(output_expr.to_string()),
        );
        for (name, value) in gate.current_outputs {
            locals.insert(name.clone(), value.clone());
        }
        if let Some(request) = gate.request {
            insert_request_locals(&mut locals, request);
        }
        if let Some(response) = gate.response {
            insert_response_locals(&mut locals, response);
        }

        self.debug_gate_checkpoint(
            gate,
            StepCheckpoint::Output {
                name: output_name.to_string(),
            },
            locals,
        )
        .await
    }

    pub(super) async fn debug_gate_action(
        &self,
        gate: &DebugGateContext<'_>,
        branch: ActionBranch,
        action_index: usize,
        action: &OnAction,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = gate
            .response
            .map(|response| response.status_code)
            .unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        insert_action_branch_locals(&mut locals, branch, action_index);
        locals.insert(
            "actionType".to_string(),
            Value::String(action.type_.to_string()),
        );
        if !action.name.is_empty() {
            locals.insert("actionName".to_string(), Value::String(action.name.clone()));
        }
        if !action.step_id.is_empty() {
            locals.insert(
                "actionStepId".to_string(),
                Value::String(action.step_id.clone()),
            );
        }
        if !action.workflow_id.is_empty() {
            locals.insert(
                "actionWorkflowId".to_string(),
                Value::String(action.workflow_id.clone()),
            );
        }
        if action.retry_after != 0 {
            locals.insert("actionRetryAfter".to_string(), json!(action.retry_after));
        }
        if let Some(rl) = action.retry_limit {
            locals.insert("actionRetryLimit".to_string(), json!(rl));
        }
        if let Some(response) = gate.response {
            insert_response_locals(&mut locals, response);
        }

        self.debug_gate_checkpoint(gate, branch.action_checkpoint(action_index), locals)
            .await
    }

    pub(super) async fn debug_gate_action_criterion(
        &self,
        gate: &DebugGateContext<'_>,
        branch: ActionBranch,
        action_index: usize,
        criterion_index: usize,
        evaluation: &CriterionEvaluation,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = gate
            .response
            .map(|response| response.status_code)
            .unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        insert_action_branch_locals(&mut locals, branch, action_index);
        locals.insert("criterionIndex".to_string(), json!(criterion_index));
        insert_criterion_locals(&mut locals, evaluation);
        if let Some(response) = gate.response {
            insert_response_locals(&mut locals, response);
        }

        self.debug_gate_checkpoint(
            gate,
            branch.criterion_checkpoint(action_index, criterion_index),
            locals,
        )
        .await
    }

    pub(super) async fn debug_gate_retry_selected(
        &self,
        debug: SelectedActionDebugContext<'_>,
        action: &OnAction,
        current_retry_count: usize,
        retry_limit_resolved: usize,
        will_execute_retry: bool,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = debug.response.map(|r| r.status_code).unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        insert_action_branch_locals(&mut locals, debug.branch, debug.action_index);
        insert_retry_locals(
            &mut locals,
            "selected",
            current_retry_count,
            retry_limit_resolved,
            action.retry_after,
        );
        locals.insert("retryWillExecute".to_string(), json!(will_execute_retry));
        if let Some(response) = debug.response {
            insert_response_locals(&mut locals, response);
        }

        let current_outputs = debug.vars.step_outputs(&debug.step.step_id);
        let gate = DebugGateContext {
            workflow_id: debug.workflow_id,
            step_id: &debug.step.step_id,
            vars: debug.vars,
            response: debug.response,
            request: None,
            current_outputs: &current_outputs,
            depth: debug.depth,
        };
        self.debug_gate_checkpoint(
            &gate,
            debug.branch.retry_selected_checkpoint(debug.action_index),
            locals,
        )
        .await
    }

    pub(super) async fn debug_gate_retry_delay(
        &self,
        debug: SelectedActionDebugContext<'_>,
        action: &OnAction,
        current_retry_count: usize,
        retry_limit_resolved: usize,
    ) -> Result<(), RuntimeError> {
        let mut locals = BTreeMap::new();
        let status_code = debug.response.map(|r| r.status_code).unwrap_or(0);
        locals.insert("statusCode".to_string(), json!(status_code));
        insert_action_branch_locals(&mut locals, debug.branch, debug.action_index);
        insert_retry_locals(
            &mut locals,
            "delay",
            current_retry_count,
            retry_limit_resolved,
            action.retry_after,
        );
        if let Some(response) = debug.response {
            insert_response_locals(&mut locals, response);
        }

        let current_outputs = debug.vars.step_outputs(&debug.step.step_id);
        let gate = DebugGateContext {
            workflow_id: debug.workflow_id,
            step_id: &debug.step.step_id,
            vars: debug.vars,
            response: debug.response,
            request: None,
            current_outputs: &current_outputs,
            depth: debug.depth,
        };
        self.debug_gate_checkpoint(
            &gate,
            debug.branch.retry_delay_checkpoint(debug.action_index),
            locals,
        )
        .await
    }

    async fn debug_gate_checkpoint(
        &self,
        gate: &DebugGateContext<'_>,
        checkpoint: StepCheckpoint,
        locals: BTreeMap<String, Value>,
    ) -> Result<(), RuntimeError> {
        let Some(controller) = &self.inner.debug_controller else {
            return Ok(());
        };

        let mut eval_ctx = self.make_eval_context(gate.vars, gate.response);
        if !gate.current_outputs.is_empty() {
            let scoped_outputs = eval_ctx.steps.entry(gate.step_id.to_string()).or_default();
            for (name, value) in gate.current_outputs {
                scoped_outputs.insert(name.clone(), value.clone());
            }
        }

        let mut scopes = gate.vars.debug_scopes();
        if !gate.current_outputs.is_empty() {
            let scoped_outputs = scopes.steps.entry(gate.step_id.to_string()).or_default();
            for (name, value) in gate.current_outputs {
                scoped_outputs.insert(name.clone(), value.clone());
            }
        }
        scopes.locals = locals;

        let controller = Arc::clone(controller);
        let workflow_id = gate.workflow_id.to_string();
        let step_id = gate.step_id.to_string();
        let depth = gate.depth;

        // DebugController uses Condvar::wait(), so we bridge via spawn_blocking
        tokio::task::spawn_blocking(move || {
            controller
                .gate_step(&workflow_id, &step_id, checkpoint, depth, &eval_ctx, scopes)
                .map_err(|err| {
                    RuntimeError::new(
                        RuntimeErrorKind::DebugController,
                        format!("debug controller: {err}"),
                    )
                })
        })
        .await
        .unwrap_or_else(|_| {
            Err(RuntimeError::new(
                RuntimeErrorKind::DebugController,
                "debug controller task panicked",
            ))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn emit_after_step_event(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        status_code: i64,
        outputs: BTreeMap<String, Value>,
        err: Option<String>,
        duration: Duration,
    ) {
        self.emit_execution_event(
            ctx,
            ExecutionEvent {
                seq: ctx
                    .execution_event_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
                kind: ExecutionEventKind::AfterStep,
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                operation_path: step_operation_path(step),
                workflow_id_ref: step_workflow_id_ref(step),
                status_code,
                outputs,
                err,
                duration_ns: duration_ns_u64(duration),
            },
        )
        .await;
    }

    /// Dispatch an event to the registered observer, if any, and stream it.
    pub(super) async fn emit_observer_event(&self, ctx: &ExecutionContext, event: ObserverEvent) {
        if let Some(observer) = &self.inner.observer {
            observer.on_event(&event);
        }
        let _ = ctx.event_tx.send(EngineEvent::Observer(event)).await;
    }

    async fn emit_execution_event(&self, ctx: &ExecutionContext, event: ExecutionEvent) {
        let _ = ctx
            .event_tx
            .send(EngineEvent::Execution(event.clone()))
            .await;
        if let Some(hook) = &self.inner.trace_hook {
            let step_event = StepEvent {
                workflow_id: event.workflow_id.clone(),
                step_id: event.step_id.clone(),
                operation_path: event.operation_path.clone(),
                workflow_id_ref: event.workflow_id_ref.clone(),
                status_code: event.status_code,
                outputs: event.outputs.clone(),
                err: event.err.clone(),
                duration: Duration::from_nanos(event.duration_ns),
            };
            match event.kind {
                ExecutionEventKind::BeforeStep => hook.before_step(&step_event),
                ExecutionEventKind::AfterStep => hook.after_step(&step_event),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn emit_step_completed_event(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        status_code: i64,
        duration: Duration,
        outputs: BTreeMap<String, Value>,
        error: Option<String>,
        criteria_passed: bool,
    ) {
        self.emit_observer_event(
            ctx,
            ObserverEvent::StepCompleted {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                status_code,
                duration,
                outputs,
                error,
                criteria_passed,
            },
        )
        .await;
    }

    pub(super) fn next_trace_seq(ctx: &ExecutionContext) -> u64 {
        ctx.trace_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    pub(super) fn next_attempt(ctx: &ExecutionContext, workflow_id: &str, step_id: &str) -> u32 {
        match ctx.step_attempts.lock() {
            Ok(mut guard) => {
                let key = (workflow_id.to_string(), step_id.to_string());
                let next = guard.get(&key).copied().unwrap_or(0).saturating_add(1);
                guard.insert(key, next);
                next
            }
            Err(_) => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_step_trace_record(
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        attempt: u32,
        duration: Duration,
        trace: &StepTraceData,
        decision: TraceDecision,
        outputs: BTreeMap<String, Value>,
        error: Option<String>,
    ) -> TraceStepRecord {
        TraceStepRecord {
            seq: Self::next_trace_seq(ctx),
            workflow_id: workflow_id.to_string(),
            step_id: step.step_id.clone(),
            attempt,
            kind: step_kind(step),
            operation_path: step_operation_path(step),
            workflow_id_ref: step_workflow_id_ref(step),
            duration_ms: duration_ms_u64(duration),
            request: trace.request.clone(),
            response: trace.response.clone(),
            criteria: trace.criteria.clone(),
            warnings: trace.warnings.clone(),
            decision,
            outputs,
            error,
        }
    }

    pub(super) async fn push_trace_record(ctx: &ExecutionContext, record: TraceStepRecord) {
        let _ = ctx.event_tx.send(EngineEvent::TraceStep(record)).await;
    }
}

pub(super) struct DebugGateContext<'a> {
    pub workflow_id: &'a str,
    pub step_id: &'a str,
    pub vars: &'a VarStore,
    pub response: Option<&'a Response>,
    pub request: Option<&'a TraceRequest>,
    pub current_outputs: &'a BTreeMap<String, Value>,
    pub depth: usize,
}

pub(super) fn duration_ms_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn duration_ns_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

pub(super) fn step_kind(step: &Step) -> String {
    if matches!(&step.target, Some(StepTarget::WorkflowId(_))) {
        "workflow".to_string()
    } else {
        "http".to_string()
    }
}

pub(super) fn step_operation_path(step: &Step) -> String {
    match &step.target {
        Some(StepTarget::OperationPath(p)) => p.clone(),
        Some(StepTarget::OperationId(id)) => id.clone(),
        _ => String::new(),
    }
}

pub(super) fn step_workflow_id_ref(step: &Step) -> String {
    match &step.target {
        Some(StepTarget::WorkflowId(id)) => id.clone(),
        _ => String::new(),
    }
}

pub(super) fn build_trace_response(response: &Response) -> TraceResponse {
    let body_preview = if response.body.is_empty() {
        None
    } else {
        let max = TRACE_BODY_PREVIEW_MAX_BYTES.min(response.body.len());
        let mut preview = String::from_utf8_lossy(&response.body[..max]).to_string();
        if response.body.len() > max {
            preview.push_str("...");
        }
        Some(preview)
    };

    TraceResponse {
        status_code: response.status_code,
        content_type: response.content_type.clone(),
        headers: response.headers.clone(),
        body_bytes: u64::try_from(response.body.len()).unwrap_or(u64::MAX),
        body_preview,
    }
}

fn insert_criterion_locals(locals: &mut BTreeMap<String, Value>, evaluation: &CriterionEvaluation) {
    locals.insert(
        "criterionCondition".to_string(),
        Value::String(evaluation.condition.clone()),
    );
    locals.insert(
        "criterionConditionResult".to_string(),
        json!(evaluation.condition_result),
    );
    locals.insert("criterionMatched".to_string(), json!(evaluation.matched));
    if !evaluation.context_expr.is_empty() {
        locals.insert(
            "criterionContext".to_string(),
            Value::String(evaluation.context_expr.clone()),
        );
    }
    if !evaluation.type_name.is_empty() {
        locals.insert(
            "criterionType".to_string(),
            Value::String(evaluation.type_name.clone()),
        );
    }
    if let Some(version) = &evaluation.type_version {
        locals.insert(
            "criterionTypeVersion".to_string(),
            Value::String(version.clone()),
        );
    }
    locals.insert(
        "criterionContextValue".to_string(),
        evaluation.context_value.clone(),
    );
    if let Some(error) = &evaluation.error {
        locals.insert("criterionError".to_string(), Value::String(error.clone()));
    }
}

pub(super) fn insert_action_branch_locals(
    locals: &mut BTreeMap<String, Value>,
    branch: ActionBranch,
    action_index: usize,
) {
    locals.insert(
        "actionBranch".to_string(),
        Value::String(branch.label().to_string()),
    );
    locals.insert("actionIndex".to_string(), json!(action_index));
}

pub(super) fn insert_retry_locals(
    locals: &mut BTreeMap<String, Value>,
    stage: &str,
    current_retry_count: usize,
    retry_limit_resolved: usize,
    retry_after: u64,
) {
    locals.insert("actionType".to_string(), Value::String("retry".to_string()));
    locals.insert("retryStage".to_string(), Value::String(stage.to_string()));
    locals.insert("retryCountCurrent".to_string(), json!(current_retry_count));
    locals.insert(
        "retryCountNext".to_string(),
        json!(current_retry_count.saturating_add(1)),
    );
    locals.insert(
        "retryLimitResolved".to_string(),
        json!(retry_limit_resolved),
    );
    locals.insert("retryAfterSeconds".to_string(), json!(retry_after));
}

fn insert_request_locals(locals: &mut BTreeMap<String, Value>, request: &TraceRequest) {
    locals.insert(
        "requestMethod".to_string(),
        Value::String(request.method.clone()),
    );
    locals.insert("requestUrl".to_string(), Value::String(request.url.clone()));
    locals.insert("requestHeaders".to_string(), json!(request.headers));
    if let Some(body) = &request.body {
        locals.insert("requestBody".to_string(), body.clone());
    }
}

pub(super) fn insert_response_locals(locals: &mut BTreeMap<String, Value>, response: &Response) {
    locals.insert(
        "responseStatusCode".to_string(),
        Value::Number(response.status_code.into()),
    );
    locals.insert(
        "responseContentType".to_string(),
        Value::String(response.content_type.to_string()),
    );
    locals.insert("responseHeaders".to_string(), json!(response.headers));

    if !response.body.is_empty() {
        let raw = String::from_utf8_lossy(&response.body).to_string();
        locals.insert("responseBodyRaw".to_string(), Value::String(raw.clone()));

        let max = TRACE_BODY_PREVIEW_MAX_BYTES.min(response.body.len());
        let mut preview = String::from_utf8_lossy(&response.body[..max]).to_string();
        if response.body.len() > max {
            preview.push_str("...");
        }
        locals.insert("responseBodyPreview".to_string(), Value::String(preview));
    }
}
