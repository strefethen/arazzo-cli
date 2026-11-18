//! A value that exists but cannot be read is a hard resolution error, never
//! `null` plus a warning (plan `http-body-media-types-and-binary.md`, D1).
//!
//! The producer here is a response body classified as bytes (`image/jpeg`).
//! Every site a workflow can put in front of that body, except a whole step
//! output that carries it as bytes (`tests/binary_responses.rs`), is driven end
//! to end against a hermetic server: each fails its step with
//! `RUNTIME_VALUE_UNAVAILABLE`, names the step and the reason, and sends no
//! later request. Sites that run before a response exists are proved in
//! `src/runtime_core/value_unavailable_tests.rs`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{DebugController, EngineBuilder, RuntimeError, StepBreakpoint};
use arazzo_spec::{ArazzoSpec, OutputValue};
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const CODE: &str = "RUNTIME_VALUE_UNAVAILABLE";
const REASON: &str = "response body is bytes (image/jpeg), which is not yet a readable value";
const SOURCE: &str = "https://fixture.invalid/openapi.json";
const JPEG: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xE0];

struct PhotoServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl PhotoServer {
    fn start() -> Self {
        let server =
            Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("binding server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(format!("{} {}", request.method(), request.url()));
                let (body, media_type) = if request.url() == "/photo" {
                    (JPEG.to_vec(), "image/jpeg")
                } else {
                    (br#"{"ok":true}"#.to_vec(), "application/json")
                };
                let header = Header::from_bytes("Content-Type", media_type)
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request.respond(TinyResponse::from_data(body).with_header(header));
            }
        });
        Self {
            base_url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn take_requests(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .requests
                .lock()
                .unwrap_or_else(|_| panic!("reading requests")),
        )
    }
}

impl Drop for PhotoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn builder(base: &str, fetch: Value) -> EngineBuilder {
    let mut step = json!({"stepId":"fetch", "operationId":"photo"});
    for (name, value) in fetch.as_object().into_iter().flatten() {
        step[name] = value.clone();
    }
    let spec: ArazzoSpec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"unavailable", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[
            {"workflowId":"main", "steps":[
                step,
                {"stepId":"next", "operationId":"next", "dependsOn":["fetch"]}
            ]},
            {"workflowId":"child", "steps":[{"stepId":"only", "operationId":"next"}]}
        ]
    }));
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":base}],
    "paths":{
        "/photo":{"get":{"operationId":"photo", "responses":{"200":{"description":"ok"}}}},
        "/next":{"get":{"operationId":"next", "responses":{"200":{"description":"ok"}}}}
    }});
    EngineBuilder::new(spec).openapi_spec(openapi.to_string().into_bytes(), None)
}

async fn run(server: &PhotoServer, fetch: Value) -> Result<BTreeMap<String, Value>, RuntimeError> {
    builder(&server.base_url, fetch)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", BTreeMap::new())
        .await
        .outputs
}

fn selector(pointer: &str) -> Value {
    json!({"context":"$response.body", "selector":pointer, "type":"jsonpointer"})
}

fn typed(kind: &str, context: Option<&str>, condition: &str) -> Value {
    let mut criterion = json!({"condition":condition, "type":kind});
    if kind == "xpath" {
        criterion["type"] = json!({"type":"xpath", "version":"xpath-10"});
    }
    if let Some(context) = context {
        criterion["context"] = json!(context);
    }
    criterion
}

#[tokio::test]
async fn every_site_a_workflow_reaches_fails_the_step_instead_of_reading_null() {
    let server = PhotoServer::start();
    let criteria = |criterion: Value| json!({"successCriteria":[criterion]});
    let failing = json!({"condition":"$statusCode == 500"});
    let cases = [
        (
            "pointer",
            json!({"outputs":{"id":"$response.body#/id"}}),
            "step fetch: output \"id\"",
            "$response.body#/id",
        ),
        (
            "Selector Object context as an output",
            json!({"outputs":{"picked":selector("/id")}}),
            "step fetch: output \"picked\"",
            "$response.body",
        ),
        (
            "bare XPath output",
            json!({"outputs":{"pet":"//pet"}}),
            "step fetch: output \"pet\"",
            "//pet",
        ),
        (
            "simple operand",
            criteria(json!({"condition":"$response.body == 'cat'"})),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "simple pointer operand after a passing criterion",
            json!({"successCriteria":[
                {"condition":"$statusCode == 200"},
                {"condition":"$response.body#/id != null"}
            ]}),
            "step fetch: successCriteria[1]",
            "$response.body#/id",
        ),
        (
            "explicit regex context",
            criteria(typed("regex", Some("$response.body"), "^cat")),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "explicit jsonpath context",
            criteria(typed("jsonpath", Some("$response.body#/pets"), "$[?@.id]")),
            "step fetch: successCriteria[0]",
            "$response.body#/pets",
        ),
        (
            "explicit xpath context",
            criteria(typed("xpath", Some("$response.body"), "//pet")),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "default regex context",
            criteria(typed("regex", None, "^cat")),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "default jsonpath context",
            criteria(typed("jsonpath", None, "$[?@.id]")),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "default xpath context",
            criteria(typed("xpath", None, "//pet")),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "interpolation in a typed condition",
            criteria(typed(
                "regex",
                Some("$response.header.Content-Type"),
                "^{$response.body}$",
            )),
            "step fetch: successCriteria[0]",
            "$response.body",
        ),
        (
            "onSuccess action criterion",
            json!({"onSuccess":[{"name":"stop", "type":"end",
                "criteria":[{"condition":"$response.body == 1"}]}]}),
            "step fetch: onSuccess[0].criteria[0]",
            "$response.body",
        ),
        (
            "onFailure action criterion with the default context",
            json!({"successCriteria":[failing], "onFailure":[{"name":"again", "type":"retry",
                "retryAfter":0, "criteria":[typed("regex", None, "busy")]}]}),
            "step fetch: onFailure[0].criteria[0]",
            "$response.body",
        ),
        (
            "interpolated goto step",
            json!({"onSuccess":[{"name":"jump", "type":"goto", "stepId":"{$response.body}"}]}),
            "step fetch: action stepId",
            "$response.body",
        ),
        (
            "interpolated goto workflow",
            json!({"onSuccess":[{"name":"jump", "type":"goto",
                "workflowId":"{$response.body#/workflow}"}]}),
            "step fetch: action workflowId",
            "$response.body#/workflow",
        ),
        (
            "action parameter",
            json!({"onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"child",
                "parameters":[{"name":"photo", "value":"$response.body"}]}]}),
            "step fetch: action parameter \"photo\"",
            "$response.body",
        ),
        (
            "interpolated action parameter",
            json!({"onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"child",
                "parameters":[{"name":"photo", "value":{"caption":"cat {$response.body}"}}]}]}),
            "step fetch: action parameter \"photo\"",
            "$response.body",
        ),
        (
            "Selector Object context as an action parameter",
            json!({"onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"child",
                "parameters":[{"name":"photo", "value":selector("/id")}]}]}),
            "step fetch: action parameter \"photo\"",
            "$response.body",
        ),
        (
            "retry reference parameter",
            json!({"successCriteria":[failing], "onFailure":[{"name":"recover", "type":"retry",
                "retryAfter":0, "workflowId":"child",
                "parameters":[{"name":"photo", "value":"$response.body"}]}]}),
            "step fetch: action parameter \"photo\"",
            "$response.body",
        ),
        (
            "onFailure action criterion after an unavailable output",
            json!({"outputs":{"raw":"$response.body#/id"},
                "onFailure":[{"name":"skip", "type":"goto", "stepId":"next",
                    "criteria":[{"condition":"$response.body == null"}]}]}),
            "step fetch: onFailure[0].criteria[0]",
            "$response.body",
        ),
    ];
    for (site, fetch, location, expression) in cases {
        let error = match run(&server, fetch).await {
            Ok(outputs) => panic!("{site}: the step must fail, got {outputs:?}"),
            Err(error) => error,
        };
        assert_eq!(error.code(), CODE, "{site}: {error}");
        assert_eq!(
            error.message,
            format!("{location}: {expression}: {REASON}"),
            "{site}"
        );
        assert_eq!(server.take_requests(), ["GET /photo"], "{site}");
    }
}

#[tokio::test]
async fn failure_routing_after_an_unavailable_output_reads_the_received_response() {
    let server = PhotoServer::start();
    let outputs = run(
        &server,
        json!({"outputs":{"raw":"$response.body#/id"},
        "onFailure":[{"name":"skip", "type":"goto", "stepId":"next", "criteria":[
            {"condition":"$statusCode == 200"},
            typed("regex", Some("$response.header.Content-Type"), "^image/jpeg$")
        ]}]}),
    )
    .await;
    assert!(outputs.is_ok(), "{outputs:?}");
    assert_eq!(server.take_requests(), ["GET /photo", "GET /next"]);
}

#[tokio::test]
async fn an_attempt_failed_by_an_unavailable_output_keeps_its_exchange_on_record() {
    let server = PhotoServer::start();
    let result = builder(
        &server.base_url,
        json!({"successCriteria":[{"condition":"$statusCode == 200"}],
            "outputs":{"status":"$statusCode", "raw":"$response.body#/id"}}),
    )
    .trace(true)
    .capture_run(true)
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"))
    .execute_collect("main", BTreeMap::new())
    .await;

    let message = format!("step fetch: output \"raw\": $response.body#/id: {REASON}");
    let error = match &result.outputs {
        Ok(outputs) => panic!("the step must fail, got {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(error.code(), CODE, "{error}");
    assert_eq!(error.message, message);
    assert_eq!(server.take_requests(), ["GET /photo"]);

    let traces = result.trace_steps();
    assert_eq!(traces.len(), 1, "{traces:?}");
    let attempt = traces[0];
    assert_eq!(attempt.step_id, "fetch");
    assert_eq!(attempt.error.as_deref(), Some(message.as_str()));
    assert!(attempt.outputs.is_empty(), "{:?}", attempt.outputs);
    let request = attempt
        .request
        .as_ref()
        .unwrap_or_else(|| panic!("the trace must keep the request: {attempt:?}"));
    assert_eq!(request.method, "GET");
    assert_eq!(request.url, format!("{}/photo", server.base_url));
    let response = attempt
        .response
        .as_ref()
        .unwrap_or_else(|| panic!("the trace must keep the response: {attempt:?}"));
    assert_eq!(response.status_code, 200);
    assert_eq!(response.body_bytes, 4);
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-type")
                && value == "image/jpeg"),
        "{:?}",
        response.headers
    );
    assert_eq!(attempt.criteria.len(), 1, "{:?}", attempt.criteria);
    assert!(attempt.criteria[0].result);

    let captured = result.run_steps();
    assert_eq!(captured.len(), 1, "{captured:?}");
    assert!(captured[0].request.is_some(), "{:?}", captured[0]);
    assert!(captured[0].response.is_some(), "{:?}", captured[0]);
}

#[tokio::test]
async fn criteria_and_outputs_that_do_not_read_the_body_still_pass() {
    let server = PhotoServer::start();
    for criteria in [
        json!([{"condition":"$statusCode == 200"}]),
        json!([{"condition":"$statusCode == 200 || $response.body == 1"}]),
        json!([typed(
            "regex",
            Some("$response.header.Content-Type"),
            "^image/"
        )]),
    ] {
        let outputs = run(
            &server,
            json!({"successCriteria":criteria,
                "outputs":{"type":"$response.header.Content-Type", "status":"$statusCode"}}),
        )
        .await;
        assert!(outputs.is_ok(), "{criteria}: {outputs:?}");
        assert_eq!(
            server.take_requests(),
            ["GET /photo", "GET /next"],
            "{criteria}"
        );
    }
}

#[tokio::test]
async fn a_failure_action_that_ends_the_workflow_keeps_the_unavailable_code() {
    let server = PhotoServer::start();
    let error = match run(
        &server,
        json!({"outputs":{"raw":"$response.body#/id"},
            "onFailure":[{"name":"give-up", "type":"end"}]}),
    )
    .await
    {
        Ok(outputs) => panic!("the step must fail, got {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(error.code(), CODE, "{error}");
    assert!(error.message.contains("fetch"), "{error}");
    assert_eq!(server.take_requests(), ["GET /photo"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debugger_evaluate_watch_and_condition_requests_report_the_unavailable_value() {
    let server = PhotoServer::start();
    let controller = Arc::new(DebugController::new());
    let engine = builder(
        &server.base_url,
        json!({"successCriteria":[{"condition":"$statusCode == 200"}]}),
    )
    .debug_controller(Arc::clone(&controller))
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"));
    controller
        .set_breakpoints(vec![
            StepBreakpoint::new("main", "fetch").at_success_criterion(0)
        ])
        .unwrap_or_else(|error| panic!("breakpoint: {error}"));
    let handle = engine.execute("main", BTreeMap::new());
    let stopped = controller
        .wait_for_stop_count(1, Duration::from_secs(5))
        .unwrap_or_else(|error| panic!("waiting for stop: {error}"));

    let unavailable = |expression: &str| format!("{CODE}: step fetch: {expression}: {REASON}");
    let observed = [
        (
            "evaluate",
            controller.evaluate_expression("$response.body").map(|_| ()),
            "$response.body",
        ),
        (
            "watch",
            controller
                .evaluate_watch_expression("$response.body#/id")
                .map(|_| ()),
            "$response.body#/id",
        ),
        (
            "watch list",
            controller
                .evaluate_watches(&["$statusCode".to_owned(), "$response.body".to_owned()])
                .map(|_| ()),
            "$response.body",
        ),
        (
            "condition",
            controller
                .evaluate_condition("$response.body == 1")
                .map(|_| ()),
            "$response.body",
        ),
        (
            "condition expression",
            controller
                .try_evaluate_condition_expression("$response.body#/id == 1")
                .map(|_| ()),
            "$response.body#/id",
        ),
        (
            "output preview",
            controller
                .evaluate_output_value(&OutputValue::RuntimeExpression("$response.body".to_owned()))
                .map(|_| ()),
            "$response.body",
        ),
        (
            "Selector Object output preview",
            controller
                .evaluate_output_value(&parse(selector("/id")))
                .map(|_| ()),
            "$response.body",
        ),
    ];
    let status = controller.evaluate_expression("$statusCode");
    let _ = controller.continue_execution();
    let result = handle.collect().await;

    assert!(stopped, "the criterion breakpoint must stop the run");
    for (request, outcome, expression) in observed {
        assert_eq!(outcome, Err(unavailable(expression)), "{request}");
    }
    assert_eq!(status, Ok(json!(200)));
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breakpoint_condition_over_an_unavailable_value_ends_the_run() {
    let server = PhotoServer::start();
    let controller = Arc::new(DebugController::new());
    let engine = builder(
        &server.base_url,
        json!({"successCriteria":[{"condition":"$statusCode == 200"}]}),
    )
    .debug_controller(Arc::clone(&controller))
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"));
    controller
        .set_breakpoints(vec![StepBreakpoint::new("main", "fetch")
            .at_success_criterion(0)
            .with_condition("$response.body == 1")])
        .unwrap_or_else(|error| panic!("breakpoint: {error}"));

    let error = match engine
        .execute_collect("main", BTreeMap::new())
        .await
        .outputs
    {
        Ok(outputs) => panic!("the gate error must end the run, got {outputs:?}"),
        Err(error) => error,
    };

    assert_eq!(error.code(), "RUNTIME_DEBUG_CONTROLLER", "{error}");
    assert_eq!(
        error.message,
        format!(
            "debug controller: {CODE}: breakpoint condition: \"$response.body == 1\" in workflow: \"main\", step: \"fetch\": $response.body: {REASON}"
        )
    );
    assert_eq!(server.take_requests(), ["GET /photo"]);
    assert!(controller
        .stop_events()
        .unwrap_or_else(|error| panic!("stop events: {error}"))
        .is_empty());
}
