use super::operation_target::{resolved_operation, ResolvedOperation};
use super::*;

impl Engine {
    /// Configured `--insecure-host` entries no live request targeted.
    /// Empty for replay engines, under a blanket exception, or when
    /// every entry was consumed. Callers surface this as the
    /// end-of-run unused-exception note.
    pub fn unused_insecure_hosts(&self) -> Vec<String> {
        self.inner.client.unused_insecure_entries()
    }

    /// Emits the once-per-host cleartext-credential warning when a
    /// prepared live request would send Authorization/Cookie or URL userinfo over
    /// non-loopback http. The structured event is emitted regardless;
    /// `transport_warnings: false` squelches only the stderr line.
    async fn warn_cleartext_credentials(
        &self,
        exec_ctx: &ExecutionContext,
        prep: &PreparedRequest,
    ) {
        let client = &self.inner.client;
        if !client.is_live() {
            return;
        }
        let Ok(url) = url_crate::Url::parse(&prep.url_result.url) else {
            return;
        };
        if url.scheme() != "http" {
            return;
        }
        let Some(host) = url.host_str() else {
            return;
        };
        if is_loopback_host(host) {
            return;
        }
        let has_userinfo = !url.username().is_empty() || url.password().is_some();
        let has_credentials = has_userinfo
            || prep
                .headers
                .keys()
                .chain(client.default_headers().keys())
                .any(|name| {
                    name.eq_ignore_ascii_case("authorization")
                        || name.eq_ignore_ascii_case("cookie")
                });
        if !has_credentials {
            return;
        }
        if !client.note_cleartext_warned(host) {
            return;
        }
        let warning = TransportWarning {
            kind: TransportWarningKind::CleartextCredentials,
            hosts: vec![host.to_string()],
            message: format!(
                "credentials ({}) sent over cleartext http to non-loopback host \"{host}\"",
                if has_userinfo {
                    "URL userinfo"
                } else {
                    "Authorization/Cookie header"
                }
            ),
        };
        if client.transport_warnings_enabled() {
            eprintln!("warning: {}", warning.message);
        }
        let _ = exec_ctx
            .event_tx
            .send(EngineEvent::TransportWarning(warning))
            .await;
    }

    /// Resolves an `operationId` to the `(method, path)` it names, discarding
    /// the source description that owns it. Preserved for callers that need
    /// only the pair; the engine itself uses
    /// `Self::resolve_operation_target`, because the owning source is what
    /// decides which server the request is sent to.
    pub fn resolve_operation_id(
        &self,
        operation_id: &str,
    ) -> Result<(String, String), RuntimeError> {
        let resolved = self.resolve_operation_target(operation_id)?;
        Ok((resolved.method, resolved.path))
    }

    /// Resolves a step's `operationId` — bare or source-qualified — to the
    /// request it names and the server base that request belongs to.
    ///
    /// Every value that cannot be resolved to exactly one operation is refused
    /// here, before a URL is built and therefore before anything is sent. Each
    /// refusal says whose limit it is — the specification's or this runtime's —
    /// because the remedy differs.
    pub(crate) fn resolve_operation_target(
        &self,
        operation_id: &str,
    ) -> Result<ResolvedOperation, RuntimeError> {
        match classify_operation_id(operation_id) {
            OperationIdTarget::SourceQualified {
                source_name,
                operation_id: name,
            } => self.resolve_in_source(operation_id, source_name, name),
            OperationIdTarget::Bare(name) => self.resolve_bare(operation_id, name),
            OperationIdTarget::Malformed(reason) => Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedOperationIdForm,
                format!(
                    "operationId \"{operation_id}\" {reason}; supported forms are \
                     {SUPPORTED_OPERATION_ID_FORMS}"
                ),
            )),
        }
    }

    /// Resolves `$sourceDescriptions.<name>.<operationId>` against that one
    /// Source Description and no other. Unclaimed provided documents cannot
    /// satisfy or override a qualified target.
    fn resolve_in_source(
        &self,
        target: &str,
        source_name: &str,
        operation_id: &str,
    ) -> Result<ResolvedOperation, RuntimeError> {
        let index = &self.inner.index;
        let source = index
            .spec
            .source_descriptions
            .iter()
            .find(|sd| sd.name == source_name)
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::SourceDescriptionNotFound,
                    format!(
                        "sourceDescription \"{source_name}\" referenced by operationId \
                         \"{target}\" was not found"
                    ),
                )
            })?;
        // The declared `type` decides this, never the url text. The two
        // non-openapi types are refused for different reasons, so they get
        // different messages.
        //
        // An arazzo source is the specification's own limit. Step Object,
        // `operationId`: *"The name of an existing, resolvable operation"*,
        // while `workflowId` is what *"MUST be specified using a Runtime
        // Expression"* when *"the referenced workflow is contained within an
        // arazzo type sourceDescription"*. An arazzo document holds workflows,
        // so no operationId can name one and the remedy is a different field,
        // not a different runtime.
        if source.type_ == SourceType::Arazzo {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedSourceDescriptionType,
                format!(
                    "operationId \"{target}\" names sourceDescription \"{source_name}\", whose \
                     type is \"{declared}\"; an arazzo source describes workflows rather than \
                     operations, so the specification does not let an operationId reference one \
                     — workflowId is the specification-correct field for workflow references, but \
                     cross-document workflow references are unsupported by this runtime",
                    declared = source.type_,
                ),
            ));
        }
        // Any other non-openapi type is this runtime's limit, not the
        // specification's: v1.1.0 shows `operationId:
        // $sourceDescriptions.asyncOrderApi.placeOrder` in its own async step
        // example, so an asyncapi source naming an operation is conformant
        // Arazzo. Nothing here implements AsyncAPI transport, and the message
        // says whose limit it is.
        if source.type_ != SourceType::OpenApi {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedSourceDescriptionType,
                format!(
                    "operationId \"{target}\" names sourceDescription \"{source_name}\", whose \
                     type is \"{declared}\"; this runtime resolves an operationId only against a \
                     source of type \"{expected}\"",
                    declared = source.type_,
                    expected = SourceType::OpenApi,
                ),
            ));
        }
        let base = index
            .source_bases
            .get(source_name)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    format!("sourceDescription \"{source_name}\" has no effective request base"),
                )
            })?;
        match index.source_ops.in_source(source_name, operation_id) {
            OperationMatch::One(entry) => resolved_operation(target, entry, &base),
            OperationMatch::Missing => Err(RuntimeError::new(
                RuntimeErrorKind::OperationIdNotFound,
                format!(
                    "operationId \"{operation_id}\" was not found in sourceDescription \
                     \"{source_name}\", named by \"{target}\""
                ),
            )),
            OperationMatch::Ambiguous(entries) => Err(ambiguous_operation_id(target, &entries)),
        }
    }

    /// Resolves an unqualified `operationId`.
    ///
    /// Step Object, `operationId`: *"If multiple (non arazzo type)
    /// sourceDescriptions are defined, then the operationId MUST be specified
    /// using a Runtime Expression […] to avoid ambiguity or potential
    /// clashes."* The refusal below counts the sources the document declares
    /// rather than checking whether this particular id happens to be unique in
    /// the documents indexed today — otherwise a step keeps working until a
    /// second document starts defining the same name, and then silently
    /// changes which server it targets.
    fn resolve_bare(
        &self,
        target: &str,
        operation_id: &str,
    ) -> Result<ResolvedOperation, RuntimeError> {
        let index = &self.inner.index;
        let sources: Vec<&str> = index
            .spec
            .source_descriptions
            .iter()
            .filter(|sd| sd.type_ != SourceType::Arazzo)
            .map(|sd| sd.name.as_str())
            .collect();
        match sources.as_slice() {
            [source] => self.resolve_in_source(target, source, operation_id),
            [] => Err(RuntimeError::new(RuntimeErrorKind::OperationIdNotFound,
                format!("operationId \"{operation_id}\" has no declared operation source; provided OpenAPI documents must bind to a sourceDescription"))),
            _ => Err(RuntimeError::new(RuntimeErrorKind::OperationIdAmbiguous,
                format!("operationId \"{target}\" is unqualified, but {} non-arazzo sourceDescriptions are defined ({}); name the source with \"$sourceDescriptions.<name>.{target}\"", sources.len(), sources.join(", ")))),
        }
    }

    #[cfg(test)]
    pub(crate) fn prepare_http_request(
        &self,
        step: &Step,
        vars: &VarStore,
    ) -> Result<PreparedRequest, RuntimeError> {
        self.prepare_http_request_for_workflow("test", step, vars)
    }

    fn prepare_http_request_for_workflow(
        &self,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
    ) -> Result<PreparedRequest, RuntimeError> {
        if matches!(&step.target, Some(StepTarget::ChannelPath(_))) || step.action.is_some() {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedAsyncApiTransport,
                format!(
                    "step \"{}\" requires AsyncAPI channel transport, which is not implemented",
                    step.step_id
                ),
            ));
        }

        let resolved = match &step.target {
            Some(StepTarget::OperationPath(path)) => self.resolve_operation_path(path)?,
            Some(StepTarget::OperationId(id)) => self.resolve_operation_target(id)?,
            _ => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    format!("step \"{}\" has no HTTP operation target", step.step_id),
                ))
            }
        };
        let method = resolved.method;
        let eval = ExpressionEvaluator::new(super::eval_context::request(self, vars, &method));
        let admission = request_admission::RequestAdmission::new(&step.step_id, vars);
        let admission = if self.inner.dry_run_mode {
            admission.with_presentation(workflow_id)
        } else {
            admission
        };
        let (url_result, projected_url) =
            self.build_url_with_evaluator(&resolved.path, &resolved.base, step, &eval, &admission)?;

        let mut prep_warnings = url_result.warnings.clone();
        request_body::refuse_content_type_parameter(step)?;
        let request_body::PreparedBody {
            wire: body,
            content_type,
            logical,
            projection: projected_body,
            warnings: body_warnings,
        } = request_body::prepare(step, vars, &eval, &admission, self.inner.dry_run_mode)?;
        prep_warnings.extend(body_warnings);

        let mut headers = BTreeMap::new();
        if let Some(content_type) = content_type {
            headers.insert("Content-Type".to_string(), content_type);
        }
        let mut projected_headers = headers.clone();
        let mut cookie_parts = Vec::new();
        let mut projected_cookies = Vec::new();
        for (index, param) in step.parameters.iter().enumerate() {
            if param.in_ == Some(ParamLocation::Header) {
                let location =
                    request_admission::RequestAdmission::parameter_location(index, param);
                let resolution = admission.resolve(&param.value, &eval, &location)?;
                let projection = admission.project(&resolution, &location)?;
                let value = resolution.value;
                let warnings = resolution.warnings;
                prep_warnings.extend(
                    warnings
                        .into_iter()
                        .map(|warning| format!("parameter {:?}: {warning}", param.name)),
                );
                let resolved = value_to_string(&value);
                headers.insert(param.name.clone(), resolved);
                projected_headers.insert(param.name.clone(), value_to_string(&projection));
            } else if param.in_ == Some(ParamLocation::Cookie) {
                let location =
                    request_admission::RequestAdmission::parameter_location(index, param);
                let resolution = admission.resolve(&param.value, &eval, &location)?;
                let projection = admission.project(&resolution, &location)?;
                let value = resolution.value;
                let warnings = resolution.warnings;
                prep_warnings.extend(
                    warnings
                        .into_iter()
                        .map(|warning| format!("parameter {:?}: {warning}", param.name)),
                );
                let resolved = value_to_string(&value);
                let encoded = encode_cookie_value(&resolved);
                cookie_parts.push(format!("{}={}", param.name, encoded));
                projected_cookies.push(format!(
                    "{}={}",
                    param.name,
                    encode_cookie_value(&value_to_string(&projection))
                ));
            }
        }
        if !cookie_parts.is_empty() {
            headers.insert("Cookie".to_string(), cookie_parts.join("; "));
            projected_headers.insert("Cookie".to_string(), projected_cookies.join("; "));
        }

        let trace_request = TraceRequest {
            method: method.clone(),
            url: url_result.url.clone(),
            headers: headers.clone(),
            body: logical.json().cloned(),
            body_blobs: logical.blobs(),
            redirects: Vec::new(),
        };

        prep_warnings.extend(admission.presentation_warnings(step));
        let presentation = TraceRequest {
            method: method.clone(),
            url: projected_url.url,
            headers: projected_headers,
            body: projected_body,
            body_blobs: BTreeMap::new(),
            redirects: Vec::new(),
        };

        Ok(PreparedRequest {
            method,
            url_result,
            headers,
            body,
            logical,
            trace_request,
            presentation,
            warnings: prep_warnings,
        })
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
        let prep = self.prepare_http_request_for_workflow(workflow_id, step, vars)?;

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
            return self
                .execute_dry_run_step(exec_ctx, workflow_id, step, vars, depth, prep)
                .await;
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

        self.warn_cleartext_credentials(exec_ctx, &prep).await;

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
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    method: prep.method.clone(),
                    url: prep.url_result.url.clone(),
                    headers: prep.headers.clone(),
                    body: prep.body.clone(),
                },
                prep.logical.json(),
                &exec_ctx.cancel,
                &exec_ctx.is_timeout,
            )
            .await?;

        let response = Arc::new(response);
        self.evaluate_step_response(exec_ctx, workflow_id, step, vars, depth, &response, &prep)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_dry_run_step(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
        prep: PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let fake = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: b"{}".to_vec(),
            body_value: arazzo_expr::BodySlot::present(json!({})),
            media_type: arazzo_spec::media_type::MediaType::parse("application/json"),
            redirects: Vec::new(),
        };
        let dry_ctx = super::eval_context::post_request(self, vars, Some(&fake), &prep);
        let dry_eval = ExpressionEvaluator::new(dry_ctx);
        let mut outputs = ProvenancedValues::default();
        let mut warnings = prep.warnings.clone();
        for (name, expr) in &step.outputs {
            let (value, expr_warnings, metadata) = evaluate_step_output(
                expr,
                &dry_eval,
                Some(&fake),
                BlobSources {
                    request_body: prep.logical.blob(),
                    ..BlobSources::new(vars)
                },
                Some(ValueProducer {
                    workflow_id,
                    step_id: &step.step_id,
                    site: ValueProducerSite::Output(name),
                    transport: ValueProducerTransport::Http,
                }),
            )
            .map_err(|unavailable| {
                RuntimeError::value_unavailable(&format!("output {name:?}"), unavailable)
            })?;
            let producer_warnings =
                dry_run::output_warnings(workflow_id, &step.step_id, name, expr, &metadata);
            for warning in expr_warnings {
                if !dry_run::transport_skipped_source(
                    workflow_id,
                    &step.step_id,
                    name,
                    &metadata,
                    &warning.expression,
                ) {
                    warnings.push(format!("output \"{name}\": {warning}"));
                }
            }
            outputs.insert(name.clone(), value, metadata);
            warnings.extend(producer_warnings);
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(&fake),
                request: Some(&prep.trace_request),
                message: None,
                current_outputs: &outputs,
                depth,
                cancel: &exec_ctx.cancel,
                is_timeout: &exec_ctx.is_timeout,
            };
            self.debug_gate_output(&gate, name, expr).await?;
        }
        let req = DryRunRequest {
            step_id: step.step_id.clone(),
            method: prep.method.clone(),
            url: prep.presentation.url.clone(),
            headers: prep.presentation.headers.clone(),
            body: prep.presentation.body.clone(),
            warnings: warnings.clone(),
        };
        let trace_response = build_trace_response(&fake);
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(Arc::new(fake)),
                message: None,
                err: None,
                err_kind: None,
            },
            outputs,
            dry_run_request: Some(req),
            trace: StepTraceData {
                request: Some(prep.trace_request),
                response: Some(trace_response),
                criteria: Vec::new(),
                warnings,
                message: None,
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
        response: &Arc<Response>,
        prep: &PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let post_ctx = super::eval_context::post_request(self, vars, Some(response), prep);
        let eval = ExpressionEvaluator::new(post_ctx);
        let mut outputs = ProvenancedValues::default();
        let mut criteria = Vec::new();
        let mut step_warnings = prep.warnings.clone();
        // Surface the followed redirect chain in the step's trace request.
        let mut trace_request = prep.trace_request.clone();
        trace_request.redirects = response.redirects.clone();
        for (index, criterion) in step.success_criteria.iter().enumerate() {
            let evaluation = evaluate_criterion_detailed(
                criterion,
                &eval,
                Some(response),
                &self.inner.regex_cache,
            );
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
                message: None,
                current_outputs: &outputs,
                depth,
                cancel: &exec_ctx.cancel,
                is_timeout: &exec_ctx.is_timeout,
            };
            self.debug_gate_success_criterion(&gate, index, &evaluation)
                .await?;
            if !evaluation.matched {
                return Ok(Self::failed_exchange(
                    response,
                    trace_request,
                    criteria,
                    step_warnings,
                    evaluation.unavailable_error(&format!("successCriteria[{index}]")),
                ));
            }
        }

        let blobs = BlobSources {
            request_body: prep.logical.blob(),
            ..BlobSources::new(vars)
        };
        for (name, expr) in &step.outputs {
            let (value, expr_warnings, metadata) =
                match evaluate_step_output(expr, &eval, Some(response), blobs, None) {
                    Ok(output) => output,
                    Err(unavailable) => {
                        return Ok(Self::failed_exchange(
                            response,
                            trace_request,
                            criteria,
                            step_warnings,
                            Some(RuntimeError::value_unavailable(
                                &format!("output {name:?}"),
                                unavailable,
                            )),
                        ));
                    }
                };
            outputs.insert(name.clone(), value, metadata);
            for warning in expr_warnings {
                step_warnings.push(format!("output \"{name}\": {warning}"));
            }
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: Some(response),
                request: Some(&prep.trace_request),
                message: None,
                current_outputs: &outputs,
                depth,
                cancel: &exec_ctx.cancel,
                is_timeout: &exec_ctx.is_timeout,
            };
            self.debug_gate_output(&gate, name, expr).await?;
        }

        let trace_response = build_trace_response(response);
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(Arc::clone(response)),
                message: None,
                err: None,
                err_kind: None,
            },
            outputs,
            dry_run_request: None,
            trace: StepTraceData {
                request: Some(trace_request),
                response: Some(trace_response),
                criteria,
                warnings: step_warnings,
                message: None,
            },
        })
    }

    fn failed_exchange(
        response: &Arc<Response>,
        request: TraceRequest,
        criteria: Vec<TraceCriterionResult>,
        warnings: Vec<String>,
        unavailable: Option<RuntimeError>,
    ) -> StepExecution {
        StepExecution {
            result: StepResult {
                success: false,
                response: Some(Arc::clone(response)),
                message: None,
                err_kind: unavailable.as_ref().map(|error| error.kind),
                err: unavailable.map(|error| error.message),
            },
            outputs: ProvenancedValues::default(),
            dry_run_request: None,
            trace: StepTraceData {
                request: Some(request),
                response: Some(build_trace_response(response)),
                criteria,
                warnings,
                message: None,
            },
        }
    }

    /// Builds the request URL for a step's already-resolved target.
    ///
    /// Both target forms have already selected an actual bound operation.
    #[cfg(test)]
    pub(crate) fn build_url_from_path(
        &self,
        op_path: &str,
        source_base: &str,
        step: &Step,
        vars: &VarStore,
    ) -> Result<UrlBuildResult, RuntimeError> {
        let eval = ExpressionEvaluator::new(super::eval_context::request(self, vars, ""));
        let admission = request_admission::RequestAdmission::new(&step.step_id, vars);
        self.build_url_with_evaluator(op_path, source_base, step, &eval, &admission)
            .map(|pair| pair.0)
    }

    fn build_url_with_evaluator(
        &self,
        op_path: &str,
        source_base: &str,
        step: &Step,
        eval: &ExpressionEvaluator,
        admission: &request_admission::RequestAdmission<'_>,
    ) -> Result<(UrlBuildResult, UrlBuildResult), RuntimeError> {
        let mut projected = BTreeMap::new();
        let ordinary = self.build_url_resolved(op_path, source_base, step, |index, param| {
            let location = request_admission::RequestAdmission::parameter_location(index, param);
            let resolution = admission.resolve(&param.value, eval, &location)?;
            if param.in_.is_some() {
                let mut projection = admission.project(&resolution, &location)?;
                if param.in_ == Some(ParamLocation::Querystring) && projection != resolution.value {
                    if let Value::String(marker) = &projection {
                        projection = Value::String(
                            url_crate::form_urlencoded::byte_serialize(marker.as_bytes()).collect(),
                        );
                    }
                }
                projected.insert(index, projection);
            } else {
                projected.insert(index, resolution.value.clone());
            }
            let value = resolution.value;
            let param_warnings = resolution.warnings;
            let named: Vec<String> = param_warnings
                .into_iter()
                .map(|warning| format!("parameter {:?}: {warning}", param.name))
                .collect();
            Ok((value, named))
        })?;
        if !self.inner.dry_run_mode {
            return Ok((ordinary.clone(), ordinary));
        }
        let presentation = self.build_url_resolved(op_path, source_base, step, |index, _| {
            let value = projected.remove(&index).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    "missing resolved parameter projection",
                )
            })?;
            Ok((value, Vec::new()))
        })?;
        Ok((ordinary, presentation))
    }

    fn build_url_resolved(
        &self,
        op_path: &str,
        source_base: &str,
        step: &Step,
        mut resolve: impl FnMut(usize, &Parameter) -> Result<(Value, Vec<String>), RuntimeError>,
    ) -> Result<UrlBuildResult, RuntimeError> {
        let mut target = format!("{}{}", source_base.trim_end_matches('/'), op_path);

        let mut path_params = BTreeMap::<String, String>::new();
        let mut query_params_vec = Vec::<(String, String)>::new();
        // The whole query component, supplied by an `in: querystring` parameter.
        // Last one wins: `merge_workflow_params` prepends inherited parameters,
        // so a step-level declaration overrides a workflow-level one.
        let mut querystring: Option<(String, String)> = None;
        let mut warnings = Vec::<String>::new();

        for (index, param) in step.parameters.iter().enumerate() {
            // No catch-all, and no guarded arm: a new `ParamLocation` variant
            // must be a compile error here, not a parameter this runtime
            // silently drops out of the request it sends.
            match param.in_ {
                Some(ParamLocation::Path) => {
                    let (value, named) = resolve(index, param)?;
                    warnings.extend(named);
                    path_params.insert(param.name.clone(), value_to_string(&value));
                }
                Some(ParamLocation::Query) => {
                    let (value, named) = resolve(index, param)?;
                    warnings.extend(named);
                    if !value.is_null() {
                        match &value {
                            Value::Array(arr) => {
                                // Exploded form: emit one key-value pair per element.
                                for elem in arr {
                                    query_params_vec
                                        .push((param.name.clone(), value_to_string(elem)));
                                }
                            }
                            Value::Object(_) => {
                                // Serialize objects as JSON strings.
                                query_params_vec.push((
                                    param.name.clone(),
                                    serde_json::to_string(&value).unwrap_or_default(),
                                ));
                            }
                            _ => {
                                query_params_vec
                                    .push((param.name.clone(), value_to_string(&value)));
                            }
                        }
                    }
                }
                Some(ParamLocation::Querystring) => {
                    let (value, named) = resolve(index, param)?;
                    warnings.extend(named);
                    match &value {
                        // Null is skipped in silence, exactly as an unset `query`
                        // parameter is.
                        Value::Null => {}
                        Value::String(text) => {
                            if let Some((discarded, _)) = &querystring {
                                // The specification forbids a second `querystring`
                                // parameter and `arazzo-validate` rejects it;
                                // `merge_workflow_params` only lets distinct names
                                // get this far. An unvalidated spec reaching here
                                // still gets a defined URL — last one wins, which
                                // is what makes the same-name workflow/step
                                // override work — and the loser is named.
                                warnings.push(format!(
                                    "parameter {:?}: in: querystring supplies the entire query \
                                     component and cannot appear more than once, so the earlier \
                                     in: querystring parameter {discarded:?} was dropped",
                                    param.name
                                ));
                            }
                            querystring = Some((param.name.clone(), text.clone()));
                        }
                        other => {
                            // A structure cannot be a query component. Stringifying
                            // it would send `{"a":1}` as the query and call it
                            // resolved; dropping it would send the request with no
                            // query at all. Both send the wrong request, so the
                            // step fails instead.
                            return Err(RuntimeError::new(
                                RuntimeErrorKind::InvalidParameterValue,
                                format!(
                                    "step \"{}\": parameter {:?} (in: querystring) requires a \
                                     string value (the entire already-encoded query component); \
                                     got {}",
                                    step.step_id,
                                    param.name,
                                    json_type_name(other)
                                ),
                            ));
                        }
                    }
                }
                // Header and cookie parameters are resolved where the request
                // headers are built, not in URL assembly — so they are not
                // resolved here either, or every warning they raise would be
                // reported once from each site.
                Some(ParamLocation::Header) | Some(ParamLocation::Cookie) => {}
                // No location names no part of the request, so there is nothing
                // to bind. The value is still resolved so that an unresolvable
                // one is reported: `arazzo-validate` only advises on a missing
                // `in`, so such a parameter does reach this loop.
                None => warnings.extend(resolve(index, param)?.1),
            }
        }

        // Merge duplicate query params with comma (HTTP convention) for the
        // expression context. The URL itself retains all individual params.
        let mut query_params = BTreeMap::<String, String>::new();
        for (k, v) in &query_params_vec {
            query_params
                .entry(k.clone())
                .and_modify(|existing| {
                    existing.push(',');
                    existing.push_str(v);
                })
                .or_insert_with(|| v.clone());
        }

        let path_validation = if !path_params.is_empty() && target.contains('{') {
            let (substituted, validation) = replace_path_params(&target, &path_params);
            target = substituted;
            Some(validation)
        } else {
            None
        };
        if let Some((name, raw)) = querystring {
            // `querystring` *is* the query component, so it replaces whatever
            // the target carried rather than being appended to it, and it is
            // written verbatim: the value is already encoded, and running it
            // through `form_urlencoded` would turn `a=1&b=2` into
            // `a%3D1%26b%3D2`.
            let query = raw.strip_prefix('?').unwrap_or(&raw);
            if query.contains('#') {
                // Appended as-is this would end the query and start a fragment,
                // truncating the request silently. `%23` is the encoding for a
                // literal `#`, and only the author can say which was meant.
                return Err(RuntimeError::new(
                    RuntimeErrorKind::InvalidParameterValue,
                    format!(
                        "step \"{}\": parameter {name:?} (in: querystring) resolved to \
                         {query:?}, which contains \"#\"; a query component cannot contain \
                         a raw \"#\" — it would start a URL fragment and truncate the \
                         query. Percent-encode it as \"%23\".",
                        step.step_id
                    ),
                ));
            }
            if !query_params_vec.is_empty() {
                // The specification forbids this combination and `arazzo-validate`
                // rejects it; an unvalidated spec reaching here still gets a
                // defined URL rather than a merged one.
                let dropped = query_params_vec
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                warnings.push(format!(
                    "parameter {name:?}: in: querystring supplies the entire query \
                     component, so the in: query parameter(s) [{dropped}] were dropped"
                ));
                query_params.clear();
            }
            // `$request.query.<name>` must describe the query actually sent, so
            // the verbatim component is parsed back into decoded pairs.
            for (key, value) in url_crate::form_urlencoded::parse(query.as_bytes()) {
                query_params
                    .entry(key.into_owned())
                    .and_modify(|existing| {
                        existing.push(',');
                        existing.push_str(&value);
                    })
                    .or_insert_with(|| value.into_owned());
            }
            if let Some(position) = target.find('?') {
                let existing = &target[position + 1..];
                if !existing.is_empty() {
                    // Two declarations of the same query component. Silently
                    // replacing or merging would send a URL the author never
                    // wrote, so the step fails and names both sides.
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::InvalidParameterValue,
                        format!(
                            "step \"{}\": parameter {name:?} (in: querystring) supplies the \
                             entire query component, but the resolved operation URL already \
                             carries a query ({existing:?}); remove the parameter or the \
                             URL's query",
                            step.step_id
                        ),
                    ));
                }
                // A bare trailing "?" is an empty query component — nothing
                // was declared twice, so the querystring simply takes over.
                target.truncate(position);
            }
            if !query.is_empty() {
                target.push('?');
                target.push_str(query);
            }
        } else if !query_params_vec.is_empty() {
            let mut serializer = url_crate::form_urlencoded::Serializer::new(String::new());
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
        if let Some(validation) = path_validation {
            validation.validate(&target).map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorKind::InvalidParameterValue,
                    format!(
                        "step {:?}: path parameter(s) {:?} would produce a complete URL path \
                         segment that the WHATWG URL parser normalizes as `.` or `..`; the \
                         runtime path safety policy refuses this request",
                        step.step_id, error.parameter_names
                    ),
                )
            })?;
        }
        Ok(UrlBuildResult {
            url: target,
            path_params,
            query_params,
            warnings,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedRequest {
    pub method: String,
    pub url_result: UrlBuildResult,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub logical: request_body::LogicalBody,
    pub trace_request: TraceRequest,
    presentation: TraceRequest,
    pub warnings: Vec<String>,
}

/// The refusal for an `operationId` that more than one indexed document
/// defines.
///
/// Naming every definition is the point: the previous index kept only the last
/// one indexed, so a clash between two documents was invisible until the
/// request arrived at the wrong host.
fn ambiguous_operation_id(target: &str, entries: &[&OperationEntry]) -> RuntimeError {
    // Deduplicated, because one document defining the same operationId under
    // two paths is a real case and naming that document twice reads as a bug
    // in the message rather than a clash inside the document.
    let mut origins: Vec<String> = Vec::with_capacity(entries.len());
    for origin in entries.iter().map(|entry| entry.origin.describe()) {
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    RuntimeError::new(
        RuntimeErrorKind::OperationIdAmbiguous,
        format!(
            "operationId \"{target}\" is defined {count} times ({origins}); it does not name one \
             operation",
            count = entries.len(),
            origins = origins.join(", "),
        ),
    )
}
