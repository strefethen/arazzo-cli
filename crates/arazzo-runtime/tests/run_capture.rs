//! Public run-capture contract across scheduler and failure paths.

#![allow(clippy::expect_used)] // Test setup and assertions use explicit failure labels.

mod common;

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    EngineBuilder, ExecutionHandle, ExecutionResult, RunStepOutcome, RuntimeErrorKind,
    TraceDecisionPath,
};
use arazzo_spec::{ActionType, OnAction, RequestBody, Step, StepTarget, Workflow};
use common::{
    make_spec_with_base, start_server, start_server_concurrent, success_200, to_yaml,
    MockHttpResponse,
};
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(5);

async fn collect(handle: ExecutionHandle) -> ExecutionResult {
    tokio::time::timeout(WAIT, handle.collect())
        .await
        .expect("execution completed within bound")
}

fn http_step(id: &str, path: &str) -> Step {
    Step {
        step_id: id.to_string(),
        target: Some(StepTarget::OperationPath(path.to_string())),
        success_criteria: success_200(),
        ..Step::default()
    }
}

fn workflow(id: &str, steps: Vec<Step>) -> Workflow {
    Workflow {
        workflow_id: id.to_string(),
        steps,
        ..Workflow::default()
    }
}

#[tokio::test]
async fn captures_typed_declared_outputs_and_only_body_free_metadata() {
    let secret = "undeclared-body-sentinel";
    let body = format!(
        "{{\"items\":[1,true,{{\"key\":\"{}\"}}],\"padding\":\"{}\"}}",
        secret,
        "x".repeat(4096)
    );
    let body_for_server = body.clone();
    let server = start_server(move |method, _url, _headers, request_body| {
        assert_eq!(method, "POST");
        assert!(request_body.contains("request-body-sentinel"));
        MockHttpResponse::json(200, &body_for_server)
    });
    let mut step = http_step("read", "POST /read");
    step.request_body = Some(RequestBody {
        payload: Some(to_yaml(json!({"payload": "request-body-sentinel"})).into()),
        ..RequestBody::default()
    });
    step.outputs
        .insert("full".to_string(), "$response.body".into());
    step.outputs
        .insert("nested".to_string(), "$response.body#/items".into());
    let spec = make_spec_with_base(&server.base_url, vec![workflow("wf", vec![step.clone()])]);
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let result = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(result.outputs.is_ok());
    assert!(
        result.trace_steps().is_empty(),
        "capture must not enable trace"
    );
    let records = result.run_steps();
    assert_eq!(records.len(), 1);
    let record = records[0];
    assert_eq!((record.seq, record.attempt), (1, 1));
    assert_eq!(record.outcome, RunStepOutcome::Success);
    assert_eq!(
        record.outputs["full"],
        serde_json::from_str::<Value>(&body).expect("json")
    );
    assert_eq!(record.outputs["nested"], json!([1, true, {"key": secret}]));
    assert_eq!(
        record.response.as_ref().expect("response").body_bytes,
        body.len() as u64
    );
    assert_eq!(record.response.as_ref().expect("response").status_code, 200);
    assert_eq!(record.request.as_ref().expect("request").method, "POST");
    let serialized = serde_json::to_value(record).expect("serialize capture");
    assert!(serialized["request"].get("body").is_none());
    assert!(serialized["response"].get("body").is_none());
    assert!(serialized["response"].get("bodyPreview").is_none());
    assert!(!serialized.to_string().contains("request-body-sentinel"));

    // Without an output declaration, the same response body never appears in capture.
    let mut undeclared = http_step("read", "POST /read");
    undeclared.request_body = step.request_body;
    let spec = make_spec_with_base(&server.base_url, vec![workflow("wf", vec![undeclared])]);
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let result = collect(engine.execute("wf", BTreeMap::new())).await;
    let serialized = serde_json::to_string(result.run_steps()[0]).expect("serialize capture");
    assert!(!serialized.contains(secret));
    assert!(!serialized.contains("padding"));
}

#[tokio::test]
async fn capture_defaults_off_and_coexists_with_unchanged_trace_attempts() {
    let server = start_server(|_, _, _, _| MockHttpResponse::empty(200));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![http_step("one", "/one")])],
    );
    let off = EngineBuilder::new(spec.clone()).build().expect("engine");
    let off_result = collect(off.execute("wf", BTreeMap::new())).await;
    assert!(off_result.run_steps().is_empty());
    let trace = EngineBuilder::new(spec.clone())
        .trace(true)
        .build()
        .expect("engine");
    let trace_result = collect(trace.execute("wf", BTreeMap::new())).await;
    let both = EngineBuilder::new(spec)
        .trace(true)
        .capture_run(true)
        .build()
        .expect("engine");
    let both_result = collect(both.execute("wf", BTreeMap::new())).await;
    assert_eq!(trace_result.trace_steps().len(), 1);
    assert_eq!(both_result.trace_steps().len(), 1);
    assert_eq!(
        trace_result.trace_steps()[0].attempt,
        both_result.trace_steps()[0].attempt
    );
    assert_eq!(
        trace_result.trace_steps()[0].seq,
        both_result.trace_steps()[0].seq
    );
    assert_eq!(
        both_result.run_steps()[0].attempt,
        both_result.trace_steps()[0].attempt
    );
    assert_eq!(
        off_result.outputs.expect("off outputs"),
        both_result.outputs.expect("both outputs")
    );
}

#[tokio::test]
async fn retries_and_revisits_keep_each_attempt_in_order() {
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = Arc::clone(&hits);
    let server = start_server(move |_, url, _, _| {
        if url == "/a" {
            let n = server_hits.fetch_add(1, Ordering::SeqCst);
            MockHttpResponse::json(if n == 0 { 503 } else { 200 }, &format!("{{\"n\":{n}}}"))
        } else {
            MockHttpResponse::empty(200)
        }
    });
    let mut a = http_step("a", "/a");
    a.outputs
        .insert("n".to_string(), "$response.body#/n".into());
    a.on_failure.push(OnAction {
        name: "retry-a".to_string(),
        type_: Some(ActionType::Retry),
        retry_limit: Some(1),
        ..OnAction::default()
    });
    let engine = EngineBuilder::new(make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![a])],
    ))
    .capture_run(true)
    .build()
    .expect("engine");
    let result = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(result.outputs.is_ok());
    let records = result.run_steps();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .map(|r| (r.seq, r.attempt))
            .collect::<Vec<_>>(),
        [(1, 1), (2, 2)]
    );
    assert_eq!(
        records.iter().map(|r| r.outcome).collect::<Vec<_>>(),
        [RunStepOutcome::Failure, RunStepOutcome::Success]
    );
    assert_eq!(records[0].decision.path, TraceDecisionPath::Retry);
    assert!(records[0].outputs.is_empty());
    assert_eq!(records[1].outputs["n"], json!(1));

    // Revisit via goto after a later step fails, using the same numbering key.
    let server = start_server({
        let a_hits = AtomicUsize::new(0);
        move |_, url, _, _| match url.as_str() {
            "/a" => MockHttpResponse::empty(if a_hits.fetch_add(1, Ordering::SeqCst) == 0 {
                200
            } else {
                500
            }),
            _ => MockHttpResponse::empty(500),
        }
    });
    let mut b = http_step("b", "/b");
    b.on_failure.push(OnAction {
        name: "again".to_string(),
        type_: Some(ActionType::Goto),
        step_id: "a".to_string(),
        ..OnAction::default()
    });
    let engine = EngineBuilder::new(make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![http_step("a", "/a"), b])],
    ))
    .capture_run(true)
    .build()
    .expect("engine");
    let result = collect(engine.execute("wf", BTreeMap::new())).await;
    let records = result.run_steps();
    assert_eq!(
        records
            .iter()
            .map(|r| (r.step_id.as_str(), r.attempt))
            .collect::<Vec<_>>(),
        [("a", 1), ("b", 1), ("a", 2)]
    );
    assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
}

#[tokio::test]
async fn nested_workflows_and_execute_step_preserve_workflow_identity() {
    let server = start_server(|_, _, _, _| MockHttpResponse::empty(200));
    let child = workflow("child", vec![http_step("same", "/child")]);
    let parent = workflow(
        "parent",
        vec![
            Step {
                step_id: "call".to_string(),
                target: Some(StepTarget::WorkflowId("child".to_string())),
                ..Step::default()
            },
            http_step("same", "/parent"),
        ],
    );
    let spec = make_spec_with_base(&server.base_url, vec![parent, child]);
    let engine = EngineBuilder::new(spec.clone())
        .capture_run(true)
        .build()
        .expect("engine");
    let result = collect(engine.execute("parent", BTreeMap::new())).await;
    assert!(result.outputs.is_ok());
    let records = result.run_steps();
    assert_eq!(
        records
            .iter()
            .map(|r| (r.workflow_id.as_str(), r.step_id.as_str()))
            .collect::<Vec<_>>(),
        [("child", "same"), ("parent", "call"), ("parent", "same")]
    );
    assert_eq!(
        records.iter().map(|r| r.attempt).collect::<Vec<_>>(),
        [1, 1, 1]
    );
    assert_eq!(records.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(records[1].request.is_none());
    assert!(records[1].response.is_none());

    let engine = EngineBuilder::new(spec.clone())
        .capture_run(true)
        .build()
        .expect("engine");
    let single = collect(engine.execute_step("parent", "call", BTreeMap::new(), false)).await;
    assert_eq!(
        single
            .run_steps()
            .iter()
            .map(|r| r.workflow_id.as_str())
            .collect::<Vec<_>>(),
        ["child", "parent"]
    );

    let dry = EngineBuilder::new(spec)
        .dry_run(true)
        .capture_run(true)
        .build()
        .expect("engine");
    let simulated = collect(dry.execute("parent", BTreeMap::new())).await;
    assert!(simulated
        .run_steps()
        .iter()
        .all(|r| r.outcome == RunStepOutcome::DryRun && r.response.is_none()));
}

#[tokio::test]
async fn parallel_capture_replays_records_in_step_order() {
    let server = start_server_concurrent(|_, url, _, _| {
        if url == "/slow" {
            thread::sleep(Duration::from_millis(70));
        }
        MockHttpResponse::empty(200)
    });
    let engine = EngineBuilder::new(make_spec_with_base(
        &server.base_url,
        vec![workflow(
            "wf",
            vec![http_step("slow", "/slow"), http_step("fast", "/fast")],
        )],
    ))
    .parallel(true)
    .capture_run(true)
    .build()
    .expect("engine");
    let result = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(result.outputs.is_ok());
    assert!(result.sequential_fallbacks().is_empty());
    assert_eq!(
        result
            .run_steps()
            .iter()
            .map(|r| (r.seq, r.step_id.as_str()))
            .collect::<Vec<_>>(),
        [(1, "slow"), (2, "fast")]
    );
}

#[tokio::test]
async fn criterion_transport_and_dry_run_outcomes_are_truthful() {
    let server = start_server(|_, _, _, _| {
        MockHttpResponse::json(503, r#"{"message":"failed-body-sentinel"}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![http_step("fail", "/fail")])],
    );
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let failed = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(failed.outputs.is_err());
    let record = failed.run_steps()[0];
    assert_eq!(record.outcome, RunStepOutcome::Failure);
    assert_eq!(
        record.response.as_ref().expect("real response").status_code,
        503
    );
    assert!(record.outputs.is_empty());
    assert!(record
        .error
        .as_deref()
        .is_some_and(|error| error.contains("status=503")));
    assert!(!serde_json::to_string(record)
        .expect("serialize failed record")
        .contains("failed-body-sentinel"));

    let child = workflow("child", vec![http_step("child-step", "/fail")]);
    let parent = workflow(
        "parent",
        vec![Step {
            step_id: "call".to_string(),
            target: Some(StepTarget::WorkflowId("child".to_string())),
            ..Step::default()
        }],
    );
    let spec = make_spec_with_base(&server.base_url, vec![parent, child]);
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let nested_failure = collect(engine.execute("parent", BTreeMap::new())).await;
    assert!(nested_failure.outputs.is_err());
    assert_eq!(nested_failure.run_steps().len(), 2);
    for record in nested_failure.run_steps() {
        assert!(!serde_json::to_string(record)
            .expect("serialize nested failure")
            .contains("failed-body-sentinel"));
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused port");
    let unused = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);
    let spec = make_spec_with_base(
        &unused,
        vec![workflow("wf", vec![http_step("fail", "/fail")])],
    );
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let transport = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(transport.outputs.is_err());
    let record = transport.run_steps()[0];
    assert_eq!(record.outcome, RunStepOutcome::Failure);
    assert!(record.response.is_none());
    assert!(record.error.is_some());

    let spec = make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![http_step("simulated", "/simulated")])],
    );
    let engine = EngineBuilder::new(spec)
        .dry_run(true)
        .capture_run(true)
        .build()
        .expect("engine");
    let dry = collect(engine.execute("wf", BTreeMap::new())).await;
    let record = dry.run_steps()[0];
    assert_eq!(record.outcome, RunStepOutcome::DryRun);
    assert!(record.request.is_some());
    assert!(record.response.is_none());

    let missing_child = workflow(
        "wf",
        vec![Step {
            step_id: "missing-child".to_string(),
            target: Some(StepTarget::WorkflowId("absent".to_string())),
            ..Step::default()
        }],
    );
    let spec = make_spec_with_base(&server.base_url, vec![missing_child]);
    let engine = EngineBuilder::new(spec)
        .dry_run(true)
        .capture_run(true)
        .build()
        .expect("engine");
    let failed_dry = collect(engine.execute("wf", BTreeMap::new())).await;
    assert!(failed_dry.outputs.is_err());
    assert_eq!(failed_dry.run_steps()[0].outcome, RunStepOutcome::Failure);
    assert!(failed_dry.run_steps()[0].response.is_none());
}

#[tokio::test]
async fn timeout_and_cancellation_retain_settled_failures() {
    let server = start_server_concurrent(|_, _, _, _| {
        thread::sleep(Duration::from_millis(300));
        MockHttpResponse::empty(200)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![workflow("wf", vec![http_step("wait", "/wait")])],
    );
    let engine = EngineBuilder::new(spec)
        .capture_run(true)
        .build()
        .expect("engine");
    let timed =
        collect(engine.execute_with_timeout("wf", BTreeMap::new(), Duration::from_millis(100)))
            .await;
    assert_eq!(
        timed.outputs.as_ref().err().map(|e| e.kind),
        Some(RuntimeErrorKind::ExecutionTimeout)
    );
    assert_eq!(timed.run_steps().len(), 1);
    assert_eq!(timed.run_steps()[0].outcome, RunStepOutcome::Failure);
    assert!(timed.run_steps()[0].response.is_none());

    let handle = engine.execute("wf", BTreeMap::new());
    let token = handle.cancel_token().clone();
    tokio::time::sleep(Duration::from_millis(100)).await;
    token.cancel();
    let cancelled = collect(handle).await;
    assert_eq!(
        cancelled.outputs.as_ref().err().map(|e| e.kind),
        Some(RuntimeErrorKind::ExecutionCancelled)
    );
    assert_eq!(cancelled.run_steps().len(), 1);
    assert_eq!(cancelled.run_steps()[0].outcome, RunStepOutcome::Failure);
    assert!(cancelled.run_steps()[0].response.is_none());
}
