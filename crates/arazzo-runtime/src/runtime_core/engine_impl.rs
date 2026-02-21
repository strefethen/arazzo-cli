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
            dry_run_reqs: Arc::new(Mutex::new(Vec::new())),
            trace_hook: None,
        })
    }

    pub fn set_trace_hook(&mut self, hook: Arc<dyn TraceHook>) {
        self.trace_hook = Some(hook);
    }

    pub fn set_parallel_mode(&mut self, enabled: bool) {
        self.parallel_mode = enabled;
    }

    pub fn set_dry_run_mode(&mut self, enabled: bool) {
        self.dry_run_mode = enabled;
    }

    pub fn dry_run_requests(&self) -> Vec<DryRunRequest> {
        match self.dry_run_reqs.lock() {
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

        if self.dry_run_mode {
            if let Ok(mut guard) = self.dry_run_reqs.lock() {
                guard.clear();
            }
        }

        if self.parallel_mode && can_execute_parallel(&workflow) {
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

            if let Some(hook) = &self.trace_hook {
                hook.before_step(&StepEvent {
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    operation_path: step.operation_path.clone(),
                    workflow_id_ref: step.workflow_id.clone(),
                    ..StepEvent::default()
                });
            }

            let start = Instant::now();
            let result = self.execute_step_with_result(&step, &mut vars, depth, options)?;
            let duration = start.elapsed();

            if let Some(hook) = &self.trace_hook {
                hook.after_step(&StepEvent {
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    operation_path: step.operation_path.clone(),
                    workflow_id_ref: step.workflow_id.clone(),
                    status_code: result.response.as_ref().map(|r| r.status_code).unwrap_or(0),
                    outputs: vars.step_outputs(&step.step_id),
                    err: result.err.clone(),
                    duration,
                });
            }

            let action = self.handle_step_result(
                &workflow,
                step_index,
                &result,
                &vars,
                &mut retry_count,
                options,
            )?;

            match action {
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
    ) -> Result<StepResult, RuntimeError> {
        if !step.workflow_id.is_empty() {
            return self.execute_subworkflow_step(step, vars, depth, options);
        }

        let execution = self.execute_http_step(step, vars, options)?;
        if let Some(req) = execution.dry_run_request {
            if let Ok(mut guard) = self.dry_run_reqs.lock() {
                guard.push(req);
            }
        }
        for (name, value) in execution.outputs {
            vars.set_step_output(&step.step_id, &name, value);
        }
        Ok(execution.result)
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

        if self.dry_run_mode {
            let req = DryRunRequest {
                step_id: step.step_id.clone(),
                method,
                url,
                headers,
                body: body_json,
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
        for criterion in &step.success_criteria {
            if !evaluate_criterion(criterion, &eval, Some(&response)) {
                return Ok(StepExecution {
                    result: StepResult {
                        success: false,
                        response: Some(response),
                        err: None,
                    },
                    outputs: BTreeMap::new(),
                    dry_run_request: None,
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

        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(response),
                err: None,
            },
            outputs,
            dry_run_request: None,
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
        retry_count: &mut BTreeMap<usize, usize>,
        options: &ExecutionOptions,
    ) -> Result<FlowDecision, RuntimeError> {
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
            return Ok(FlowDecision::Next(step_idx + 1));
        }

        let action = self.find_matching_action(&step.on_failure, vars, result.response.as_ref());
        if let Some(action) = action {
            return self.execute_action(workflow, action, step_idx, true, retry_count, options);
        }

        if let Some(err) = &result.err {
            return Err(RuntimeError::unspecified(format!(
                "step {}: {err}",
                step.step_id
            )));
        }
        if let Some(resp) = &result.response {
            let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
            if body_preview.len() > 500 {
                body_preview.truncate(500);
                body_preview.push_str("...");
            }
            return Err(RuntimeError::unspecified(format!(
                "step {}: success criteria not met (status={}, body={})",
                step.step_id, resp.status_code, body_preview
            )));
        }

        Err(RuntimeError::unspecified(format!(
            "step {}: success criteria not met",
            step.step_id
        )))
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
    ) -> Result<FlowDecision, RuntimeError> {
        match action.type_.as_str() {
            "end" => {
                if is_failure_path {
                    Err(RuntimeError::unspecified(format!(
                        "step {}: workflow ended by onFailure action",
                        workflow.steps[current_idx].step_id
                    )))
                } else {
                    Ok(FlowDecision::Done)
                }
            }
            "goto" => {
                if !action.step_id.is_empty() {
                    let idx = self
                        .find_step_index(workflow, &action.step_id)
                        .ok_or_else(|| {
                            RuntimeError::new(
                                RuntimeErrorKind::GotoTargetNotFound,
                                format!("goto: step \"{}\" not found", action.step_id),
                            )
                        })?;
                    return Ok(FlowDecision::Next(idx));
                }
                if !action.workflow_id.is_empty() {
                    return Ok(FlowDecision::GotoWorkflow(action.workflow_id.clone()));
                }
                Err(RuntimeError::new(
                    RuntimeErrorKind::GotoTargetMissing,
                    "goto: no stepId or workflowId specified",
                ))
            }
            "retry" => {
                let mut limit = MAX_RETRIES_PER_STEP;
                if action.retry_limit > 0 {
                    limit = usize::try_from(action.retry_limit).unwrap_or(MAX_RETRIES_PER_STEP);
                }
                let current = retry_count.get(&current_idx).copied().unwrap_or(0);
                if current >= limit {
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::RetryLimitExceeded,
                        format!(
                            "step {}: max retries ({limit}) exceeded",
                            workflow.steps[current_idx].step_id
                        ),
                    ));
                }
                if action.retry_after > 0 {
                    sleep_with_checks(
                        Duration::from_secs(u64::try_from(action.retry_after).unwrap_or(0)),
                        options,
                    )?;
                }
                Ok(FlowDecision::Retry(current_idx))
            }
            _ => Ok(FlowDecision::Next(current_idx + 1)),
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
        for level in levels {
            options.check()?;
            let level_vars = vars.clone();
            let mut level_results =
                Vec::<(usize, Step, Result<StepExecution, RuntimeError>)>::new();

            std::thread::scope(|scope| -> Result<(), RuntimeError> {
                let mut handles = Vec::new();

                for idx in level.iter().copied() {
                    let step = workflow.steps.get(idx).cloned().ok_or_else(|| {
                        RuntimeError::unspecified("invalid step index".to_string())
                    })?;
                    let step_vars = level_vars.clone();
                    let wf_id = workflow_id.to_string();
                    let opts = options.clone();
                    handles.push(scope.spawn(move || {
                        let result = self.execute_parallel_step(&wf_id, &step, &step_vars, &opts);
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
                let execution = execution_result?;
                if !execution.result.success {
                    return Err(step_result_error(&step.step_id, &execution.result));
                }
                if let Some(req) = execution.dry_run_request {
                    if let Ok(mut guard) = self.dry_run_reqs.lock() {
                        guard.push(req);
                    }
                }
                for (name, value) in execution.outputs {
                    vars.set_step_output(&step.step_id, &name, value);
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
    ) -> Result<StepExecution, RuntimeError> {
        options.check()?;
        if let Some(hook) = &self.trace_hook {
            hook.before_step(&StepEvent {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                operation_path: step.operation_path.clone(),
                workflow_id_ref: step.workflow_id.clone(),
                ..StepEvent::default()
            });
        }

        let start = Instant::now();
        let execution = self.execute_http_step(step, vars, options)?;
        let duration = start.elapsed();

        if let Some(hook) = &self.trace_hook {
            hook.after_step(&StepEvent {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                operation_path: step.operation_path.clone(),
                workflow_id_ref: step.workflow_id.clone(),
                status_code: execution
                    .result
                    .response
                    .as_ref()
                    .map(|r| r.status_code)
                    .unwrap_or(0),
                outputs: execution.outputs.clone(),
                err: execution.result.err.clone(),
                duration,
            });
        }
        Ok(execution)
    }
}

#[derive(Debug, Clone)]
enum FlowDecision {
    Next(usize),
    Retry(usize),
    Done,
    GotoWorkflow(String),
}
