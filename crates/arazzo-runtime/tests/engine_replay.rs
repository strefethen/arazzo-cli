#![forbid(unsafe_code)]

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{
    BytesDescriptor, RuntimeErrorKind, TraceDecision, TraceDecisionPath, TraceRequest,
    TraceResponse, TraceStepRecord,
};
use arazzo_spec::{Step, StepTarget, SuccessCriterion, Workflow};
use common::make_spec_with_base;
use serde_json::json;

fn replay_spec() -> arazzo_spec::ArazzoSpec {
    make_spec_with_base(
        "https://replay.invalid",
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath(
                    "{$sourceDescriptions.test.url}#/paths/~1items/get".to_string(),
                )),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([(
                    "value".to_string(),
                    "$response.body#/value".to_string().into(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "final".to_string(),
                "$steps.s1.outputs.value".to_string().into(),
            )]),
            ..Workflow::default()
        }],
    )
}

fn replay_trace(url: &str) -> Vec<TraceStepRecord> {
    vec![TraceStepRecord {
        seq: 1,
        workflow_id: "wf".to_string(),
        step_id: "s1".to_string(),
        attempt: 1,
        kind: "http".to_string(),
        operation_path: "/items".to_string(),
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
            media_type: Some("application/json".to_string()),
            headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            body_bytes: 14,
            body: Some(r#"{"value":"ok"}"#.to_string()),
            ..TraceResponse::default()
        }),
        criteria: Vec::new(),
        warnings: Vec::new(),
        decision: TraceDecision::with_path(TraceDecisionPath::Next),
        outputs: BTreeMap::new(),
        error: None,
        message: None,
    }]
}

#[tokio::test]
async fn replay_executes_without_live_network() {
    let engine = match common::fixture_engine_builder(replay_spec())
        .trace(true)
        .replay_trace_steps(replay_trace("https://replay.invalid/items"))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };

    let outputs = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected replay success, got: {err}"),
    };

    assert_eq!(outputs.get("final"), Some(&json!("ok")));
}

#[tokio::test]
async fn replay_classifies_the_recorded_body_like_a_live_response() {
    let mut untyped = replay_trace("https://replay.invalid/items");
    let mut undecodable = replay_trace("https://replay.invalid/items");
    if let Some(response) = untyped[0].response.as_mut() {
        response.headers.clear();
        response.media_type = None;
    }
    if let Some(response) = undecodable[0].response.as_mut() {
        response.body = None;
        response.body_blob = Some(BytesDescriptor::of(
            b"{\xFF",
            Some("application/json".into()),
            None,
        ));
        response.body_error =
            Some("response body is not valid JSON (application/json): recorded".into());
    }
    let cases = [
        (
            untyped,
            "response has no Content-Type header, so its body is bytes, which is not yet a readable value",
        ),
        (
            undecodable,
            "response body is not valid JSON (application/json): recorded",
        ),
    ];
    for (trace, reason) in cases {
        let engine = match common::fixture_engine_builder(replay_spec())
            .trace(true)
            .replay_trace_steps(trace)
            .build()
        {
            Ok(engine) => engine,
            Err(err) => panic!("building replay engine: {err}"),
        };
        let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
            Ok(outputs) => panic!("expected an unavailable body, got outputs: {outputs:?}"),
            Err(err) => err,
        };
        assert_eq!(err.kind, RuntimeErrorKind::ValueUnavailable, "{err}");
        assert!(err.message.ends_with(reason), "{err}");
    }
}

#[tokio::test]
async fn replay_reports_request_drift() {
    let engine = match common::fixture_engine_builder(replay_spec())
        .trace(true)
        .replay_trace_steps(replay_trace("https://replay.invalid/other"))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };

    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(outputs) => panic!("expected replay drift error, got outputs: {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::ReplayRequestMismatch);
}

#[tokio::test]
async fn replay_propagates_workflow_dependency_error_without_consuming_requests() {
    let mut spec = replay_spec();
    spec.workflows[0].workflow_id = "dependent".to_string();
    spec.workflows[0].depends_on = vec!["missing".to_string()];
    let engine = match common::fixture_engine_builder(spec)
        .trace(true)
        .replay_trace_steps(Vec::new())
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };

    let err = match engine
        .execute_collect("dependent", BTreeMap::new())
        .await
        .outputs
    {
        Ok(outputs) => panic!("expected dependency rejection, got outputs: {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowDependencyUnsatisfied);
    assert_eq!(err.code(), "RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED");
}

#[tokio::test]
async fn replay_uses_the_full_recorded_body() {
    let large_value = "x".repeat(3000);
    let full_body = format!(r#"{{"value":"{large_value}"}}"#);

    let trace = vec![TraceStepRecord {
        seq: 1,
        workflow_id: "wf".to_string(),
        step_id: "s1".to_string(),
        attempt: 1,
        kind: "http".to_string(),
        operation_path: "/items".to_string(),
        workflow_id_ref: String::new(),
        duration_ms: 0,
        request: Some(TraceRequest {
            method: "GET".to_string(),
            url: "https://replay.invalid/items".to_string(),
            headers: BTreeMap::new(),
            body: None,
            redirects: Vec::new(),
        }),
        response: Some(TraceResponse {
            status_code: 200,
            media_type: Some("application/json".to_string()),
            headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            body_bytes: full_body.len() as u64,
            body: Some(full_body.clone()),
            ..TraceResponse::default()
        }),
        criteria: Vec::new(),
        warnings: Vec::new(),
        decision: TraceDecision::with_path(TraceDecisionPath::Next),
        outputs: BTreeMap::new(),
        error: None,
        message: None,
    }];

    let engine = match common::fixture_engine_builder(replay_spec())
        .trace(true)
        .replay_trace_steps(trace)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };

    let outputs = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected replay success with full body, got: {err}"),
    };

    // Expression evaluation should resolve the full value, not truncated
    assert_eq!(outputs.get("final"), Some(&json!(large_value)));
}

fn body_replay_builder(base: &str, content_type: &str) -> arazzo_runtime::EngineBuilder {
    let spec = serde_json::from_value(json!({
        "arazzo":"1.1.0","info":{"title":"replay","version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://fixture.invalid/api","type":"openapi"}],
        "workflows":[{"workflowId":"wf","steps":[{"stepId":"s1","operationId":"send",
            "parameters":[{"name":"Authorization","in":"header","value":"Bearer actual-secret"},
                {"name":"Cookie","in":"header","value":"session=actual-cookie"},
                {"name":"token","in":"query","value":"actual-query"}],
            "requestBody":{"contentType":content_type,"payload":"$inputs.body"}}]}]
    }))
    .unwrap_or_else(|err| panic!("replay fixture: {err}"));
    let api = json!({"openapi":"3.2.0","$self":"https://fixture.invalid/api","info":{"title":"api","version":"1"},
        "servers":[{"url":base}],"paths":{"/send":{"post":{"operationId":"send","responses":{"200":{"description":"ok"}}}}}});
    arazzo_runtime::EngineBuilder::new(spec)
        .openapi_spec(api.to_string().into_bytes(), None)
        .trace(true)
}

#[tokio::test]
async fn replay_logical_json_and_form_preserve_drift() {
    use arazzo_runtime::{redact_headers, redact_json_value, redact_url};
    use common::{
        logged_requests, new_request_log, record_request, start_server, MockHttpResponse,
    };
    use std::sync::Arc;
    for (ct, wire) in [
        ("application/json", r#"{"n":42,"nested":[{"password":"body-secret","safe":"visible"}],"tag":["a",null,"b"]}"#),
        ("application/x-www-form-urlencoded", "n=42&nested=%7B%22password%22%3A%22body-secret%22%2C%22safe%22%3A%22visible%22%7D&tag=a&tag=b"),
    ] {
        let logical=json!({"n":42,"nested":[{"password":"body-secret","safe":"visible"}],"tag":["a",null,"b"]});
        let log=new_request_log();
        let capture=Arc::clone(&log);
        let server=start_server(move |m,u,h,b| {record_request(&capture,&m,&u,&h,&b); MockHttpResponse::json(200,"{}")});
        let base=server.base_url.clone();
        let result=body_replay_builder(&base,ct).build().unwrap_or_else(|err| panic!("replay fixture: {err}")).execute_collect("wf",BTreeMap::from([("body".into(),logical.clone())])).await;
        assert!(result.outputs.is_ok(), "{:?}",result.outputs);
        assert_eq!(logged_requests(&log)[0].body, wire);
        let mut steps=result.trace_steps().into_iter().cloned().collect::<Vec<_>>();
        let request=steps[0].request.as_mut().unwrap_or_else(|| panic!("missing request"));
        assert_eq!(request.body,Some(logical.clone()));
        redact_headers(&mut request.headers);
        redact_url(&mut request.url);
        redact_json_value(request.body.as_mut().unwrap_or_else(|| panic!("missing request")));
        drop(server);
        for (body, succeeds) in [
            (logical.clone(),true),
            (json!({"n":42,"nested":[{"password":"changed","safe":"visible"}],"tag":["a",null,"b"]}),true),
            (json!({"n":"42","nested":[{"password":"changed","safe":"visible"}],"tag":["a",null,"b"]}),false),
            (json!({"n":42,"nested":[{"password":"changed","safe":"visible"}],"tag":["a","b"]}),false),
            (json!({"n":42,"nested":[{"safe":"visible"}],"tag":["a",null,"b"]}),false),
            (json!({"n":42,"nested":[{"password":"changed","safe":"[REDACTED]"}],"tag":["a",null,"b"]}),false),
        ] {
            let r=body_replay_builder(&base,ct).replay_trace_steps(steps.clone()).build().unwrap_or_else(|err| panic!("replay fixture: {err}")).execute_collect("wf",BTreeMap::from([("body".into(),body)])).await;
            assert_eq!(r.outputs.is_ok(),succeeds,"{ct}: {:?}",r.outputs);
            if let Err(e)=r.outputs { assert_eq!(e.kind,RuntimeErrorKind::ReplayRequestMismatch); }
        }
    }
}

#[tokio::test]
async fn replay_empty_form_wire_keeps_logical_shape() {
    use common::{
        logged_requests, new_request_log, record_request, start_server, MockHttpResponse,
    };
    use std::sync::Arc;
    let forms = [
        json!({}),
        json!({"ordinary":null}),
        json!({"ordinary":[]}),
        json!(""),
    ];
    for logical in &forms {
        let log = new_request_log();
        let capture = Arc::clone(&log);
        let server = start_server(move |m, u, h, b| {
            record_request(&capture, &m, &u, &h, &b);
            MockHttpResponse::json(200, "{}")
        });
        let base = server.base_url.clone();
        let ct = "application/x-www-form-urlencoded";
        let result = body_replay_builder(&base, ct)
            .build()
            .unwrap_or_else(|err| panic!("replay fixture: {err}"))
            .execute_collect("wf", BTreeMap::from([("body".into(), logical.clone())]))
            .await;
        assert!(result.outputs.is_ok());
        assert_eq!(logged_requests(&log)[0].body, "");
        let mut trace = result
            .trace_steps()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        arazzo_runtime::redact_trace_request(
            trace[0]
                .request
                .as_mut()
                .unwrap_or_else(|| panic!("missing request")),
        );
        drop(server);
        for other in &forms {
            let r = body_replay_builder(&base, ct)
                .replay_trace_steps(trace.clone())
                .build()
                .unwrap_or_else(|err| panic!("replay fixture: {err}"))
                .execute_collect("wf", BTreeMap::from([("body".into(), other.clone())]))
                .await;
            assert_eq!(
                r.outputs.is_ok(),
                logical == other,
                "{logical} versus {other}"
            );
        }
    }
}

const PLANTED_PNG: &[u8] = b"\x89PNG\r\n\x1a\nPLANTED-PNG-CONTENT";

struct RawServer {
    base_url: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RawServer {
    fn start() -> Self {
        use std::sync::atomic::Ordering;
        let server = tiny_http::Server::http("127.0.0.1:0")
            .unwrap_or_else(|err| panic!("binding raw server: {err}"));
        let base_url = format!("http://{}", server.server_addr());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = std::sync::Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(std::time::Duration::from_millis(20))
                else {
                    continue;
                };
                let (media_type, body): (&str, &[u8]) = match request.url() {
                    "/latin" => ("text/plain; charset=iso-8859-1", b"caf\xE9 au lait"),
                    _ => ("image/png", PLANTED_PNG),
                };
                let response = tiny_http::Response::from_data(body).with_header(
                    tiny_http::Header::from_bytes("Content-Type", media_type)
                        .unwrap_or_else(|()| panic!("content type header")),
                );
                let _ = request.respond(response);
            }
        });
        Self {
            base_url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn download_builder(base: &str) -> arazzo_runtime::EngineBuilder {
    let step = |operation: &str, output: &str| {
        json!({"stepId":"s1","operationId":operation,
            "successCriteria":[{"condition":"$statusCode == 200"}],
            "outputs":{"value":output}})
    };
    let spec = serde_json::from_value(json!({
        "arazzo":"1.1.0","info":{"title":"replay","version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://fixture.invalid/api","type":"openapi"}],
        "workflows":[
            {"workflowId":"text","steps":[step("latin","$response.body")],
                "outputs":{"value":"$steps.s1.outputs.value"}},
            {"workflowId":"bytes","steps":[step("png","$statusCode")],
                "outputs":{"value":"$steps.s1.outputs.value"}}]
    }))
    .unwrap_or_else(|err| panic!("download fixture: {err}"));
    let get = |id: &str| json!({"get":{"operationId":id,"responses":{"200":{"description":"ok"}}}});
    let api = json!({"openapi":"3.2.0","$self":"https://fixture.invalid/api",
        "info":{"title":"api","version":"1"},"servers":[{"url":base}],
        "paths":{"/latin":get("latin"),"/png":get("png")}});
    arazzo_runtime::EngineBuilder::new(spec)
        .openapi_spec(api.to_string().into_bytes(), None)
        .trace(true)
}

#[tokio::test]
async fn a_v2_trace_round_trips_through_json_and_replays_the_same_outcome() {
    let server = RawServer::start();
    for (workflow, expected) in [("text", json!("caf\u{e9} au lait")), ("bytes", json!(200))] {
        let live = download_builder(&server.base_url)
            .build()
            .unwrap_or_else(|err| panic!("live engine: {err}"))
            .execute_collect(workflow, BTreeMap::new())
            .await;
        let outputs = live
            .outputs
            .as_ref()
            .unwrap_or_else(|err| panic!("{workflow} live run: {err}"))
            .clone();
        assert_eq!(outputs.get("value"), Some(&expected), "{workflow}");
        let steps: Vec<TraceStepRecord> = live.trace_steps().into_iter().cloned().collect();
        let serialized =
            serde_json::to_string(&steps).unwrap_or_else(|err| panic!("serializing: {err}"));
        assert!(!serialized.contains("PLANTED"), "{serialized}");
        assert!(!serialized.contains('\u{fffd}'), "{serialized}");
        let response = steps[0]
            .response
            .as_ref()
            .unwrap_or_else(|| panic!("{workflow} response"));
        match workflow {
            "text" => {
                assert_eq!(response.body.as_deref(), Some("caf\u{e9} au lait"));
                assert_eq!(
                    response.media_type.as_deref(),
                    Some("text/plain;charset=iso-8859-1")
                );
                assert!(response.body_blob.is_none());
            }
            _ => {
                assert!(response.body.is_none());
                assert_eq!(
                    response.body_blob,
                    Some(BytesDescriptor::of(
                        PLANTED_PNG,
                        Some("image/png".into()),
                        None
                    ))
                );
                assert!(response.body_error.is_none());
            }
        }
        let restored: Vec<TraceStepRecord> =
            serde_json::from_str(&serialized).unwrap_or_else(|err| panic!("restoring: {err}"));
        assert_eq!(restored, steps);
        let replayed = download_builder(&server.base_url)
            .replay_trace_steps(restored)
            .build()
            .unwrap_or_else(|err| panic!("replay engine: {err}"))
            .execute_collect(workflow, BTreeMap::new())
            .await
            .outputs
            .unwrap_or_else(|err| panic!("{workflow} replay: {err}"));
        assert_eq!(replayed, outputs, "{workflow}");
    }
}
