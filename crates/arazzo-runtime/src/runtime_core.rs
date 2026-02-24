//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{DebugController, DebugScopes, StepCheckpoint};
use arazzo_expr::{is_truthy, EvalContext, ExpressionEvaluator};
use arazzo_spec::{ActionType, ArazzoSpec, OnAction, ParamLocation, Parameter, Step, SuccessCriterion, Workflow};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_RETRIES_PER_STEP: usize = 3;
const MAX_CALL_DEPTH: usize = 10;
const SLEEP_CHECK_INTERVAL: Duration = Duration::from_millis(25);
pub(crate) const TRACE_BODY_PREVIEW_MAX_BYTES: usize = 2048;

/// Runtime error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorKind {
    Unspecified,
    ExecutionTimeout,
    ExecutionCancelled,
    WorkflowNotFound,
    OperationIdNotFound,
    MaxCallDepthExceeded,
    RetryLimitExceeded,
    DependencyCycle,
    GotoTargetNotFound,
    GotoTargetMissing,
    InvalidHttpMethod,
    HttpClientBuild,
    HttpRequest,
    HttpResponseRead,
    RateLimiterLockPoisoned,
    ParallelThreadPanic,
}

impl RuntimeErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unspecified => "RUNTIME_UNSPECIFIED",
            Self::ExecutionTimeout => "RUNTIME_EXECUTION_TIMEOUT",
            Self::ExecutionCancelled => "RUNTIME_EXECUTION_CANCELLED",
            Self::WorkflowNotFound => "RUNTIME_WORKFLOW_NOT_FOUND",
            Self::OperationIdNotFound => "RUNTIME_OPERATION_ID_NOT_FOUND",
            Self::MaxCallDepthExceeded => "RUNTIME_MAX_CALL_DEPTH_EXCEEDED",
            Self::RetryLimitExceeded => "RUNTIME_RETRY_LIMIT_EXCEEDED",
            Self::DependencyCycle => "RUNTIME_DEPENDENCY_CYCLE",
            Self::GotoTargetNotFound => "RUNTIME_GOTO_TARGET_NOT_FOUND",
            Self::GotoTargetMissing => "RUNTIME_GOTO_TARGET_MISSING",
            Self::InvalidHttpMethod => "RUNTIME_INVALID_HTTP_METHOD",
            Self::HttpClientBuild => "RUNTIME_HTTP_CLIENT_BUILD",
            Self::HttpRequest => "RUNTIME_HTTP_REQUEST",
            Self::HttpResponseRead => "RUNTIME_HTTP_RESPONSE_READ",
            Self::RateLimiterLockPoisoned => "RUNTIME_RATE_LIMITER_LOCK_POISONED",
            Self::ParallelThreadPanic => "RUNTIME_PARALLEL_THREAD_PANIC",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
}

impl RuntimeError {
    pub fn new(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn unspecified(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Unspecified, message)
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
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
                return Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionTimeout,
                    "execution timeout exceeded",
                ));
            }
        }
        if let Some(flag) = &self.cancel_flag {
            if flag.load(Ordering::Relaxed) {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionCancelled,
                    "execution cancelled",
                ));
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
            .map_err(|err| {
                RuntimeError::new(
                    RuntimeErrorKind::HttpClientBuild,
                    format!("building HTTP client: {err}"),
                )
            })?;
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
        let method = reqwest::Method::from_bytes(cfg.method.as_bytes()).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::InvalidHttpMethod,
                format!("invalid HTTP method {}: {err}", cfg.method),
            )
        })?;
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

        let resp = req.send().map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::HttpRequest,
                format!("executing request: {err}"),
            )
        })?;

        let status_code = i64::from(resp.status().as_u16());
        let mut headers = BTreeMap::new();
        for (k, v) in resp.headers() {
            let value = v.to_str().unwrap_or_default().to_string();
            headers.insert(k.to_string(), value);
        }
        let body = resp
            .bytes()
            .map_err(|err| {
                RuntimeError::new(
                    RuntimeErrorKind::HttpResponseRead,
                    format!("reading response body: {err}"),
                )
            })?
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
                let mut limiter = self.rate_limiter.lock().map_err(|_| {
                    RuntimeError::new(
                        RuntimeErrorKind::RateLimiterLockPoisoned,
                        "rate limiter lock poisoned",
                    )
                })?;
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

/// Result of building a URL from an operationPath, including resolved parameters.
#[derive(Debug, Clone)]
pub(crate) struct UrlBuildResult {
    pub url: String,
    pub path_params: BTreeMap<String, String>,
    pub query_params: BTreeMap<String, String>,
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// Trace path chosen after a step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum TraceDecisionPath {
    #[default]
    Next,
    Done,
    GotoStep,
    GotoWorkflow,
    Retry,
    Error,
}

/// Trace decision metadata for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceDecision {
    pub path: TraceDecisionPath,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_step_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_workflow_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_limit: Option<i64>,
}

/// Trace request payload for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceRequest {
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Trace response payload for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceResponse {
    pub status_code: i64,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub body_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
}

/// Trace result of evaluating one success criterion.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceCriterionResult {
    pub index: usize,
    #[serde(rename = "type", default, skip_serializing_if = "String::is_empty")]
    pub type_: String,
    pub condition: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    pub result: bool,
}

/// Runtime trace record for one step attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceStepRecord {
    pub seq: u64,
    pub workflow_id: String,
    pub step_id: String,
    pub attempt: u32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub operation_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workflow_id_ref: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<TraceRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<TraceResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<TraceCriterionResult>,
    pub decision: TraceDecision,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

/// Canonical runtime execution event kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionEventKind {
    BeforeStep,
    AfterStep,
}

/// Canonical runtime execution event emitted for every step lifecycle transition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvent {
    pub seq: u64,
    pub kind: ExecutionEventKind,
    pub workflow_id: String,
    pub step_id: String,
    pub operation_path: String,
    pub workflow_id_ref: String,
    pub status_code: i64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
    pub duration_ns: u64,
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
    trace: StepTraceData,
}

#[derive(Debug, Clone, Default)]
struct StepTraceData {
    request: Option<TraceRequest>,
    response: Option<TraceResponse>,
    criteria: Vec<TraceCriterionResult>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VarStore {
    inputs: BTreeMap<String, Value>,
    steps: BTreeMap<String, BTreeMap<String, Value>>,
}

impl VarStore {
    pub(crate) fn set_input(&mut self, name: &str, value: Value) {
        self.inputs.insert(name.to_string(), value);
    }

    pub(crate) fn set_step_output(&mut self, step_id: &str, name: &str, value: Value) {
        self.steps
            .entry(step_id.to_string())
            .or_default()
            .insert(name.to_string(), value);
    }

    pub(crate) fn step_outputs(&self, step_id: &str) -> BTreeMap<String, Value> {
        self.steps.get(step_id).cloned().unwrap_or_default()
    }

    pub(crate) fn debug_scopes(&self) -> DebugScopes {
        DebugScopes {
            locals: BTreeMap::new(),
            inputs: self.inputs.clone(),
            steps: self.steps.clone(),
        }
    }

    pub(crate) fn eval_context(&self, response: Option<&Response>) -> EvalContext {
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
    pub(crate) base_url: String,
    source_descriptions_map: BTreeMap<String, String>,
    workflow_index: BTreeMap<String, usize>,
    step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    op_index: BTreeMap<String, OperationEntry>,
    parallel_mode: bool,
    dry_run_mode: bool,
    trace_enabled: bool,
    dry_run_reqs: Arc<Mutex<Vec<DryRunRequest>>>,
    trace_steps: Arc<Mutex<Vec<TraceStepRecord>>>,
    trace_seq: Arc<Mutex<u64>>,
    execution_events: Arc<Mutex<Vec<ExecutionEvent>>>,
    execution_event_seq: Arc<Mutex<u64>>,
    step_attempts: Arc<Mutex<BTreeMap<(String, String), u32>>>,
    trace_hook: Option<Arc<dyn TraceHook>>,
    debug_controller: Option<Arc<DebugController>>,
}

mod engine_impl;
mod helpers;

use helpers::{
    can_execute_parallel, parse_source_prefix, replace_path_params, resolve_payload,
    sleep_with_checks, step_result_error, value_to_string,
};

pub(crate) use helpers::{
    build_levels, evaluate_criterion, evaluate_criterion_detailed, evaluate_output_expression,
    extract_xpath, parse_method, CriterionEvaluation,
};
#[cfg(test)]
pub(crate) use helpers::{extract_step_refs, has_control_flow};
