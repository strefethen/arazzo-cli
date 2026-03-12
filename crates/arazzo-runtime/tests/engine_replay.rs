#![forbid(unsafe_code)]

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{
    ContentType, EngineBuilder, RuntimeErrorKind, TraceDecision, TraceDecisionPath, TraceRequest,
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
                target: Some(StepTarget::OperationPath("/items".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([(
                    "value".to_string(),
                    "$response.body.value".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("final".to_string(), "$steps.s1.outputs.value".to_string())]),
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
    }]
}

#[tokio::test]
async fn replay_executes_without_live_network() {
    let engine = match EngineBuilder::new(replay_spec())
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
    let engine = match EngineBuilder::new(replay_spec())
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
    }];

    let engine = match EngineBuilder::new(replay_spec())
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
