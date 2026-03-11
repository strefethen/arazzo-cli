use super::*;

impl Engine {
    pub fn resolve_operation_id(
        &self,
        operation_id: &str,
    ) -> Result<(String, String), RuntimeError> {
        self.inner
            .index
            .op_index
            .get(operation_id)
            .map(|entry| (entry.method.clone(), entry.path.clone()))
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::OperationIdNotFound,
                    format!("operationId \"{operation_id}\" not found in loaded OpenAPI specs"),
                )
            })
    }

    fn prepare_http_request(
        &self,
        step: &Step,
        vars: &VarStore,
    ) -> Result<PreparedRequest, RuntimeError> {
        let operation_path = match &step.target {
            Some(StepTarget::OperationPath(path)) => path.clone(),
            Some(StepTarget::OperationId(id)) => {
                let (method, path) = self.resolve_operation_id(id)?;
                format!("{method} {path}")
            }
            _ => String::new(),
        };

        let (explicit_method, op_path) = parse_method(&operation_path);
        let url_result = self.build_url_from_path(op_path, step, vars)?;

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
                let mut ctx = self.make_eval_context(vars, None);
                ctx.method = Some(method.clone());
                let eval = ExpressionEvaluator::new(ctx);
                Some(resolve_payload(payload, &eval))
            } else {
                None
            }
        } else {
            None
        };
        // Content-type-aware serialization: non-JSON string payloads (e.g. XML/SOAP)
        // are sent as raw bytes instead of being JSON-serialized (which would double-quote them).
        let body = body_json.as_ref().map(|value| {
            let ct = step
                .request_body
                .as_ref()
                .map(|rb| rb.content_type.as_str())
                .unwrap_or("application/json");
            if !ct.contains("json") {
                if let Value::String(s) = value {
                    return s.as_bytes().to_vec();
                }
            }
            serde_json::to_vec(value).unwrap_or_default()
        });

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
        let mut hdr_ctx = self.make_eval_context(vars, None);
        hdr_ctx.method = Some(method.clone());
        let eval = ExpressionEvaluator::new(hdr_ctx);
        let mut cookie_parts = Vec::new();
        for param in &step.parameters {
            if param.in_ == Some(ParamLocation::Header) {
                let value_str = param.value_as_str();
                let resolved = value_to_string(&eval.resolve_value(&value_str));
                headers.insert(param.name.clone(), resolved);
            } else if param.in_ == Some(ParamLocation::Cookie) {
                let value_str = param.value_as_str();
                let resolved = value_to_string(&eval.resolve_value(&value_str));
                cookie_parts.push(format!("{}={}", param.name, resolved));
            }
        }
        if !cookie_parts.is_empty() {
            headers.insert("Cookie".to_string(), cookie_parts.join("; "));
        }

        let trace_request = TraceRequest {
            method: method.clone(),
            url: url_result.url.clone(),
            headers: headers.clone(),
            body: body_json.clone(),
        };

        Ok(PreparedRequest {
            method,
            url_result,
            headers,
            body,
            body_json,
            trace_request,
        })
    }

    fn make_post_request_eval_context(
        &self,
        vars: &VarStore,
        response: Option<&Response>,
        prep: &PreparedRequest,
    ) -> EvalContext {
        let mut ctx = self.make_eval_context(vars, response);
        ctx.method = Some(prep.method.clone());
        ctx.url = Some(prep.url_result.url.clone());
        ctx.request_headers = prep.headers.clone();
        ctx.request_query = prep.url_result.query_params.clone();
        ctx.request_path = prep.url_result.path_params.clone();
        ctx.request_body = prep.body_json.clone();
        ctx
    }

    pub(super) async fn execute_http_step(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
    ) -> Result<StepExecution, RuntimeError> {
        exec_ctx.check_cancelled()?;
        let prep = self.prepare_http_request(step, vars)?;

        if self.inner.dry_run_mode {
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::RequestPrepared {
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    method: prep.method.clone(),
                    url: prep.url_result.url.clone(),
                    headers: prep.headers.clone(),
                    has_body: prep.body.is_some(),
                },
            )
            .await;
            return self.execute_dry_run_step(step, vars, prep);
        }

        self.emit_observer_event(
            exec_ctx,
            ObserverEvent::RequestPrepared {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                method: prep.method.clone(),
                url: prep.url_result.url.clone(),
                headers: prep.headers.clone(),
                has_body: prep.body.is_some(),
            },
        )
        .await;

        self.emit_observer_event(
            exec_ctx,
            ObserverEvent::RequestSent {
                workflow_id: workflow_id.to_string(),
                step_id: step.step_id.clone(),
                method: prep.method.clone(),
                url: prep.url_result.url.clone(),
            },
        )
        .await;

        let response = self
            .inner
            .client
            .request(
                RequestConfig {
                    method: prep.method.clone(),
                    url: prep.url_result.url.clone(),
                    headers: prep.headers.clone(),
                    body: prep.body.clone(),
                },
                &exec_ctx.cancel,
                &exec_ctx.is_timeout,
            )
            .await?;

        self.evaluate_step_response(exec_ctx, workflow_id, step, vars, depth, &response, &prep)
            .await
    }

    fn execute_dry_run_step(
        &self,
        step: &Step,
        vars: &VarStore,
        prep: PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let req = DryRunRequest {
            step_id: step.step_id.clone(),
            method: prep.method.clone(),
            url: prep.url_result.url.clone(),
            headers: prep.headers.clone(),
            body: prep.body_json.clone(),
        };
        let fake = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: b"{}".to_vec(),
            body_json: Some(json!({})),
            content_type: ContentType::Json,
        };
        let dry_ctx = self.make_post_request_eval_context(vars, Some(&fake), &prep);
        let dry_eval = ExpressionEvaluator::new(dry_ctx);
        let mut outputs = BTreeMap::new();
        let mut warnings = Vec::<String>::new();
        for (name, expr) in &step.outputs {
            let (value, expr_warnings) =
                evaluate_output_expression_detailed(expr, &dry_eval, Some(&fake));
            outputs.insert(name.clone(), value);
            for warning in expr_warnings {
                warnings.push(format!("output \"{name}\": {warning}"));
            }
        }
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(fake),
                err: None,
            },
            outputs,
            dry_run_request: Some(req),
            trace: StepTraceData {
                request: Some(prep.trace_request),
                response: Some(TraceResponse {
                    status_code: 200,
                    content_type: ContentType::Json,
                    headers: BTreeMap::new(),
                    body_bytes: 2,
                    body_preview: Some("{}".to_string()),
                }),
                criteria: Vec::new(),
                warnings,
            },
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn evaluate_step_response(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
        response: &Response,
        prep: &PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let post_ctx = self.make_post_request_eval_context(vars, Some(response), prep);
        let eval = ExpressionEvaluator::new(post_ctx);
        let mut checkpoint_outputs = BTreeMap::<String, Value>::new();
        let mut criteria = Vec::new();
        let mut step_warnings = Vec::<String>::new();
        for (index, criterion) in step.success_criteria.iter().enumerate() {
            let evaluation = evaluate_criterion_detailed(criterion, &eval, Some(response), &self.inner.regex_cache);
            for warning in &evaluation.warnings {
                step_warnings.push(format!("successCriteria[{index}]: {warning}"));
            }
            criteria.push(TraceCriterionResult {
                index,
                type_: evaluation.type_name.clone(),
                condition: evaluation.condition.clone(),
                context: evaluation.context_expr.clone(),
                result: evaluation.matched,
                warnings: evaluation.warnings.iter().map(|w| w.to_string()).collect(),
            });
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::CriterionEvaluated {
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    index,
                    condition: evaluation.condition.clone(),
                    passed: evaluation.matched,
                },
            )
            .await;

            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(response),
                request: Some(&prep.trace_request),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_success_criterion(&gate, index, &evaluation)
                .await?;
            if !evaluation.matched {
                let trace_response = build_trace_response(response);
                return Ok(StepExecution {
                    result: StepResult {
                        success: false,
                        response: Some(response.clone()),
                        err: None,
                    },
                    outputs: BTreeMap::new(),
                    dry_run_request: None,
                    trace: StepTraceData {
                        request: Some(prep.trace_request.clone()),
                        response: Some(trace_response),
                        criteria,
                        warnings: step_warnings,
                    },
                });
            }
        }

        let mut outputs = BTreeMap::new();
        for (name, expr) in &step.outputs {
            let (value, expr_warnings) =
                evaluate_output_expression_detailed(expr, &eval, Some(response));
            outputs.insert(name.clone(), value.clone());
            checkpoint_outputs.insert(name.clone(), value);
            for warning in expr_warnings {
                step_warnings.push(format!("output \"{name}\": {warning}"));
            }
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(response),
                request: Some(&prep.trace_request),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_output(&gate, name, expr).await?;
        }

        let trace_response = build_trace_response(response);
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(response.clone()),
                err: None,
            },
            outputs,
            dry_run_request: None,
            trace: StepTraceData {
                request: Some(prep.trace_request.clone()),
                response: Some(trace_response),
                criteria,
                warnings: step_warnings,
            },
        })
    }

    pub(crate) fn build_url_from_path(
        &self,
        op_path: &str,
        step: &Step,
        vars: &VarStore,
    ) -> Result<UrlBuildResult, RuntimeError> {
        let (resolved_base, resolved_path) = if let Some((name, path)) =
            parse_source_prefix(op_path)
        {
            if let Some(source_url) = self.inner.index.source_descriptions_map.get(name) {
                (source_url.as_str(), path)
            } else {
                return Err(RuntimeError::new(
                        RuntimeErrorKind::SourceDescriptionNotFound,
                        format!(
                            "sourceDescription \"{name}\" referenced by operationPath \"{op_path}\" was not found"
                        ),
                    ));
            }
        } else {
            (self.inner.index.base_url.as_str(), op_path)
        };

        let mut target =
            if resolved_path.starts_with("http://") || resolved_path.starts_with("https://") {
                resolved_path.to_string()
            } else {
                format!("{}{}", resolved_base.trim_end_matches('/'), resolved_path)
            };

        let eval = ExpressionEvaluator::new(self.make_eval_context(vars, None));
        let mut path_params = BTreeMap::<String, String>::new();
        let mut query_params_vec = Vec::<(String, String)>::new();

        for param in &step.parameters {
            let value_str = param.value_as_str();
            let value = eval.resolve_value(&value_str);
            match param.in_ {
                Some(ParamLocation::Path) => {
                    path_params.insert(param.name.clone(), value_to_string(&value));
                }
                Some(ParamLocation::Query) => {
                    if !value.is_null() {
                        query_params_vec.push((param.name.clone(), value_to_string(&value)));
                    }
                }
                _ => {}
            }
        }

        let query_params: BTreeMap<String, String> = query_params_vec.iter().cloned().collect();

        if !path_params.is_empty() && target.contains('{') {
            target = replace_path_params(&target, &path_params);
        }
        if !query_params_vec.is_empty() {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in query_params_vec {
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
        Ok(UrlBuildResult {
            url: target,
            path_params,
            query_params,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRequest {
    pub method: String,
    pub url_result: UrlBuildResult,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub body_json: Option<Value>,
    pub trace_request: TraceRequest,
}
