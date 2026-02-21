#![forbid(unsafe_code)]

mod runtime_core;

pub use runtime_core::*;

#[cfg(test)]
use arazzo_expr::{EvalContext, ExpressionEvaluator};
#[cfg(test)]
use arazzo_spec::{ArazzoSpec, OnAction, Step, SuccessCriterion, Workflow};
#[cfg(test)]
use runtime_core::{
    build_levels, evaluate_criterion, extract_step_refs, extract_xpath, has_control_flow,
    parse_method, VarStore,
};

#[cfg(test)]
mod tests {
    use super::{
        build_levels, evaluate_criterion, extract_step_refs, has_control_flow, parse_method,
        ArazzoSpec, ClientConfig, Engine, EvalContext, ExecutionOptions, ExpressionEvaluator,
        OnAction, Response, RuntimeError, RuntimeErrorKind, Step, StepEvent, SuccessCriterion,
        TraceDecisionPath, TraceHook, Workflow,
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
            err.message
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
        assert_eq!(err.message, "step s1: workflow ended by onFailure action");
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
        assert_eq!(err.message, "step s1: max retries (3) exceeded");
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
        assert_eq!(err.message, "step s1: max retries (2) exceeded");
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
        assert_eq!(err.message, "execution timeout exceeded");
        assert_eq!(err.kind, RuntimeErrorKind::ExecutionTimeout);
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
        assert_eq!(err.message, "execution cancelled");
        assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
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
            .message
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
        assert_eq!(
            bad_goto_err.message,
            r#"goto: step "nonexistent" not found"#
        );
        assert_eq!(bad_goto_err.kind, RuntimeErrorKind::GotoTargetNotFound);

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
        assert_eq!(
            empty_goto_err.message,
            "goto: no stepId or workflowId specified"
        );
        assert_eq!(empty_goto_err.kind, RuntimeErrorKind::GotoTargetMissing);
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
        assert_eq!(err.message, r#"workflow "nonexistent" not found"#);
        assert_eq!(err.kind, RuntimeErrorKind::WorkflowNotFound);
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
        assert!(err.message.contains("sub-workflow child"));
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
        assert!(err.message.contains("max call depth"));
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
        assert!(err.message.contains(r#"workflow "nonexistent" not found"#));
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
        assert!(err.message.contains("operationId"));
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
        assert!(err.message.contains("step fail"));
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
    fn trace_records_sequential_include_request_response_criteria_decision() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(200, r#"{"ok":true,"value":7}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                operation_path: "/ok".to_string(),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "value".to_string(),
                    "$response.body.value".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("value".to_string(), "$steps.s1.outputs.value".to_string())]),
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_trace_enabled(true);
        let outputs = match engine.execute("wf", BTreeMap::new()) {
            Ok(outputs) => outputs,
            Err(err) => panic!("expected success, got: {err}"),
        };
        assert_eq!(outputs.get("value"), Some(&json!(7)));

        let trace = engine.trace_steps();
        assert_eq!(trace.len(), 1);
        let record = &trace[0];
        assert_eq!(record.seq, 1);
        assert_eq!(record.workflow_id, "wf");
        assert_eq!(record.step_id, "s1");
        assert_eq!(record.attempt, 1);
        assert_eq!(record.kind, "http");
        assert_eq!(record.operation_path, "/ok");
        assert_eq!(record.decision.path, TraceDecisionPath::Next);
        assert_eq!(
            record
                .request
                .as_ref()
                .map(|request| request.method.as_str()),
            Some("GET")
        );
        assert!(record
            .request
            .as_ref()
            .map(|request| request.url.contains("/ok"))
            .unwrap_or(false));
        assert_eq!(
            record
                .response
                .as_ref()
                .map(|response| response.status_code),
            Some(200)
        );
        assert_eq!(record.criteria.len(), 1);
        assert!(record.criteria[0].result);
        assert_eq!(record.outputs.get("value"), Some(&json!(7)));
        assert_eq!(record.error, None);
    }

    #[test]
    fn trace_records_retry_attempts_increment() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            let attempt = calls_clone.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                MockHttpResponse::empty(429)
            } else {
                MockHttpResponse::json(200, r#"{"ok":true}"#)
            }
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "retry-step".to_string(),
                operation_path: "/retry".to_string(),
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
        engine.set_trace_enabled(true);
        if let Err(err) = engine.execute("wf", BTreeMap::new()) {
            panic!("expected success, got: {err}");
        }

        let trace = engine.trace_steps();
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].seq, 1);
        assert_eq!(trace[1].seq, 2);
        assert_eq!(trace[0].attempt, 1);
        assert_eq!(trace[1].attempt, 2);
        assert_eq!(trace[0].decision.path, TraceDecisionPath::Retry);
        assert_eq!(
            trace[0]
                .response
                .as_ref()
                .map(|response| response.status_code),
            Some(429)
        );
        assert_eq!(trace[1].decision.path, TraceDecisionPath::Next);
        assert_eq!(
            trace[1]
                .response
                .as_ref()
                .map(|response| response.status_code),
            Some(200)
        );
    }

    #[test]
    fn trace_records_capture_step_error() {
        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "broken".to_string(),
                operation_path: "http://[::1".to_string(),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine("http://localhost", spec);
        engine.set_trace_enabled(true);
        let result = engine.execute("wf", BTreeMap::new());
        assert!(result.is_err());

        let trace = engine.trace_steps();
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].step_id, "broken");
        assert_eq!(trace[0].decision.path, TraceDecisionPath::Error);
        assert!(trace[0].error.is_some());
        assert_eq!(trace[0].request, None);
        assert_eq!(trace[0].response, None);
    }

    #[test]
    fn trace_records_subworkflow_parent_and_child_workflow_ids() {
        let server = start_server(|_method, _url, _headers, _body| {
            MockHttpResponse::json(200, r#"{"ok":true}"#)
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
                    step_id: "child-step".to_string(),
                    operation_path: "/child".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_trace_enabled(true);
        if let Err(err) = engine.execute("parent", BTreeMap::new()) {
            panic!("expected success, got: {err}");
        }

        let trace = engine.trace_steps();
        assert_eq!(trace.len(), 2);

        let parent_record = match trace.iter().find(|record| record.step_id == "call-child") {
            Some(record) => record,
            None => panic!("missing parent trace record"),
        };
        assert_eq!(parent_record.workflow_id, "parent");
        assert_eq!(parent_record.kind, "workflow");
        assert_eq!(parent_record.workflow_id_ref, "child");
        assert_eq!(parent_record.request, None);
        assert_eq!(parent_record.response, None);
        assert_eq!(parent_record.decision.path, TraceDecisionPath::Next);

        let child_record = match trace.iter().find(|record| record.step_id == "child-step") {
            Some(record) => record,
            None => panic!("missing child trace record"),
        };
        assert_eq!(child_record.workflow_id, "child");
        assert_eq!(child_record.kind, "http");
        assert_eq!(child_record.attempt, 1);
        assert_eq!(child_record.decision.path, TraceDecisionPath::Next);
        assert_eq!(
            child_record
                .response
                .as_ref()
                .map(|response| response.status_code),
            Some(200)
        );
    }

    #[test]
    fn trace_records_parallel_order_is_deterministic_by_seq() {
        let server = start_server_concurrent(|_method, url, _headers, _body| {
            match url.as_str() {
                "/slow" => thread::sleep(Duration::from_millis(60)),
                "/fast" => thread::sleep(Duration::from_millis(5)),
                "/mid" => thread::sleep(Duration::from_millis(20)),
                _ => {}
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        });

        let spec = make_spec(vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    operation_path: "/slow".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    operation_path: "/fast".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    operation_path: "/mid".to_string(),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]);

        let mut engine = new_test_engine(&server.base_url, spec);
        engine.set_parallel_mode(true);
        engine.set_trace_enabled(true);
        if let Err(err) = engine.execute("wf", BTreeMap::new()) {
            panic!("expected success, got: {err}");
        }

        let trace = engine.trace_steps();
        assert_eq!(trace.len(), 3);
        assert_eq!(trace[0].seq, 1);
        assert_eq!(trace[1].seq, 2);
        assert_eq!(trace[2].seq, 3);
        assert_eq!(trace[0].step_id, "s1");
        assert_eq!(trace[1].step_id, "s2");
        assert_eq!(trace[2].step_id, "s3");
        assert_eq!(trace[0].decision.path, TraceDecisionPath::Next);
        assert_eq!(trace[1].decision.path, TraceDecisionPath::Next);
        assert_eq!(trace[2].decision.path, TraceDecisionPath::Next);
        assert_eq!(trace[0].attempt, 1);
        assert_eq!(trace[1].attempt, 1);
        assert_eq!(trace[2].attempt, 1);
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
        assert!(cycle_err.message.contains("dependency cycle detected"));
        assert_eq!(cycle_err.kind, RuntimeErrorKind::DependencyCycle);
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
        let err = RuntimeError::unspecified("boom".to_string());
        assert_eq!(err.to_string(), "boom".to_string());
    }

    #[test]
    fn runtime_error_kind_has_stable_code() {
        let err = RuntimeError::new(RuntimeErrorKind::WorkflowNotFound, "workflow missing");
        assert_eq!(err.code(), "RUNTIME_WORKFLOW_NOT_FOUND");
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
