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
    workflow_index: BTreeMap<String, usize>,
    step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    op_index: BTreeMap<String, OperationEntry>,
    parallel_mode: bool,
    dry_run_mode: bool,
    dry_run_reqs: Arc<Mutex<Vec<DryRunRequest>>>,
    trace_hook: Option<Arc<dyn TraceHook>>,
}

mod engine_impl;
mod helpers;

use helpers::{
    can_execute_parallel, replace_path_params, resolve_payload, sleep_with_checks,
    step_result_error, to_json_path, value_to_string,
};

pub(crate) use helpers::{build_levels, evaluate_criterion, extract_xpath, parse_method};
#[cfg(test)]
pub(crate) use helpers::{extract_step_refs, has_control_flow};
