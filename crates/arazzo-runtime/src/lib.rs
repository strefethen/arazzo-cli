#![forbid(unsafe_code)]

//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arazzo_expr::{EvalContext, ExpressionEvaluator};
use arazzo_spec::{ArazzoSpec, OnAction, Step, SuccessCriterion, Workflow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_RETRIES_PER_STEP: usize = 3;
const MAX_CALL_DEPTH: usize = 10;
const SLEEP_CHECK_INTERVAL: Duration = Duration::from_millis(25);

/// Runtime error.
#[derive(Debug, Clone)]
pub struct RuntimeError(pub String);

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RuntimeError {}

/// Per-execution controls for deadline and external cancellation.
#[derive(Debug, Clone, Default)]
pub struct ExecutionOptions {
    pub deadline: Option<Instant>,
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

impl ExecutionOptions {
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            cancel_flag: None,
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self::with_deadline(Instant::now() + timeout)
    }

    pub fn with_cancel_flag(cancel_flag: Arc<AtomicBool>) -> Self {
        Self {
            deadline: None,
            cancel_flag: Some(cancel_flag),
        }
    }

    fn check(&self) -> Result<(), RuntimeError> {
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return Err(RuntimeError("execution timeout exceeded".to_string()));
            }
        }
        if let Some(flag) = &self.cancel_flag {
            if flag.load(Ordering::Relaxed) {
                return Err(RuntimeError("execution cancelled".to_string()));
            }
        }
        Ok(())
    }
}

/// Runtime rate limiter settings.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub requests_per_second: f64,
    pub burst: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 10.0,
            burst: 20,
        }
    }
}

/// HTTP client settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub timeout: Duration,
    pub default_headers: BTreeMap<String, String>,
    pub rate_limit: RateLimitConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        let mut default_headers = BTreeMap::new();
        default_headers.insert("User-Agent".to_string(), "arazzo-cli/0.1".to_string());
        Self {
            timeout: Duration::from_secs(30),
            default_headers,
            rate_limit: RateLimitConfig::default(),
        }
    }
}

#[derive(Debug)]
struct RateLimiterState {
    requests_per_second: f64,
    burst: f64,
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiterState {
    fn new(cfg: &RateLimitConfig) -> Self {
        let burst = cfg.burst.max(1) as f64;
        Self {
            requests_per_second: cfg.requests_per_second,
            burst,
            tokens: burst,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self, now: Instant) {
        if self.requests_per_second <= 0.0 {
            self.tokens = self.burst;
            self.last_refill = now;
            return;
        }
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        if elapsed <= 0.0 {
            return;
        }
        let gained = elapsed * self.requests_per_second;
        self.tokens = (self.tokens + gained).min(self.burst);
        self.last_refill = now;
    }

    fn acquire_wait(&mut self, now: Instant) -> Option<Duration> {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return None;
        }
        if self.requests_per_second <= 0.0 {
            return None;
        }
        let missing = 1.0 - self.tokens;
        let wait = missing / self.requests_per_second;
        Some(Duration::from_secs_f64(wait.max(0.0)))
    }
}

#[derive(Debug, Clone)]
struct HttpClient {
    inner: reqwest::blocking::Client,
    default_headers: BTreeMap<String, String>,
    rate_limiter: Arc<Mutex<RateLimiterState>>,
}

impl HttpClient {
    fn new(config: &ClientConfig) -> Result<Self, RuntimeError> {
        let inner = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|err| RuntimeError(format!("building HTTP client: {err}")))?;
        Ok(Self {
            inner,
            default_headers: config.default_headers.clone(),
            rate_limiter: Arc::new(Mutex::new(RateLimiterState::new(&config.rate_limit))),
        })
    }

    fn request(
        &self,
        cfg: RequestConfig,
        options: &ExecutionOptions,
    ) -> Result<Response, RuntimeError> {
        self.wait_for_rate_limit(options)?;
        let method = reqwest::Method::from_bytes(cfg.method.as_bytes())
            .map_err(|err| RuntimeError(format!("invalid HTTP method {}: {err}", cfg.method)))?;
        let mut req = self.inner.request(method, cfg.url);

        for (k, v) in &self.default_headers {
            req = req.header(k, v);
        }
        for (k, v) in cfg.headers {
            req = req.header(k, v);
        }
        if let Some(body) = cfg.body {
            req = req.body(body);
        }

        let resp = req
            .send()
            .map_err(|err| RuntimeError(format!("executing request: {err}")))?;

        let status_code = i64::from(resp.status().as_u16());
        let mut headers = BTreeMap::new();
        for (k, v) in resp.headers() {
            let value = v.to_str().unwrap_or_default().to_string();
            headers.insert(k.to_string(), value);
        }
        let body = resp
            .bytes()
            .map_err(|err| RuntimeError(format!("reading response body: {err}")))?
            .to_vec();

        let content_type = headers
            .get("content-type")
            .or_else(|| headers.get("Content-Type"))
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let is_xml = content_type.contains("xml") || content_type.contains("rss");
        let body_json = if is_xml {
            None
        } else {
            serde_json::from_slice::<Value>(&body).ok()
        };

        Ok(Response {
            status_code,
            headers,
            body,
            body_json,
            content_type: if is_xml {
                "xml".to_string()
            } else {
                "json".to_string()
            },
        })
    }

    fn wait_for_rate_limit(&self, options: &ExecutionOptions) -> Result<(), RuntimeError> {
        loop {
            options.check()?;
            let wait = {
                let now = Instant::now();
                let mut limiter = self
                    .rate_limiter
                    .lock()
                    .map_err(|_| RuntimeError("rate limiter lock poisoned".to_string()))?;
                limiter.acquire_wait(now)
            };
            match wait {
                None => return Ok(()),
                Some(delay) => sleep_with_checks(delay, options)?,
            }
        }
    }
}

/// Request settings used by the runtime client.
#[derive(Debug, Clone)]
pub struct RequestConfig {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
}

/// Response returned by the runtime client.
#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: i64,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub body_json: Option<Value>,
    pub content_type: String,
}

/// Captured request emitted during dry-run mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DryRunRequest {
    pub step_id: String,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Trace payload for step lifecycle events.
#[derive(Debug, Clone, Default)]
pub struct StepEvent {
    pub workflow_id: String,
    pub step_id: String,
    pub operation_path: String,
    pub workflow_id_ref: String,
    pub status_code: i64,
    pub outputs: BTreeMap<String, Value>,
    pub err: Option<String>,
    pub duration: Duration,
}

/// Hook for step-level tracing.
pub trait TraceHook: Send + Sync {
    fn before_step(&self, event: &StepEvent);
    fn after_step(&self, event: &StepEvent);
}

#[derive(Debug, Clone)]
struct OperationEntry {
    method: String,
    path: String,
}

#[derive(Debug, Clone)]
struct StepResult {
    success: bool,
    response: Option<Response>,
    err: Option<String>,
}

#[derive(Debug, Clone)]
struct StepExecution {
    result: StepResult,
    outputs: BTreeMap<String, Value>,
    dry_run_request: Option<DryRunRequest>,
}

#[derive(Debug, Clone, Default)]
struct VarStore {
    inputs: BTreeMap<String, Value>,
    steps: BTreeMap<String, BTreeMap<String, Value>>,
}

impl VarStore {
    fn set_input(&mut self, name: &str, value: Value) {
        self.inputs.insert(name.to_string(), value);
    }

    fn set_step_output(&mut self, step_id: &str, name: &str, value: Value) {
        self.steps
            .entry(step_id.to_string())
            .or_default()
            .insert(name.to_string(), value);
    }

    fn step_outputs(&self, step_id: &str) -> BTreeMap<String, Value> {
        self.steps.get(step_id).cloned().unwrap_or_default()
    }

    fn eval_context(&self, response: Option<&Response>) -> EvalContext {
        let mut ctx = EvalContext {
            inputs: self.inputs.clone(),
            steps: self.steps.clone(),
            ..EvalContext::default()
        };
        if let Some(resp) = response {
            ctx.status_code = Some(resp.status_code);
            ctx.response_headers = resp.headers.clone();
            ctx.response_body = resp.body_json.clone();
        }
        ctx
    }
}

/// Runtime engine for executing Arazzo workflows.
pub struct Engine {
    client: HttpClient,
    spec: ArazzoSpec,
    base_url: String,
    workflow_index: BTreeMap<String, usize>,
    step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    op_index: BTreeMap<String, OperationEntry>,
    parallel_mode: bool,
    dry_run_mode: bool,
    dry_run_reqs: Arc<Mutex<Vec<DryRunRequest>>>,
    trace_hook: Option<Arc<dyn TraceHook>>,
}

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
            .map_err(|err| RuntimeError(format!("parsing OpenAPI spec: {err}")))?;
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
            return Err(RuntimeError(format!(
                "max call depth ({MAX_CALL_DEPTH}) exceeded calling workflow \"{workflow_id}\""
            )));
        }

        let workflow = self
            .get_workflow(workflow_id)
            .cloned()
            .ok_or_else(|| RuntimeError(format!("workflow \"{workflow_id}\" not found")))?;

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
                StepFlow::Done => break,
                StepFlow::Next(idx) => {
                    if idx == step_index {
                        let value = retry_count.entry(step_index).or_insert(0);
                        *value += 1;
                    } else {
                        retry_count.remove(&step_index);
                    }
                    step_index = idx;
                }
                StepFlow::GotoWorkflow(next_wf) => {
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

    fn resolve_operation_id(&self, operation_id: &str) -> Result<(String, String), RuntimeError> {
        self.op_index
            .get(operation_id)
            .map(|entry| (entry.method.clone(), entry.path.clone()))
            .ok_or_else(|| {
                RuntimeError(format!(
                    "operationId \"{operation_id}\" not found in loaded OpenAPI specs"
                ))
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
            .map_err(|err| RuntimeError(format!("sub-workflow {}: {}", step.workflow_id, err.0)))?;

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
    ) -> Result<StepFlow, RuntimeError> {
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
            return Ok(StepFlow::Next(step_idx + 1));
        }

        let action = self.find_matching_action(&step.on_failure, vars, result.response.as_ref());
        if let Some(action) = action {
            return self.execute_action(workflow, action, step_idx, true, retry_count, options);
        }

        if let Some(err) = &result.err {
            return Err(RuntimeError(format!("step {}: {err}", step.step_id)));
        }
        if let Some(resp) = &result.response {
            let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
            if body_preview.len() > 500 {
                body_preview.truncate(500);
                body_preview.push_str("...");
            }
            return Err(RuntimeError(format!(
                "step {}: success criteria not met (status={}, body={})",
                step.step_id, resp.status_code, body_preview
            )));
        }

        Err(RuntimeError(format!(
            "step {}: success criteria not met",
            step.step_id
        )))
    }

    fn find_matching_action<'a>(
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
    ) -> Result<StepFlow, RuntimeError> {
        match action.type_.as_str() {
            "end" => {
                if is_failure_path {
                    Err(RuntimeError(format!(
                        "step {}: workflow ended by onFailure action",
                        workflow.steps[current_idx].step_id
                    )))
                } else {
                    Ok(StepFlow::Done)
                }
            }
            "goto" => {
                if !action.step_id.is_empty() {
                    let idx = self
                        .find_step_index(workflow, &action.step_id)
                        .ok_or_else(|| {
                            RuntimeError(format!("goto: step \"{}\" not found", action.step_id))
                        })?;
                    return Ok(StepFlow::Next(idx));
                }
                if !action.workflow_id.is_empty() {
                    return Ok(StepFlow::GotoWorkflow(action.workflow_id.clone()));
                }
                Err(RuntimeError(
                    "goto: no stepId or workflowId specified".to_string(),
                ))
            }
            "retry" => {
                let mut limit = MAX_RETRIES_PER_STEP;
                if action.retry_limit > 0 {
                    limit = usize::try_from(action.retry_limit).unwrap_or(MAX_RETRIES_PER_STEP);
                }
                let current = retry_count.get(&current_idx).copied().unwrap_or(0);
                if current >= limit {
                    return Err(RuntimeError(format!(
                        "step {}: max retries ({limit}) exceeded",
                        workflow.steps[current_idx].step_id
                    )));
                }
                if action.retry_after > 0 {
                    sleep_with_checks(
                        Duration::from_secs(u64::try_from(action.retry_after).unwrap_or(0)),
                        options,
                    )?;
                }
                Ok(StepFlow::Next(current_idx))
            }
            _ => Ok(StepFlow::Next(current_idx + 1)),
        }
    }

    fn find_step_index(&self, workflow: &Workflow, step_id: &str) -> Option<usize> {
        self.step_indexes
            .get(&workflow.workflow_id)
            .and_then(|index| index.get(step_id).copied())
    }

    fn build_outputs(&self, workflow: &Workflow, vars: &VarStore) -> BTreeMap<String, Value> {
        let eval = ExpressionEvaluator::new(vars.eval_context(None));
        let mut outputs = BTreeMap::new();
        for (name, expr) in &workflow.outputs {
            outputs.insert(name.clone(), eval.evaluate(expr));
        }
        outputs
    }

    fn build_url_from_path(&self, op_path: &str, step: &Step, vars: &VarStore) -> String {
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
                    let step = workflow
                        .steps
                        .get(idx)
                        .cloned()
                        .ok_or_else(|| RuntimeError("invalid step index".to_string()))?;
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
                            return Err(RuntimeError("parallel step thread panicked".to_string()));
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
enum StepFlow {
    Next(usize),
    Done,
    GotoWorkflow(String),
}

/// Extract a value from XML/RSS body using an XPath expression.
fn extract_xpath(body: &[u8], expr: &str) -> Value {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };
    // Strip default namespace declarations so simple XPath expressions
    // work on both RSS 2.0 (no namespace) and Atom (xmlns="...") feeds.
    // Preserves prefixed namespaces like xmlns:media="...".
    let Ok(re) = Regex::new(r#"xmlns="[^"]*""#) else {
        return Value::Null;
    };
    let text = re.replace_all(text, "");
    let package = match sxd_document::parser::parse(&text) {
        Ok(p) => p,
        Err(_) => return Value::Null,
    };
    let doc = package.as_document();
    match sxd_xpath::evaluate_xpath(&doc, expr) {
        Ok(val) => {
            let s = val.string();
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Err(_) => Value::Null,
    }
}

fn evaluate_criterion(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> bool {
    match criterion.type_.as_str() {
        "regex" => {
            let context_value = eval.evaluate_string(&criterion.context);
            match Regex::new(&criterion.condition) {
                Ok(re) => re.is_match(&context_value),
                Err(_) => false,
            }
        }
        "jsonpath" => {
            if let Some(resp) = response {
                if resp.content_type == "xml" {
                    return is_truthy(&extract_xpath(&resp.body, &criterion.condition));
                }
            }
            let value = eval.evaluate(&format!("$response.body.{}", criterion.condition));
            is_truthy(&value)
        }
        _ => eval.evaluate_condition(&criterion.condition),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}

fn parse_method(operation_path: &str) -> (&str, &str) {
    let Some(idx) = operation_path.find(' ') else {
        return ("", operation_path);
    };
    if idx == 0 || idx > 7 {
        return ("", operation_path);
    }
    let candidate = &operation_path[..idx];
    let valid = matches!(
        candidate,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    );
    if valid {
        return (candidate, &operation_path[idx + 1..]);
    }
    ("", operation_path)
}

fn replace_path_params(path: &str, params: &BTreeMap<String, String>) -> String {
    let mut remaining = path;
    let mut out = String::with_capacity(path.len());

    loop {
        let Some(open) = remaining.find('{') else {
            out.push_str(remaining);
            break;
        };
        let Some(close_rel) = remaining[open + 1..].find('}') else {
            out.push_str(remaining);
            break;
        };
        let close = open + 1 + close_rel;
        out.push_str(&remaining[..open]);
        let key = &remaining[open + 1..close];
        if let Some(value) = params.get(key) {
            out.push_str(value);
        } else {
            out.push_str(&remaining[open..=close]);
        }
        remaining = &remaining[close + 1..];
    }

    out
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

fn resolve_payload(value: &serde_yaml::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(v) => Value::Bool(*v),
        serde_yaml::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::String(v) => {
            if v.starts_with('$') {
                eval.evaluate(v)
            } else {
                Value::String(v.clone())
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(resolve_payload(item, eval));
            }
            Value::Array(out)
        }
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                out.insert(key, resolve_payload(v, eval));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

fn to_json_path(expr: &str) -> String {
    if let Some(path) = expr.strip_prefix("$response.body.") {
        return path.to_string();
    }
    if let Some(path) = expr.strip_prefix("$response.body") {
        return path.trim_start_matches('.').to_string();
    }
    expr.to_string()
}

fn step_result_error(step_id: &str, result: &StepResult) -> RuntimeError {
    if let Some(err) = &result.err {
        return RuntimeError(format!("step {step_id}: {err}"));
    }
    if let Some(resp) = &result.response {
        let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
        if body_preview.len() > 500 {
            body_preview.truncate(500);
            body_preview.push_str("...");
        }
        return RuntimeError(format!(
            "step {step_id}: success criteria not met (status={}, body={})",
            resp.status_code, body_preview
        ));
    }
    RuntimeError(format!("step {step_id}: success criteria not met"))
}

fn sleep_with_checks(delay: Duration, options: &ExecutionOptions) -> Result<(), RuntimeError> {
    if delay.is_zero() {
        return Ok(());
    }

    let start = Instant::now();
    loop {
        options.check()?;
        let elapsed = start.elapsed();
        if elapsed >= delay {
            return Ok(());
        }
        let remaining = delay - elapsed;
        std::thread::sleep(remaining.min(SLEEP_CHECK_INTERVAL));
    }
}

fn can_execute_parallel(workflow: &Workflow) -> bool {
    !has_control_flow(workflow)
        && workflow
            .steps
            .iter()
            .all(|step| step.workflow_id.is_empty())
}

fn has_control_flow(workflow: &Workflow) -> bool {
    for step in &workflow.steps {
        for action in &step.on_success {
            if matches!(action.type_.as_str(), "goto" | "retry" | "end") {
                return true;
            }
        }
        for action in &step.on_failure {
            if matches!(action.type_.as_str(), "goto" | "retry" | "end") {
                return true;
            }
        }
    }
    false
}

fn build_levels(workflow: &Workflow) -> Result<Vec<Vec<usize>>, RuntimeError> {
    let mut step_id_to_index = BTreeMap::<String, usize>::new();
    for (idx, step) in workflow.steps.iter().enumerate() {
        step_id_to_index.insert(step.step_id.clone(), idx);
    }

    let mut deps = vec![BTreeSet::<usize>::new(); workflow.steps.len()];
    for (idx, step) in workflow.steps.iter().enumerate() {
        for dep_id in extract_step_refs(step) {
            if let Some(dep_idx) = step_id_to_index.get(&dep_id) {
                deps[idx].insert(*dep_idx);
            }
        }
    }

    let mut indegree = deps.iter().map(BTreeSet::len).collect::<Vec<_>>();
    let mut assigned = vec![false; workflow.steps.len()];
    let mut remaining = workflow.steps.len();
    let mut levels = Vec::<Vec<usize>>::new();

    while remaining > 0 {
        let mut level = Vec::new();
        for idx in 0..workflow.steps.len() {
            if !assigned[idx] && indegree[idx] == 0 {
                level.push(idx);
            }
        }
        if level.is_empty() {
            return Err(RuntimeError(format!(
                "dependency cycle detected in workflow \"{}\"",
                workflow.workflow_id
            )));
        }
        for idx in &level {
            assigned[*idx] = true;
            remaining -= 1;
            for dep_idx in 0..deps.len() {
                if deps[dep_idx].remove(idx) {
                    indegree[dep_idx] -= 1;
                }
            }
        }
        levels.push(level);
    }

    Ok(levels)
}

fn extract_step_refs(step: &Step) -> Vec<String> {
    let mut refs = BTreeSet::<String>::new();
    let pattern = Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")
        .unwrap_or_else(|err| panic!("failed to compile step-ref regex: {err}"));

    let mut scan = |s: &str| {
        for captures in pattern.captures_iter(s) {
            if let Some(m) = captures.get(1) {
                refs.insert(m.as_str().to_string());
            }
        }
    };

    scan(&step.operation_path);
    for p in &step.parameters {
        scan(&p.value);
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            scan_payload_refs(payload, &mut scan);
        }
    }
    for c in &step.success_criteria {
        scan(&c.condition);
        scan(&c.context);
    }
    for expr in step.outputs.values() {
        scan(expr);
    }
    for action in &step.on_success {
        for c in &action.criteria {
            scan(&c.condition);
        }
    }
    for action in &step.on_failure {
        for c in &action.criteria {
            scan(&c.condition);
        }
    }

    refs.into_iter().collect()
}

fn scan_payload_refs(value: &serde_yaml::Value, scan: &mut impl FnMut(&str)) {
    match value {
        serde_yaml::Value::String(s) => {
            if s.starts_with('$') {
                scan(s);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                scan_payload_refs(item, scan);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for (_, v) in map {
                scan_payload_refs(v, scan);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_levels, evaluate_criterion, extract_step_refs, has_control_flow, parse_method,
        ArazzoSpec, ClientConfig, Engine, EvalContext, ExecutionOptions, ExpressionEvaluator,
        OnAction, Response, RuntimeError, Step, StepEvent, SuccessCriterion, TraceHook, Workflow,
    };
    use arazzo_spec::{Info, RequestBody, SourceDescription};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};
    use url::Url;

    #[derive(Debug, Clone)]
    struct MockHttpResponse {
        status: u16,
        headers: BTreeMap<String, String>,
        body: String,
    }

    impl MockHttpResponse {
        fn json(status: u16, body: &str) -> Self {
            let mut headers = BTreeMap::new();
            headers.insert("Content-Type".to_string(), "application/json".to_string());
            Self {
                status,
                headers,
                body: body.to_string(),
            }
        }

        fn empty(status: u16) -> Self {
            Self {
                status,
                headers: BTreeMap::new(),
                body: String::new(),
            }
        }
    }

    struct TestServer {
        base_url: String,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn start_server<F>(handler: F) -> TestServer
    where
        F: Fn(String, String, BTreeMap<String, String>, String) -> MockHttpResponse
            + Send
            + Sync
            + 'static,
    {
        let server = match Server::http("127.0.0.1:0") {
            Ok(server) => server,
            Err(err) => panic!("binding test server: {err}"),
        };
        let base_url = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handler = Arc::new(handler);
        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match server.recv_timeout(Duration::from_millis(20)) {
                    Ok(Some(mut request)) => {
                        let method = request.method().as_str().to_string();
                        let url = request.url().to_string();
                        let mut headers = BTreeMap::new();
                        for header in request.headers() {
                            headers.insert(
                                header.field.as_str().to_string(),
                                header.value.as_str().to_string(),
                            );
                        }
                        let mut body = String::new();
                        let _ = request.as_reader().read_to_string(&mut body);

                        let response_data = handler(method, url, headers, body);
                        let mut response = TinyResponse::from_string(response_data.body)
                            .with_status_code(StatusCode(response_data.status));
                        for (name, value) in response_data.headers {
                            if let Ok(header) =
                                Header::from_bytes(name.as_bytes(), value.as_bytes())
                            {
                                response = response.with_header(header);
                            }
                        }
                        let _ = request.respond(response);
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });

        TestServer {
            base_url,
            stop,
            handle: Some(handle),
        }
    }

    fn start_server_concurrent<F>(handler: F) -> TestServer
    where
        F: Fn(String, String, BTreeMap<String, String>, String) -> MockHttpResponse
            + Send
            + Sync
            + 'static,
    {
        let server = match Server::http("127.0.0.1:0") {
            Ok(server) => server,
            Err(err) => panic!("binding test server: {err}"),
        };
        let base_url = format!("http://{}", server.server_addr());
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handler = Arc::new(handler);
        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match server.recv_timeout(Duration::from_millis(20)) {
                    Ok(Some(mut request)) => {
                        let handler = Arc::clone(&handler);
                        let _worker = thread::spawn(move || {
                            let method = request.method().as_str().to_string();
                            let url = request.url().to_string();
                            let mut headers = BTreeMap::new();
                            for header in request.headers() {
                                headers.insert(
                                    header.field.as_str().to_string(),
                                    header.value.as_str().to_string(),
                                );
                            }
                            let mut body = String::new();
                            let _ = request.as_reader().read_to_string(&mut body);

                            let response_data = handler(method, url, headers, body);
                            let mut response = TinyResponse::from_string(response_data.body)
                                .with_status_code(StatusCode(response_data.status));
                            for (name, value) in response_data.headers {
                                if let Ok(header) =
                                    Header::from_bytes(name.as_bytes(), value.as_bytes())
                                {
                                    response = response.with_header(header);
                                }
                            }
                            let _ = request.respond(response);
                        });
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });

        TestServer {
            base_url,
            stop,
            handle: Some(handle),
        }
    }

    fn make_spec(workflows: Vec<Workflow>) -> ArazzoSpec {
        ArazzoSpec {
            arazzo: "1.0.0".to_string(),
            info: Info {
                title: "test".to_string(),
                version: "1.0.0".to_string(),
                description: String::new(),
            },
            source_descriptions: vec![SourceDescription {
                name: "test".to_string(),
                url: "http://localhost".to_string(),
                type_: "openapi".to_string(),
            }],
            workflows,
            components: None,
        }
    }

    fn new_test_engine(base_url: &str, mut spec: ArazzoSpec) -> Engine {
        if let Some(source) = spec.source_descriptions.get_mut(0) {
            source.url = base_url.to_string();
        }
        match Engine::new(spec) {
            Ok(engine) => engine,
            Err(err) => panic!("creating engine: {err}"),
        }
    }

    fn success_200() -> Vec<SuccessCriterion> {
        vec![SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        }]
    }

    fn to_yaml(value: Value) -> serde_yaml::Value {
        match serde_yaml::to_value(value) {
            Ok(v) => v,
            Err(err) => panic!("converting json to yaml: {err}"),
        }
    }

    fn header_value(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
        for (key, value) in headers {
            if key.eq_ignore_ascii_case(name) {
                return Some(value.clone());
            }
        }
        None
    }

    #[derive(Default)]
    struct TestTraceHook {
        before_events: Mutex<Vec<StepEvent>>,
        after_events: Mutex<Vec<StepEvent>>,
    }

    impl TraceHook for TestTraceHook {
        fn before_step(&self, event: &StepEvent) {
            match self.before_events.lock() {
                Ok(mut guard) => guard.push(event.clone()),
                Err(_) => panic!("capturing before_step event"),
            }
        }

        fn after_step(&self, event: &StepEvent) {
            match self.after_events.lock() {
                Ok(mut guard) => guard.push(event.clone()),
                Err(_) => panic!("capturing after_step event"),
            }
        }
    }

    #[test]
    fn execute_sequential_steps() {
        let server = start_server(|_method, url, _headers, _body| match url.as_str() {
            "/step1" => MockHttpResponse::json(200, r#"{"value":"hello"}"#),
            "/step2" => MockHttpResponse::json(200, r#"{"result":"world"}"#),
            _ => MockHttpResponse::empty(404),
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "sequential".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/step1".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/step2".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("sequential", BTreeMap::new());
        match result {
            Ok(outputs) => assert!(outputs.is_empty()),
            Err(err) => panic!("expected success, got: {err}"),
        }
    }

    #[test]
    fn execute_failure_no_handler() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(500, r#"{"error":"server error"}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "fail-no-handler".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/fail".to_string(),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("fail-no-handler", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected error for unhandled failure"),
            Err(err) => err,
        };
        assert!(
            err.0
                .contains("step s1: success criteria not met (status=500"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn execute_on_failure_end() {
        let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

        let spec = make_spec(vec![Workflow {
            workflow_id: "fail-end".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/fail".to_string(),
                    success_criteria: success_200(),
                    on_failure: vec![OnAction {
                        type_: "end".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/should-not-reach".to_string(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("fail-end", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected error from onFailure end action"),
            Err(err) => err,
        };
        assert_eq!(err.0, "step s1: workflow ended by onFailure action");
    }

    #[test]
    fn execute_on_success_end() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("recording request path"),
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "success-end".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/ok".to_string(),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: "end".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/should-not-reach".to_string(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("success-end", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert_eq!(observed, vec!["/ok".to_string()]);
    }

    #[test]
    fn execute_on_failure_goto() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("recording request path"),
            }
            match url.as_str() {
                "/fail" => MockHttpResponse::empty(500),
                "/fallback" => MockHttpResponse::json(200, r#"{"fallback":true}"#),
                _ => MockHttpResponse::empty(404),
            }
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "fail-goto".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/fail".to_string(),
                    success_criteria: success_200(),
                    on_failure: vec![OnAction {
                        type_: "goto".to_string(),
                        step_id: "fallback".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "skipped".to_string(),
                    operation_path: "/should-not-reach".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "fallback".to_string(),
                    operation_path: "/fallback".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("fail-goto", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert_eq!(observed.len(), 2);
        assert!(observed.iter().any(|p| p == "/fail"));
        assert!(observed.iter().any(|p| p == "/fallback"));
        assert!(!observed.iter().any(|p| p == "/should-not-reach"));
    }

    #[test]
    fn execute_on_success_goto() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("recording request path"),
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "success-goto".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/start".to_string(),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: "goto".to_string(),
                        step_id: "s3".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/skipped".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    operation_path: "/target".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("success-goto", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert!(!observed.iter().any(|p| p == "/skipped"));
        assert_eq!(observed, vec!["/start".to_string(), "/target".to_string()]);
    }

    #[test]
    fn execute_on_failure_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current < 3 {
                return MockHttpResponse::empty(500);
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/flaky".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("retry", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success after retries, got: {err}");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn execute_retry_exceeds_max() {
        let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry-max".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/always-fail".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("retry-max", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected max-retries error"),
            Err(err) => err,
        };
        assert_eq!(err.0, "step s1: max retries (3) exceeded");
    }

    #[test]
    fn execute_retry_custom_limit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current <= 5 {
                return MockHttpResponse::empty(500);
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry-limit".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/flaky".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    retry_limit: 6,
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("retry-limit", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn execute_retry_custom_limit_exceeded() {
        let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry-limit-exceeded".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/always-fail".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    retry_limit: 2,
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("retry-limit-exceeded", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected retry limit exceeded error"),
            Err(err) => err,
        };
        assert_eq!(err.0, "step s1: max retries (2) exceeded");
    }

    #[test]
    fn execute_retry_with_delay() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current < 2 {
                return MockHttpResponse::empty(500);
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry-delay".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/flaky".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    retry_after: 1,
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let started = Instant::now();
        let result = engine.execute("retry-delay", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success with retry delay, got: {err}");
        }
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn execute_retry_delay_honors_execution_timeout() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            calls_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(500)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "retry-delay-timeout".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/flaky".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "retry".to_string(),
                    retry_after: 2,
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let started = Instant::now();
        let result = engine.execute_with_options(
            "retry-delay-timeout",
            BTreeMap::new(),
            ExecutionOptions::with_timeout(Duration::from_millis(120)),
        );
        let err = match result {
            Ok(_) => panic!("expected execution timeout"),
            Err(err) => err,
        };
        assert_eq!(err.0, "execution timeout exceeded");
        assert!(started.elapsed() < Duration::from_millis(900));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn execute_honors_external_cancel_flag() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            calls_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "cancelled".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/ok".to_string(),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let result = engine.execute_with_options(
            "cancelled",
            BTreeMap::new(),
            ExecutionOptions::with_cancel_flag(cancel_flag),
        );
        let err = match result {
            Ok(_) => panic!("expected execution cancellation"),
            Err(err) => err,
        };
        assert_eq!(err.0, "execution cancelled");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn execute_respects_client_rate_limit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            calls_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "rate-limit".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/one".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/two".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut cfg = ClientConfig::default();
        cfg.rate_limit.requests_per_second = 1.0;
        cfg.rate_limit.burst = 1;

        let mut engine = match Engine::with_client_config(spec, cfg) {
            Ok(engine) => engine,
            Err(err) => panic!("creating engine: {err}"),
        };
        engine.base_url = server.base_url.clone();

        let started = Instant::now();
        let result = engine.execute("rate-limit", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }
        assert!(started.elapsed() >= Duration::from_millis(850));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn execute_on_failure_criteria_matching() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("recording request path"),
            }
            match url.as_str() {
                "/main" => MockHttpResponse::empty(429),
                "/rate-limit-handler" => MockHttpResponse::empty(200),
                _ => MockHttpResponse::empty(404),
            }
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "criteria-match".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/main".to_string(),
                    success_criteria: success_200(),
                    on_failure: vec![
                        OnAction {
                            type_: "goto".to_string(),
                            step_id: "rate-handler".to_string(),
                            criteria: vec![SuccessCriterion {
                                condition: "$statusCode == 429".to_string(),
                                ..SuccessCriterion::default()
                            }],
                            ..OnAction::default()
                        },
                        OnAction {
                            type_: "goto".to_string(),
                            step_id: "server-error-handler".to_string(),
                            criteria: vec![SuccessCriterion {
                                condition: "$statusCode == 500".to_string(),
                                ..SuccessCriterion::default()
                            }],
                            ..OnAction::default()
                        },
                        OnAction {
                            type_: "end".to_string(),
                            ..OnAction::default()
                        },
                    ],
                    ..Step::default()
                },
                Step {
                    step_id: "rate-handler".to_string(),
                    operation_path: "/rate-limit-handler".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "server-error-handler".to_string(),
                    operation_path: "/should-not-reach".to_string(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("criteria-match", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert!(observed.iter().any(|path| path == "/main"));
        assert!(observed.iter().any(|path| path == "/rate-limit-handler"));
    }

    #[test]
    fn execute_on_failure_criteria_none_match() {
        let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(418));

        let spec = make_spec(vec![Workflow {
            workflow_id: "no-criteria-match".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/teapot".to_string(),
                success_criteria: success_200(),
                on_failure: vec![
                    OnAction {
                        type_: "retry".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 429".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: "goto".to_string(),
                        step_id: "handler".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 500".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                ],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("no-criteria-match", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected error when no criteria match"),
            Err(err) => err,
        };
        assert!(err
            .0
            .contains("step s1: success criteria not met (status=418"));
    }

    #[test]
    fn execute_goto_errors() {
        let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

        let bad_goto_spec = make_spec(vec![Workflow {
            workflow_id: "bad-goto".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/fail".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "goto".to_string(),
                    step_id: "nonexistent".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut bad_goto_engine = new_test_engine(&server.base_url, bad_goto_spec);
        let bad_goto_result = bad_goto_engine.execute("bad-goto", BTreeMap::new());
        let bad_goto_err = match bad_goto_result {
            Ok(_) => panic!("expected error for goto to missing step"),
            Err(err) => err,
        };
        assert_eq!(bad_goto_err.0, r#"goto: step "nonexistent" not found"#);

        let empty_goto_spec = make_spec(vec![Workflow {
            workflow_id: "goto-no-target".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/fail".to_string(),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: "goto".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut empty_goto_engine = new_test_engine(&server.base_url, empty_goto_spec);
        let empty_goto_result = empty_goto_engine.execute("goto-no-target", BTreeMap::new());
        let empty_goto_err = match empty_goto_result {
            Ok(_) => panic!("expected error for goto without step/workflow target"),
            Err(err) => err,
        };
        assert_eq!(empty_goto_err.0, "goto: no stepId or workflowId specified");
    }

    #[test]
    fn execute_workflow_not_found() {
        let spec = make_spec(Vec::new());
        let mut engine = new_test_engine("http://localhost", spec);
        let result = engine.execute("nonexistent", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected workflow-not-found error"),
            Err(err) => err,
        };
        assert_eq!(err.0, r#"workflow "nonexistent" not found"#);
    }

    #[test]
    fn execute_default_sequential_without_on_success() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url),
                Err(_) => panic!("recording request path"),
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "default-seq".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    operation_path: "/c".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("default-seq", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert_eq!(
            observed,
            vec!["/a".to_string(), "/b".to_string(), "/c".to_string()]
        );
    }

    #[test]
    fn execute_unknown_action_type_moves_to_next_step() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url),
                Err(_) => panic!("recording request path"),
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "unknown-action".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: "unknown-type".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("unknown-action", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured paths"),
        };
        assert_eq!(observed, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn execute_response_header_expression() {
        let server = start_server(|_method, _url, _headers, _body| {
            let mut response = MockHttpResponse::json(200, r#"{"ok":true}"#);
            response
                .headers
                .insert("X-Request-Id".to_string(), "abc-123".to_string());
            response
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "header-extract".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/test".to_string(),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "request_id".to_string(),
                    "$response.header.X-Request-Id".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "request_id".to_string(),
                "$steps.s1.outputs.request_id".to_string(),
            )]),
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("header-extract", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("request_id"), Some(&json!("abc-123")));
    }

    #[test]
    fn execute_env_expression() {
        std::env::set_var("ARAZZO_RUNTIME_TEST_TOKEN", "secret-42");
        let server = start_server(|_method, _url, headers, _body| {
            let auth = header_value(&headers, "Authorization").unwrap_or_default();
            MockHttpResponse::json(200, &format!(r#"{{"auth":"{auth}"}}"#))
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "env-test".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/protected".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "Authorization".to_string(),
                    in_: "header".to_string(),
                    value: "$env.ARAZZO_RUNTIME_TEST_TOKEN".to_string(),
                    ..arazzo_spec::Parameter::default()
                }],
                success_criteria: success_200(),
                outputs: BTreeMap::from([("auth".to_string(), "$response.body.auth".to_string())]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("auth".to_string(), "$steps.s1.outputs.auth".to_string())]),
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("env-test", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("auth"), Some(&json!("secret-42")));
    }

    #[test]
    fn build_url_encodes_query_params() {
        let engine = new_test_engine("http://localhost", make_spec(Vec::new()));
        let mut vars = super::VarStore::default();
        vars.set_input("q", json!("hello world&more=stuff"));

        let step = Step {
            operation_path: "/search".to_string(),
            parameters: vec![
                arazzo_spec::Parameter {
                    name: "q".to_string(),
                    in_: "query".to_string(),
                    value: "$inputs.q".to_string(),
                    ..arazzo_spec::Parameter::default()
                },
                arazzo_spec::Parameter {
                    name: "tag".to_string(),
                    in_: "query".to_string(),
                    value: "a=b".to_string(),
                    ..arazzo_spec::Parameter::default()
                },
            ],
            ..Step::default()
        };

        let url = engine.build_url_from_path("/search", &step, &vars);
        let parsed = match Url::parse(&url) {
            Ok(v) => v,
            Err(err) => panic!("parsing url {url}: {err}"),
        };
        let query = parsed
            .query_pairs()
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(query.get("q"), Some(&"hello world&more=stuff".to_string()));
        assert_eq!(query.get("tag"), Some(&"a=b".to_string()));
    }

    #[test]
    fn build_url_avoids_double_slash() {
        let engine = new_test_engine("https://api.example.com/", make_spec(Vec::new()));
        let vars = super::VarStore::default();
        let step = Step {
            operation_path: "/users".to_string(),
            ..Step::default()
        };

        let url = engine.build_url_from_path(&step.operation_path, &step, &vars);
        assert_eq!(url, "https://api.example.com/users");
        assert!(!url.contains("//users"));
    }

    #[test]
    fn execute_request_body_content_type_and_method_selection() {
        let put_method = Arc::new(Mutex::new(String::new()));
        let put_method_ref = Arc::clone(&put_method);
        let put_content_type = Arc::new(Mutex::new(String::new()));
        let put_content_type_ref = Arc::clone(&put_content_type);
        let put_server = start_server(move |method, _url, headers, _body| {
            let content_type = header_value(&headers, "Content-Type").unwrap_or_default();
            match put_content_type_ref.lock() {
                Ok(mut guard) => *guard = content_type,
                Err(_) => panic!("capturing content type"),
            }
            match put_method_ref.lock() {
                Ok(mut guard) => *guard = method,
                Err(_) => panic!("capturing HTTP method"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let put_spec = make_spec(vec![Workflow {
            workflow_id: "put".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "PUT /users/123".to_string(),
                request_body: Some(RequestBody {
                    content_type: "application/x-www-form-urlencoded".to_string(),
                    payload: Some(to_yaml(json!({"key":"val"}))),
                    ..RequestBody::default()
                }),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut put_engine = new_test_engine(&put_server.base_url, put_spec);
        let put_result = put_engine.execute("put", BTreeMap::new());
        if let Err(err) = put_result {
            panic!("expected PUT workflow success, got: {err}");
        }
        let captured_put = match put_method.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading PUT method"),
        };
        assert_eq!(captured_put, "PUT");
        let captured_put_content_type = match put_content_type.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading PUT content type"),
        };
        assert_eq!(
            captured_put_content_type,
            "application/x-www-form-urlencoded"
        );

        let delete_method = Arc::new(Mutex::new(String::new()));
        let delete_method_ref = Arc::clone(&delete_method);
        let delete_server = start_server(move |method, _url, _headers, _body| {
            match delete_method_ref.lock() {
                Ok(mut guard) => *guard = method,
                Err(_) => panic!("capturing DELETE method"),
            }
            MockHttpResponse::json(204, "{}")
        });
        let delete_spec = make_spec(vec![Workflow {
            workflow_id: "delete".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "DELETE /users/123".to_string(),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 204".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut delete_engine = new_test_engine(&delete_server.base_url, delete_spec);
        let delete_result = delete_engine.execute("delete", BTreeMap::new());
        if let Err(err) = delete_result {
            panic!("expected DELETE workflow success, got: {err}");
        }
        let captured_delete = match delete_method.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading DELETE method"),
        };
        assert_eq!(captured_delete, "DELETE");

        let patch_method = Arc::new(Mutex::new(String::new()));
        let patch_method_ref = Arc::clone(&patch_method);
        let patch_server = start_server(move |method, _url, _headers, _body| {
            match patch_method_ref.lock() {
                Ok(mut guard) => *guard = method,
                Err(_) => panic!("capturing PATCH method"),
            }
            MockHttpResponse::json(200, r#"{"patched":true}"#)
        });
        let patch_spec = make_spec(vec![Workflow {
            workflow_id: "patch".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "PATCH /items/42".to_string(),
                request_body: Some(RequestBody {
                    payload: Some(to_yaml(json!({"status":"active"}))),
                    ..RequestBody::default()
                }),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut patch_engine = new_test_engine(&patch_server.base_url, patch_spec);
        let patch_result = patch_engine.execute("patch", BTreeMap::new());
        if let Err(err) = patch_result {
            panic!("expected PATCH workflow success, got: {err}");
        }
        let captured_patch = match patch_method.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading PATCH method"),
        };
        assert_eq!(captured_patch, "PATCH");

        let get_method = Arc::new(Mutex::new(String::new()));
        let get_method_ref = Arc::clone(&get_method);
        let get_server = start_server(move |method, _url, _headers, _body| {
            match get_method_ref.lock() {
                Ok(mut guard) => *guard = method,
                Err(_) => panic!("capturing GET method"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });
        let get_spec = make_spec(vec![Workflow {
            workflow_id: "fallback-get".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/health".to_string(),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut get_engine = new_test_engine(&get_server.base_url, get_spec);
        let get_result = get_engine.execute("fallback-get", BTreeMap::new());
        if let Err(err) = get_result {
            panic!("expected fallback GET success, got: {err}");
        }
        let captured_get = match get_method.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading GET method"),
        };
        assert_eq!(captured_get, "GET");
    }

    #[test]
    fn execute_sub_workflow_step() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(200, r#"{"token":"xyz-789"}"#)
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    workflow_id: "child".to_string(),
                    ..Step::default()
                }],
                outputs: BTreeMap::from([(
                    "token".to_string(),
                    "$steps.call-child.outputs.token".to_string(),
                )]),
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "get-token".to_string(),
                    operation_path: "/auth".to_string(),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([(
                        "token".to_string(),
                        "$response.body.token".to_string(),
                    )]),
                    ..Step::default()
                }],
                outputs: BTreeMap::from([(
                    "token".to_string(),
                    "$steps.get-token.outputs.token".to_string(),
                )]),
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("parent", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("token"), Some(&json!("xyz-789")));
    }

    #[test]
    fn execute_sub_workflow_with_inputs() {
        let got_path = Arc::new(Mutex::new(String::new()));
        let got_path_ref = Arc::clone(&got_path);
        let server = start_server(move |_method, url, _headers, _body| {
            match got_path_ref.lock() {
                Ok(mut guard) => *guard = url,
                Err(_) => panic!("capturing request path"),
            }
            MockHttpResponse::json(200, r#"{"name":"Alice"}"#)
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    workflow_id: "child".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "userId".to_string(),
                        value: "$inputs.uid".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "get-user".to_string(),
                    operation_path: "/users/{userId}".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "userId".to_string(),
                        in_: "path".to_string(),
                        value: "$inputs.userId".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let inputs = BTreeMap::from([("uid".to_string(), json!("42"))]);
        let result = engine.execute("parent", inputs);
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }
        let observed = match got_path.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading captured path"),
        };
        assert_eq!(observed, "/users/42");
    }

    #[test]
    fn execute_sub_workflow_failure() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(500, r#"{"error":"fail"}"#)
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    workflow_id: "child".to_string(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "s1".to_string(),
                    operation_path: "/fail".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("parent", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected child workflow failure"),
            Err(err) => err,
        };
        assert!(err.0.contains("sub-workflow child"));
    }

    #[test]
    fn execute_goto_workflow() {
        let server = start_server(|_method, url, _headers, _body| match url.as_str() {
            "/main" => MockHttpResponse::json(500, "{}"),
            "/fallback" => MockHttpResponse::json(200, r#"{"fallback":true}"#),
            _ => MockHttpResponse::empty(404),
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "main-wf".to_string(),
                steps: vec![Step {
                    step_id: "s1".to_string(),
                    operation_path: "/main".to_string(),
                    success_criteria: success_200(),
                    on_failure: vec![OnAction {
                        type_: "goto".to_string(),
                        workflow_id: "fallback-wf".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "fallback-wf".to_string(),
                steps: vec![Step {
                    step_id: "fb".to_string(),
                    operation_path: "/fallback".to_string(),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([(
                        "ok".to_string(),
                        "$response.body.fallback".to_string(),
                    )]),
                    ..Step::default()
                }],
                outputs: BTreeMap::from([("ok".to_string(), "$steps.fb.outputs.ok".to_string())]),
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let result = engine.execute("main-wf", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("ok"), Some(&json!(true)));
    }

    #[test]
    fn execute_recursion_guard() {
        let spec = make_spec(vec![
            Workflow {
                workflow_id: "wf-a".to_string(),
                steps: vec![Step {
                    step_id: "call-b".to_string(),
                    workflow_id: "wf-b".to_string(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "wf-b".to_string(),
                steps: vec![Step {
                    step_id: "call-a".to_string(),
                    workflow_id: "wf-a".to_string(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine("http://localhost", spec);
        let result = engine.execute("wf-a", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected recursion guard error"),
            Err(err) => err,
        };
        assert!(err.0.contains("max call depth"));
    }

    #[test]
    fn execute_sub_workflow_not_found() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "parent".to_string(),
            steps: vec![Step {
                step_id: "call-missing".to_string(),
                workflow_id: "nonexistent".to_string(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine("http://localhost", spec);
        let result = engine.execute("parent", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected missing sub-workflow error"),
            Err(err) => err,
        };
        assert!(err.0.contains(r#"workflow "nonexistent" not found"#));
    }

    #[test]
    fn load_openapi_spec_and_resolve_operation_ids() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            ..Workflow::default()
        }]);
        let mut engine = new_test_engine("http://localhost", spec);

        let openapi = br#"
openapi: "3.0.0"
paths:
  /users:
    get:
      operationId: listUsers
    post:
      operationId: createUser
  /users/{id}:
    delete:
      operationId: deleteUser
"#;

        if let Err(err) = engine.load_openapi_spec(openapi) {
            panic!("failed loading OpenAPI: {err}");
        }

        let list = match engine.resolve_operation_id("listUsers") {
            Ok(v) => v,
            Err(err) => panic!("resolving listUsers: {err}"),
        };
        assert_eq!(list, ("GET".to_string(), "/users".to_string()));

        let create = match engine.resolve_operation_id("createUser") {
            Ok(v) => v,
            Err(err) => panic!("resolving createUser: {err}"),
        };
        assert_eq!(create, ("POST".to_string(), "/users".to_string()));

        let delete = match engine.resolve_operation_id("deleteUser") {
            Ok(v) => v,
            Err(err) => panic!("resolving deleteUser: {err}"),
        };
        assert_eq!(delete, ("DELETE".to_string(), "/users/{id}".to_string()));
    }

    #[test]
    fn load_openapi_spec_not_found_and_skips_non_http_fields() {
        let spec = make_spec(Vec::new());
        let mut engine = new_test_engine("http://localhost", spec);

        let openapi = br#"
openapi: "3.0.0"
paths:
  /items:
    parameters:
      - name: format
    get:
      operationId: listItems
"#;

        if let Err(err) = engine.load_openapi_spec(openapi) {
            panic!("failed loading OpenAPI: {err}");
        }

        let list = match engine.resolve_operation_id("listItems") {
            Ok(v) => v,
            Err(err) => panic!("resolving listItems: {err}"),
        };
        assert_eq!(list, ("GET".to_string(), "/items".to_string()));

        let missing = engine.resolve_operation_id("nonexistent");
        assert!(missing.is_err());
    }

    #[test]
    fn execute_operation_id_and_path_params() {
        let got_method = Arc::new(Mutex::new(String::new()));
        let got_path = Arc::new(Mutex::new(String::new()));
        let got_method_ref = Arc::clone(&got_method);
        let got_path_ref = Arc::clone(&got_path);
        let server = start_server(move |method, url, _headers, _body| {
            match got_method_ref.lock() {
                Ok(mut guard) => *guard = method,
                Err(_) => panic!("capturing method"),
            }
            match got_path_ref.lock() {
                Ok(mut guard) => *guard = url,
                Err(_) => panic!("capturing path"),
            }
            MockHttpResponse::json(200, r#"{"users":[]}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_id: "getUser".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "id".to_string(),
                    in_: "path".to_string(),
                    value: "$inputs.userId".to_string(),
                    ..arazzo_spec::Parameter::default()
                }],
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        let openapi =
            br#"{"openapi":"3.0.0","paths":{"/users/{id}":{"get":{"operationId":"getUser"}}}}"#;
        if let Err(err) = engine.load_openapi_spec(openapi) {
            panic!("loading OpenAPI: {err}");
        }

        let inputs = BTreeMap::from([("userId".to_string(), json!("42"))]);
        let result = engine.execute("wf", inputs);
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let method = match got_method.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading method"),
        };
        let path = match got_path.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading path"),
        };
        assert_eq!(method, "GET");
        assert_eq!(path, "/users/42");
    }

    #[test]
    fn execute_operation_id_not_loaded() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_id: "listUsers".to_string(),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        let mut engine = new_test_engine("http://localhost", spec);
        let result = engine.execute("wf", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected unresolved operationId error"),
            Err(err) => err,
        };
        assert!(err.0.contains("operationId"));
    }

    #[test]
    fn dry_run_captures_requests_and_headers() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "GET /users".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "POST /items".to_string(),
                    request_body: Some(RequestBody {
                        payload: Some(to_yaml(json!({"name":"test"}))),
                        ..RequestBody::default()
                    }),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine("http://localhost", spec);
        engine.set_dry_run_mode(true);
        let result = engine.execute("wf", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let reqs = engine.dry_run_requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].method, "GET");
        assert!(reqs[0].url.ends_with("/users"));
        assert_eq!(reqs[0].step_id, "s1");

        assert_eq!(reqs[1].method, "POST");
        assert!(reqs[1].url.ends_with("/items"));
        assert_eq!(reqs[1].step_id, "s2");
        assert_eq!(reqs[1].body, Some(json!({"name":"test"})));
    }

    #[test]
    fn dry_run_resolves_expressions_and_skips_http_calls() {
        let hit_count = Arc::new(AtomicUsize::new(0));
        let hit_count_ref = Arc::clone(&hit_count);
        let server = start_server(move |_method, _url, _headers, _body| {
            hit_count_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(500)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "GET /users/{id}".to_string(),
                parameters: vec![
                    arazzo_spec::Parameter {
                        name: "id".to_string(),
                        in_: "path".to_string(),
                        value: "$inputs.userId".to_string(),
                        ..arazzo_spec::Parameter::default()
                    },
                    arazzo_spec::Parameter {
                        name: "Authorization".to_string(),
                        in_: "header".to_string(),
                        value: "$inputs.token".to_string(),
                        ..arazzo_spec::Parameter::default()
                    },
                    arazzo_spec::Parameter {
                        name: "format".to_string(),
                        in_: "query".to_string(),
                        value: "json".to_string(),
                        ..arazzo_spec::Parameter::default()
                    },
                ],
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_dry_run_mode(true);
        let inputs = BTreeMap::from([
            ("userId".to_string(), json!("42")),
            ("token".to_string(), json!("Bearer secret")),
        ]);
        let result = engine.execute("wf", inputs);
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        assert_eq!(hit_count.load(Ordering::Relaxed), 0);
        let reqs = engine.dry_run_requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].url.contains("/users/42"));
        assert!(reqs[0].url.contains("format=json"));
        assert_eq!(
            reqs[0].headers.get("Authorization"),
            Some(&"Bearer secret".to_string())
        );
    }

    #[test]
    fn dry_run_multi_step_and_custom_headers() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/create".to_string(),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("id".to_string(), "$response.body.id".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/get/{id}".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "id".to_string(),
                        in_: "path".to_string(),
                        value: "$steps.s1.outputs.id".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    operation_path: "PUT /data".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "X-Custom".to_string(),
                        in_: "header".to_string(),
                        value: "custom-value".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    request_body: Some(RequestBody {
                        content_type: "application/xml".to_string(),
                        payload: Some(to_yaml(json!({"key":"val"}))),
                        ..RequestBody::default()
                    }),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine("http://localhost", spec);
        engine.set_dry_run_mode(true);
        let result = engine.execute("wf", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let reqs = engine.dry_run_requests();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0].step_id, "s1");
        assert_eq!(reqs[1].step_id, "s2");
        assert_eq!(reqs[2].step_id, "s3");
        assert_eq!(reqs[2].method, "PUT");
        assert_eq!(
            reqs[2].headers.get("Content-Type"),
            Some(&"application/xml".to_string())
        );
        assert_eq!(
            reqs[2].headers.get("X-Custom"),
            Some(&"custom-value".to_string())
        );
    }

    #[test]
    fn execute_parallel_independent_steps() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_ref = Arc::clone(&hits);
        let server = start_server(move |_method, _url, _headers, _body| {
            hits_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "parallel-ind".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    operation_path: "/c".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("parallel-ind", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }
        assert_eq!(hits.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn execute_parallel_runs_independent_steps_concurrently() {
        let delay = Duration::from_millis(300);
        let server = start_server_concurrent(move |_method, _url, _headers, _body| {
            std::thread::sleep(delay);
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "parallel-speed".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut sequential = new_test_engine(&server.base_url, spec.clone());
        let seq_started = Instant::now();
        let seq_result = sequential.execute("parallel-speed", BTreeMap::new());
        if let Err(err) = seq_result {
            panic!("expected sequential success, got: {err}");
        }
        let seq_elapsed = seq_started.elapsed();

        let mut parallel = new_test_engine(&server.base_url, spec);
        parallel.set_parallel_mode(true);
        let par_started = Instant::now();
        let par_result = parallel.execute("parallel-speed", BTreeMap::new());
        if let Err(err) = par_result {
            panic!("expected parallel success, got: {err}");
        }
        let par_elapsed = par_started.elapsed();

        assert!(
            par_elapsed + Duration::from_millis(150) < seq_elapsed,
            "expected true concurrency, got sequential={seq_elapsed:?}, parallel={par_elapsed:?}"
        );
    }

    #[test]
    fn execute_parallel_with_dependencies() {
        let server = start_server(|_method, url, _headers, _body| match url.as_str() {
            "/a" => MockHttpResponse::json(200, r#"{"id":"42"}"#),
            "/b?id=42" => MockHttpResponse::json(200, r#"{"name":"Alice"}"#),
            _ => MockHttpResponse::json(200, "{}"),
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "parallel-dep".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("id".to_string(), "$response.body.id".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "id".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.a.outputs.id".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([(
                        "name".to_string(),
                        "$response.body.name".to_string(),
                    )]),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::from([("name".to_string(), "$steps.b.outputs.name".to_string())]),
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("parallel-dep", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("name"), Some(&json!("Alice")));
    }

    #[test]
    fn execute_parallel_fallback_on_control_flow() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url),
                Err(_) => panic!("capturing path"),
            }
            MockHttpResponse::empty(200)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "cf-fallback".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: "end".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("cf-fallback", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading paths"),
        };
        assert_eq!(observed, vec!["/a".to_string()]);
    }

    #[test]
    fn execute_parallel_fallback_on_subworkflow() {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths_ref = Arc::clone(&paths);
        let server = start_server(move |_method, url, _headers, _body| {
            match paths_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("capturing path"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![
                    Step {
                        step_id: "call-child".to_string(),
                        workflow_id: "child".to_string(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "after".to_string(),
                        operation_path: "/after".to_string(),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "child-step".to_string(),
                    operation_path: "/child".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("parent", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let observed = match paths.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading paths"),
        };
        assert_eq!(observed, vec!["/child".to_string(), "/after".to_string()]);
    }

    #[test]
    fn execute_parallel_step_failure() {
        let server = start_server(|_method, url, _headers, _body| match url.as_str() {
            "/ok" => MockHttpResponse::json(200, r#"{"ok":true}"#),
            "/fail" => MockHttpResponse::json(500, r#"{"error":"boom"}"#),
            _ => MockHttpResponse::empty(404),
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "parallel-fail".to_string(),
            steps: vec![
                Step {
                    step_id: "ok".to_string(),
                    operation_path: "/ok".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "fail".to_string(),
                    operation_path: "/fail".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("parallel-fail", BTreeMap::new());
        let err = match result {
            Ok(_) => panic!("expected failure"),
            Err(err) => err,
        };
        assert!(err.0.contains("step fail"));
    }

    #[test]
    fn execute_parallel_outputs_preserved_and_diamond_dependency() {
        let request_order = Arc::new(Mutex::new(Vec::<String>::new()));
        let request_order_ref = Arc::clone(&request_order);
        let server = start_server(move |_method, url, _headers, _body| {
            match request_order_ref.lock() {
                Ok(mut guard) => guard.push(url.clone()),
                Err(_) => panic!("capturing request order"),
            }
            match url.as_str() {
                "/a" => MockHttpResponse::json(200, r#"{"val":"alpha"}"#),
                "/b?x=alpha" => MockHttpResponse::json(200, r#"{"val":"beta"}"#),
                "/c?x=alpha" => MockHttpResponse::json(200, r#"{"val":"gamma"}"#),
                "/d?y=beta&z=gamma" => MockHttpResponse::json(200, r#"{"ok":true}"#),
                _ => MockHttpResponse::json(200, r#"{"val":"unknown"}"#),
            }
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "diamond".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("x".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.a.outputs.x".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("y".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    operation_path: "/c".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.a.outputs.x".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("z".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "d".to_string(),
                    operation_path: "/d".to_string(),
                    parameters: vec![
                        arazzo_spec::Parameter {
                            name: "y".to_string(),
                            in_: "query".to_string(),
                            value: "$steps.b.outputs.y".to_string(),
                            ..arazzo_spec::Parameter::default()
                        },
                        arazzo_spec::Parameter {
                            name: "z".to_string(),
                            in_: "query".to_string(),
                            value: "$steps.c.outputs.z".to_string(),
                            ..arazzo_spec::Parameter::default()
                        },
                    ],
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::from([
                ("a_val".to_string(), "$steps.a.outputs.x".to_string()),
                ("b_val".to_string(), "$steps.b.outputs.y".to_string()),
                ("c_val".to_string(), "$steps.c.outputs.z".to_string()),
            ]),
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        let result = engine.execute("diamond", BTreeMap::new());
        let outputs = match result {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("a_val"), Some(&json!("alpha")));
        assert_eq!(outputs.get("b_val"), Some(&json!("beta")));
        assert_eq!(outputs.get("c_val"), Some(&json!("gamma")));

        let order = match request_order.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading request order"),
        };
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], "/a".to_string());
        assert!(order[1] == "/b?x=alpha" || order[1] == "/c?x=alpha");
        assert!(order[2] == "/b?x=alpha" || order[2] == "/c?x=alpha");
        assert_ne!(order[1], order[2]);
        assert_eq!(order[3], "/d?y=beta&z=gamma".to_string());
    }

    #[test]
    fn trace_hook_invoked_and_captures_fields() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/a".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/b".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let hook = Arc::new(TestTraceHook::default());
        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_trace_hook(hook.clone());
        let result = engine.execute("wf", BTreeMap::new());
        if let Err(err) = result {
            panic!("expected success, got: {err}");
        }

        let before = match hook.before_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading before events"),
        };
        let after = match hook.after_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading after events"),
        };
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        assert_eq!(before[0].step_id, "s1".to_string());
        assert_eq!(before[1].step_id, "s2".to_string());
        assert_eq!(after[0].status_code, 200);
        assert!(after[0].duration > Duration::from_nanos(0));
    }

    #[test]
    fn trace_hook_workflow_id_subworkflow_and_error_capture() {
        let server = start_server(|_method, url, _headers, _body| match url.as_str() {
            "/api" => MockHttpResponse::json(200, r#"{"ok":true}"#),
            "/fail" => MockHttpResponse::json(500, r#"{"error":"fail"}"#),
            _ => MockHttpResponse::json(200, "{}"),
        });

        let spec = make_spec(vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    workflow_id: "child".to_string(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![
                    Step {
                        step_id: "s1".to_string(),
                        operation_path: "/api".to_string(),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "s2".to_string(),
                        operation_path: "/fail".to_string(),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
        ]);

        let hook = Arc::new(TestTraceHook::default());
        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_trace_hook(hook.clone());
        let _ = engine.execute("parent", BTreeMap::new());

        let before = match hook.before_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading before events"),
        };
        let after = match hook.after_events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("reading after events"),
        };
        assert!(!before.is_empty());
        assert_eq!(before[0].workflow_id, "parent".to_string());
        assert_eq!(before[0].workflow_id_ref, "child".to_string());
        assert!(before.iter().any(|ev| ev.operation_path == "/api"));
        assert!(after.iter().any(|ev| ev.status_code == 500));
    }

    #[test]
    fn parse_method_supports_known_verbs() {
        assert_eq!(parse_method("GET /items"), ("GET", "/items"));
        assert_eq!(parse_method("POST /items"), ("POST", "/items"));
        assert_eq!(parse_method("DELETE /items/1"), ("DELETE", "/items/1"));
        assert_eq!(parse_method("PATCH /items/1"), ("PATCH", "/items/1"));
        assert_eq!(parse_method("HEAD /health"), ("HEAD", "/health"));
        assert_eq!(parse_method("OPTIONS /api"), ("OPTIONS", "/api"));
        assert_eq!(parse_method("/items"), ("", "/items"));
        assert_eq!(parse_method(""), ("", ""));
        assert_eq!(parse_method("UNKNOWN /items"), ("", "UNKNOWN /items"));
    }

    #[test]
    fn evaluate_criterion_modes() {
        let eval = ExpressionEvaluator::new(EvalContext {
            status_code: Some(200),
            response_body: Some(json!({"name":"alice","ok":true})),
            ..EvalContext::default()
        });

        let plain = SuccessCriterion {
            condition: "$statusCode == 200".to_string(),
            ..SuccessCriterion::default()
        };
        assert!(evaluate_criterion(&plain, &eval, None));

        let regex = SuccessCriterion {
            type_: "regex".to_string(),
            context: "$response.body.name".to_string(),
            condition: "^[a-z]+$".to_string(),
        };
        assert!(evaluate_criterion(&regex, &eval, None));

        let jsonpath = SuccessCriterion {
            type_: "jsonpath".to_string(),
            condition: "ok".to_string(),
            ..SuccessCriterion::default()
        };
        assert!(evaluate_criterion(&jsonpath, &eval, None));

        let regex_fail = SuccessCriterion {
            type_: "regex".to_string(),
            context: "$statusCode".to_string(),
            condition: "^5\\d{2}$".to_string(),
        };
        assert!(!evaluate_criterion(&regex_fail, &eval, None));

        let jsonpath_fail = SuccessCriterion {
            type_: "jsonpath".to_string(),
            condition: "missing.path".to_string(),
            ..SuccessCriterion::default()
        };
        assert!(!evaluate_criterion(&jsonpath_fail, &eval, None));
    }

    #[test]
    fn evaluate_criterion_jsonpath_uses_xpath_for_xml_responses() {
        let criterion = SuccessCriterion {
            type_: "jsonpath".to_string(),
            condition: "//item[1]/title".to_string(),
            ..SuccessCriterion::default()
        };
        let response = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: br#"<?xml version="1.0"?><rss><channel><item><title>Hello</title></item></channel></rss>"#
                .to_vec(),
            body_json: None,
            content_type: "xml".to_string(),
        };
        let eval = ExpressionEvaluator::new(EvalContext::default());

        assert!(evaluate_criterion(&criterion, &eval, Some(&response)));
    }

    #[test]
    fn find_matching_action_behavior() {
        let engine = new_test_engine("http://localhost", make_spec(Vec::new()));
        let vars = super::VarStore::default();

        let no_criteria = vec![OnAction {
            type_: "end".to_string(),
            ..OnAction::default()
        }];
        let first = engine.find_matching_action(&no_criteria, &vars, None);
        assert!(first.is_some());
        if let Some(action) = first {
            assert_eq!(action.type_, "end".to_string());
        }

        let with_criteria = vec![OnAction {
            type_: "retry".to_string(),
            criteria: vec![SuccessCriterion {
                condition: "$statusCode == 429".to_string(),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }];
        let none = engine.find_matching_action(&with_criteria, &vars, None);
        assert!(none.is_none());

        let response = super::Response {
            status_code: 429,
            headers: BTreeMap::new(),
            body: Vec::new(),
            body_json: None,
            content_type: "json".to_string(),
        };
        let ordered = vec![
            OnAction {
                name: "first".to_string(),
                type_: "retry".to_string(),
                criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 429".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..OnAction::default()
            },
            OnAction {
                name: "second".to_string(),
                type_: "end".to_string(),
                ..OnAction::default()
            },
        ];
        let matched = engine.find_matching_action(&ordered, &vars, Some(&response));
        assert!(matched.is_some());
        if let Some(action) = matched {
            assert_eq!(action.name, "first".to_string());
        }
    }

    #[test]
    fn build_outputs_evaluates_inputs_and_step_values() {
        let engine = new_test_engine("http://localhost", make_spec(Vec::new()));
        let workflow = Workflow {
            outputs: BTreeMap::from([
                ("inputName".to_string(), "$inputs.name".to_string()),
                (
                    "stepResult".to_string(),
                    "$steps.s1.outputs.result".to_string(),
                ),
            ]),
            ..Workflow::default()
        };

        let mut vars = super::VarStore::default();
        vars.set_input("name", json!("test"));
        vars.set_step_output("s1", "result", json!("hello"));

        let outputs = engine.build_outputs(&workflow, &vars);
        assert_eq!(outputs.get("inputName"), Some(&json!("test")));
        assert_eq!(outputs.get("stepResult"), Some(&json!("hello")));
    }

    #[test]
    fn extract_step_refs_and_control_flow() {
        let step = Step {
            step_id: "s2".to_string(),
            operation_path: "/items/$steps.s1.outputs.id".to_string(),
            parameters: vec![arazzo_spec::Parameter {
                name: "q".to_string(),
                in_: "query".to_string(),
                value: "$steps.s1.outputs.query".to_string(),
                ..arazzo_spec::Parameter::default()
            }],
            outputs: BTreeMap::from([("val".to_string(), "$steps.s1.outputs.value".to_string())]),
            on_failure: vec![OnAction {
                type_: "retry".to_string(),
                criteria: vec![SuccessCriterion {
                    condition: "$steps.s1.outputs.code == 429".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..OnAction::default()
            }],
            ..Step::default()
        };

        let refs = extract_step_refs(&step);
        assert_eq!(refs, vec!["s1".to_string()]);

        let wf_no_flow = Workflow {
            workflow_id: "no-flow".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/ok".to_string(),
                ..Step::default()
            }],
            ..Workflow::default()
        };
        assert!(!has_control_flow(&wf_no_flow));

        let wf_with_flow = Workflow {
            workflow_id: "with-flow".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/ok".to_string(),
                on_failure: vec![OnAction {
                    type_: "goto".to_string(),
                    step_id: "fallback".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        };
        assert!(has_control_flow(&wf_with_flow));
    }

    #[test]
    fn build_levels_supports_independent_chain_and_cycle() {
        let independent = Workflow {
            workflow_id: "independent".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let independent_levels = match build_levels(&independent) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(independent_levels, vec![vec![0, 1]]);

        let chain = Workflow {
            workflow_id: "chain".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/one".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/two".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.s1.outputs.id".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let chain_levels = match build_levels(&chain) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(chain_levels, vec![vec![0], vec![1]]);

        let cycle = Workflow {
            workflow_id: "cycle".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.s2.outputs.id".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "from".to_string(),
                        in_: "query".to_string(),
                        value: "$steps.s1.outputs.id".to_string(),
                        ..arazzo_spec::Parameter::default()
                    }],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };
        let cycle_result = build_levels(&cycle);
        let cycle_err = match cycle_result {
            Ok(_) => panic!("expected cycle detection error"),
            Err(err) => err,
        };
        assert!(cycle_err.0.contains("dependency cycle detected"));
    }

    #[test]
    fn build_levels_supports_diamond_dependency() {
        let workflow = Workflow {
            workflow_id: "diamond".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    operation_path: "/a".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    operation_path: "/b".to_string(),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    parameters: vec![
                        arazzo_spec::Parameter {
                            name: "x".to_string(),
                            in_: "query".to_string(),
                            value: "$steps.a.outputs.id".to_string(),
                            ..arazzo_spec::Parameter::default()
                        },
                        arazzo_spec::Parameter {
                            name: "y".to_string(),
                            in_: "query".to_string(),
                            value: "$steps.b.outputs.id".to_string(),
                            ..arazzo_spec::Parameter::default()
                        },
                    ],
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        };

        let levels = match build_levels(&workflow) {
            Ok(levels) => levels,
            Err(err) => panic!("building levels: {err}"),
        };
        assert_eq!(levels, vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn runtime_error_is_displayable() {
        let err = RuntimeError("boom".to_string());
        assert_eq!(err.to_string(), "boom".to_string());
    }

    // --- XPath extraction tests ---

    use super::extract_xpath;

    #[test]
    fn test_xpath_extraction() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title>First Story</title>
      <link>https://example.com/1</link>
    </item>
    <item>
      <title>Second Story</title>
      <link>https://example.com/2</link>
    </item>
  </channel>
</rss>"#;
        assert_eq!(
            extract_xpath(xml, "//item[1]/title"),
            Value::String("First Story".to_string())
        );
        assert_eq!(
            extract_xpath(xml, "//item[2]/title"),
            Value::String("Second Story".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_cdata() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title><![CDATA[Story with <special> chars]]></title>
    </item>
  </channel>
</rss>"#;
        assert_eq!(
            extract_xpath(xml, "//item[1]/title"),
            Value::String("Story with <special> chars".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_atom() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
  <title>top scoring links : technology</title>
  <entry>
    <title>First Reddit Post</title>
    <link href="https://reddit.com/1"/>
  </entry>
  <entry>
    <title>Second Reddit Post</title>
    <link href="https://reddit.com/2"/>
  </entry>
</feed>"#;
        assert_eq!(
            extract_xpath(xml, "//entry[1]/title"),
            Value::String("First Reddit Post".to_string())
        );
        assert_eq!(
            extract_xpath(xml, "//entry[2]/title"),
            Value::String("Second Reddit Post".to_string())
        );
    }

    #[test]
    fn test_xpath_extraction_no_match() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel></channel></rss>"#;
        assert_eq!(extract_xpath(xml, "//item[1]/title"), Value::Null);
    }

    #[test]
    fn test_xpath_extraction_invalid_xml() {
        let body = b"this is not xml at all <broken>";
        assert_eq!(extract_xpath(body, "//item[1]/title"), Value::Null);
    }
}
