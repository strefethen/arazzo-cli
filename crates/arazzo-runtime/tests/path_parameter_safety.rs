#![forbid(unsafe_code)]

//! Runtime safety policy for parameter-induced URL navigation. Arazzo permits
//! dotted values; refusal applies when transport would change the request target.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_runtime::{
    ContentType, EngineBuilder, ExecutionResult, RuntimeErrorKind, TraceDecision,
    TraceDecisionPath, TraceRequest, TraceResponse, TraceStepRecord,
};
use arazzo_spec::{
    ArazzoSpec, Info, ParamLocation, Parameter, SourceDescription, SourceType, Step, StepTarget,
    Workflow,
};
use common::{
    logged_requests, new_request_log, record_request, start_server, success_200, MockHttpResponse,
    TestObserver,
};
use serde_json::json;

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const PATH: &str = "/v1/pets/{id}/details";

fn parameter(name: &str, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Path),
        value: serde_yaml_ng::Value::String(value.to_string()).into(),
        ..Parameter::default()
    }
}

fn target_step(value: &str) -> Step {
    Step {
        step_id: "target".to_string(),
        target: Some(StepTarget::OperationId("target".to_string())),
        parameters: vec![parameter("id", value)],
        success_criteria: success_200(),
        ..Step::default()
    }
}

/// Bind supplied OpenAPI bytes by identity. The only network traffic possible
/// in live tests is to the loopback server declared in `servers`.
fn builder(base: &str, path: &str, steps: Vec<Step>) -> EngineBuilder {
    let spec = ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "path parameter safety".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "api".to_string(),
            url: SOURCE.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps,
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    };
    let openapi = json!({
        "openapi": "3.2.0",
        "$self": SOURCE,
        "info": {"title": "fixture", "version": "1.0.0"},
        "servers": [{"url": base}],
        "paths": {
            "/seed": {"get": {"operationId": "seed", "responses": {"200": {"description": "OK"}}}},
            path: {"get": {
                "operationId": "target",
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "OK"}}
            }}
        }
    });
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .trace(true)
}

fn assert_refused(result: &ExecutionResult, observer: &TestObserver) {
    let error = match &result.outputs {
        Err(error) => error,
        Ok(outputs) => panic!("unsafe expansion must fail the workflow: {outputs:?}"),
    };
    assert_eq!(error.kind, RuntimeErrorKind::InvalidParameterValue);
    assert_eq!(error.kind.code(), "RUNTIME_INVALID_PARAMETER_VALUE");
    assert!(error.message.contains("target"), "{error}");
    assert!(error.message.contains("id"), "{error}");
    assert!(result.dry_run_requests().is_empty());
    let events = observer.events();
    for forbidden in [
        "RequestPrepared:target:",
        "RequestSent:target:",
        "CriterionEvaluated:target:",
    ] {
        assert!(
            !events.iter().any(|event| event.starts_with(forbidden)),
            "unsafe step advanced: {events:?}"
        );
    }
    assert!(result
        .trace_steps()
        .iter()
        .filter(|step| step.step_id == "target")
        .all(|step| step.request.is_none() && step.response.is_none() && step.criteria.is_empty()));
}

#[tokio::test]
async fn dot_values_send_no_http_request_for_operation_id_or_operation_path() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });

    for value in [".", ".."] {
        for target in [
            StepTarget::OperationId("target".to_string()),
            StepTarget::OperationPath(PATH.to_string()),
            StepTarget::OperationPath(format!("{{api}}.{PATH}")),
            StepTarget::OperationPath(format!("{}{PATH}", server.base_url)),
        ] {
            let mut step = target_step(value);
            step.target = Some(target);
            let observer = Arc::new(TestObserver::default());
            let engine = builder(&server.base_url, PATH, vec![step])
                .observer(observer.clone())
                .build()
                .unwrap_or_else(|error| panic!("build: {error}"));
            let result = engine.execute_collect("wf", BTreeMap::new()).await;
            assert_refused(&result, &observer);
            assert!(logged_requests(&log).is_empty());
        }
    }
}

#[tokio::test]
async fn response_output_cannot_retarget_the_next_request() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        // Even the unintended endpoint would return success, exposing a
        // regression as both an extra request and an incorrectly passing run.
        MockHttpResponse::json(200, r#"{"id":".."}"#)
    });
    let seed = Step {
        step_id: "seed".to_string(),
        target: Some(StepTarget::OperationId("seed".to_string())),
        success_criteria: success_200(),
        outputs: BTreeMap::from([("id".to_string(), "$response.body#/id".to_string().into())]),
        ..Step::default()
    };
    let observer = Arc::new(TestObserver::default());
    let engine = builder(
        &server.base_url,
        PATH,
        vec![seed, target_step("$steps.seed.outputs.id")],
    )
    .observer(observer.clone())
    .build()
    .unwrap_or_else(|error| panic!("build: {error}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;

    assert_refused(&result, &observer);
    let requests = logged_requests(&log);
    assert_eq!(requests.len(), 1, "only the seed request may be sent");
    assert_eq!(requests[0].url, "/seed");
    let seed_trace = result
        .trace_steps()
        .into_iter()
        .find(|step| step.step_id == "seed")
        .unwrap_or_else(|| panic!("seed trace missing"));
    assert_eq!(seed_trace.outputs.get("id"), Some(&json!("..")));
}

#[tokio::test]
async fn terminal_whitespace_in_an_operation_path_cannot_hide_navigation() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    let observer = Arc::new(TestObserver::default());
    let engine = builder(&server.base_url, "/v1/pets/{id} ", vec![target_step("..")])
        .observer(observer.clone())
        .build()
        .unwrap_or_else(|error| panic!("build: {error}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert_refused(&result, &observer);
    assert!(logged_requests(&log).is_empty());
}

#[tokio::test]
async fn safe_values_keep_their_encoded_wire_path() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });

    for (path, value, expected) in [
        (PATH, "1.2.3", "/v1/pets/1.2.3/details"),
        (PATH, "file.json", "/v1/pets/file.json/details"),
        (PATH, "a..b", "/v1/pets/a..b/details"),
        (PATH, "...", "/v1/pets/.../details"),
        (PATH, "....", "/v1/pets/..../details"),
        (PATH, "%2e%2e", "/v1/pets/%252e%252e/details"),
        (PATH, "../..", "/v1/pets/..%2F../details"),
        (PATH, "a/../b", "/v1/pets/a%2F..%2Fb/details"),
        (PATH, "..\\..\\admin", "/v1/pets/..%5C..%5Cadmin/details"),
        (PATH, "café /?#", "/v1/pets/caf%C3%A9%20%2F%3F%23/details"),
        (
            "/v1/pets/{id}.json/details",
            "..",
            "/v1/pets/...json/details",
        ),
    ] {
        let before = logged_requests(&log).len();
        let engine = builder(&server.base_url, path, vec![target_step(value)])
            .build()
            .unwrap_or_else(|error| panic!("build: {error}"));
        let result = engine.execute_collect("wf", BTreeMap::new()).await;
        assert!(result.outputs.is_ok(), "{value:?}: {:?}", result.outputs);
        let requests = logged_requests(&log);
        assert_eq!(requests.len(), before + 1);
        assert_eq!(requests[before].url, expected, "value {value:?}");
    }
}

#[tokio::test]
async fn dry_run_refuses_before_planning_across_supported_operation_paths() {
    let base = "https://api.invalid";
    for target in [
        StepTarget::OperationId("target".to_string()),
        StepTarget::OperationPath(PATH.to_string()),
        StepTarget::OperationPath(format!("{{api}}.{PATH}")),
        StepTarget::OperationPath(format!("{base}{PATH}")),
        StepTarget::OperationPath(format!("{base}/v1/pets/{{id}}?secret=private-query")),
        StepTarget::OperationPath(format!("{base}/v1/pets/{{id}}#private-fragment")),
    ] {
        let mut step = target_step("..");
        step.target = Some(target);
        let observer = Arc::new(TestObserver::default());
        let engine = builder(base, PATH, vec![step])
            .dry_run(true)
            .observer(observer.clone())
            .build()
            .unwrap_or_else(|error| panic!("build: {error}"));
        let result = engine.execute_collect("wf", BTreeMap::new()).await;
        assert_refused(&result, &observer);
        let error = match &result.outputs {
            Err(error) => error,
            Ok(_) => unreachable!("refused above"),
        };
        for secret in [base, "private-query", "private-fragment"] {
            assert!(!error.message.contains(secret), "{error}");
        }
    }
}

fn replay_record(url: &str) -> TraceStepRecord {
    TraceStepRecord {
        seq: 1,
        workflow_id: "wf".to_string(),
        step_id: "target".to_string(),
        attempt: 1,
        kind: "http".to_string(),
        operation_path: PATH.to_string(),
        workflow_id_ref: String::new(),
        duration_ms: 0,
        request: Some(TraceRequest {
            method: "GET".to_string(),
            url: url.to_string(),
            headers: BTreeMap::new(),
            body: None,
            redirects: Vec::new(),
        }),
        response: Some(TraceResponse {
            status_code: 200,
            content_type: ContentType::Json,
            headers: BTreeMap::new(),
            body_bytes: 2,
            body_preview: Some("{}".to_string()),
            body: Some("{}".to_string()),
            body_lossy: false,
        }),
        criteria: Vec::new(),
        warnings: Vec::new(),
        decision: TraceDecision::with_path(TraceDecisionPath::Next),
        outputs: BTreeMap::new(),
        error: None,
    }
}

#[tokio::test]
async fn replay_refusal_does_not_consume_the_next_record() {
    let base = "https://replay.invalid";
    let observer = Arc::new(TestObserver::default());
    let engine = builder(base, PATH, vec![target_step("$inputs.id")])
        .observer(observer.clone())
        .replay_trace_steps(vec![replay_record(&format!("{base}/v1/pets/good/details"))])
        .build()
        .unwrap_or_else(|error| panic!("build: {error}"));

    let refused = engine
        .execute_collect("wf", BTreeMap::from([("id".to_string(), json!(".."))]))
        .await;
    assert_refused(&refused, &observer);

    let accepted = engine
        .execute_collect("wf", BTreeMap::from([("id".to_string(), json!("good"))]))
        .await;
    assert!(
        accepted.outputs.is_ok(),
        "refusal consumed the replay record: {:?}",
        accepted.outputs
    );
}
