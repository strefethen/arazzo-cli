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
    /// prepared live request would send Authorization/Cookie over
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
        let has_credentials = prep
            .headers
            .keys()
            .chain(client.default_headers().keys())
            .any(|name| {
                name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("cookie")
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
                "credentials (Authorization/Cookie header) sent over cleartext http to non-loopback host \"{host}\""
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
    /// [`Self::resolve_operation_target`], because the owning source is what
    /// decides which server the request is sent to.
    pub fn resolve_operation_id(
        &self,
        operation_id: &str,
    ) -> Result<(String, String), RuntimeError> {
        let resolved = self.resolve_operation_target(operation_id)?;
        Ok((resolved.method, resolved.path))
    }

    /// The operation index, parsed on first access.
    ///
    /// Lazy-init: parse OpenAPI specs on first access (stable OnceLock pattern).
    /// If two threads race, both compute the same deterministic index and one
    /// `set()` silently no-ops — OnceLock guarantees a single stored value.
    /// Source-description documents (indexed eagerly at build) go first, then
    /// explicitly provided specs, so the build-time override warning keeps
    /// announcing the same direction it always has.
    fn operation_index(&self) -> Result<&OperationIndex, RuntimeError> {
        let index = &self.inner.index;
        if index.op_index.get().is_none() {
            let mut idx = index.source_ops.clone();
            for (ordinal, spec_data) in index.openapi_specs_raw.iter().enumerate() {
                let origin = OperationOrigin::ExplicitSpec {
                    ordinal: ordinal + 1,
                };
                parse_openapi_into_index(spec_data, &origin, &mut idx)?;
            }
            let _ = index.op_index.set(idx);
        }
        index.op_index.get().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "operation index initialization failed unexpectedly",
            )
        })
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
    /// Source Description and no other — an explicitly provided spec belongs
    /// to no source, so it can neither satisfy a qualified target nor
    /// override one.
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
                     — name it from workflowId as \
                     \"$sourceDescriptions.{source_name}.<workflowId>\" instead",
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
        match self.operation_index()?.in_source(source_name, operation_id) {
            OperationMatch::One(entry) => Ok(ResolvedOperation {
                method: entry.method.clone(),
                path: entry.path.clone(),
                base: Some(base),
            }),
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
        if sources.len() > 1 {
            // The refusal covers operations supplied through
            // `EngineBuilder::openapi_spec` too — the MUST is about the
            // document's own sourceDescriptions, whatever this engine happens
            // to have indexed. Those operations belong to no source
            // description, so the qualified form cannot reach them and naming
            // it alone would be a remedy that fails.
            let explicit_note = if index.openapi_specs_raw.is_empty() {
                String::new()
            } else {
                // Naming only the two forms above would be worse than saying
                // nothing: `{<name>}./<path>` joins that source's host to a
                // path the source does not define, which is the silently-wrong
                // host this refusal exists to prevent.
                //
                // The conformant remedy leads: declaring that document as a
                // sourceDescription is what puts its operations in reach of
                // the qualified form, and a relative url resolves through the
                // `source_base_dir` the CLI already passes. The absolute-URL
                // operationPath named after it is F1 debt in
                // `plans/assessments/arazzo-spec-conformance-audit.md`, not a
                // specification form, so it never stands alone — and it takes
                // the optional method token, without which a PUT or DELETE
                // step would silently inherit the GET/POST default.
                ". An operation from a separately provided OpenAPI spec belongs to no \
                 sourceDescription, so neither of those forms can reach it — declare that \
                 document as a sourceDescription of type \"openapi\" and use the qualified form \
                 above, or give that step an absolute-URL operationPath, written \
                 \"<METHOD> <url>\" unless the default of POST with a requestBody and GET \
                 without is the method the step wants"
                    .to_string()
            };
            return Err(RuntimeError::new(
                RuntimeErrorKind::OperationIdAmbiguous,
                format!(
                    "operationId \"{target}\" is unqualified, but {count} non-arazzo \
                     sourceDescriptions are defined ({names}); name the source with \
                     \"$sourceDescriptions.<name>.{target}\", or address the operation by path \
                     with an operationPath of \"{{<name>}}./<path>\"{explicit_note}",
                    count = sources.len(),
                    names = sources.join(", "),
                ),
            ));
        }
        match self.operation_index()?.bare(operation_id) {
            OperationMatch::One(entry) => Ok(ResolvedOperation {
                method: entry.method.clone(),
                path: entry.path.clone(),
                // An operation from an explicitly provided spec belongs to no
                // source description, so it keeps the engine-wide base.
                base: entry
                    .origin
                    .source_name()
                    .and_then(|name| index.source_bases.get(name).cloned()),
            }),
            OperationMatch::Missing => Err(RuntimeError::new(
                RuntimeErrorKind::OperationIdNotFound,
                format!("operationId \"{operation_id}\" not found in loaded OpenAPI specs"),
            )),
            OperationMatch::Ambiguous(entries) => Err(ambiguous_operation_id(target, &entries)),
        }
    }

    pub(crate) fn prepare_http_request(
        &self,
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

        // A resolved `operationId` carries the effective request base of the
        // source description that owns it. Formatting the resolution back into
        // a `"<METHOD> <path>"` string and re-classifying that, as this used
        // to, is what discarded the owning source and sent every resolved
        // operation to the first source description's host.
        let (explicit_method, op_path, source_base) = match &step.target {
            Some(StepTarget::OperationPath(path)) => {
                let (method, remainder) = parse_method(path);
                (method.to_string(), remainder.to_string(), None)
            }
            Some(StepTarget::OperationId(id)) => {
                let resolved = self.resolve_operation_target(id)?;
                (resolved.method, resolved.path, resolved.base)
            }
            _ => (String::new(), String::new(), None),
        };

        let url_result = self.build_url_from_path(&op_path, source_base.as_deref(), step, vars)?;

        let method = if explicit_method.is_empty() {
            if step.request_body.is_some() {
                "POST".to_string()
            } else {
                "GET".to_string()
            }
        } else {
            explicit_method
        };

        let mut prep_warnings = url_result.warnings.clone();
        let body_json = if let Some(req_body) = &step.request_body {
            if let Some(payload) = &req_body.payload {
                let mut ctx = self.make_eval_context(vars, None);
                ctx.method = Some(method.clone());
                let eval = ExpressionEvaluator::new(ctx);
                let (mut body, payload_warnings) = resolve_payload_detailed(payload, &eval);
                prep_warnings.extend(
                    payload_warnings
                        .into_iter()
                        .map(|warning| format!("requestBody.payload: {warning}")),
                );
                if !req_body.replacements.is_empty() {
                    let (mutated, warnings) = apply_replacements(
                        body,
                        req_body.content_type.as_str(),
                        &req_body.replacements,
                        &eval,
                    );
                    body = mutated;
                    prep_warnings.extend(warnings);
                }
                Some(body)
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
                let (value, warnings) = resolve_value_source(&param.value, &eval);
                prep_warnings.extend(
                    warnings
                        .into_iter()
                        .map(|warning| format!("parameter {:?}: {warning}", param.name)),
                );
                let resolved = value_to_string(&value);
                headers.insert(param.name.clone(), resolved);
            } else if param.in_ == Some(ParamLocation::Cookie) {
                let (value, warnings) = resolve_value_source(&param.value, &eval);
                prep_warnings.extend(
                    warnings
                        .into_iter()
                        .map(|warning| format!("parameter {:?}: {warning}", param.name)),
                );
                let resolved = value_to_string(&value);
                let encoded = encode_cookie_value(&resolved);
                cookie_parts.push(format!("{}={}", param.name, encoded));
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
            redirects: Vec::new(),
        };

        Ok(PreparedRequest {
            method,
            url_result,
            headers,
            body,
            body_json,
            trace_request,
            warnings: prep_warnings,
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
                &exec_ctx.cancel,
                &exec_ctx.is_timeout,
            )
            .await?;

        let response = Arc::new(response);
        self.evaluate_step_response(exec_ctx, workflow_id, step, vars, depth, &response, &prep)
            .await
    }

    fn execute_dry_run_step(
        &self,
        step: &Step,
        vars: &VarStore,
        prep: PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let fake = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: b"{}".to_vec(),
            body_json: Some(json!({})),
            content_type: ContentType::Json,
            redirects: Vec::new(),
        };
        let dry_ctx = self.make_post_request_eval_context(vars, Some(&fake), &prep);
        let dry_eval = ExpressionEvaluator::new(dry_ctx);
        let mut outputs = BTreeMap::new();
        let mut warnings = prep.warnings.clone();
        for (name, expr) in &step.outputs {
            let (value, expr_warnings) =
                evaluate_output_value_detailed(expr, &dry_eval, Some(&fake));
            outputs.insert(name.clone(), value);
            for warning in expr_warnings {
                warnings.push(format!("output \"{name}\": {warning}"));
            }
        }
        let req = DryRunRequest {
            step_id: step.step_id.clone(),
            method: prep.method.clone(),
            url: prep.url_result.url.clone(),
            headers: prep.headers.clone(),
            body: prep.body_json.clone(),
            warnings: warnings.clone(),
        };
        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: Some(Arc::new(fake)),
                err: None,
                err_kind: None,
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
                    body: Some("{}".to_string()),
                    body_lossy: false,
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
        response: &Arc<Response>,
        prep: &PreparedRequest,
    ) -> Result<StepExecution, RuntimeError> {
        let post_ctx = self.make_post_request_eval_context(vars, Some(response), prep);
        let eval = ExpressionEvaluator::new(post_ctx);
        let mut checkpoint_outputs = BTreeMap::<String, Value>::new();
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
                        response: Some(Arc::clone(response)),
                        err: None,
                        err_kind: None,
                    },
                    outputs: BTreeMap::new(),
                    dry_run_request: None,
                    trace: StepTraceData {
                        request: Some(trace_request),
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
                evaluate_output_value_detailed(expr, &eval, Some(response));
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
                response: Some(Arc::clone(response)),
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
            },
        })
    }

    /// Builds the request URL for a step's already-resolved target.
    ///
    /// `source_base` is the effective request base of the source description
    /// that owns a resolved `operationId`. When it is present, `op_path` is
    /// that document's own path and carries no `operationPath` syntax to
    /// classify — `/pets/{petId}` is a path, not a `{sourceName}.` reference.
    pub(crate) fn build_url_from_path(
        &self,
        op_path: &str,
        source_base: Option<&str>,
        step: &Step,
        vars: &VarStore,
    ) -> Result<UrlBuildResult, RuntimeError> {
        // `{name}.` routing resolves to that source's *effective* request base
        // (derived from `servers` for document sources, the literal url for
        // legacy sources). `$sourceDescriptions.{name}.url` expressions keep
        // evaluating to the literal url via `source_descriptions_map`.
        //
        // Classification happens here, before any string is joined, so no
        // caller can construct a URL out of a value this runtime cannot resolve.
        let (resolved_base, resolved_path) = match source_base {
            Some(base) => (base, op_path),
            None => match classify_operation_path(op_path).form {
                OperationPathForm::Unsupported(reason) => {
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::UnsupportedOperationPathForm,
                        format!(
                            "step \"{}\": operationPath \"{op_path}\" carries {reason}; \
                             resolving the specification form (source reference plus JSON \
                             Pointer) is not implemented. Supported forms are \
                             {SUPPORTED_OPERATION_PATH_FORMS}.",
                            step.step_id
                        ),
                    ));
                }
                OperationPathForm::SourceRouted { source_name, path } => {
                    match self.inner.index.source_bases.get(source_name) {
                        Some(base) => (base.as_str(), path),
                        None => {
                            return Err(RuntimeError::new(
                                RuntimeErrorKind::SourceDescriptionNotFound,
                                format!(
                                    "sourceDescription \"{source_name}\" referenced by operationPath \"{op_path}\" was not found"
                                ),
                            ));
                        }
                    }
                }
                // An absolute URL keeps the base only to satisfy the tuple; the
                // `starts_with("http")` test below discards it, as it always has.
                OperationPathForm::AbsoluteUrl(url) => (self.inner.index.base_url.as_str(), url),
                OperationPathForm::BasePath(path) => (self.inner.index.base_url.as_str(), path),
            },
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
        // The whole query component, supplied by an `in: querystring` parameter.
        // Last one wins: `merge_workflow_params` prepends inherited parameters,
        // so a step-level declaration overrides a workflow-level one.
        let mut querystring: Option<(String, String)> = None;
        let mut warnings = Vec::<String>::new();

        // Resolution happens in the arm that owns the location, never once for
        // every parameter up front. Resolving a header or cookie value here as
        // well as where the request headers are built is what emitted each of
        // their value-source warnings twice: `prepare_http_request` seeds its
        // own warning list from this result and then resolves those two
        // locations again.
        let resolve = |param: &Parameter| {
            let (value, param_warnings) = resolve_value_source(&param.value, &eval);
            let named: Vec<String> = param_warnings
                .into_iter()
                .map(|warning| format!("parameter {:?}: {warning}", param.name))
                .collect();
            (value, named)
        };

        for param in &step.parameters {
            // No catch-all, and no guarded arm: a new `ParamLocation` variant
            // must be a compile error here, not a parameter this runtime
            // silently drops out of the request it sends.
            match param.in_ {
                Some(ParamLocation::Path) => {
                    let (value, named) = resolve(param);
                    warnings.extend(named);
                    path_params.insert(param.name.clone(), value_to_string(&value));
                }
                Some(ParamLocation::Query) => {
                    let (value, named) = resolve(param);
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
                    let (value, named) = resolve(param);
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
                None => warnings.extend(resolve(param).1),
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

        if !path_params.is_empty() && target.contains('{') {
            target = replace_path_params(&target, &path_params);
        }
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
    pub body_json: Option<Value>,
    pub trace_request: TraceRequest,
    pub warnings: Vec<String>,
}

/// An `operationId` resolved to the request it names.
pub(crate) struct ResolvedOperation {
    pub method: String,
    pub path: String,
    /// Effective request base of the source description that defines the
    /// operation. `None` when it came from an explicitly provided OpenAPI
    /// spec, which belongs to no source description and so keeps the
    /// engine-wide base.
    pub base: Option<String>,
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
