#![forbid(unsafe_code)]

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{
    ContentType, RuntimeErrorKind, TraceDecision, TraceDecisionPath, TraceRequest, TraceResponse,
    TraceStepRecord,
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
            content_type: ContentType::Json,
            headers: BTreeMap::new(),
            body_bytes: 14,
            body_preview: Some(r#"{"value":"ok"}"#.to_string()),
            body: Some(r#"{"value":"ok"}"#.to_string()),
            body_lossy: false,
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

// ── Bug #7: replay uses full body, not truncated body_preview ────

#[tokio::test]
async fn replay_uses_full_body_not_truncated_preview() {
    // Build a JSON body larger than TRACE_BODY_PREVIEW_MAX_BYTES (2048)
    let large_value = "x".repeat(3000);
    let full_body = format!(r#"{{"value":"{large_value}"}}"#);
    let preview = format!("{}...", &full_body[..2048]);

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
            content_type: ContentType::Json,
            headers: BTreeMap::new(),
            body_bytes: full_body.len() as u64,
            body_preview: Some(preview),
            body: Some(full_body.clone()),
            body_lossy: false,
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
