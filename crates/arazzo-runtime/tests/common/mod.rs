//! Shared test infrastructure for arazzo-runtime integration tests.

#![allow(dead_code)]

use arazzo_runtime::{
    Engine, EngineBuilder, ExecutionObserver, ObserverEvent, StepEvent, TraceHook,
};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, SuccessCriterion, Workflow};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

// ── Mock HTTP response ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MockHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl MockHttpResponse {
    pub fn json(status: u16, body: &str) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            status,
            headers,
            body: body.to_string(),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: String::new(),
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("Location".to_string(), location.to_string());
        Self {
            status,
            headers,
            body: String::new(),
        }
    }
}

// ── Test server ─────────────────────────────────────────────────────

pub struct TestServer {
    pub base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                // Test helper shutdown: server thread panic does not affect assertions.
            }
        }
    }
}

pub fn start_server<F>(handler: F) -> TestServer
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
                    if request.as_reader().read_to_string(&mut body).is_err() {
                        // Test helper: unreadable request body is treated as empty.
                    }

                    let response_data = handler(method, url, headers, body);
                    let mut response = TinyResponse::from_string(response_data.body)
                        .with_status_code(StatusCode(response_data.status));
                    for (name, value) in response_data.headers {
                        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                            response = response.with_header(header);
                        }
                    }
                    if request.respond(response).is_err() {
                        // Test helper: client may disconnect before reading response.
                    }
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

/// Like [`start_server`], but the handler also receives the server's own
/// base URL (useful for emitting absolute redirect Location headers).
pub fn start_server_with_base<F>(handler: F) -> TestServer
where
    F: Fn(&str, String, String, BTreeMap<String, String>, String) -> MockHttpResponse
        + Send
        + Sync
        + 'static,
{
    let server = match Server::http("127.0.0.1:0") {
        Ok(server) => server,
        Err(err) => panic!("binding test server: {err}"),
    };
    let base_url = format!("http://{}", server.server_addr());
    let handler_base = base_url.clone();
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
                    if request.as_reader().read_to_string(&mut body).is_err() {
                        // Test helper: unreadable request body is treated as empty.
                    }

                    let response_data = handler(&handler_base, method, url, headers, body);
                    let mut response = TinyResponse::from_string(response_data.body)
                        .with_status_code(StatusCode(response_data.status));
                    for (name, value) in response_data.headers {
                        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                            response = response.with_header(header);
                        }
                    }
                    if request.respond(response).is_err() {
                        // Test helper: client may disconnect before reading response.
                    }
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

pub fn start_server_concurrent<F>(handler: F) -> TestServer
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
                        if request.as_reader().read_to_string(&mut body).is_err() {
                            // Test helper: unreadable request body is treated as empty.
                        }

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
                        if request.respond(response).is_err() {
                            // Test helper: client may disconnect before reading response.
                        }
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

// ── TLS test server ─────────────────────────────────────────────────

/// One request observed by a recording test server.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

/// Shared recorder used to observe requests arriving at a test server.
pub type RequestLog = Arc<Mutex<Vec<RecordedRequest>>>;

pub fn new_request_log() -> RequestLog {
    Arc::new(Mutex::new(Vec::new()))
}

pub fn record_request(
    log: &RequestLog,
    method: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    body: &str,
) {
    match log.lock() {
        Ok(mut guard) => guard.push(RecordedRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: headers.clone(),
            body: body.to_string(),
        }),
        Err(_) => panic!("recording request"),
    }
}

pub fn logged_requests(log: &RequestLog) -> Vec<RecordedRequest> {
    match log.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading recorded requests"),
    }
}

/// Starts an in-process HTTPS server backed by a fresh self-signed
/// certificate generated at test time (never OS-trusted). Hermetic:
/// binds 127.0.0.1 only. Built directly on rustls 0.23 (the workspace
/// TLS backend) rather than tiny_http's ssl feature, which pins
/// rustls 0.20/ring 0.16 and trips `cargo audit`.
pub fn start_tls_server<F>(handler: F) -> TestServer
where
    F: Fn(String, String, BTreeMap<String, String>, String) -> MockHttpResponse
        + Send
        + Sync
        + 'static,
{
    use std::io::{Read, Write};

    let certified = match rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ]) {
        Ok(v) => v,
        Err(err) => panic!("generating self-signed certificate: {err}"),
    };
    let cert_der = certified.cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()),
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_config = match rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|builder| {
            builder
                .with_no_client_auth()
                .with_single_cert(vec![cert_der], key_der)
        }) {
        Ok(config) => Arc::new(config),
        Err(err) => panic!("building rustls server config: {err}"),
    };

    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(err) => panic!("binding TLS test server: {err}"),
    };
    if let Err(err) = listener.set_nonblocking(true) {
        panic!("configuring TLS test listener: {err}");
    }
    let addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(err) => panic!("reading TLS test listener addr: {err}"),
    };
    let base_url = format!("https://{addr}");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handler = Arc::new(handler);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            let (tcp, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
            };
            if tcp.set_nonblocking(false).is_err() {
                continue;
            }
            let conn = match rustls::ServerConnection::new(Arc::clone(&server_config)) {
                Ok(conn) => conn,
                Err(_) => continue,
            };
            let mut stream = rustls::StreamOwned::new(conn, tcp);

            // Read the request head, then the body per Content-Length.
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 4096];
            let head_end = loop {
                if let Some(pos) = find_double_crlf(&buf) {
                    break pos;
                }
                match stream.read(&mut chunk) {
                    Ok(0) => break usize::MAX,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => break usize::MAX,
                }
            };
            if head_end == usize::MAX {
                continue;
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or_default();
            let mut parts = request_line.split(' ');
            let method = parts.next().unwrap_or_default().to_string();
            let url = parts.next().unwrap_or_default().to_string();
            let mut headers = BTreeMap::new();
            for line in lines {
                if let Some((name, value)) = line.split_once(':') {
                    headers.insert(name.trim().to_string(), value.trim().to_string());
                }
            }
            let content_length = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(0);
            let mut body_bytes = buf[head_end + 4..].to_vec();
            while body_bytes.len() < content_length {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => body_bytes.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            let body = String::from_utf8_lossy(&body_bytes).to_string();

            let response_data = handler(method, url, headers, body);
            let mut out = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                response_data.status,
                reason_phrase(response_data.status),
                response_data.body.len()
            );
            for (name, value) in &response_data.headers {
                out.push_str(&format!("{name}: {value}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&response_data.body);
            if stream.write_all(out.as_bytes()).is_err() {
                continue;
            }
            stream.conn.send_close_notify();
            let _ = stream.flush();
        }
    });

    TestServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        404 => "Not Found",
        _ => "Status",
    }
}

// ── Spec builders ───────────────────────────────────────────────────

pub fn make_spec(workflows: Vec<Workflow>) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "test".to_string(),
            summary: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows,
        components: None,
        ..ArazzoSpec::default()
    }
}

pub fn make_spec_with_base(base_url: &str, workflows: Vec<Workflow>) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "test".to_string(),
            summary: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows,
        components: None,
        ..ArazzoSpec::default()
    }
}

pub fn new_test_engine(base_url: &str, mut spec: ArazzoSpec) -> Engine {
    if let Some(source) = spec.source_descriptions.get_mut(0) {
        source.url = base_url.to_string();
    }
    match Engine::new(spec) {
        Ok(engine) => engine,
        Err(err) => panic!("creating engine: {err}"),
    }
}

// ── Assertion helpers ───────────────────────────────────────────────

pub fn success_200() -> Vec<SuccessCriterion> {
    vec![SuccessCriterion {
        condition: "$statusCode == 200".to_string(),
        ..SuccessCriterion::default()
    }]
}

pub fn to_yaml(value: Value) -> serde_yaml_ng::Value {
    match serde_yaml_ng::to_value(value) {
        Ok(v) => v,
        Err(err) => panic!("converting json to yaml: {err}"),
    }
}

pub fn header_value(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
    for (key, value) in headers {
        if key.eq_ignore_ascii_case(name) {
            return Some(value.clone());
        }
    }
    None
}

// ── Trace hook ──────────────────────────────────────────────────────

#[derive(Default)]
pub struct TestTraceHook {
    pub before_events: Mutex<Vec<StepEvent>>,
    pub after_events: Mutex<Vec<StepEvent>>,
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

// ── Observer ────────────────────────────────────────────────────────

#[derive(Default)]
pub struct TestObserver {
    events: Mutex<Vec<String>>,
}

#[allow(unreachable_patterns)]
impl ExecutionObserver for TestObserver {
    fn on_event(&self, event: &ObserverEvent) {
        // non_exhaustive: wildcard needed for forward compat, but all current
        // variants are matched — allow the unreachable_patterns warning.
        let tag = match event {
            ObserverEvent::StepStarted { step_id, .. } => {
                format!("StepStarted:{step_id}")
            }
            ObserverEvent::RequestPrepared {
                step_id, method, ..
            } => {
                format!("RequestPrepared:{step_id}:{method}")
            }
            ObserverEvent::RequestSent {
                step_id, method, ..
            } => {
                format!("RequestSent:{step_id}:{method}")
            }
            ObserverEvent::CriterionEvaluated {
                step_id,
                index,
                passed,
                ..
            } => {
                format!("CriterionEvaluated:{step_id}:{index}:{passed}")
            }
            ObserverEvent::RetryScheduled {
                step_id,
                attempt,
                max_attempts,
                ..
            } => {
                format!("RetryScheduled:{step_id}:{attempt}/{max_attempts}")
            }
            ObserverEvent::StepCompleted {
                step_id,
                criteria_passed,
                ..
            } => {
                format!("StepCompleted:{step_id}:{criteria_passed}")
            }
            ObserverEvent::SubWorkflowStarted {
                child_workflow_id, ..
            } => {
                format!("SubWorkflowStarted:{child_workflow_id}")
            }
            ObserverEvent::WorkflowCompleted {
                workflow_id, error, ..
            } => {
                let status = if error.is_some() { "error" } else { "ok" };
                format!("WorkflowCompleted:{workflow_id}:{status}")
            }
            _ => "Unknown".to_string(),
        };
        if let Ok(mut guard) = self.events.lock() {
            guard.push(tag);
        }
    }
}

impl TestObserver {
    pub fn events(&self) -> Vec<String> {
        match self.events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => panic!("TestObserver events lock poisoned"),
        }
    }
}

pub fn build_observer_engine(spec: ArazzoSpec, observer: Arc<dyn ExecutionObserver>) -> Engine {
    match EngineBuilder::new(spec).observer(observer).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building observer engine: {err}"),
    }
}

pub fn find_event_pos(events: &[String], needle: &str) -> usize {
    match events.iter().position(|e| e == needle) {
        Some(pos) => pos,
        None => panic!("event {needle:?} not found in {events:?}"),
    }
}
