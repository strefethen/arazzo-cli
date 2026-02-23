use super::*;

impl Engine {
    pub fn new(spec: ArazzoSpec) -> Result<Self, RuntimeError> {
        Self::with_client_config(spec, ClientConfig::default())
    }

    pub fn with_client_config(
        spec: ArazzoSpec,
        config: ClientConfig,
    ) -> Result<Self, RuntimeError> {
        let client = HttpClient::new(&config)?;
        let base_url = spec
            .source_descriptions
            .first()
            .map(|s| s.url.clone())
            .unwrap_or_default();

        let mut workflow_index = BTreeMap::new();
        let mut step_indexes = BTreeMap::new();
        for (wf_idx, wf) in spec.workflows.iter().enumerate() {
            workflow_index.insert(wf.workflow_id.clone(), wf_idx);
            let mut step_idx_map = BTreeMap::new();
            for (step_idx, step) in wf.steps.iter().enumerate() {
                step_idx_map.insert(step.step_id.clone(), step_idx);
            }
            step_indexes.insert(wf.workflow_id.clone(), step_idx_map);
        }

        Ok(Self {
            client,
            spec,
            base_url,
            workflow_index,
            step_indexes,
            op_index: BTreeMap::new(),
            parallel_mode: false,
            dry_run_mode: false,
            trace_enabled: false,
            dry_run_reqs: Arc::new(Mutex::new(Vec::new())),
            trace_steps: Arc::new(Mutex::new(Vec::new())),
            trace_seq: Arc::new(Mutex::new(0)),
            execution_events: Arc::new(Mutex::new(Vec::new())),
            execution_event_seq: Arc::new(Mutex::new(0)),
            step_attempts: Arc::new(Mutex::new(BTreeMap::new())),
            trace_hook: None,
            debug_controller: None,
        })
    }

    pub fn set_trace_hook(&mut self, hook: Arc<dyn TraceHook>) {
        self.trace_hook = Some(hook);
    }

    pub fn set_debug_controller(&mut self, controller: Arc<DebugController>) {
        self.debug_controller = Some(controller);
    }

    pub fn set_parallel_mode(&mut self, enabled: bool) {
        self.parallel_mode = enabled;
    }

    pub fn set_dry_run_mode(&mut self, enabled: bool) {
        self.dry_run_mode = enabled;
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.trace_enabled = enabled;
    }

    pub fn dry_run_requests(&self) -> Vec<DryRunRequest> {
        match self.dry_run_reqs.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    pub fn trace_steps(&self) -> Vec<TraceStepRecord> {
        match self.trace_steps.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    pub fn execution_events(&self) -> Vec<ExecutionEvent> {
        match self.execution_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    pub fn spec(&self) -> &ArazzoSpec {
        &self.spec
    }

    pub fn workflows(&self) -> Vec<String> {
        self.spec
            .workflows
            .iter()
            .map(|wf| wf.workflow_id.clone())
            .collect()
    }

    pub fn load_openapi_spec(&mut self, data: &[u8]) -> Result<(), RuntimeError> {
        let root: serde_yaml::Value = serde_yaml::from_slice(data)
            .map_err(|err| RuntimeError::unspecified(format!("parsing OpenAPI spec: {err}")))?;
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
                    .get(serde_yaml::Value::String("operationId".to_string()))
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if op_id.is_empty() {
                    continue;
                }
                self.op_index.insert(
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

    pub fn execute(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        self.execute_with_options(workflow_id, inputs, ExecutionOptions::default())
    }

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

            let step = workflow.steps[step_index].clone();
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
                        self.push_trace_record(TraceStepRecord {
                            seq: self.next_trace_seq(),
                            workflow_id: workflow_id.to_string(),
                            step_id: step.step_id.clone(),
                            attempt,
                            kind: step_kind(&step),
                            operation_path: step.operation_path.clone(),
                            workflow_id_ref: step.workflow_id.clone(),
                            duration_ms: duration_ms_u64(duration),
                            request: None,
                            response: None,
                            criteria: Vec::new(),
                            decision: TraceDecision {
                                path: TraceDecisionPath::Error,
                                ..TraceDecision::default()
                            },
                            outputs: BTreeMap::new(),
                            error: Some(err.message.clone()),
                        });
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
                self.push_trace_record(TraceStepRecord {
                    seq: self.next_trace_seq(),
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    attempt,
                    kind: step_kind(&step),
                    operation_path: step.operation_path.clone(),
                    workflow_id_ref: step.workflow_id.clone(),
                    duration_ms: duration_ms_u64(duration),
                    request: execution.trace.request.clone(),
                    response: execution.trace.response.clone(),
                    criteria: execution.trace.criteria.clone(),
                    decision: action.trace.clone(),
                    outputs: step_outputs,
                    error: trace_err,
                });
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
        self.workflow_index
            .get(workflow_id)
            .and_then(|idx| self.spec.workflows.get(*idx))
    }

    pub(crate) fn resolve_operation_id(
        &self,
        operation_id: &str,
    ) -> Result<(String, String), RuntimeError> {
        self.op_index
            .get(operation_id)
            .map(|entry| (entry.method.clone(), entry.path.clone()))
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::OperationIdNotFound,
                    format!("operationId \"{operation_id}\" not found in loaded OpenAPI specs"),
                )
            })
    }

    fn execute_step_with_result(
        &mut self,
        workflow_id: &str,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<StepExecution, RuntimeError> {
        if !step.workflow_id.is_empty() {
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

    fn execute_http_step(
        &self,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<StepExecution, RuntimeError> {
        options.check()?;
        let mut operation_path = step.operation_path.clone();
        if !step.operation_id.is_empty() && operation_path.is_empty() {
            let (method, path) = self.resolve_operation_id(&step.operation_id)?;
            operation_path = format!("{method} {path}");
        }

        let (explicit_method, op_path) = parse_method(&operation_path);
        let url = self.build_url_from_path(op_path, step, vars);

        let method = if explicit_method.is_empty() {
            if step.request_body.is_some() {
                "POST".to_string()
            } else {
                "GET".to_string()
            }
        } else {
            explicit_method.to_string()
        };

        let method_for_ctx = method.clone();

        let body_json = if let Some(req_body) = &step.request_body {
            if let Some(payload) = &req_body.payload {
                let mut ctx = vars.eval_context(None);
                ctx.method = Some(method_for_ctx.clone());
                let eval = ExpressionEvaluator::new(ctx);
                Some(resolve_payload(payload, &eval))
            } else {
                None
            }
        } else {
            None
        };
        let body = body_json
            .as_ref()
            .and_then(|value| serde_json::to_vec(value).ok());

        let mut headers = BTreeMap::new();
        if body.is_some() {
            let content_type = step
                .request_body
                .as_ref()
                .map(|rb| rb.content_type.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "application/json".to_string());
            headers.insert("Content-Type".to_string(), content_type);
        }
        let mut hdr_ctx = vars.eval_context(None);
        hdr_ctx.method = Some(method_for_ctx.clone());
        let eval = ExpressionEvaluator::new(hdr_ctx);
        let mut cookie_parts = Vec::new();
        for param in &step.parameters {
            if param.in_ == "header" {
                headers.insert(param.name.clone(), eval.interpolate_string(&param.value));
            } else if param.in_ == "cookie" {
                cookie_parts.push(format!(
                    "{}={}",
                    param.name,
                    eval.interpolate_string(&param.value)
                ));
            }
        }
        if !cookie_parts.is_empty() {
            headers.insert("Cookie".to_string(), cookie_parts.join("; "));
        }

        let trace_request = TraceRequest {
            method: method.clone(),
            url: url.clone(),
            headers: headers.clone(),
            body: body_json.clone(),
        };

        if self.dry_run_mode {
            let req = DryRunRequest {
                step_id: step.step_id.clone(),
                method: method.clone(),
                url: url.clone(),
                headers: headers.clone(),
                body: body_json.clone(),
            };
            let fake = Response {
                status_code: 200,
                headers: BTreeMap::new(),
                body: b"{}".to_vec(),
                body_json: Some(json!({})),
                content_type: "json".to_string(),
            };
            return Ok(StepExecution {
                result: StepResult {
                    success: true,
                    response: Some(fake),
                    err: None,
                },
                outputs: BTreeMap::new(),
                dry_run_request: Some(req),
                trace: StepTraceData {
                    request: Some(trace_request),
                    response: Some(TraceResponse {
                        status_code: 200,
                        content_type: "json".to_string(),
                        headers: BTreeMap::new(),
                        body_bytes: 2,
                        body_preview: Some("{}".to_string()),
                    }),
                    criteria: Vec::new(),
                },
            });
        }

        let response = self.client.request(
            RequestConfig {
                method,
                url,
                headers,
                body,
            },
            options,
        )?;

        let mut post_ctx = vars.eval_context(Some(&response));
        post_ctx.method = Some(method_for_ctx.clone());
        let eval = ExpressionEvaluator::new(post_ctx);
        let mut checkpoint_outputs = BTreeMap::<String, Value>::new();
        let mut criteria = Vec::new();
        for (index, criterion) in step.success_criteria.iter().enumerate() {
            let evaluation = evaluate_criterion_detailed(criterion, &eval, Some(&response));
            criteria.push(TraceCriterionResult {
                index,
                type_: evaluation.type_name.clone(),
                condition: evaluation.condition.clone(),
                context: evaluation.context_expr.clone(),
                result: evaluation.matched,
            });
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(&response),
                request: Some(&trace_request),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_success_criterion(&gate, index, &evaluation)?;
            if !evaluation.matched {
                let trace_response = build_trace_response(&response);
                return Ok(StepExecution {
                    result: StepResult {
                        success: false,
                        response: Some(response),
                        err: None,
                    },
                    outputs: BTreeMap::new(),
                    dry_run_request: None,
                    trace: StepTraceData {
                        request: Some(trace_request),
                        response: Some(trace_response),
                        criteria,
                    },
                });
            }
        }

        let mut outputs = BTreeMap::new();
        for (name, expr) in &step.outputs {
            let value = evaluate_output_expression(expr, &eval, Some(&response));
            outputs.insert(name.clone(), value.clone());
            checkpoint_outputs.insert(name.clone(), value);
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(&response),
                request: Some(&trace_request),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_output(&gate, name, expr)?;
        }

        let trace_response = build_trace_response(&response);
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(response),
                err: None,
            },
            outputs,
            dry_run_request: None,
            trace: StepTraceData {
                request: Some(trace_request),
                response: Some(trace_response),
                criteria,
            },
        })
    }

    fn execute_subworkflow_step(
        &mut self,
        step: &Step,
        vars: &mut VarStore,
        depth: usize,
        options: &ExecutionOptions,
    ) -> Result<StepResult, RuntimeError> {
        options.check()?;
        let eval = ExpressionEvaluator::new(vars.eval_context(None));
        let mut sub_inputs = BTreeMap::new();
        for param in &step.parameters {
            let value = if param.value.contains("{$") {
                Value::String(eval.interpolate_string(&param.value))
            } else {
                eval.evaluate(&param.value)
            };
            sub_inputs.insert(param.name.clone(), value);
        }

        let outputs = self
            .execute_inner(&step.workflow_id, sub_inputs, depth + 1, options)
            .map_err(|err| {
                RuntimeError::unspecified(format!(
                    "sub-workflow {}: {}",
                    step.workflow_id, err.message
                ))
            })?;

        for (name, value) in outputs {
            vars.set_step_output(&step.step_id, &name, value);
        }

        let eval_post = ExpressionEvaluator::new(vars.eval_context(None));
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

    fn handle_step_result(&self, ctx: StepDecisionContext<'_>) -> RoutedDecision {
        let step = &ctx.workflow.steps[ctx.step_idx];

        if ctx.result.success {
            let action = match self.find_matching_action_with_debug(
                ActionSelectionContext {
                    workflow_id: ctx.workflow_id,
                    step,
                    branch: ActionBranch::Success,
                    vars: ctx.vars,
                    response: ctx.result.response.as_ref(),
                    depth: ctx.depth,
                },
                &step.on_success,
            ) {
                Ok(action) => action,
                Err(err) => return RoutedDecision::error(err),
            };
            if let Some(action) = action {
                return self.execute_action(
                    ExecuteActionContext {
                        workflow: ctx.workflow,
                        current_idx: ctx.step_idx,
                        is_failure_path: false,
                        retry_count: ctx.retry_count,
                        options: ctx.options,
                    },
                    action.action,
                    Some(SelectedActionDebugContext {
                        workflow_id: ctx.workflow_id,
                        step,
                        vars: ctx.vars,
                        response: ctx.result.response.as_ref(),
                        depth: ctx.depth,
                        branch: ActionBranch::Success,
                        action_index: action.index,
                    }),
                );
            }
            return RoutedDecision {
                flow: FlowDecision::Next(ctx.step_idx + 1),
                trace: TraceDecision {
                    path: TraceDecisionPath::Next,
                    ..TraceDecision::default()
                },
            };
        }

        let action = match self.find_matching_action_with_debug(
            ActionSelectionContext {
                workflow_id: ctx.workflow_id,
                step,
                branch: ActionBranch::Failure,
                vars: ctx.vars,
                response: ctx.result.response.as_ref(),
                depth: ctx.depth,
            },
            &step.on_failure,
        ) {
            Ok(action) => action,
            Err(err) => return RoutedDecision::error(err),
        };
        if let Some(action) = action {
            return self.execute_action(
                ExecuteActionContext {
                    workflow: ctx.workflow,
                    current_idx: ctx.step_idx,
                    is_failure_path: true,
                    retry_count: ctx.retry_count,
                    options: ctx.options,
                },
                action.action,
                Some(SelectedActionDebugContext {
                    workflow_id: ctx.workflow_id,
                    step,
                    vars: ctx.vars,
                    response: ctx.result.response.as_ref(),
                    depth: ctx.depth,
                    branch: ActionBranch::Failure,
                    action_index: action.index,
                }),
            );
        }

        RoutedDecision::error(step_result_error(&step.step_id, ctx.result))
    }

    #[cfg(test)]
    pub(crate) fn find_matching_action<'a>(
        &self,
        actions: &'a [OnAction],
        vars: &VarStore,
        response: Option<&Response>,
    ) -> Option<&'a OnAction> {
        let eval = ExpressionEvaluator::new(vars.eval_context(response));
        for action in actions {
            if action.criteria.is_empty() {
                return Some(action);
            }
            let mut all_match = true;
            for criterion in &action.criteria {
                if !evaluate_criterion(criterion, &eval, response) {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Some(action);
            }
        }
        None
    }

    fn find_matching_action_with_debug<'a>(
        &self,
        ctx: ActionSelectionContext<'_>,
        actions: &'a [OnAction],
    ) -> Result<Option<MatchedActionRef<'a>>, RuntimeError> {
        let eval = ExpressionEvaluator::new(ctx.vars.eval_context(ctx.response));
        let current_outputs = ctx.vars.step_outputs(&ctx.step.step_id);
        let gate = DebugGateContext {
            workflow_id: ctx.workflow_id,
            step_id: &ctx.step.step_id,
            vars: ctx.vars,
            response: ctx.response,
            request: None,
            current_outputs: &current_outputs,
            depth: ctx.depth,
        };

        for (action_index, action) in actions.iter().enumerate() {
            self.debug_gate_action(&gate, ctx.branch, action_index, action)?;
            if action.criteria.is_empty() {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }

            let mut all_match = true;
            for (criterion_index, criterion) in action.criteria.iter().enumerate() {
                let evaluation = evaluate_criterion_detailed(criterion, &eval, ctx.response);
                self.debug_gate_action_criterion(
                    &gate,
                    ctx.branch,
                    action_index,
                    criterion_index,
                    &evaluation,
                )?;
                if !evaluation.matched {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Ok(Some(MatchedActionRef {
                    index: action_index,
                    action,
                }));
            }
        }
        Ok(None)
    }

    fn execute_action(
        &self,
        ctx: ExecuteActionContext<'_>,
        action: &OnAction,
        debug_ctx: Option<SelectedActionDebugContext<'_>>,
    ) -> RoutedDecision {
        match action.type_.as_str() {
            "end" => {
                if ctx.is_failure_path {
                    RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::unspecified(format!(
                            "step {}: workflow ended by onFailure action",
                            ctx.workflow.steps[ctx.current_idx].step_id
                        ))),
                        trace: TraceDecision {
                            path: TraceDecisionPath::Done,
                            action_type: "end".to_string(),
                            ..TraceDecision::default()
                        },
                    }
                } else {
                    RoutedDecision {
                        flow: FlowDecision::Done,
                        trace: TraceDecision {
                            path: TraceDecisionPath::Done,
                            action_type: "end".to_string(),
                            ..TraceDecision::default()
                        },
                    }
                }
            }
            "goto" => {
                if !action.step_id.is_empty() {
                    if let Some(idx) = self.find_step_index(ctx.workflow, &action.step_id) {
                        return RoutedDecision {
                            flow: FlowDecision::Next(idx),
                            trace: TraceDecision {
                                path: TraceDecisionPath::GotoStep,
                                action_type: "goto".to_string(),
                                target_step_id: action.step_id.clone(),
                                ..TraceDecision::default()
                            },
                        };
                    }
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::GotoTargetNotFound,
                            format!("goto: step \"{}\" not found", action.step_id),
                        )),
                        trace: TraceDecision {
                            path: TraceDecisionPath::Error,
                            action_type: "goto".to_string(),
                            target_step_id: action.step_id.clone(),
                            ..TraceDecision::default()
                        },
                    };
                }
                if !action.workflow_id.is_empty() {
                    return RoutedDecision {
                        flow: FlowDecision::GotoWorkflow(action.workflow_id.clone()),
                        trace: TraceDecision {
                            path: TraceDecisionPath::GotoWorkflow,
                            action_type: "goto".to_string(),
                            target_workflow_id: action.workflow_id.clone(),
                            ..TraceDecision::default()
                        },
                    };
                }
                RoutedDecision {
                    flow: FlowDecision::Error(RuntimeError::new(
                        RuntimeErrorKind::GotoTargetMissing,
                        "goto: no stepId or workflowId specified",
                    )),
                    trace: TraceDecision {
                        path: TraceDecisionPath::Error,
                        action_type: "goto".to_string(),
                        ..TraceDecision::default()
                    },
                }
            }
            "retry" => {
                let mut limit = MAX_RETRIES_PER_STEP;
                if action.retry_limit > 0 {
                    limit = usize::try_from(action.retry_limit).unwrap_or(MAX_RETRIES_PER_STEP);
                }
                let current = ctx.retry_count.get(&ctx.current_idx).copied().unwrap_or(0);
                let will_execute_retry = current < limit;
                if let Some(debug) = debug_ctx {
                    if let Err(err) = self.debug_gate_retry_selected(
                        debug,
                        action,
                        current,
                        limit,
                        will_execute_retry,
                    ) {
                        return RoutedDecision::error(err);
                    }
                }
                if current >= limit {
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::RetryLimitExceeded,
                            format!(
                                "step {}: max retries ({limit}) exceeded",
                                ctx.workflow.steps[ctx.current_idx].step_id
                            ),
                        )),
                        trace: TraceDecision {
                            path: TraceDecisionPath::Error,
                            action_type: "retry".to_string(),
                            retry_after_seconds: Some(action.retry_after),
                            retry_limit: Some(action.retry_limit),
                            ..TraceDecision::default()
                        },
                    };
                }
                if action.retry_after > 0 {
                    if let Some(debug) = debug_ctx {
                        if let Err(err) = self.debug_gate_retry_delay(debug, action, current, limit)
                        {
                            return RoutedDecision::error(err);
                        }
                    }
                    if let Err(err) = sleep_with_checks(
                        Duration::from_secs(u64::try_from(action.retry_after).unwrap_or(0)),
                        ctx.options,
                    ) {
                        return RoutedDecision {
                            flow: FlowDecision::Error(err),
                            trace: TraceDecision {
                                path: TraceDecisionPath::Error,
                                action_type: "retry".to_string(),
                                retry_after_seconds: Some(action.retry_after),
                                retry_limit: Some(action.retry_limit),
                                ..TraceDecision::default()
                            },
                        };
                    }
                }
                RoutedDecision {
                    flow: FlowDecision::Retry(ctx.current_idx),
                    trace: TraceDecision {
                        path: TraceDecisionPath::Retry,
                        action_type: "retry".to_string(),
                        retry_after_seconds: Some(action.retry_after),
                        retry_limit: Some(action.retry_limit),
                        ..TraceDecision::default()
                    },
                }
            }
            _ => RoutedDecision {
                flow: FlowDecision::Next(ctx.current_idx + 1),
                trace: TraceDecision {
                    path: TraceDecisionPath::Next,
                    action_type: action.type_.clone(),
                    ..TraceDecision::default()
                },
            },
        }
    }

    fn find_step_index(&self, workflow: &Workflow, step_id: &str) -> Option<usize> {
        self.step_indexes
            .get(&workflow.workflow_id)
            .and_then(|index| index.get(step_id).copied())
    }

    pub(crate) fn build_outputs(
        &self,
        workflow: &Workflow,
        vars: &VarStore,
    ) -> BTreeMap<String, Value> {
        let eval = ExpressionEvaluator::new(vars.eval_context(None));
        let mut outputs = BTreeMap::new();
        for (name, expr) in &workflow.outputs {
            outputs.insert(name.clone(), eval.evaluate(expr));
        }
        outputs
    }

    pub(crate) fn build_url_from_path(
        &self,
        op_path: &str,
        step: &Step,
        vars: &VarStore,
    ) -> String {
        let mut target = if op_path.starts_with("http://") || op_path.starts_with("https://") {
            op_path.to_string()
        } else {
            format!("{}{}", self.base_url.trim_end_matches('/'), op_path)
        };

        let eval = ExpressionEvaluator::new(vars.eval_context(None));
        let mut path_params = BTreeMap::<String, String>::new();
        let mut query_params = Vec::<(String, String)>::new();

        for param in &step.parameters {
            let value = if param.value.contains("{$") {
                Value::String(eval.interpolate_string(&param.value))
            } else {
                eval.evaluate(&param.value)
            };
            match param.in_.as_str() {
                "path" => {
                    path_params.insert(param.name.clone(), value_to_string(&value));
                }
                "query" => {
                    if !value.is_null() {
                        query_params.push((param.name.clone(), value_to_string(&value)));
                    }
                }
                _ => {}
            }
        }

        if !path_params.is_empty() && target.contains('{') {
            target = replace_path_params(&target, &path_params);
        }
        if !query_params.is_empty() {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in query_params {
                serializer.append_pair(&k, &v);
            }
            let query = serializer.finish();
            if target.contains('?') {
                target.push('&');
                target.push_str(&query);
            } else {
                target.push('?');
                target.push_str(&query);
            }
        }
        target
    }

    fn execute_parallel(
        &self,
        workflow_id: &str,
        workflow: &Workflow,
        vars: &mut VarStore,
        options: &ExecutionOptions,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let levels = build_levels(workflow)?;
        for mut level in levels {
            options.check()?;
            level.sort_unstable();
            let level_vars = vars.clone();
            let mut level_results =
                Vec::<(usize, Step, Result<ParallelStepExecution, RuntimeError>)>::new();

            for idx in level.iter().copied() {
                let step =
                    workflow.steps.get(idx).cloned().ok_or_else(|| {
                        RuntimeError::unspecified("invalid step index".to_string())
                    })?;
                self.emit_before_step_event(workflow_id, &step);
            }

            std::thread::scope(|scope| -> Result<(), RuntimeError> {
                let mut handles = Vec::new();

                for idx in level.iter().copied() {
                    let step = workflow.steps.get(idx).cloned().ok_or_else(|| {
                        RuntimeError::unspecified("invalid step index".to_string())
                    })?;
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
                            self.push_trace_record(TraceStepRecord {
                                seq: self.next_trace_seq(),
                                workflow_id: workflow_id.to_string(),
                                step_id: step.step_id.clone(),
                                attempt,
                                kind: step_kind(&step),
                                operation_path: step.operation_path.clone(),
                                workflow_id_ref: step.workflow_id.clone(),
                                duration_ms: 0,
                                request: None,
                                response: None,
                                criteria: Vec::new(),
                                decision: TraceDecision {
                                    path: TraceDecisionPath::Error,
                                    ..TraceDecision::default()
                                },
                                outputs: BTreeMap::new(),
                                error: Some(err.message.clone()),
                            });
                        }
                        return Err(err);
                    }
                };
                let duration = execution.duration;
                let execution = execution.execution;

                let outputs_for_trace = execution.outputs.clone();
                self.emit_after_step_event(
                    workflow_id,
                    &step,
                    execution
                        .result
                        .response
                        .as_ref()
                        .map(|r| r.status_code)
                        .unwrap_or(0),
                    outputs_for_trace.clone(),
                    execution.result.err.clone(),
                    duration,
                );

                if !execution.result.success {
                    let err = step_result_error(&step.step_id, &execution.result);
                    if self.trace_enabled {
                        self.push_trace_record(TraceStepRecord {
                            seq: self.next_trace_seq(),
                            workflow_id: workflow_id.to_string(),
                            step_id: step.step_id.clone(),
                            attempt,
                            kind: step_kind(&step),
                            operation_path: step.operation_path.clone(),
                            workflow_id_ref: step.workflow_id.clone(),
                            duration_ms: duration_ms_u64(duration),
                            request: execution.trace.request.clone(),
                            response: execution.trace.response.clone(),
                            criteria: execution.trace.criteria.clone(),
                            decision: TraceDecision {
                                path: TraceDecisionPath::Error,
                                ..TraceDecision::default()
                            },
                            outputs: outputs_for_trace,
                            error: Some(err.message.clone()),
                        });
                    }
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
                    self.push_trace_record(TraceStepRecord {
                        seq: self.next_trace_seq(),
                        workflow_id: workflow_id.to_string(),
                        step_id: step.step_id.clone(),
                        attempt,
                        kind: step_kind(&step),
                        operation_path: step.operation_path.clone(),
                        workflow_id_ref: step.workflow_id.clone(),
                        duration_ms: duration_ms_u64(duration),
                        request: execution.trace.request.clone(),
                        response: execution.trace.response.clone(),
                        criteria: execution.trace.criteria.clone(),
                        decision: TraceDecision {
                            path: TraceDecisionPath::Next,
                            ..TraceDecision::default()
                        },
                        outputs: outputs_for_trace,
                        error: execution.result.err.clone(),
                    });
                }
            }
        }
        Ok(self.build_outputs(workflow, vars))
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

    fn emit_before_step_event(&self, workflow_id: &str, step: &Step) {
        self.emit_execution_event(ExecutionEvent {
            seq: self.next_execution_event_seq(),
            kind: ExecutionEventKind::BeforeStep,
            workflow_id: workflow_id.to_string(),
            step_id: step.step_id.clone(),
            operation_path: step.operation_path.clone(),
            workflow_id_ref: step.workflow_id.clone(),
            status_code: 0,
            outputs: BTreeMap::new(),
            err: None,
            duration_ns: 0,
        });
    }

    fn debug_gate_step(
        &self,
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
    }

    fn debug_gate_success_criterion(
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
    }

    fn debug_gate_output(
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
    }

    fn debug_gate_action(
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
            Value::String(action.type_.clone()),
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
        if action.retry_limit != 0 {
            locals.insert("actionRetryLimit".to_string(), json!(action.retry_limit));
        }
        if let Some(response) = gate.response {
            insert_response_locals(&mut locals, response);
        }

        self.debug_gate_checkpoint(gate, branch.action_checkpoint(action_index), locals)
    }

    fn debug_gate_action_criterion(
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
    }

    fn debug_gate_retry_selected(
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
    }

    fn debug_gate_retry_delay(
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
    }

    fn debug_gate_checkpoint(
        &self,
        gate: &DebugGateContext<'_>,
        checkpoint: StepCheckpoint,
        locals: BTreeMap<String, Value>,
    ) -> Result<(), RuntimeError> {
        let Some(controller) = &self.debug_controller else {
            return Ok(());
        };

        let mut eval_ctx = gate.vars.eval_context(gate.response);
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

        controller
            .gate_step(
                gate.workflow_id,
                gate.step_id,
                checkpoint,
                gate.depth,
                &eval_ctx,
                scopes,
            )
            .map_err(|err| RuntimeError::unspecified(format!("debug controller: {err}")))
    }

    fn emit_after_step_event(
        &self,
        workflow_id: &str,
        step: &Step,
        status_code: i64,
        outputs: BTreeMap<String, Value>,
        err: Option<String>,
        duration: Duration,
    ) {
        self.emit_execution_event(ExecutionEvent {
            seq: self.next_execution_event_seq(),
            kind: ExecutionEventKind::AfterStep,
            workflow_id: workflow_id.to_string(),
            step_id: step.step_id.clone(),
            operation_path: step.operation_path.clone(),
            workflow_id_ref: step.workflow_id.clone(),
            status_code,
            outputs,
            err,
            duration_ns: duration_ns_u64(duration),
        });
    }

    fn emit_execution_event(&self, event: ExecutionEvent) {
        if let Ok(mut guard) = self.execution_events.lock() {
            guard.push(event.clone());
        }
        if let Some(hook) = &self.trace_hook {
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

    fn clear_trace_state(&mut self) {
        if let Ok(mut guard) = self.trace_steps.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.trace_seq.lock() {
            *guard = 0;
        }
        if let Ok(mut guard) = self.execution_events.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.execution_event_seq.lock() {
            *guard = 0;
        }
        if let Ok(mut guard) = self.step_attempts.lock() {
            guard.clear();
        }
    }

    fn next_trace_seq(&self) -> u64 {
        match self.trace_seq.lock() {
            Ok(mut guard) => {
                *guard = guard.saturating_add(1);
                *guard
            }
            Err(_) => 0,
        }
    }

    fn next_execution_event_seq(&self) -> u64 {
        match self.execution_event_seq.lock() {
            Ok(mut guard) => {
                *guard = guard.saturating_add(1);
                *guard
            }
            Err(_) => 0,
        }
    }

    fn next_attempt(&self, workflow_id: &str, step_id: &str) -> u32 {
        match self.step_attempts.lock() {
            Ok(mut guard) => {
                let key = (workflow_id.to_string(), step_id.to_string());
                let next = guard.get(&key).copied().unwrap_or(0).saturating_add(1);
                guard.insert(key, next);
                next
            }
            Err(_) => 0,
        }
    }

    fn push_trace_record(&self, record: TraceStepRecord) {
        if let Ok(mut guard) = self.trace_steps.lock() {
            guard.push(record);
        }
    }
}

#[derive(Debug, Clone)]
enum FlowDecision {
    Next(usize),
    Retry(usize),
    Done,
    GotoWorkflow(String),
    Error(RuntimeError),
}

#[derive(Debug, Clone)]
struct RoutedDecision {
    flow: FlowDecision,
    trace: TraceDecision,
}

#[derive(Debug, Clone)]
struct ParallelStepExecution {
    execution: StepExecution,
    duration: Duration,
}

struct DebugGateContext<'a> {
    workflow_id: &'a str,
    step_id: &'a str,
    vars: &'a VarStore,
    response: Option<&'a Response>,
    request: Option<&'a TraceRequest>,
    current_outputs: &'a BTreeMap<String, Value>,
    depth: usize,
}

#[derive(Debug, Clone, Copy)]
struct MatchedActionRef<'a> {
    index: usize,
    action: &'a OnAction,
}

#[derive(Debug, Clone, Copy)]
struct SelectedActionDebugContext<'a> {
    workflow_id: &'a str,
    step: &'a Step,
    vars: &'a VarStore,
    response: Option<&'a Response>,
    depth: usize,
    branch: ActionBranch,
    action_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct ActionSelectionContext<'a> {
    workflow_id: &'a str,
    step: &'a Step,
    branch: ActionBranch,
    vars: &'a VarStore,
    response: Option<&'a Response>,
    depth: usize,
}

#[derive(Debug, Clone, Copy)]
struct StepDecisionContext<'a> {
    workflow_id: &'a str,
    workflow: &'a Workflow,
    step_idx: usize,
    result: &'a StepResult,
    vars: &'a VarStore,
    depth: usize,
    retry_count: &'a BTreeMap<usize, usize>,
    options: &'a ExecutionOptions,
}

#[derive(Debug, Clone, Copy)]
struct ExecuteActionContext<'a> {
    workflow: &'a Workflow,
    current_idx: usize,
    is_failure_path: bool,
    retry_count: &'a BTreeMap<usize, usize>,
    options: &'a ExecutionOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionBranch {
    Success,
    Failure,
}

impl ActionBranch {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "onSuccess",
            Self::Failure => "onFailure",
        }
    }

    fn action_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessAction {
                index: action_index,
            },
            Self::Failure => StepCheckpoint::OnFailureAction {
                index: action_index,
            },
        }
    }

    fn criterion_checkpoint(self, action_index: usize, criterion_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessCriterion {
                action_index,
                criterion_index,
            },
            Self::Failure => StepCheckpoint::OnFailureCriterion {
                action_index,
                criterion_index,
            },
        }
    }

    fn retry_selected_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessRetrySelected { action_index },
            Self::Failure => StepCheckpoint::OnFailureRetrySelected { action_index },
        }
    }

    fn retry_delay_checkpoint(self, action_index: usize) -> StepCheckpoint {
        match self {
            Self::Success => StepCheckpoint::OnSuccessRetryDelay { action_index },
            Self::Failure => StepCheckpoint::OnFailureRetryDelay { action_index },
        }
    }
}

impl RoutedDecision {
    fn error(err: RuntimeError) -> Self {
        Self {
            flow: FlowDecision::Error(err),
            trace: TraceDecision {
                path: TraceDecisionPath::Error,
                ..TraceDecision::default()
            },
        }
    }
}

fn duration_ms_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn duration_ns_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn step_kind(step: &Step) -> String {
    if step.workflow_id.is_empty() {
        "http".to_string()
    } else {
        "workflow".to_string()
    }
}

fn build_trace_response(response: &Response) -> TraceResponse {
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

fn insert_action_branch_locals(
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

fn insert_retry_locals(
    locals: &mut BTreeMap<String, Value>,
    stage: &str,
    current_retry_count: usize,
    retry_limit_resolved: usize,
    retry_after: i64,
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

fn insert_response_locals(locals: &mut BTreeMap<String, Value>, response: &Response) {
    locals.insert(
        "responseStatusCode".to_string(),
        Value::Number(response.status_code.into()),
    );
    locals.insert(
        "responseContentType".to_string(),
        Value::String(response.content_type.clone()),
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
