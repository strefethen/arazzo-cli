#![forbid(unsafe_code)]

//! Arazzo workflow execution engine.
//!
//! This crate provides [`Engine`] for executing Arazzo 1.0.1 workflow specifications
//! at runtime. It handles HTTP request construction, expression evaluation,
//! success criteria checking, control flow (onSuccess/onFailure actions),
//! sub-workflow calls, retry logic, and parallel step execution.
//!
//! # Usage
//!
//! ```ignore
//! use std::collections::BTreeMap;
//! use arazzo_runtime::EngineBuilder;
//!
//! let spec = arazzo_validate::parse("spec.arazzo.yaml")?;
//! let engine = EngineBuilder::new(spec)
//!     .parallel(true)
//!     .trace(true)
//!     .build()?;
//! let inputs = BTreeMap::new();
//! let result = engine.execute_collect("workflow-id", inputs).await;
//! ```

mod debug;
mod runtime_core;

pub use debug::*;
pub use runtime_core::*;

/// Stable internal runtime API baseline for trace/replay/debugger integrations.
pub const INTERNAL_RUNTIME_API_VERSION: &str = "v1";

/// Frozen v1 runtime-facing models used by CLI trace/replay/debugger plumbing.
pub mod api_v1 {
    pub use crate::{
        EngineEvent, ExecutionEvent, ExecutionEventKind, ExecutionHandle, ExecutionObserver,
        ExecutionResult, ObserverEvent, RuntimeError, RuntimeErrorKind, TraceCriterionResult,
        TraceDecision, TraceDecisionPath, TraceRequest, TraceResponse, TraceStepRecord,
    };
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::runtime_core::VarStore;
    use super::{ClientConfig, ContentType, Engine, Response, RuntimeErrorKind};
    use arazzo_spec::{
        ActionType, ArazzoSpec, CriterionExpressionType, CriterionType, Info, OnAction,
        ParamLocation, SourceDescription, SourceType, Step, StepTarget, SuccessCriterion, Workflow,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};
    use url::Url;

    // ── Minimal test infrastructure (duplicated from tests/common/) ──

    #[derive(Debug, Clone)]
    struct MockHttpResponse {
        status: u16,
        headers: BTreeMap<String, String>,
        body: String,
    }

    impl MockHttpResponse {
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
                if handle.join().is_err() {
                    // Test helper shutdown: server thread panic does not affect assertions.
                }
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

    // ── Tests requiring pub(crate) access ───────────────────────────

    #[test]
    fn find_matching_action_behavior() {
        let engine = new_test_engine("http://localhost", make_spec(Vec::new()));
        let vars = VarStore::default();

        let no_criteria = vec![OnAction {
            type_: ActionType::End,
            ..OnAction::default()
        }];
        let first = engine.find_matching_action(&no_criteria, &vars, None);
        assert!(first.is_some());
        if let Some(action) = first {
            assert_eq!(action.type_, ActionType::End);
        }

        let with_criteria = vec![OnAction {
            type_: ActionType::Retry,
            criteria: vec![SuccessCriterion {
                condition: "$statusCode == 429".to_string(),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }];
        let none = engine.find_matching_action(&with_criteria, &vars, None);
        assert!(none.is_none());

        let response = Response {
            status_code: 429,
            headers: BTreeMap::new(),
            body: Vec::new(),
            body_json: None,
            content_type: ContentType::Json,
        };
        let ordered = vec![
            OnAction {
                name: "first".to_string(),
                type_: ActionType::Retry,
                criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 429".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..OnAction::default()
            },
            OnAction {
                name: "second".to_string(),
                type_: ActionType::End,
                ..OnAction::default()
            },
        ];
        let matched = engine.find_matching_action(&ordered, &vars, Some(&response));
        assert!(matched.is_some());
        if let Some(action) = matched {
            assert_eq!(action.name, "first".to_string());
        }

        let response_with_json = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: br#"{"pets":[{"id":1}]}"#.to_vec(),
            body_json: Some(json!({"pets":[{"id":1}]})),
            content_type: ContentType::Json,
        };
        let typed = vec![OnAction {
            name: "typed".to_string(),
            type_: ActionType::Goto,
            step_id: "next".to_string(),
            criteria: vec![SuccessCriterion {
                context: "$response.body".to_string(),
                condition: "$.pets[0]".to_string(),
                type_: Some(CriterionType::ExpressionType(CriterionExpressionType {
                    type_: "jsonpath".to_string(),
                    version: "draft-goessner-dispatch-jsonpath-00".to_string(),
                })),
            }],
            ..OnAction::default()
        }];
        let typed_match = engine.find_matching_action(&typed, &vars, Some(&response_with_json));
        assert!(typed_match.is_some());
        if let Some(action) = typed_match {
            assert_eq!(action.name, "typed".to_string());
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

        let mut vars = VarStore::default();
        vars.set_input("name", json!("test"));
        vars.set_step_output("s1", "result", json!("hello"));

        let outputs = engine.build_outputs(&workflow, &vars);
        assert_eq!(outputs.get("inputName"), Some(&json!("test")));
        assert_eq!(outputs.get("stepResult"), Some(&json!("hello")));
    }

    fn make_spec_with_base(base_url: &str, workflows: Vec<Workflow>) -> ArazzoSpec {
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

    #[tokio::test]
    async fn execute_respects_client_rate_limit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            calls_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(200)
        });

        let spec = make_spec_with_base(
            &server.base_url,
            vec![Workflow {
                workflow_id: "rate-limit".to_string(),
                steps: vec![
                    Step {
                        step_id: "s1".to_string(),
                        target: Some(StepTarget::OperationPath("/one".to_string())),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "s2".to_string(),
                        target: Some(StepTarget::OperationPath("/two".to_string())),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            }],
        );

        let mut cfg = ClientConfig::default();
        cfg.rate_limit.requests_per_second = 1.0;
        cfg.rate_limit.burst = 1;

        let engine = match Engine::with_client_config(spec, cfg) {
            Ok(engine) => engine,
            Err(err) => panic!("creating engine: {err}"),
        };

        let started = Instant::now();
        let result = engine.execute_collect("rate-limit", BTreeMap::new()).await;
        if let Err(err) = result.outputs {
            panic!("expected success, got: {err}");
        }
        assert!(started.elapsed() >= Duration::from_millis(850));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn build_url_encodes_query_params() {
        let engine = new_test_engine("http://localhost", make_spec(Vec::new()));
        let mut vars = VarStore::default();
        vars.set_input("q", json!("hello world&more=stuff"));

        let step = Step {
            target: Some(StepTarget::OperationPath("/search".to_string())),
            parameters: vec![
                arazzo_spec::Parameter {
                    name: "q".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$inputs.q".to_string()),
                    ..arazzo_spec::Parameter::default()
                },
                arazzo_spec::Parameter {
                    name: "tag".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("a=b".to_string()),
                    ..arazzo_spec::Parameter::default()
                },
            ],
            ..Step::default()
        };

        let url_result = match engine.build_url_from_path("/search", &step, &vars) {
            Ok(v) => v,
            Err(err) => panic!("building URL for query encoding test: {err}"),
        };
        let parsed = match Url::parse(&url_result.url) {
            Ok(v) => v,
            Err(err) => panic!("parsing url {}: {err}", url_result.url),
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
        let vars = VarStore::default();
        let step = Step {
            target: Some(StepTarget::OperationPath("/users".to_string())),
            ..Step::default()
        };

        let url_result = match engine.build_url_from_path("/users", &step, &vars) {
            Ok(v) => v,
            Err(err) => panic!("building URL for slash normalization test: {err}"),
        };
        assert_eq!(url_result.url, "https://api.example.com/users");
        assert!(!url_result.url.contains("//users"));
    }

    #[test]
    fn build_url_errors_for_unknown_source_description_prefix() {
        let engine = new_test_engine("https://api.example.com", make_spec(Vec::new()));
        let vars = VarStore::default();
        let step = Step {
            target: Some(StepTarget::OperationPath("{missing}./users".to_string())),
            ..Step::default()
        };

        let err = match engine.build_url_from_path("{missing}./users", &step, &vars) {
            Ok(result) => panic!(
                "expected unknown sourceDescription error, got URL {}",
                result.url
            ),
            Err(err) => err,
        };
        assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionNotFound);
        assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND");
    }
}
