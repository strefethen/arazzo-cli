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

// ── Spec builders ───────────────────────────────────────────────────

pub fn make_spec(workflows: Vec<Workflow>) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "test".to_string(),
            summary: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: "http://localhost".to_string(),
            type_: SourceType::OpenApi,
        }],
        workflows,
        components: None,
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
        },
        source_descriptions: vec![SourceDescription {
            name: "test".to_string(),
            url: base_url.to_string(),
            type_: SourceType::OpenApi,
        }],
        workflows,
        components: None,
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

pub fn to_yaml(value: Value) -> serde_yml::Value {
    match serde_yml::to_value(value) {
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
