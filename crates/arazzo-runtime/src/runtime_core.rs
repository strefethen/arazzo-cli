//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::{DebugController, DebugScopes, StepCheckpoint};
use arazzo_expr::{is_truthy, EvalContext, ExpressionEvaluator};
use arazzo_spec::{
    ActionType, ArazzoSpec, OnAction, ParamLocation, Parameter, Step, StepTarget, SuccessCriterion,
    Workflow,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const MAX_RETRIES_PER_STEP: usize = 3;
const MAX_CALL_DEPTH: usize = 10;
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;
pub(crate) const TRACE_BODY_PREVIEW_MAX_BYTES: usize = 2048;
/// Default maximum response body size: 10 MiB.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Runtime error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeErrorKind {
    Unspecified,
    ExecutionTimeout,
    ExecutionCancelled,
    WorkflowNotFound,
    StepNotFound,
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
    JsonParse,
    SourceDescriptionParse,
    SourceDescriptionNotFound,
    SubWorkflowFailed,
    SuccessCriteriaFailed,
    DebugController,
    StepMissingDependency,
    InputValidation,
    InternalError,
    ResponseTooLarge,
    ReplayTraceExhausted,
    ReplayRequestMismatch,
    ReplayResponseMissing,
    IterationLimitExceeded,
}

impl RuntimeErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unspecified => "RUNTIME_UNSPECIFIED",
            Self::ExecutionTimeout => "RUNTIME_EXECUTION_TIMEOUT",
            Self::ExecutionCancelled => "RUNTIME_EXECUTION_CANCELLED",
            Self::WorkflowNotFound => "RUNTIME_WORKFLOW_NOT_FOUND",
            Self::StepNotFound => "RUNTIME_STEP_NOT_FOUND",
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
            Self::JsonParse => "RUNTIME_JSON_PARSE",
            Self::SourceDescriptionParse => "RUNTIME_SOURCE_DESCRIPTION_PARSE",
            Self::SourceDescriptionNotFound => "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND",
            Self::SubWorkflowFailed => "RUNTIME_SUB_WORKFLOW_FAILED",
            Self::SuccessCriteriaFailed => "RUNTIME_SUCCESS_CRITERIA_FAILED",
            Self::DebugController => "RUNTIME_DEBUG_CONTROLLER",
            Self::StepMissingDependency => "STEP_MISSING_DEPENDENCY",
            Self::InputValidation => "RUNTIME_INPUT_VALIDATION",
            Self::InternalError => "RUNTIME_INTERNAL_ERROR",
            Self::ResponseTooLarge => "RUNTIME_RESPONSE_TOO_LARGE",
            Self::ReplayTraceExhausted => "RUNTIME_REPLAY_TRACE_EXHAUSTED",
            Self::ReplayRequestMismatch => "RUNTIME_REPLAY_REQUEST_MISMATCH",
            Self::ReplayResponseMissing => "RUNTIME_REPLAY_RESPONSE_MISSING",
            Self::IterationLimitExceeded => "RUNTIME_ITERATION_LIMIT_EXCEEDED",
        }
    }
}

#[derive(Debug)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl RuntimeError {
    pub fn new(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        kind: RuntimeErrorKind,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
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

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<reqwest::Error> for RuntimeError {
    fn from(err: reqwest::Error) -> Self {
        let kind = if err.is_timeout() {
            RuntimeErrorKind::ExecutionTimeout
        } else {
            RuntimeErrorKind::HttpRequest
        };
        Self::with_source(kind, err.to_string(), err)
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(err: serde_json::Error) -> Self {
        Self::with_source(
            RuntimeErrorKind::JsonParse,
            format!("JSON parse error: {err}"),
            err,
        )
    }
}

// ── Streamed event types ────────────────────────────────────────────

/// Event streamed from the engine during execution via the mpsc channel.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum EngineEvent {
    TraceStep(TraceStepRecord),
    DryRunRequest(DryRunRequest),
    Execution(ExecutionEvent),
    Observer(ObserverEvent),
}

/// Handle returned by [`Engine::execute`] for streaming execution results.
///
/// The spawned task drops the event sender before sending the final result
/// via the oneshot channel, guaranteeing that `collect()` can drain all
/// events before awaiting the result.
///
/// Dropping the handle cancels the running task via the `CancellationToken`.
pub struct ExecutionHandle {
    events: Option<mpsc::Receiver<EngineEvent>>,
    result: Option<oneshot::Receiver<Result<BTreeMap<String, Value>, RuntimeError>>>,
    cancel: CancellationToken,
    is_timeout: Arc<AtomicBool>,
}

impl Drop for ExecutionHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl ExecutionHandle {
    pub(crate) fn new(
        events: mpsc::Receiver<EngineEvent>,
        result: oneshot::Receiver<Result<BTreeMap<String, Value>, RuntimeError>>,
        cancel: CancellationToken,
        is_timeout: Arc<AtomicBool>,
    ) -> Self {
        Self {
            events: Some(events),
            result: Some(result),
            cancel,
            is_timeout,
        }
    }

    /// Access the cancellation token to cancel the running task.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Access the timeout flag (set by `execute_with_timeout` watchdog).
    pub fn timeout_flag(&self) -> &Arc<AtomicBool> {
        &self.is_timeout
    }

    /// Drain all events and await the final result.
    #[allow(clippy::missing_panics_doc)]
    pub async fn collect(mut self) -> ExecutionResult {
        let mut events_rx = self
            .events
            .take()
            .unwrap_or_else(|| panic!("events already consumed"));
        let result_rx = self
            .result
            .take()
            .unwrap_or_else(|| panic!("result already consumed"));
        let mut events = Vec::new();
        while let Some(event) = events_rx.recv().await {
            events.push(event);
        }
        let outputs = result_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "execution task completed without sending result",
            ))
        });
        ExecutionResult { outputs, events }
        // Drop runs here, calling cancel() — task already finished, no-op
    }

    /// Discard events and return only the workflow result.
    #[allow(clippy::missing_panics_doc)]
    pub async fn result_only(mut self) -> Result<BTreeMap<String, Value>, RuntimeError> {
        drop(self.events.take()); // unblock event sends immediately
        let result_rx = self
            .result
            .take()
            .unwrap_or_else(|| panic!("result already consumed"));
        result_rx.await.unwrap_or_else(|_| {
            Err(RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "execution task completed without sending result",
            ))
        })
    }
}

/// Collected execution output from a completed workflow.
pub struct ExecutionResult {
    pub outputs: Result<BTreeMap<String, Value>, RuntimeError>,
    pub events: Vec<EngineEvent>,
}

impl ExecutionResult {
    /// Filter trace step records from the event stream.
    pub fn trace_steps(&self) -> Vec<&TraceStepRecord> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::TraceStep(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Filter dry-run requests from the event stream.
    pub fn dry_run_requests(&self) -> Vec<&DryRunRequest> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::DryRunRequest(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// Filter execution lifecycle events from the event stream.
    pub fn execution_events(&self) -> Vec<&ExecutionEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Execution(r) => Some(r),
                _ => None,
            })
            .collect()
    }
}

// ── Per-execution context ───────────────────────────────────────────

/// Per-execution mutable state, shared across tasks via `Arc`.
pub(super) struct ExecutionContext {
    pub event_tx: mpsc::Sender<EngineEvent>,
    pub trace_seq: AtomicU64,
    pub execution_event_seq: AtomicU64,
    pub step_attempts: Mutex<BTreeMap<(String, String), u32>>,
    pub cancel: CancellationToken,
    pub is_timeout: Arc<AtomicBool>,
}

impl ExecutionContext {
    fn check_cancelled(&self) -> Result<(), RuntimeError> {
        if self.cancel.is_cancelled() {
            Err(self.cancelled_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn cancelled_error(&self) -> RuntimeError {
        if self.is_timeout.load(Ordering::Acquire) {
            RuntimeError::new(
                RuntimeErrorKind::ExecutionTimeout,
                "execution timeout exceeded",
            )
        } else {
            RuntimeError::new(RuntimeErrorKind::ExecutionCancelled, "execution cancelled")
        }
    }
}

// ── Rate limiter ────────────────────────────────────────────────────

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
    mode: HttpClientMode,
    default_headers: BTreeMap<String, String>,
    rate_limiter: Arc<tokio::sync::Mutex<RateLimiterState>>,
    max_response_bytes: usize,
}

#[derive(Debug, Clone)]
enum HttpClientMode {
    Live(reqwest::Client),
    Replay(Arc<tokio::sync::Mutex<ReplayState>>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplayKey {
    workflow_id: String,
    step_id: String,
}

#[derive(Debug, Clone)]
struct ReplayRecord {
    seq: u64,
    attempt: u32,
    request: TraceRequest,
    response: Option<TraceResponse>,
    error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ReplayState {
    records_by_step: BTreeMap<ReplayKey, VecDeque<ReplayRecord>>,
}

impl ReplayState {
    fn from_trace_steps(steps: &[TraceStepRecord]) -> Self {
        let mut records_by_step = BTreeMap::<ReplayKey, VecDeque<ReplayRecord>>::new();
        for step in steps {
            let Some(request) = &step.request else {
                continue;
            };
            let key = ReplayKey {
                workflow_id: step.workflow_id.clone(),
                step_id: step.step_id.clone(),
            };
            records_by_step
                .entry(key)
                .or_default()
                .push_back(ReplayRecord {
                    seq: step.seq,
                    attempt: step.attempt,
                    request: request.clone(),
                    response: step.response.clone(),
                    error: step.error.clone(),
                });
        }
        Self { records_by_step }
    }
}

impl HttpClient {
    fn new(
        config: &ClientConfig,
        max_response_bytes: usize,
        replay_trace_steps: Option<Vec<TraceStepRecord>>,
    ) -> Result<Self, RuntimeError> {
        let mode = if let Some(steps) = replay_trace_steps {
            HttpClientMode::Replay(Arc::new(tokio::sync::Mutex::new(
                ReplayState::from_trace_steps(&steps),
            )))
        } else {
            let inner = reqwest::Client::builder()
                .timeout(config.timeout)
                .build()
                .map_err(|err| {
                    RuntimeError::with_source(
                        RuntimeErrorKind::HttpClientBuild,
                        format!("building HTTP client: {err}"),
                        err,
                    )
                })?;
            HttpClientMode::Live(inner)
        };
        Ok(Self {
            mode,
            default_headers: config.default_headers.clone(),
            rate_limiter: Arc::new(tokio::sync::Mutex::new(RateLimiterState::new(
                &config.rate_limit,
            ))),
            max_response_bytes,
        })
    }

    async fn request(
        &self,
        cfg: RequestConfig,
        cancel: &CancellationToken,
        is_timeout: &AtomicBool,
    ) -> Result<Response, RuntimeError> {
        if let HttpClientMode::Replay(state) = &self.mode {
            return self.replay_request(state, cfg).await;
        }

        let inner = match &self.mode {
            HttpClientMode::Live(inner) => inner,
            HttpClientMode::Replay(_) => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    "runtime entered unreachable replay HTTP mode",
                ));
            }
        };

        self.wait_for_rate_limit(cancel, is_timeout).await?;
        let method = reqwest::Method::from_bytes(cfg.method.as_bytes()).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::InvalidHttpMethod,
                format!("invalid HTTP method {}: {err}", cfg.method),
            )
        })?;
        let mut req = inner.request(method, cfg.url);

        for (k, v) in &self.default_headers {
            req = req.header(k, v);
        }
        for (k, v) in cfg.headers {
            req = req.header(k, v);
        }
        if let Some(body) = cfg.body {
            req = req.body(body);
        }

        let mut resp = req.send().await.map_err(|err| {
            RuntimeError::with_source(
                RuntimeErrorKind::HttpRequest,
                format!("executing request: {err}"),
                err,
            )
        })?;

        let status_code = i64::from(resp.status().as_u16());
        let mut headers = BTreeMap::new();
        for (k, v) in resp.headers() {
            let value = v.to_str().unwrap_or_default().to_string();
            headers.insert(k.to_string(), value);
        }

        // Fail fast if Content-Length already exceeds the limit.
        if let Some(content_length) = resp.content_length() {
            if content_length > self.max_response_bytes as u64 {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::ResponseTooLarge,
                    format!(
                        "response body too large: Content-Length {content_length} exceeds limit of {} bytes",
                        self.max_response_bytes
                    ),
                ));
            }
        }

        // Stream body in chunks, enforcing the size limit.
        let max = self.max_response_bytes;
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|err| {
            RuntimeError::with_source(
                RuntimeErrorKind::HttpResponseRead,
                format!("reading response body: {err}"),
                err,
            )
        })? {
            if body.len() + chunk.len() > max {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::ResponseTooLarge,
                    format!(
                        "response body too large: exceeded limit of {max} bytes while streaming"
                    ),
                ));
            }
            body.extend_from_slice(&chunk);
        }

        let content_type_raw = headers
            .get("content-type")
            .or_else(|| headers.get("Content-Type"))
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let is_xml = content_type_raw.contains("xml") || content_type_raw.contains("rss");
        let is_json = content_type_raw.contains("json");
        // Intentional: response body may not be valid JSON (e.g. HTML, plain text).
        // We attempt parsing and store None if it fails — expressions that reference
        // $response.body will fall back to the raw bytes.
        let body_json = if is_xml {
            None
        } else {
            serde_json::from_slice::<Value>(&body).ok()
        };

        let classified_type = if is_xml {
            ContentType::Xml
        } else if is_json || content_type_raw.is_empty() {
            // Treat missing content-type as JSON (common in APIs)
            ContentType::Json
        } else {
            ContentType::Other(content_type_raw)
        };

        Ok(Response {
            status_code,
            headers,
            body,
            body_json,
            content_type: classified_type,
        })
    }

    async fn replay_request(
        &self,
        state: &Arc<tokio::sync::Mutex<ReplayState>>,
        cfg: RequestConfig,
    ) -> Result<Response, RuntimeError> {
        let key = ReplayKey {
            workflow_id: cfg.workflow_id.clone(),
            step_id: cfg.step_id.clone(),
        };

        let record = {
            let mut guard = state.lock().await;
            let queue = guard.records_by_step.get_mut(&key).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::ReplayTraceExhausted,
                    format!(
                        "no recorded replay request for workflow \"{}\" step \"{}\"",
                        key.workflow_id, key.step_id
                    ),
                )
            })?;
            queue.pop_front().ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::ReplayTraceExhausted,
                    format!(
                        "recorded replay requests exhausted for workflow \"{}\" step \"{}\"",
                        key.workflow_id, key.step_id
                    ),
                )
            })?
        };

        validate_replay_request(&record.request, &cfg, record.seq, record.attempt)?;

        let trace_response = match (record.response, record.error) {
            (Some(response), _) => response,
            (None, Some(error)) => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::HttpRequest,
                    format!(
                        "replay trace request failed at seq {} (attempt {}): {error}",
                        record.seq, record.attempt
                    ),
                ));
            }
            (None, None) => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::ReplayResponseMissing,
                    format!(
                        "replay trace missing response for workflow \"{}\" step \"{}\" (seq {})",
                        key.workflow_id, key.step_id, record.seq
                    ),
                ));
            }
        };

        let body = trace_response
            .body
            .clone()
            .or_else(|| trace_response.body_preview.clone())
            .unwrap_or_default()
            .into_bytes();
        let body_json = match trace_response.content_type {
            ContentType::Json => serde_json::from_slice::<Value>(&body).ok(),
            _ => None,
        };

        Ok(Response {
            status_code: trace_response.status_code,
            headers: trace_response.headers,
            body,
            body_json,
            content_type: trace_response.content_type,
        })
    }

    async fn wait_for_rate_limit(
        &self,
        cancel: &CancellationToken,
        is_timeout: &AtomicBool,
    ) -> Result<(), RuntimeError> {
        loop {
            if cancel.is_cancelled() {
                return if is_timeout.load(Ordering::Acquire) {
                    Err(RuntimeError::new(
                        RuntimeErrorKind::ExecutionTimeout,
                        "execution timeout exceeded",
                    ))
                } else {
                    Err(RuntimeError::new(
                        RuntimeErrorKind::ExecutionCancelled,
                        "execution cancelled",
                    ))
                };
            }
            let wait = {
                let now = Instant::now();
                let mut limiter = self.rate_limiter.lock().await;
                limiter.acquire_wait(now)
            };
            match wait {
                None => return Ok(()),
                Some(delay) => sleep_with_cancel(delay, cancel, is_timeout).await?,
            }
        }
    }
}

fn validate_replay_request(
    expected: &TraceRequest,
    actual: &RequestConfig,
    seq: u64,
    attempt: u32,
) -> Result<(), RuntimeError> {
    if !expected.method.eq_ignore_ascii_case(&actual.method) {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: method expected \"{}\" got \"{}\"",
                expected.method, actual.method
            ),
        ));
    }

    if expected.url != actual.url {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: url expected \"{}\" got \"{}\"",
                expected.url, actual.url
            ),
        ));
    }

    if expected.headers != actual.headers {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: headers expected {:?} got {:?}",
                expected.headers, actual.headers
            ),
        ));
    }

    if !replay_body_matches(expected.body.as_ref(), actual.body.as_deref()) {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!("replay request drift at seq {seq} attempt {attempt}: request body mismatch"),
        ));
    }

    Ok(())
}

fn replay_body_matches(expected: Option<&Value>, actual: Option<&[u8]>) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(expected_value), Some(actual_bytes)) => {
            if let Ok(parsed) = serde_json::from_slice::<Value>(actual_bytes) {
                return parsed == *expected_value;
            }
            match expected_value {
                Value::String(s) => std::str::from_utf8(actual_bytes).is_ok_and(|text| text == s),
                _ => false,
            }
        }
    }
}

/// Request settings used by the runtime client.
#[derive(Debug, Clone)]
pub struct RequestConfig {
    pub workflow_id: String,
    pub step_id: String,
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

/// Content type classification for HTTP responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    #[default]
    Json,
    Xml,
    Other(String),
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json => write!(f, "json"),
            Self::Xml => write!(f, "xml"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Response returned by the runtime client.
#[derive(Debug, Clone)]
pub struct Response {
    pub status_code: i64,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub body_json: Option<Value>,
    pub content_type: ContentType,
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
#[non_exhaustive]
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
    pub retry_after_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_limit: Option<u64>,
}

impl TraceDecision {
    /// Creates a `TraceDecision` with the given `path` and all other fields defaulted.
    pub fn with_path(path: TraceDecisionPath) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }
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
    pub content_type: ContentType,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub body_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// True when the body was converted via lossy UTF-8 (non-UTF-8 bytes replaced
    /// with U+FFFD). Replay consumers should treat the body as approximate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_lossy: bool,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
#[non_exhaustive]
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

/// Rich execution event for TUI/observer integration.
///
/// Each variant captures a specific lifecycle moment during workflow execution,
/// carrying the relevant data for that moment. Observers receive these events
/// via [`ExecutionObserver::on_event`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ObserverEvent {
    /// Step is about to begin execution.
    StepStarted {
        workflow_id: String,
        step_id: String,
        operation_path: String,
        workflow_id_ref: String,
    },

    /// HTTP request has been resolved and is about to be sent.
    RequestPrepared {
        workflow_id: String,
        step_id: String,
        method: String,
        url: String,
        headers: BTreeMap<String, String>,
        has_body: bool,
    },

    /// HTTP request has been dispatched, awaiting response.
    RequestSent {
        workflow_id: String,
        step_id: String,
        method: String,
        url: String,
    },

    /// A single success criterion has been evaluated.
    CriterionEvaluated {
        workflow_id: String,
        step_id: String,
        index: usize,
        condition: String,
        passed: bool,
    },

    /// A retry action has been selected; about to wait.
    RetryScheduled {
        workflow_id: String,
        step_id: String,
        attempt: usize,
        max_attempts: usize,
        delay_seconds: u64,
    },

    /// Step completed (success or failure).
    /// Fires BEFORE the action handler decides retry/goto/end.
    StepCompleted {
        workflow_id: String,
        step_id: String,
        status_code: i64,
        duration: Duration,
        outputs: BTreeMap<String, Value>,
        error: Option<String>,
        criteria_passed: bool,
    },

    /// Sub-workflow invocation starting.
    SubWorkflowStarted {
        parent_workflow_id: String,
        parent_step_id: String,
        child_workflow_id: String,
        depth: usize,
    },

    /// Workflow execution finished.
    WorkflowCompleted {
        workflow_id: String,
        outputs: BTreeMap<String, Value>,
        duration: Duration,
        error: Option<String>,
    },
}

/// Observer trait for rich execution event streaming.
///
/// Unlike [`TraceHook`] (which provides only before/after step),
/// `ExecutionObserver` receives fine-grained events including
/// request preparation, HTTP dispatch, criterion evaluation,
/// retry scheduling, and sub-workflow invocation.
///
/// Implementations must be `Send + Sync` (called from async tasks).
/// Callbacks should be non-blocking — do not perform I/O or heavy
/// computation. Send events to a channel and process on another task.
pub trait ExecutionObserver: Send + Sync {
    fn on_event(&self, event: &ObserverEvent);
}

#[derive(Debug, Clone)]
pub(crate) struct OperationEntry {
    method: String,
    path: String,
}

#[derive(Debug, Clone)]
struct StepResult {
    success: bool,
    response: Option<Arc<Response>>,
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
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VarStore {
    inputs: BTreeMap<String, Value>,
    steps: BTreeMap<String, BTreeMap<String, Value>>,
    workflow_states: BTreeMap<String, arazzo_expr::WorkflowEvalState>,
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

    pub(crate) fn register_workflow_state(
        &mut self,
        workflow_id: &str,
        inputs: BTreeMap<String, Value>,
        outputs: BTreeMap<String, Value>,
    ) {
        self.workflow_states.insert(
            workflow_id.to_string(),
            arazzo_expr::WorkflowEvalState { inputs, outputs },
        );
    }

    pub(crate) fn eval_context(&self, response: Option<&Response>) -> EvalContext {
        let mut ctx = EvalContext {
            inputs: self.inputs.clone(),
            steps: self.steps.clone(),
            workflows: self.workflow_states.clone(),
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

/// Immutable index built once from the parsed spec.
pub(crate) struct WorkflowIndex {
    pub spec: ArazzoSpec,
    pub base_url: String,
    pub source_descriptions_map: BTreeMap<String, String>,
    pub workflow_index: BTreeMap<String, usize>,
    pub step_indexes: BTreeMap<String, BTreeMap<String, usize>>,
    openapi_specs_raw: Vec<Vec<u8>>,
    op_index: OnceLock<BTreeMap<String, OperationEntry>>,
}

/// Shared immutable core of the engine, wrapped in `Arc`.
struct EngineInner {
    index: WorkflowIndex,
    client: HttpClient,
    parallel_mode: bool,
    dry_run_mode: bool,
    trace_enabled: bool,
    strict_inputs: bool,
    channel_capacity: usize,
    trace_hook: Option<Arc<dyn TraceHook>>,
    observer: Option<Arc<dyn ExecutionObserver>>,
    debug_controller: Option<Arc<DebugController>>,
    regex_cache: helpers::RegexCache,
}

/// Runtime engine for executing Arazzo workflows.
///
/// `Engine` is cheaply cloneable (wraps `Arc<EngineInner>`) and can be
/// shared across tasks for concurrent workflow execution.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

/// Builder for constructing a fully configured [`Engine`] instance.
///
/// `EngineBuilder` replaces the setter-based configuration pattern on `Engine`.
/// All optional settings have sensible defaults, and the builder is consumed by
/// [`EngineBuilder::build`] to produce a ready-to-execute engine.
///
/// # Example
///
/// ```ignore
/// use arazzo_runtime::EngineBuilder;
///
/// let engine = EngineBuilder::new(spec)
///     .parallel(true)
///     .dry_run(false)
///     .trace(true)
///     .build()?;
/// ```
pub struct EngineBuilder {
    spec: ArazzoSpec,
    client_config: Option<ClientConfig>,
    parallel: bool,
    dry_run: bool,
    trace: bool,
    replay_trace_steps: Option<Vec<TraceStepRecord>>,
    strict_inputs: bool,
    channel_capacity: usize,
    max_response_bytes: usize,
    trace_hook: Option<Arc<dyn TraceHook>>,
    observer: Option<Arc<dyn ExecutionObserver>>,
    debug_controller: Option<Arc<DebugController>>,
    openapi_specs: Vec<Vec<u8>>,
}

impl EngineBuilder {
    /// Creates a new builder with the given Arazzo spec. All optional settings
    /// default to their inactive/off state.
    pub fn new(spec: ArazzoSpec) -> Self {
        Self {
            spec,
            client_config: None,
            parallel: false,
            dry_run: false,
            trace: false,
            replay_trace_steps: None,
            strict_inputs: false,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            trace_hook: None,
            observer: None,
            debug_controller: None,
            openapi_specs: Vec::new(),
        }
    }

    /// Sets custom HTTP client configuration. When omitted, `ClientConfig::default()` is used.
    pub fn client_config(mut self, config: ClientConfig) -> Self {
        self.client_config = Some(config);
        self
    }

    /// Enables or disables parallel execution of independent steps within a workflow.
    pub fn parallel(mut self, enabled: bool) -> Self {
        self.parallel = enabled;
        self
    }

    /// Enables or disables dry-run mode, which resolves requests without sending them.
    pub fn dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    /// Enables or disables detailed per-step trace recording during execution.
    pub fn trace(mut self, enabled: bool) -> Self {
        self.trace = enabled;
        self
    }

    /// Enables replay mode by serving recorded step responses from trace records
    /// instead of issuing live network requests.
    pub fn replay_trace_steps(mut self, steps: Vec<TraceStepRecord>) -> Self {
        self.replay_trace_steps = Some(steps);
        self
    }

    /// Enables or disables strict input validation. When enabled, missing required
    /// inputs and type mismatches cause a fatal `InputValidation` error. When
    /// disabled (default), validation issues are printed as warnings to stderr.
    pub fn strict_inputs(mut self, enabled: bool) -> Self {
        self.strict_inputs = enabled;
        self
    }

    /// Sets the bounded channel capacity for event streaming. Default: 1024.
    pub fn channel_capacity(mut self, cap: usize) -> Self {
        self.channel_capacity = cap;
        self
    }

    /// Registers a trace hook that receives step lifecycle events during execution.
    pub fn trace_hook(mut self, hook: Arc<dyn TraceHook>) -> Self {
        self.trace_hook = Some(hook);
        self
    }

    /// Registers an execution observer for rich event streaming during execution.
    pub fn observer(mut self, observer: Arc<dyn ExecutionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Attaches a debug controller for breakpoint-driven step-through execution.
    pub fn debug_controller(mut self, controller: Arc<DebugController>) -> Self {
        self.debug_controller = Some(controller);
        self
    }

    /// Sets the maximum allowed response body size in bytes. Responses exceeding
    /// this limit will produce a `ResponseTooLarge` error. Default: 10 MiB.
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }

    /// Adds an OpenAPI spec to be parsed and indexed during build.
    /// Call multiple times for multiple specs. Replaces `Engine::load_openapi_spec`.
    pub fn openapi_spec(mut self, data: Vec<u8>) -> Self {
        self.openapi_specs.push(data);
        self
    }

    /// Consumes the builder and creates a fully configured [`Engine`].
    ///
    /// Returns an error if the HTTP client cannot be constructed (e.g. invalid TLS settings).
    pub fn build(self) -> Result<Engine, RuntimeError> {
        let config = self.client_config.unwrap_or_default();
        let client = HttpClient::new(&config, self.max_response_bytes, self.replay_trace_steps)?;

        let base_url = self
            .spec
            .source_descriptions
            .first()
            .map(|s| s.url.clone())
            .unwrap_or_default();

        let mut source_descriptions_map = BTreeMap::new();
        for sd in &self.spec.source_descriptions {
            source_descriptions_map.insert(sd.name.clone(), sd.url.clone());
        }

        let mut workflow_index = BTreeMap::new();
        let mut step_indexes = BTreeMap::new();
        for (wf_idx, wf) in self.spec.workflows.iter().enumerate() {
            workflow_index.insert(wf.workflow_id.clone(), wf_idx);
            let mut step_idx_map = BTreeMap::new();
            for (step_idx, step) in wf.steps.iter().enumerate() {
                step_idx_map.insert(step.step_id.clone(), step_idx);
            }
            step_indexes.insert(wf.workflow_id.clone(), step_idx_map);
        }

        Ok(Engine {
            inner: Arc::new(EngineInner {
                index: WorkflowIndex {
                    spec: self.spec,
                    base_url,
                    source_descriptions_map,
                    workflow_index,
                    step_indexes,
                    openapi_specs_raw: self.openapi_specs,
                    op_index: OnceLock::new(),
                },
                client,
                parallel_mode: self.parallel,
                dry_run_mode: self.dry_run,
                trace_enabled: self.trace,
                strict_inputs: self.strict_inputs,
                channel_capacity: self.channel_capacity,
                trace_hook: self.trace_hook.map(|h| h as Arc<dyn TraceHook>),
                observer: self.observer,
                regex_cache: helpers::RegexCache::new(),
                debug_controller: self.debug_controller,
            }),
        })
    }
}

/// Parses an OpenAPI spec and populates the operation index.
fn parse_openapi_into_index(
    data: &[u8],
    op_index: &mut BTreeMap<String, OperationEntry>,
) -> Result<(), RuntimeError> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_slice(data).map_err(|err| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!("parsing OpenAPI spec: {err}"),
        )
    })?;
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
                .get(serde_yaml_ng::Value::String("operationId".to_string()))
                .and_then(serde_yaml_ng::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if op_id.is_empty() {
                continue;
            }
            op_index.insert(
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

mod engine_actions;
mod engine_http;
mod engine_impl;
mod engine_parallel;
mod engine_trace;
mod helpers;
mod input_validation;

use engine_actions::{ActionBranch, FlowDecision, SelectedActionDebugContext, StepDecisionContext};
use engine_impl::merge_workflow_params;
use engine_trace::{build_trace_response, DebugGateContext};
use input_validation::{validate_inputs, InputIssueSeverity};

use helpers::{
    can_execute_parallel, encode_cookie_value, parse_source_prefix, replace_path_params,
    resolve_payload, sleep_with_cancel, step_result_error, value_to_string,
};

pub(crate) use helpers::{
    build_levels, compute_transitive_deps, evaluate_criterion, evaluate_criterion_detailed,
    evaluate_output_expression, evaluate_output_expression_detailed, extract_step_refs,
    extract_xpath, parse_method, CriterionEvaluation,
};
