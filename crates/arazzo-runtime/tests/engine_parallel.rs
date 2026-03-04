mod common;

use arazzo_runtime::{
    EngineBuilder, ExecutionEventKind, ExecutionObserver, TraceDecisionPath, TraceHook,
};
use arazzo_spec::{ActionType, OnAction, ParamLocation, Step, StepTarget, Workflow};
use common::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ── Parallel execution tests ────────────────────────────────────────

#[test]
fn execute_parallel_independent_steps() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "parallel-ind".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    target: Some(StepTarget::OperationPath("/c".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "parallel-speed".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let mut sequential = new_test_engine(&server.base_url, spec.clone());
    let seq_started = Instant::now();
    let seq_result = sequential.execute("parallel-speed", BTreeMap::new());
    if let Err(err) = seq_result {
        panic!("expected sequential success, got: {err}");
    }
    let seq_elapsed = seq_started.elapsed();

    let mut parallel = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "parallel-dep".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("id".to_string(), "$response.body.id".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "id".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.a.outputs.id".to_string()),
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
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "cf-fallback".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: ActionType::End,
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![
                    Step {
                        step_id: "call-child".to_string(),
                        target: Some(StepTarget::WorkflowId("child".to_string())),
                        ..Step::default()
                    },
                    Step {
                        step_id: "after".to_string(),
                        target: Some(StepTarget::OperationPath("/after".to_string())),
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
                    target: Some(StepTarget::OperationPath("/child".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "parallel-fail".to_string(),
            steps: vec![
                Step {
                    step_id: "ok".to_string(),
                    target: Some(StepTarget::OperationPath("/ok".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "fail".to_string(),
                    target: Some(StepTarget::OperationPath("/fail".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "diamond".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("x".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.a.outputs.x".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("y".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "c".to_string(),
                    target: Some(StepTarget::OperationPath("/c".to_string())),
                    parameters: vec![arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.a.outputs.x".to_string()),
                        ..arazzo_spec::Parameter::default()
                    }],
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("z".to_string(), "$response.body.val".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "d".to_string(),
                    target: Some(StepTarget::OperationPath("/d".to_string())),
                    parameters: vec![
                        arazzo_spec::Parameter {
                            name: "y".to_string(),
                            in_: Some(ParamLocation::Query),
                            value: serde_yml::Value::String("$steps.b.outputs.y".to_string()),
                            ..arazzo_spec::Parameter::default()
                        },
                        arazzo_spec::Parameter {
                            name: "z".to_string(),
                            in_: Some(ParamLocation::Query),
                            value: serde_yml::Value::String("$steps.c.outputs.z".to_string()),
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
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

// ── Trace hook tests ────────────────────────────────────────────────

#[test]
fn trace_hook_invoked_and_captures_fields() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let hook = Arc::new(TestTraceHook::default());
    let mut engine = match EngineBuilder::new(spec)
        .trace_hook(Arc::clone(&hook) as Arc<dyn TraceHook>)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let events = engine.execution_events();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
    assert_eq!(events[3].seq, 4);
    assert_eq!(events[0].kind, ExecutionEventKind::BeforeStep);
    assert_eq!(events[1].kind, ExecutionEventKind::AfterStep);
    assert_eq!(events[2].kind, ExecutionEventKind::BeforeStep);
    assert_eq!(events[3].kind, ExecutionEventKind::AfterStep);
    assert_eq!(events[0].step_id, "s1".to_string());
    assert_eq!(events[3].step_id, "s2".to_string());
}

#[test]
fn trace_hook_workflow_id_subworkflow_and_error_capture() {
    let server = start_server(|_method, url, _headers, _body| match url.as_str() {
        "/api" => MockHttpResponse::json(200, r#"{"ok":true}"#),
        "/fail" => MockHttpResponse::json(500, r#"{"error":"fail"}"#),
        _ => MockHttpResponse::json(200, "{}"),
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    target: Some(StepTarget::WorkflowId("child".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![
                    Step {
                        step_id: "s1".to_string(),
                        target: Some(StepTarget::OperationPath("/api".to_string())),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                    Step {
                        step_id: "s2".to_string(),
                        target: Some(StepTarget::OperationPath("/fail".to_string())),
                        success_criteria: success_200(),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
        ],
    );

    let hook = Arc::new(TestTraceHook::default());
    let mut engine = match EngineBuilder::new(spec)
        .trace_hook(Arc::clone(&hook) as Arc<dyn TraceHook>)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
    if engine.execute("parent", BTreeMap::new()).is_err() {
        // Intentional: test validates emitted hook events even on execution failure.
    }

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
fn trace_hook_parallel_events_are_deterministic() {
    let server = start_server_concurrent(|_method, url, _headers, _body| {
        match url.as_str() {
            "/slow" => thread::sleep(Duration::from_millis(60)),
            "/fast" => thread::sleep(Duration::from_millis(5)),
            "/mid" => thread::sleep(Duration::from_millis(20)),
            _ => {}
        }
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/slow".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/fast".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    target: Some(StepTarget::OperationPath("/mid".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let hook = Arc::new(TestTraceHook::default());
    let mut engine = match EngineBuilder::new(spec)
        .parallel(true)
        .trace_hook(Arc::clone(&hook) as Arc<dyn TraceHook>)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
    if let Err(err) = engine.execute("wf", BTreeMap::new()) {
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
    assert_eq!(
        before
            .iter()
            .map(|ev| ev.step_id.clone())
            .collect::<Vec<_>>(),
        vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
    );
    assert_eq!(
        after
            .iter()
            .map(|ev| ev.step_id.clone())
            .collect::<Vec<_>>(),
        vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
    );
}

#[test]
fn trace_records_sequential_include_request_response_criteria_decision() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true,"value":7}"#)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "value".to_string(),
                    "$response.body.value".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("value".to_string(), "$steps.s1.outputs.value".to_string())]),
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "retry-step".to_string(),
                target: Some(StepTarget::OperationPath("/retry".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: ActionType::Retry,
                    retry_limit: Some(2),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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
            target: Some(StepTarget::OperationPath("http://[::1".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let mut engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    target: Some(StepTarget::WorkflowId("child".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "child-step".to_string(),
                    target: Some(StepTarget::OperationPath("/child".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );

    let mut engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/slow".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/fast".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    target: Some(StepTarget::OperationPath("/mid".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let mut engine = match EngineBuilder::new(spec).parallel(true).trace(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
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

// ── Observer tests ──────────────────────────────────────────────────

#[test]
fn observer_receives_full_event_sequence() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let observer = Arc::new(TestObserver::default());
    let mut engine =
        build_observer_engine(spec, Arc::clone(&observer) as Arc<dyn ExecutionObserver>);

    let result = engine.execute("wf", BTreeMap::new());
    assert!(result.is_ok(), "execution failed: {result:?}");

    let events = observer.events();
    assert!(events.contains(&"StepStarted:s1".to_string()));
    assert!(events.contains(&"RequestPrepared:s1:GET".to_string()));
    assert!(events.contains(&"RequestSent:s1:GET".to_string()));
    assert!(events.contains(&"CriterionEvaluated:s1:0:true".to_string()));
    assert!(events.contains(&"StepCompleted:s1:true".to_string()));
    assert!(events.contains(&"WorkflowCompleted:wf:ok".to_string()));

    // Verify ordering: StepStarted before RequestPrepared before StepCompleted
    let start_pos = find_event_pos(&events, "StepStarted:s1");
    let prep_pos = find_event_pos(&events, "RequestPrepared:s1:GET");
    let sent_pos = find_event_pos(&events, "RequestSent:s1:GET");
    let complete_pos = find_event_pos(&events, "StepCompleted:s1:true");
    let wf_pos = find_event_pos(&events, "WorkflowCompleted:wf:ok");
    assert!(start_pos < prep_pos);
    assert!(prep_pos < sent_pos);
    assert!(sent_pos < complete_pos);
    assert!(complete_pos < wf_pos);
}

#[test]
fn observer_and_trace_hook_coexist() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let observer = Arc::new(TestObserver::default());
    let trace_hook = Arc::new(TestTraceHook::default());
    let mut engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .trace_hook(Arc::clone(&trace_hook) as Arc<dyn TraceHook>)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute("wf", BTreeMap::new());
    assert!(result.is_ok());

    // Observer got events
    assert!(!observer.events().is_empty());
    // TraceHook also got events
    let before_count = match trace_hook.before_events.lock() {
        Ok(guard) => guard.len(),
        Err(_) => panic!("before_events lock poisoned"),
    };
    let after_count = match trace_hook.after_events.lock() {
        Ok(guard) => guard.len(),
        Err(_) => panic!("after_events lock poisoned"),
    };
    assert!(before_count > 0);
    assert!(after_count > 0);
}

#[test]
fn observer_receives_dry_run_request_prepared() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/test".to_string())),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let observer = Arc::new(TestObserver::default());
    let mut engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .dry_run(true)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute("wf", BTreeMap::new());
    assert!(result.is_ok());

    let events = observer.events();
    // In dry-run mode: RequestPrepared fires but NOT RequestSent
    assert!(events.contains(&"RequestPrepared:s1:GET".to_string()));
    assert!(!events.iter().any(|e| e.starts_with("RequestSent:")));
}

#[test]
fn observer_no_events_without_observer() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/test".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    // Build WITHOUT observer — verify no panics and existing behavior preserved
    let mut engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute("wf", BTreeMap::new());
    assert!(result.is_ok());
}

#[test]
fn observer_parallel_receives_events_for_all_steps() {
    let server = start_server_concurrent(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "a".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
                Step {
                    step_id: "b".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );
    let observer = Arc::new(TestObserver::default());
    let mut engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .parallel(true)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute("wf", BTreeMap::new());
    assert!(result.is_ok(), "execution failed: {result:?}");

    let events = observer.events();
    // Both steps received StepStarted and StepCompleted
    assert!(events.contains(&"StepStarted:a".to_string()));
    assert!(events.contains(&"StepStarted:b".to_string()));
    assert!(events.contains(&"StepCompleted:a:true".to_string()));
    assert!(events.contains(&"StepCompleted:b:true".to_string()));
    assert!(events.contains(&"WorkflowCompleted:wf:ok".to_string()));
}
