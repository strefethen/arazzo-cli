use super::*;

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
pub(super) struct HttpClient {
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

impl HttpClient {
    pub(super) fn new(
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

    pub(super) async fn request(
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
            let key = k.to_string();
            headers
                .entry(key.clone())
                .and_modify(|existing: &mut String| {
                    // RFC 7230 §3.2.2 allows comma-joining multi-valued headers,
                    // except Set-Cookie (RFC 6265) whose values may contain commas.
                    // For Set-Cookie, keep only the last value (no lossless
                    // representation in BTreeMap<String, String>).
                    if key == "set-cookie" {
                        *existing = value.clone();
                    } else {
                        existing.push_str(", ");
                        existing.push_str(&value);
                    }
                })
                .or_insert(value);
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
        // $response.body will fall back to the raw body decoded as a UTF-8 string.
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
