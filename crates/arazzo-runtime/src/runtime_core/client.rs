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

/// Default redirect-hop limit. Pins reqwest's documented default (10)
/// explicitly instead of inheriting it.
pub const DEFAULT_MAX_REDIRECTS: usize = 10;

/// HTTP client settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub timeout: Duration,
    pub default_headers: BTreeMap<String, String>,
    pub rate_limit: RateLimitConfig,
    /// TLS verification exceptions, as `host` (matches any port) or
    /// `host:port` (exact) entries. Hosts compare case-insensitively;
    /// IPv6 literals stay bracketed (`[::1]:8443`). Requests to hosts
    /// not listed here keep full certificate verification.
    pub insecure_hosts: BTreeSet<String>,
    /// Blanket TLS exception (curl `-k` parity): disables certificate
    /// verification for every host in the run. Prefer `insecure_hosts`.
    pub insecure_all: bool,
    /// Follow redirects that downgrade https→http. Default false:
    /// downgrade redirects are refused with an error naming both URLs.
    pub allow_downgrade_redirects: bool,
    /// Maximum redirect hops to follow per request.
    pub max_redirects: usize,
    /// Emit transport warnings (cleartext credentials, insecure
    /// exceptions) on stderr. Structured warning events are emitted
    /// regardless — this controls only the stderr text.
    pub transport_warnings: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        let mut default_headers = BTreeMap::new();
        default_headers.insert("User-Agent".to_string(), "arazzo-cli/0.1".to_string());
        Self {
            timeout: Duration::from_secs(30),
            default_headers,
            rate_limit: RateLimitConfig::default(),
            insecure_hosts: BTreeSet::new(),
            insecure_all: false,
            allow_downgrade_redirects: false,
            max_redirects: DEFAULT_MAX_REDIRECTS,
            transport_warnings: true,
        }
    }
}

// ── Transport policy (pure, decidable from URL text alone) ──────────

/// Returns `true` for loopback hosts by URL text alone (no DNS):
/// `localhost`, `*.localhost`, `127.0.0.0/8` literals, and `[::1]`.
pub fn is_loopback_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let lower = bare.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return true;
    }
    if let Ok(v4) = lower.parse::<std::net::Ipv4Addr>() {
        return v4.octets()[0] == 127;
    }
    if let Ok(v6) = lower.parse::<std::net::Ipv6Addr>() {
        return v6 == std::net::Ipv6Addr::LOCALHOST;
    }
    false
}

/// Outcome of the pure redirect policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RedirectDecision {
    Follow,
    /// https→http downgrade refused (`allow_downgrade_redirects` off).
    RefuseDowngrade,
    /// Following would exceed `max_redirects`.
    RefuseLimit,
}

/// Decides whether to follow one redirect hop. Pure: consults only the
/// two URLs, the count of hops already taken, and the policy knobs.
pub fn decide_redirect(
    prev: &url_crate::Url,
    next: &url_crate::Url,
    hops_taken: usize,
    allow_downgrade_redirects: bool,
    max_redirects: usize,
) -> RedirectDecision {
    if prev.scheme() == "https" && next.scheme() == "http" && !allow_downgrade_redirects {
        return RedirectDecision::RefuseDowngrade;
    }
    if hops_taken >= max_redirects {
        return RedirectDecision::RefuseLimit;
    }
    RedirectDecision::Follow
}

/// Lowercased host key for insecure-host matching; IPv6 literals are
/// normalized to bracketed form to match `insecure_hosts` entries.
fn url_host_key(url: &url_crate::Url) -> Option<String> {
    let raw = url.host_str()?.to_ascii_lowercase();
    match url.host() {
        Some(url_crate::Host::Ipv6(_)) if !raw.starts_with('[') => Some(format!("[{raw}]")),
        Some(_) => Some(raw),
        None => None,
    }
}

/// Splits a `host:port` exception entry. Bracketed IPv6 entries keep
/// their brackets in the host part; a bare `host` entry returns `None`.
fn split_host_port(entry: &str) -> Option<(&str, u16)> {
    if entry.starts_with('[') {
        let end = entry.find(']')?;
        let host = &entry[..=end];
        let port = entry[end + 1..].strip_prefix(':')?.parse().ok()?;
        return Some((host, port));
    }
    let (host, port) = entry.rsplit_once(':')?;
    if host.contains(':') {
        // Unbracketed IPv6 literal — not a host:port entry.
        return None;
    }
    Some((host, port.parse().ok()?))
}

/// Returns the `insecure_hosts` entry matching the URL, if any: a
/// `host:port` entry matches exactly (against the explicit or
/// scheme-default port), a bare `host` entry matches on any port.
fn matched_insecure_entry<'a>(
    insecure_hosts: &'a BTreeSet<String>,
    url: &url_crate::Url,
) -> Option<&'a str> {
    let host = url_host_key(url)?;
    let port = url.port_or_known_default();
    for entry in insecure_hosts {
        let lowered = entry.to_ascii_lowercase();
        match split_host_port(&lowered) {
            Some((entry_host, entry_port)) => {
                if entry_host == host && Some(entry_port) == port {
                    return Some(entry.as_str());
                }
            }
            None => {
                if lowered == host {
                    return Some(entry.as_str());
                }
            }
        }
    }
    None
}

/// reqwest's cross-host predicate for sensitive-header stripping,
/// pinned by the transport characterization tests: a hop is cross-host
/// when the host or the effective (explicit or scheme-default) port
/// changes.
fn is_cross_host_hop(prev: &url_crate::Url, next: &url_crate::Url) -> bool {
    prev.host_str() != next.host_str()
        || prev.port_or_known_default() != next.port_or_known_default()
}

/// `host:port` label for error messages and `--insecure-host` hints.
fn host_port_label(url: &url_crate::Url) -> String {
    let host = url_host_key(url).unwrap_or_else(|| url.as_str().to_string());
    match url.port_or_known_default() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// True when any error in the source chain mentions a certificate —
/// the signal that TLS verification (not connectivity) failed.
fn error_chain_mentions_certificate(err: &reqwest::Error) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(source) = current {
        if source
            .to_string()
            .to_ascii_lowercase()
            .contains("certificate")
        {
            return true;
        }
        current = source.source();
    }
    false
}

/// Referer value for a followed hop (reqwest default `referer(true)`
/// baseline): previous URL with credentials/fragment stripped, never
/// stamped on an https→http downgrade hop.
fn make_referer(
    prev: &url_crate::Url,
    next: &url_crate::Url,
) -> Option<reqwest::header::HeaderValue> {
    if next.scheme() == "http" && prev.scheme() == "https" {
        return None;
    }
    let mut referer = prev.clone();
    let _ = referer.set_username("");
    let _ = referer.set_password(None);
    referer.set_fragment(None);
    referer.as_str().parse().ok()
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

/// Shared mutable transport bookkeeping for one client (== one engine).
#[derive(Debug, Clone, Default)]
struct TransportState {
    /// `insecure_hosts` entries at least one request actually targeted.
    used_insecure_entries: Arc<Mutex<BTreeSet<String>>>,
    /// Hosts already warned about credentialed cleartext requests.
    warned_cleartext_hosts: Arc<Mutex<BTreeSet<String>>>,
}

fn lock_recovering<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        // A panicked holder cannot leave a BTreeSet insert half-done;
        // recover the data rather than propagating the poison.
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Maps a fired cancellation token to the timeout or cancelled error.
fn cancel_error(is_timeout: &AtomicBool) -> RuntimeError {
    if is_timeout.load(Ordering::Acquire) {
        RuntimeError::new(
            RuntimeErrorKind::ExecutionTimeout,
            "execution timeout exceeded",
        )
    } else {
        RuntimeError::new(RuntimeErrorKind::ExecutionCancelled, "execution cancelled")
    }
}

#[derive(Debug, Clone)]
pub(super) struct HttpClient {
    mode: HttpClientMode,
    default_headers: BTreeMap<String, String>,
    rate_limiter: Arc<tokio::sync::Mutex<RateLimiterState>>,
    max_response_bytes: usize,
    timeout: Duration,
    insecure_hosts: BTreeSet<String>,
    insecure_all: bool,
    allow_downgrade_redirects: bool,
    max_redirects: usize,
    transport_warnings: bool,
    transport_state: TransportState,
}

#[derive(Debug, Clone)]
enum HttpClientMode {
    Live {
        verified: reqwest::Client,
        /// Second client with certificate verification disabled; built
        /// only when an insecure exception is configured, selected per
        /// request by `insecure_hosts` / `insecure_all` match.
        insecure: Option<reqwest::Client>,
    },
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
            // Redirects are followed by an explicit loop in `request` so
            // hop chains stay attributable per request and policy stays
            // decidable per hop; both clients disable reqwest's own
            // following. The whole-chain deadline is enforced via
            // per-hop request timeouts instead of a builder timeout.
            let build = |accept_invalid_certs: bool| {
                let mut builder =
                    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
                if accept_invalid_certs {
                    builder = builder.danger_accept_invalid_certs(true);
                }
                builder.build().map_err(|err| {
                    RuntimeError::with_source(
                        RuntimeErrorKind::HttpClientBuild,
                        format!("building HTTP client: {err}"),
                        err,
                    )
                })
            };
            let verified = build(false)?;
            let insecure = if config.insecure_all || !config.insecure_hosts.is_empty() {
                Some(build(true)?)
            } else {
                None
            };
            HttpClientMode::Live { verified, insecure }
        };
        Ok(Self {
            mode,
            default_headers: config.default_headers.clone(),
            rate_limiter: Arc::new(tokio::sync::Mutex::new(RateLimiterState::new(
                &config.rate_limit,
            ))),
            max_response_bytes,
            timeout: config.timeout,
            insecure_hosts: config.insecure_hosts.clone(),
            insecure_all: config.insecure_all,
            allow_downgrade_redirects: config.allow_downgrade_redirects,
            max_redirects: config.max_redirects,
            transport_warnings: config.transport_warnings,
            transport_state: TransportState::default(),
        })
    }

    /// True when this client issues live HTTP requests (not replay).
    pub(super) fn is_live(&self) -> bool {
        matches!(self.mode, HttpClientMode::Live { .. })
    }

    /// Whether transport warnings should be written to stderr.
    pub(super) fn transport_warnings_enabled(&self) -> bool {
        self.transport_warnings
    }

    pub(super) fn default_headers(&self) -> &BTreeMap<String, String> {
        &self.default_headers
    }

    /// Records the first credentialed-cleartext warning per host;
    /// returns `false` when the host was already warned this run.
    pub(super) fn note_cleartext_warned(&self, host: &str) -> bool {
        lock_recovering(&self.transport_state.warned_cleartext_hosts).insert(host.to_string())
    }

    /// Configured `insecure_hosts` entries no request targeted. Empty
    /// under a blanket exception (`insecure_all`), where per-entry
    /// usage is meaningless.
    pub(super) fn unused_insecure_entries(&self) -> Vec<String> {
        if self.insecure_all {
            return Vec::new();
        }
        let used = lock_recovering(&self.transport_state.used_insecure_entries);
        self.insecure_hosts
            .iter()
            .filter(|entry| !used.contains(*entry))
            .cloned()
            .collect()
    }

    /// Picks the verified or insecure client for one hop, recording
    /// which exception entry (if any) the hop consumed.
    fn select_client<'a>(
        &'a self,
        verified: &'a reqwest::Client,
        insecure: Option<&'a reqwest::Client>,
        url: &url_crate::Url,
    ) -> Result<&'a reqwest::Client, RuntimeError> {
        let wants_insecure = if self.insecure_all {
            true
        } else if let Some(entry) = matched_insecure_entry(&self.insecure_hosts, url) {
            lock_recovering(&self.transport_state.used_insecure_entries).insert(entry.to_string());
            true
        } else {
            false
        };
        if !wants_insecure {
            return Ok(verified);
        }
        insecure.ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "insecure exception matched but no insecure client was built",
            )
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

        let (verified, insecure) = match &self.mode {
            HttpClientMode::Live { verified, insecure } => (verified, insecure.as_ref()),
            HttpClientMode::Replay(_) => {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    "runtime entered unreachable replay HTTP mode",
                ));
            }
        };

        // One rate-limit token per logical request, not per hop —
        // redirect hops never consumed tokens under reqwest either.
        self.wait_for_rate_limit(cancel, is_timeout).await?;
        let method = reqwest::Method::from_bytes(cfg.method.as_bytes()).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::InvalidHttpMethod,
                format!("invalid HTTP method {}: {err}", cfg.method),
            )
        })?;

        let mut current_url = url_crate::Url::parse(&cfg.url).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::HttpRequest,
                format!("executing request: invalid URL \"{}\": {err}", cfg.url),
            )
        })?;

        // Baseline header semantics: defaults appended first, then
        // per-request headers appended (duplicate names send both
        // values, exactly as reqwest's RequestBuilder::header did).
        let mut current_headers = reqwest::header::HeaderMap::new();
        for (k, v) in self.default_headers.iter().chain(cfg.headers.iter()) {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes()).map_err(|err| {
                RuntimeError::new(
                    RuntimeErrorKind::HttpRequest,
                    format!("executing request: invalid header name \"{k}\": {err}"),
                )
            })?;
            let value = reqwest::header::HeaderValue::from_str(v).map_err(|err| {
                RuntimeError::new(
                    RuntimeErrorKind::HttpRequest,
                    format!("executing request: invalid value for header \"{k}\": {err}"),
                )
            })?;
            current_headers.append(name, value);
        }

        let mut current_method = method;
        let mut current_body = cfg.body;
        let mut hops: Vec<TraceRedirectHop> = Vec::new();
        // Whole-chain deadline: reqwest's builder timeout spanned the
        // full redirect chain plus body; per-hop budgets reproduce it.
        let deadline = Instant::now() + self.timeout;

        let mut resp = loop {
            if cancel.is_cancelled() {
                return Err(cancel_error(is_timeout));
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(RuntimeError::new(
                    RuntimeErrorKind::HttpRequest,
                    format!(
                        "executing request: operation timed out after {:?} across redirect chain",
                        self.timeout
                    ),
                ));
            };
            let client = self.select_client(verified, insecure, &current_url)?;
            let mut req = client
                .request(current_method.clone(), current_url.clone())
                .timeout(remaining)
                .headers(current_headers.clone());
            if let Some(body) = &current_body {
                req = req.body(body.clone());
            }
            let resp = req.send().await.map_err(|err| {
                let mut message = format!("executing request: {err}");
                if error_chain_mentions_certificate(&err) {
                    let host = host_port_label(&current_url);
                    message.push_str(&format!(
                        " (TLS certificate verification failed for \"{host}\"; if this host is intentionally serving an untrusted certificate, pass --insecure-host {host})"
                    ));
                }
                RuntimeError::with_source(RuntimeErrorKind::HttpRequest, message, err)
            })?;

            let status = resp.status().as_u16();
            if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                break resp;
            }
            let next_url = match resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| current_url.join(location).ok())
            {
                Some(next) => next,
                // Missing/unparseable Location: return the 30x as-is
                // (baseline reqwest behavior).
                None => break resp,
            };

            match decide_redirect(
                &current_url,
                &next_url,
                hops.len(),
                self.allow_downgrade_redirects,
                self.max_redirects,
            ) {
                RedirectDecision::Follow => {}
                RedirectDecision::RefuseDowngrade => {
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::RedirectDowngradeRefused,
                        format!(
                            "refusing https→http downgrade redirect from \"{current_url}\" to \"{next_url}\" (pass --allow-downgrade-redirects to follow it)"
                        ),
                    ));
                }
                RedirectDecision::RefuseLimit => {
                    return Err(RuntimeError::new(
                        RuntimeErrorKind::RedirectLimitExceeded,
                        format!(
                            "redirect limit of {} exceeded at \"{next_url}\" (raise --max-redirects to follow further)",
                            self.max_redirects
                        ),
                    ));
                }
            }

            hops.push(TraceRedirectHop {
                status_code: i64::from(status),
                from: current_url.to_string(),
                to: next_url.to_string(),
            });

            // 30x semantics pinned by the characterization tests:
            // 301/302/303 become GET and drop the body (and its
            // headers); 307/308 preserve method and body.
            if matches!(status, 301..=303) {
                if current_method != reqwest::Method::GET && current_method != reqwest::Method::HEAD
                {
                    current_method = reqwest::Method::GET;
                }
                current_body = None;
                for header in [
                    reqwest::header::TRANSFER_ENCODING,
                    reqwest::header::CONTENT_ENCODING,
                    reqwest::header::CONTENT_TYPE,
                    reqwest::header::CONTENT_LENGTH,
                ] {
                    current_headers.remove(header);
                }
            }
            if is_cross_host_hop(&current_url, &next_url) {
                for header in [
                    reqwest::header::AUTHORIZATION,
                    reqwest::header::COOKIE,
                    reqwest::header::PROXY_AUTHORIZATION,
                    reqwest::header::WWW_AUTHENTICATE,
                ] {
                    current_headers.remove(header);
                }
                current_headers.remove("cookie2");
            }
            if let Some(referer) = make_referer(&current_url, &next_url) {
                current_headers.insert(reqwest::header::REFERER, referer);
            }
            current_url = next_url;
        };

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
            redirects: hops,
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
            redirects: record.request.redirects.clone(),
        })
    }

    async fn wait_for_rate_limit(
        &self,
        cancel: &CancellationToken,
        is_timeout: &AtomicBool,
    ) -> Result<(), RuntimeError> {
        loop {
            if cancel.is_cancelled() {
                return Err(cancel_error(is_timeout));
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
    /// Redirect hops followed before this response, oldest first.
    pub redirects: Vec<TraceRedirectHop>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(raw: &str) -> url_crate::Url {
        match url_crate::Url::parse(raw) {
            Ok(url) => url,
            Err(err) => panic!("parsing test URL {raw}: {err}"),
        }
    }

    #[test]
    fn loopback_hosts_by_text_alone() {
        for host in [
            "localhost",
            "LOCALHOST",
            "api.localhost",
            "127.0.0.1",
            "127.9.8.7",
            "[::1]",
            "::1",
        ] {
            assert!(is_loopback_host(host), "{host} should be loopback");
        }
        for host in ["example.com", "192.0.2.1", "10.0.0.1", "[::2]", "128.0.0.1"] {
            assert!(!is_loopback_host(host), "{host} should not be loopback");
        }
    }

    #[test]
    fn decide_redirect_scheme_matrix() {
        let cases = [
            ("https://a/", "https://a/b", RedirectDecision::Follow),
            ("http://a/", "http://a/b", RedirectDecision::Follow),
            ("http://a/", "https://a/b", RedirectDecision::Follow),
            (
                "https://a/",
                "http://a/b",
                RedirectDecision::RefuseDowngrade,
            ),
        ];
        for (prev, next, expected) in cases {
            assert_eq!(
                decide_redirect(&u(prev), &u(next), 0, false, DEFAULT_MAX_REDIRECTS),
                expected,
                "{prev} -> {next}"
            );
        }
        // The downgrade follows once explicitly allowed.
        assert_eq!(
            decide_redirect(&u("https://a/"), &u("http://a/b"), 0, true, 10),
            RedirectDecision::Follow
        );
    }

    #[test]
    fn decide_redirect_hop_limits() {
        let prev = u("http://a/");
        let next = u("http://a/b");
        assert_eq!(
            decide_redirect(&prev, &next, 1, false, 2),
            RedirectDecision::Follow
        );
        assert_eq!(
            decide_redirect(&prev, &next, 2, false, 2),
            RedirectDecision::RefuseLimit
        );
        assert_eq!(
            decide_redirect(&prev, &next, 0, false, 0),
            RedirectDecision::RefuseLimit
        );
        // Downgrade refusal outranks the hop limit.
        assert_eq!(
            decide_redirect(&u("https://a/"), &u("http://a/b"), 5, false, 2),
            RedirectDecision::RefuseDowngrade
        );
    }

    #[test]
    fn insecure_entry_matching_rules() {
        let hosts: BTreeSet<String> = [
            "bare.example".to_string(),
            "Exact.Example:8443".to_string(),
            "[::1]:8443".to_string(),
        ]
        .into_iter()
        .collect();

        // Bare host entries match any port.
        assert!(matched_insecure_entry(&hosts, &u("https://bare.example/")).is_some());
        assert!(matched_insecure_entry(&hosts, &u("https://bare.example:9999/")).is_some());
        // host:port entries match exactly, case-insensitively, including
        // the scheme-default port for URLs without an explicit one.
        assert!(matched_insecure_entry(&hosts, &u("https://exact.example:8443/")).is_some());
        assert!(matched_insecure_entry(&hosts, &u("https://EXACT.example:8443/")).is_some());
        assert!(matched_insecure_entry(&hosts, &u("https://exact.example:8444/")).is_none());
        assert!(matched_insecure_entry(&hosts, &u("https://exact.example/")).is_none());
        // IPv6 literals stay bracketed.
        assert!(matched_insecure_entry(&hosts, &u("https://[::1]:8443/")).is_some());
        assert!(matched_insecure_entry(&hosts, &u("https://[::1]:8444/")).is_none());
        // Unlisted hosts never match.
        assert!(matched_insecure_entry(&hosts, &u("https://other.example/")).is_none());
    }

    #[test]
    fn ipv6_url_hosts_normalize_to_bracketed_keys() {
        assert_eq!(
            url_host_key(&u("https://[::1]:8443/")).as_deref(),
            Some("[::1]")
        );
        assert_eq!(
            url_host_key(&u("https://example.com/")).as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn cross_host_hop_predicate_matches_reqwest_baseline() {
        assert!(!is_cross_host_hop(&u("http://a:1/x"), &u("http://a:1/y")));
        assert!(is_cross_host_hop(&u("http://a:1/"), &u("http://a:2/")));
        assert!(is_cross_host_hop(&u("http://a/"), &u("http://b/")));
        // Scheme change flips the known-default port (80 vs 443).
        assert!(is_cross_host_hop(&u("http://a/"), &u("https://a/")));
        // Explicit ports equal across a scheme change: same effective
        // authority, not cross-host (reqwest's predicate).
        assert!(!is_cross_host_hop(
            &u("https://a:9000/"),
            &u("http://a:9000/")
        ));
    }

    #[test]
    fn split_host_port_entries() {
        assert_eq!(split_host_port("host:8443"), Some(("host", 8443)));
        assert_eq!(split_host_port("host"), None);
        assert_eq!(split_host_port("[::1]:8443"), Some(("[::1]", 8443)));
        assert_eq!(split_host_port("[::1]"), None);
        assert_eq!(split_host_port("::1"), None);
        assert_eq!(split_host_port("host:notaport"), None);
    }
}
