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
            let execution = match self.execute_step_with_result(&step, &mut vars, depth, options) {
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

            let action = self.handle_step_result(
                &workflow,
                step_index,
                &execution.result,
                &vars,
                &retry_count,
                options,
            );

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

        let execution = self.execute_http_step(step, vars, options)?;
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
        step: &Step,
        vars: &VarStore,
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

        let body_json = if let Some(req_body) = &step.request_body {
            if let Some(payload) = &req_body.payload {
                let eval = ExpressionEvaluator::new(vars.eval_context(None));
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
        let eval = ExpressionEvaluator::new(vars.eval_context(None));
        for param in &step.parameters {
            if param.in_ == "header" {
                headers.insert(param.name.clone(), eval.evaluate_string(&param.value));
            }
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

        let eval = ExpressionEvaluator::new(vars.eval_context(Some(&response)));
        let mut criteria = Vec::new();
        for (index, criterion) in step.success_criteria.iter().enumerate() {
            let matches = evaluate_criterion(criterion, &eval, Some(&response));
            criteria.push(TraceCriterionResult {
                index,
                type_: criterion.type_.clone(),
                condition: criterion.condition.clone(),
                context: criterion.context.clone(),
                result: matches,
            });
            if !matches {
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
            let value = if expr.starts_with('/') {
                extract_xpath(&response.body, expr)
            } else if expr.starts_with("$response.header.") || expr.starts_with("$statusCode") {
                eval.evaluate(expr)
            } else {
                let json_path = to_json_path(expr);
                eval.evaluate(&format!("$response.body.{json_path}"))
            };
            outputs.insert(name.clone(), value);
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
            sub_inputs.insert(param.name.clone(), eval.evaluate(&param.value));
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

    fn handle_step_result(
        &self,
        workflow: &Workflow,
        step_idx: usize,
        result: &StepResult,
        vars: &VarStore,
        retry_count: &BTreeMap<usize, usize>,
        options: &ExecutionOptions,
    ) -> RoutedDecision {
        let step = workflow.steps[step_idx].clone();

        if result.success {
            let action =
                self.find_matching_action(&step.on_success, vars, result.response.as_ref());
            if let Some(action) = action {
                return self.execute_action(
                    workflow,
                    action,
                    step_idx,
                    false,
                    retry_count,
                    options,
                );
            }
            return RoutedDecision {
                flow: FlowDecision::Next(step_idx + 1),
                trace: TraceDecision {
                    path: TraceDecisionPath::Next,
                    ..TraceDecision::default()
                },
            };
        }

        let action = self.find_matching_action(&step.on_failure, vars, result.response.as_ref());
        if let Some(action) = action {
            return self.execute_action(workflow, action, step_idx, true, retry_count, options);
        }

        RoutedDecision::error(step_result_error(&step.step_id, result))
    }

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
                if !eval.evaluate_condition(&criterion.condition) {
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

    fn execute_action(
        &self,
        workflow: &Workflow,
        action: &OnAction,
        current_idx: usize,
        is_failure_path: bool,
        retry_count: &BTreeMap<usize, usize>,
        options: &ExecutionOptions,
    ) -> RoutedDecision {
        match action.type_.as_str() {
            "end" => {
                if is_failure_path {
                    RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::unspecified(format!(
                            "step {}: workflow ended by onFailure action",
                            workflow.steps[current_idx].step_id
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
                    if let Some(idx) = self.find_step_index(workflow, &action.step_id) {
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
                let current = retry_count.get(&current_idx).copied().unwrap_or(0);
                if current >= limit {
                    return RoutedDecision {
                        flow: FlowDecision::Error(RuntimeError::new(
                            RuntimeErrorKind::RetryLimitExceeded,
                            format!(
                                "step {}: max retries ({limit}) exceeded",
                                workflow.steps[current_idx].step_id
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
                    if let Err(err) = sleep_with_checks(
                        Duration::from_secs(u64::try_from(action.retry_after).unwrap_or(0)),
                        options,
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
                    flow: FlowDecision::Retry(current_idx),
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
                flow: FlowDecision::Next(current_idx + 1),
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
            let value = eval.evaluate(&param.value);
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
                        let result = self.execute_parallel_step(&step, &step_vars, &opts);
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
        step: &Step,
        vars: &VarStore,
        options: &ExecutionOptions,
    ) -> Result<ParallelStepExecution, RuntimeError> {
        options.check()?;

        let start = Instant::now();
        let execution = self.execute_http_step(step, vars, options)?;
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
        let Some(controller) = &self.debug_controller else {
            return Ok(());
        };
        let eval_ctx = vars.eval_context(None);
        let scopes = vars.debug_scopes();
        controller
            .gate_step(workflow_id, &step.step_id, depth, &eval_ctx, scopes)
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
