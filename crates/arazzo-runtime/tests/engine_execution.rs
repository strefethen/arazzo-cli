mod common;

use arazzo_runtime::{
    EngineBuilder, ExecutionObserver, ObserverEvent, RuntimeError, RuntimeErrorKind,
    TraceDecisionPath,
};
use arazzo_spec::{
    ActionType, ArazzoSpec, CriterionExpressionType, CriterionType, OnAction, OutputValue,
    ParamLocation, Parameter, Replacement, RequestBody, SelectorObject, SelectorType,
    SourceDescription, SourceType, Step, StepAction, StepTarget, SuccessCriterion, ValueSource,
    Workflow,
};
use common::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

#[derive(Default)]
struct RetryCancellationObserver {
    failed_step_completed: Notify,
    retry_scheduled: AtomicUsize,
}

impl ExecutionObserver for RetryCancellationObserver {
    fn on_event(&self, event: &ObserverEvent) {
        match event {
            ObserverEvent::StepCompleted { step_id, .. } if step_id == "retry-step" => {
                self.failed_step_completed.notify_one();
            }
            ObserverEvent::RetryScheduled { .. } => {
                self.retry_scheduled.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct RetryDelayObserver {
    delays: Mutex<Vec<f64>>,
}

impl RetryDelayObserver {
    fn delays(&self) -> Vec<f64> {
        match self.delays.lock() {
            Ok(delays) => delays.clone(),
            Err(_) => panic!("reading retry-delay observer events"),
        }
    }
}

impl ExecutionObserver for RetryDelayObserver {
    fn on_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::RetryScheduled { delay_seconds, .. } = event {
            match self.delays.lock() {
                Ok(mut delays) => delays.push(*delay_seconds),
                Err(_) => panic!("recording retry-delay observer event"),
            }
        }
    }
}

// ── Basic execution tests ─────────────────────────────────────────

#[tokio::test]
async fn workflow_dependencies_fail_closed_and_accept_explicit_completion() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::SeqCst);
        MockHttpResponse::empty(200)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "prepare".to_string(),
                steps: vec![Step {
                    step_id: "prepare-step".to_string(),
                    target: Some(StepTarget::OperationPath("/prepare".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "dependent".to_string(),
                depends_on: vec!["prepare".to_string()],
                steps: vec![Step {
                    step_id: "dependent-step".to_string(),
                    target: Some(StepTarget::OperationPath("/dependent".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let err = match engine
        .execute_collect("dependent", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("missing workflow completion must fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowDependencyUnsatisfied);
    assert_eq!(err.code(), "RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED");
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let completed = BTreeSet::from(["prepare".to_string(), "unrelated".to_string()]);
    let result = engine
        .execute_with_completed_workflows("dependent", BTreeMap::new(), &completed)
        .collect()
        .await;
    assert!(result.outputs.is_ok());
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let step_err = match engine
        .execute_step("dependent", "dependent-step", BTreeMap::new(), true)
        .collect()
        .await
        .outputs
    {
        Ok(_) => panic!("step entry must use the same workflow guard"),
        Err(err) => err,
    };
    assert_eq!(
        step_err.kind,
        RuntimeErrorKind::WorkflowDependencyUnsatisfied
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let external_spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "external-dependent".to_string(),
            depends_on: vec!["$sourceDescriptions.shared.other".to_string()],
            steps: vec![Step {
                step_id: "external-step".to_string(),
                target: Some(StepTarget::OperationPath("/external".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let external_engine = new_test_engine(&server.base_url, external_spec);
    let external_err = match external_engine
        .execute_collect("external-dependent", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("external completion cannot be established"),
        Err(err) => err,
    };
    assert_eq!(
        external_err.kind,
        RuntimeErrorKind::WorkflowDependencyUnsatisfied
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nested_workflow_completion_satisfies_later_dependency() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::SeqCst);
        MockHttpResponse::empty(200)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "first-child".to_string(),
                steps: vec![Step {
                    step_id: "first-step".to_string(),
                    target: Some(StepTarget::OperationPath("/first".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "second-child".to_string(),
                depends_on: vec!["first-child".to_string()],
                steps: vec![Step {
                    step_id: "second-step".to_string(),
                    target: Some(StepTarget::OperationPath("/second".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![
                    Step {
                        step_id: "first-call".to_string(),
                        target: Some(StepTarget::WorkflowId("first-child".to_string())),
                        ..Step::default()
                    },
                    Step {
                        step_id: "second-call".to_string(),
                        target: Some(StepTarget::WorkflowId("second-child".to_string())),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            },
        ],
    );
    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs;
    assert!(
        result.is_ok(),
        "nested dependency should be satisfied: {result:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_nested_workflow_counts_as_completion_for_goto_dependency() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::SeqCst);
        if url == "/first" {
            MockHttpResponse::empty(500)
        } else {
            MockHttpResponse::empty(200)
        }
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "first-child".to_string(),
                steps: vec![Step {
                    step_id: "first-step".to_string(),
                    target: Some(StepTarget::OperationPath("/first".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "second-child".to_string(),
                depends_on: vec!["first-child".to_string()],
                steps: vec![Step {
                    step_id: "second-step".to_string(),
                    target: Some(StepTarget::OperationPath("/second".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "first-call".to_string(),
                    target: Some(StepTarget::WorkflowId("first-child".to_string())),
                    on_failure: vec![OnAction {
                        type_: Some(ActionType::Goto),
                        workflow_id: "second-child".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );
    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("parent", BTreeMap::new()).await;
    assert!(
        result.outputs.is_ok(),
        "failure-handling goto should succeed"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn nested_dependency_rejection_preserves_stable_error_and_makes_no_request() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::SeqCst);
        MockHttpResponse::empty(200)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![
            Workflow {
                workflow_id: "dependent".to_string(),
                depends_on: vec!["missing".to_string()],
                steps: vec![Step {
                    step_id: "dependent-step".to_string(),
                    target: Some(StepTarget::OperationPath("/dependent".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "parent".to_string(),
                steps: vec![Step {
                    step_id: "call".to_string(),
                    target: Some(StepTarget::WorkflowId("dependent".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );
    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("nested dependency rejection must fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowDependencyUnsatisfied);
    assert_eq!(err.code(), "RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unrelated_completion_evidence_does_not_satisfy_unknown_dependency() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::SeqCst);
        MockHttpResponse::empty(200)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "dependent".to_string(),
            depends_on: vec!["missing".to_string()],
            steps: vec![Step {
                step_id: "step".to_string(),
                target: Some(StepTarget::OperationPath("/dependent".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);
    let completion = BTreeSet::from(["unrelated".to_string()]);
    let err = match engine
        .execute_with_completed_workflows("dependent", BTreeMap::new(), &completion)
        .collect()
        .await
        .outputs
    {
        Ok(_) => panic!("unrelated completion evidence must not satisfy dependency"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowDependencyUnsatisfied);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn channel_execution_and_dry_run_fail_before_http_with_the_same_error() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_method, _url, _headers, _body| {
        hits_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::json(200, r#"{"unexpected":true}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "async-channel".to_string(),
            steps: vec![Step {
                step_id: "receive-event".to_string(),
                target: Some(StepTarget::ChannelPath(
                    "{$sourceDescriptions.api.url}#/channels/events".to_string(),
                )),
                action: Some(StepAction::Receive),
                timeout: Some(5000),
                correlation_id: Some("event-1".to_string()),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let mut observed = Vec::new();
    for dry_run in [false, true] {
        let engine = match EngineBuilder::new(spec.clone()).dry_run(dry_run).build() {
            Ok(engine) => engine,
            Err(err) => panic!("building channel engine: {err}"),
        };
        let err = match engine
            .execute_collect("async-channel", BTreeMap::new())
            .await
            .outputs
        {
            Ok(_) => panic!("channel execution should fail closed (dry_run={dry_run})"),
            Err(err) => err,
        };
        assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
        assert_eq!(err.code(), "RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT");
        observed.push((err.kind, err.message));
    }

    assert_eq!(observed[0], observed[1]);
    assert_eq!(hits.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn execute_sequential_steps() {
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
                target: Some(StepTarget::OperationPath("/step1".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/step2".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("sequential", BTreeMap::new())
        .await
        .outputs;
    match result {
        Ok(outputs) => assert!(outputs.is_empty()),
        Err(err) => panic!("expected success, got: {err}"),
    }
}

/// Workflow Parameters join an operation Step by the exact `(name, in)` key:
/// the Step's `shared` query value overrides the workflow value, while the
/// other workflow parameter remains. A workflow-targeting Step inherits none.
#[tokio::test]
async fn workflow_parameter_merge_matches_validation_effective_list() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let paths_ref = Arc::clone(&paths);
    let server = start_server(move |_method, url, _headers, _body| {
        match paths_ref.lock() {
            Ok(mut observed) => observed.push(url),
            Err(_) => panic!("recording request path"),
        }
        MockHttpResponse::empty(200)
    });
    let query_parameter = |name: &str, value: &str| Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Query),
        value: serde_yaml_ng::Value::String(value.to_string()).into(),
        ..Parameter::default()
    };
    let spec = make_spec(vec![
        Workflow {
            workflow_id: "parent".to_string(),
            parameters: vec![
                query_parameter("inherited", "global"),
                query_parameter("shared", "workflow"),
            ],
            steps: vec![
                Step {
                    step_id: "operation".to_string(),
                    target: Some(StepTarget::OperationPath("/request".to_string())),
                    parameters: vec![query_parameter("shared", "step")],
                    ..Step::default()
                },
                Step {
                    step_id: "child".to_string(),
                    target: Some(StepTarget::WorkflowId("child".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "child-operation".to_string(),
                target: Some(StepTarget::OperationPath("/child".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let result = new_test_engine(&server.base_url, spec)
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = result {
        panic!("expected workflow to execute: {err}");
    }

    let observed = match paths.lock() {
        Ok(paths) => paths.clone(),
        Err(_) => panic!("reading request paths"),
    };
    assert_eq!(
        observed,
        vec![
            "/request?inherited=global&shared=step".to_string(),
            "/child".to_string(),
        ]
    );
}

#[tokio::test]
async fn execute_on_success_goto_interpolation_bug() {
    let server = start_server(|_method, url, _headers, _body| match url.as_str() {
        "/s1" => MockHttpResponse::json(200, r#"{"ok":true}"#),
        "/final" => MockHttpResponse::json(200, r#"{"done":true}"#),
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "test_workflow".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/s1".to_string())),
                    success_criteria: success_200(),
                    on_success: vec![OnAction {
                        type_: Some(ActionType::Goto),
                        step_id: "target_{$inputs.target}".to_string(), // Dynamic target
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "target_final".to_string(),
                    target: Some(StepTarget::OperationPath("/final".to_string())),
                    success_criteria: success_200(),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );

    let engine = new_test_engine(&server.base_url, spec);
    let mut inputs = BTreeMap::new();
    inputs.insert("target".to_string(), json!("final"));

    let result = engine.execute_collect("test_workflow", inputs).await;
    assert!(result.outputs.is_ok(), "goto interpolation failed");
}

#[tokio::test]
async fn execute_failure_no_handler() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(500, r#"{"error":"server error"}"#)
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "fail-no-handler".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("fail-no-handler", BTreeMap::new())
        .await
        .outputs;
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

#[tokio::test]
async fn execute_on_failure_end() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

    let spec = make_spec(vec![Workflow {
        workflow_id: "fail-end".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/should-not-reach".to_string())),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("fail-end", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected error from onFailure end action"),
        Err(err) => err,
    };
    assert_eq!(err.message, "step s1: workflow ended by onFailure action");
}

#[tokio::test]
async fn execute_on_success_end() {
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
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                success_criteria: success_200(),
                on_success: vec![OnAction {
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/should-not-reach".to_string())),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("success-end", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }

    let observed = match paths.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading captured paths"),
    };
    assert_eq!(observed, vec!["/ok".to_string()]);
}

#[tokio::test]
async fn execute_on_failure_goto() {
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
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Goto),
                    step_id: "fallback".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "skipped".to_string(),
                target: Some(StepTarget::OperationPath("/should-not-reach".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "fallback".to_string(),
                target: Some(StepTarget::OperationPath("/fallback".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("fail-goto", BTreeMap::new())
        .await
        .outputs;
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

#[tokio::test]
async fn execute_on_success_goto() {
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
                target: Some(StepTarget::OperationPath("/start".to_string())),
                success_criteria: success_200(),
                on_success: vec![OnAction {
                    type_: Some(ActionType::Goto),
                    step_id: "s3".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/skipped".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "s3".to_string(),
                target: Some(StepTarget::OperationPath("/target".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("success-goto", BTreeMap::new())
        .await
        .outputs;
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

#[tokio::test]
async fn execute_on_failure_retry() {
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
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(2),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("retry", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = result {
        panic!("expected success after retries, got: {err}");
    }
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn execute_retry_exceeds_max() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

    let spec = make_spec(vec![Workflow {
        workflow_id: "retry-max".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("retry-max", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected max-retries error"),
        Err(err) => err,
    };
    assert_eq!(err.message, "step s1: max retries (1) exceeded");
}

#[tokio::test]
async fn execute_retry_custom_limit() {
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
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(6),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("retry-limit", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }
    assert_eq!(calls.load(Ordering::Relaxed), 6);
}

#[tokio::test]
async fn execute_retry_custom_limit_exceeded() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

    let spec = make_spec(vec![Workflow {
        workflow_id: "retry-limit-exceeded".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(2),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("retry-limit-exceeded", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected retry limit exceeded error"),
        Err(err) => err,
    };
    assert_eq!(err.message, "step s1: max retries (2) exceeded");
}

#[tokio::test]
async fn execute_retry_limit_zero_means_no_retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "zero-retry".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_limit: Some(0),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("zero-retry", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected retry limit exceeded error"),
        Err(err) => err,
    };
    assert_eq!(err.message, "step s1: max retries (0) exceeded");
    // Only 1 call — the initial request, no retries
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn arazzo_11_retry_limit_counts_omitted_zero_one_and_two() {
    for (case_name, retry_limit, expected_retries) in [
        ("omitted", None, 1usize),
        ("zero", Some(0), 0),
        ("one", Some(1), 1),
        ("two", Some(2), 2),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let server = start_server(move |_method, _url, _headers, _body| {
            calls_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(500)
        });
        let mut spec = make_spec(vec![Workflow {
            workflow_id: format!("retry-limit-{case_name}"),
            steps: vec![Step {
                step_id: "retry-step".to_string(),
                target: Some(StepTarget::OperationPath("/always-fail".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    name: format!("retry-{case_name}"),
                    type_: Some(ActionType::Retry),
                    retry_limit,
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }]);
        spec.arazzo = "1.1.0".to_string();

        let engine = new_test_engine(&server.base_url, spec);
        let err = match engine
            .execute_collect(&format!("retry-limit-{case_name}"), BTreeMap::new())
            .await
            .outputs
        {
            Ok(_) => panic!("{case_name}: expected retry-limit error"),
            Err(err) => err,
        };
        assert_eq!(err.kind, RuntimeErrorKind::RetryLimitExceeded);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            expected_retries + 1,
            "{case_name} must count only scheduled retries"
        );
    }
}

#[tokio::test]
async fn arazzo_11_workflow_failure_retry_fallback_uses_the_same_omitted_limit() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "workflow-retry-fallback".to_string(),
        failure_actions: vec![OnAction {
            name: "workflow-omitted-retry".to_string(),
            type_: Some(ActionType::Retry),
            ..OnAction::default()
        }],
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();

    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine
        .execute_collect("workflow-retry-fallback", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("workflow retry fallback must exhaust"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RetryLimitExceeded);
    assert_eq!(err.message, "step retry-step: max retries (1) exceeded");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn arazzo_11_exhausted_retry_falls_through_to_matching_end_with_final_trace_route() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-then-end".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "exhaust-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    name: "end-second".to_string(),
                    type_: Some(ActionType::End),
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
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building traced engine: {err}"),
    };
    let result = engine
        .execute_collect("retry-then-end", BTreeMap::new())
        .await;
    let err = match &result.outputs {
        Ok(_) => panic!("matching end must fail the original step"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(result.trace_steps().len(), 1);
    assert_eq!(
        result.trace_steps()[0].decision.path,
        TraceDecisionPath::Done
    );
}

#[tokio::test]
async fn arazzo_11_exhausted_retry_skips_nonmatching_actions_then_goto() {
    let paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let paths_ref = Arc::clone(&paths);
    let server = start_server(move |_method, url, _headers, _body| {
        match paths_ref.lock() {
            Ok(mut paths) => paths.push(url.clone()),
            Err(_) => panic!("recording request path"),
        }
        match url.as_str() {
            "/fail" => MockHttpResponse::empty(500),
            "/fallback" => MockHttpResponse::empty(200),
            _ => MockHttpResponse::empty(404),
        }
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-then-goto".to_string(),
        steps: vec![
            Step {
                step_id: "retry-step".to_string(),
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: success_200(),
                on_failure: vec![
                    OnAction {
                        name: "exhaust-first".to_string(),
                        type_: Some(ActionType::Retry),
                        retry_limit: Some(0),
                        ..OnAction::default()
                    },
                    OnAction {
                        name: "nonmatching-end".to_string(),
                        type_: Some(ActionType::End),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 503".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        name: "goto-third".to_string(),
                        type_: Some(ActionType::Goto),
                        step_id: "fallback".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 500".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                ],
                ..Step::default()
            },
            Step {
                step_id: "fallback".to_string(),
                target: Some(StepTarget::OperationPath("/fallback".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("retry-then-goto", BTreeMap::new())
        .await
        .outputs;
    assert!(
        result.is_ok(),
        "later matching goto must handle failure: {result:?}"
    );
    assert_eq!(
        paths
            .lock()
            .unwrap_or_else(|_| panic!("reading request paths"))
            .clone(),
        vec!["/fail".to_string(), "/fallback".to_string()]
    );
}

#[tokio::test]
async fn arazzo_11_exhausted_retry_nonmatching_later_actions_returns_stable_limit() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-no-later-handler".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "retry-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(2),
                    ..OnAction::default()
                },
                OnAction {
                    name: "nonmatching-end".to_string(),
                    type_: Some(ActionType::End),
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 503".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                },
                OnAction {
                    name: "nonmatching-goto".to_string(),
                    type_: Some(ActionType::Goto),
                    step_id: "missing".to_string(),
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 503".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                },
                OnAction {
                    name: "nonmatching-retry".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(9),
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 503".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();

    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine
        .execute_collect("retry-no-later-handler", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("nonmatching later actions must not handle failure"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RetryLimitExceeded);
    assert_eq!(err.message, "step retry-step: max retries (2) exceeded");
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn arazzo_11_retry_sites_are_independent_in_serial_and_execute_step_observers() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "independent-retry-sites".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/always-fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "first-retry".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(1),
                    ..OnAction::default()
                },
                OnAction {
                    name: "second-retry".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(2),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let serial_observer = Arc::new(TestObserver::default());
    let serial_engine = match EngineBuilder::new(spec.clone())
        .trace(true)
        .observer(Arc::clone(&serial_observer) as Arc<dyn arazzo_runtime::ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building serial observer engine: {err}"),
    };
    let serial_result = serial_engine
        .execute_collect("independent-retry-sites", BTreeMap::new())
        .await;
    let serial_err = match &serial_result.outputs {
        Ok(_) => panic!("all retry sites must eventually exhaust"),
        Err(err) => err,
    };
    assert_eq!(serial_err.kind, RuntimeErrorKind::RetryLimitExceeded);
    assert_eq!(
        serial_err.message,
        "step retry-step: max retries (2) exceeded"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    assert_eq!(
        serial_observer
            .events()
            .into_iter()
            .filter(|event| event.starts_with("RetryScheduled:"))
            .collect::<Vec<_>>(),
        vec![
            "RetryScheduled:retry-step:1/1".to_string(),
            "RetryScheduled:retry-step:1/2".to_string(),
            "RetryScheduled:retry-step:2/2".to_string(),
        ]
    );
    assert_eq!(serial_result.trace_steps().len(), 4);
    assert_eq!(
        serial_result
            .trace_steps()
            .iter()
            .map(|record| record.decision.path.clone())
            .collect::<Vec<_>>(),
        vec![
            TraceDecisionPath::Retry,
            TraceDecisionPath::Retry,
            TraceDecisionPath::Retry,
            TraceDecisionPath::Error,
        ]
    );
    assert_eq!(serial_result.trace_steps()[0].decision.retry_limit, Some(1));
    assert_eq!(serial_result.trace_steps()[1].decision.retry_limit, Some(2));

    let step_observer = Arc::new(TestObserver::default());
    let step_engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&step_observer) as Arc<dyn arazzo_runtime::ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building execute_step observer engine: {err}"),
    };
    let step_err = match step_engine
        .execute_step(
            "independent-retry-sites",
            "retry-step",
            BTreeMap::new(),
            false,
        )
        .collect()
        .await
        .outputs
    {
        Ok(_) => panic!("execute_step must exhaust the same retry sites"),
        Err(err) => err,
    };
    assert_eq!(step_err.kind, RuntimeErrorKind::RetryLimitExceeded);
    assert_eq!(calls.load(Ordering::Relaxed), 8);
    assert_eq!(
        step_observer
            .events()
            .into_iter()
            .filter(|event| event.starts_with("RetryScheduled:"))
            .collect::<Vec<_>>(),
        vec![
            "RetryScheduled:retry-step:1/1".to_string(),
            "RetryScheduled:retry-step:1/2".to_string(),
            "RetryScheduled:retry-step:2/2".to_string(),
        ]
    );
}

#[tokio::test]
async fn arazzo_11_omitted_retry_limit_stays_absent_in_trace_and_uses_one_observer_attempt() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        let call = calls_ref.fetch_add(1, Ordering::Relaxed);
        if call == 0 {
            MockHttpResponse::empty(500)
        } else {
            MockHttpResponse::empty(200)
        }
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "omitted-trace-limit".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                name: "omitted-retry".to_string(),
                type_: Some(ActionType::Retry),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(TestObserver::default());
    let engine = match EngineBuilder::new(spec)
        .trace(true)
        .observer(Arc::clone(&observer) as Arc<dyn arazzo_runtime::ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building observer engine: {err}"),
    };
    let result = engine
        .execute_collect("omitted-trace-limit", BTreeMap::new())
        .await;
    assert!(result.outputs.is_ok(), "omitted retry must retry once");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        result.trace_steps()[0].decision.path,
        TraceDecisionPath::Retry
    );
    assert_eq!(result.trace_steps()[0].decision.retry_limit, None);
    assert!(observer
        .events()
        .contains(&"RetryScheduled:retry-step:1/1".to_string()));
}

/// Configured retryAfter remains in the trace while RetryScheduled reports the
/// effective HTTP Retry-After override. Header zero must schedule immediately.
#[tokio::test]
async fn retry_after_trace_and_observer_keep_configured_and_effective_delays_distinct() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        let attempt = calls_ref.fetch_add(1, Ordering::Relaxed);
        if attempt == 0 {
            return MockHttpResponse {
                status: 503,
                headers: BTreeMap::from([("Retry-After".to_string(), "0".to_string())]),
                body: String::new(),
            };
        }
        MockHttpResponse::empty(200)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-after-observer".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_after: 0.25,
                retry_limit: Some(1),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(RetryDelayObserver::default());
    let engine = match EngineBuilder::new(spec)
        .trace(true)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building retry-after observer engine: {err}"),
    };
    let result = engine
        .execute_collect("retry-after-observer", BTreeMap::new())
        .await;
    assert!(
        result.outputs.is_ok(),
        "Retry-After: 0 must retry without waiting"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        result.trace_steps()[0].decision.retry_after_seconds,
        Some(0.25),
        "trace retains the configured Arazzo decimal"
    );
    assert_eq!(
        observer.delays(),
        vec![0.0],
        "observer reports the effective header-overridden delay"
    );
}

/// A finite but unrepresentable direct-model delay is unused after exhaustion,
/// so ac-f2160 fallthrough stays visible and no retry is scheduled.
#[tokio::test]
async fn exhausted_retry_does_not_convert_or_schedule_an_unused_finite_overflow_delay() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(503)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "exhausted-overflow".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    type_: Some(ActionType::Retry),
                    retry_after: f64::MAX,
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(RetryDelayObserver::default());
    let engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building exhausted-overflow engine: {err}"),
    };
    let err = match engine
        .execute_collect("exhausted-overflow", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("later end action must fail the workflow"),
        Err(err) => err,
    };
    assert_ne!(err.kind, RuntimeErrorKind::InputValidation);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(observer.delays().is_empty());
}

/// Invalid typed retryAfter values fail before the exhausted-retry fallthrough;
/// only finite overflow is unused after exhaustion.
#[tokio::test]
async fn invalid_retry_after_is_rejected_before_exhausted_retry_fallthrough() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(503)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "invalid-exhausted-retry".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    type_: Some(ActionType::Retry),
                    retry_after: f64::NAN,
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(RetryDelayObserver::default());
    let engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building invalid exhausted-retry engine: {err}"),
    };
    let err = match engine
        .execute_collect("invalid-exhausted-retry", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("invalid retryAfter must fail before the later end action"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(
        observer.delays().is_empty(),
        "invalid retryAfter must not schedule a retry"
    );
}

#[tokio::test]
async fn arazzo_11_later_action_errors_do_not_become_retry_exhaustion() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-then-invalid-goto".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "exhaust-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    name: "invalid-goto-second".to_string(),
                    type_: Some(ActionType::Goto),
                    step_id: "missing".to_string(),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();

    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine
        .execute_collect("retry-then-invalid-goto", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("invalid later goto must fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::GotoTargetNotFound);
}

#[tokio::test]
async fn arazzo_11_later_retry_reference_failure_does_not_resume_action_scanning() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-reference-failure".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "exhaust-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    name: "reference-second".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(1),
                    workflow_id: "missing-recovery".to_string(),
                    ..OnAction::default()
                },
                OnAction {
                    name: "end-never-reached".to_string(),
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(TestObserver::default());
    let engine = match EngineBuilder::new(spec)
        .observer(Arc::clone(&observer) as Arc<dyn arazzo_runtime::ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building retry-reference observer engine: {err}"),
    };
    let err = match engine
        .execute_collect("retry-reference-failure", BTreeMap::new())
        .await
        .outputs
    {
        Ok(_) => panic!("failed retry reference must fail the workflow"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RetryReferenceFailed);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(
        observer
            .events()
            .into_iter()
            .all(|event| !event.starts_with("RetryScheduled:")),
        "a retry reference that fails before scheduling must not emit RetryScheduled"
    );
}

#[tokio::test]
async fn arazzo_11_later_retry_delay_timeout_does_not_resume_action_scanning() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-delay-timeout-fallthrough".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "exhaust-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    name: "delayed-retry-second".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_after: 2.0,
                    retry_limit: Some(1),
                    ..OnAction::default()
                },
                OnAction {
                    name: "end-never-reached".to_string(),
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();

    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine
        .execute_with_timeout(
            "retry-delay-timeout-fallthrough",
            BTreeMap::new(),
            Duration::from_millis(120),
        )
        .collect()
        .await
        .outputs
    {
        Ok(_) => panic!("timeout during later retry delay must fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionTimeout);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn arazzo_11_later_retry_delay_cancellation_does_not_resume_action_scanning() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, _url, _headers, _body| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "retry-delay-cancellation-fallthrough".to_string(),
        steps: vec![Step {
            step_id: "retry-step".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    name: "exhaust-first".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(0),
                    ..OnAction::default()
                },
                OnAction {
                    name: "delayed-retry-second".to_string(),
                    type_: Some(ActionType::Retry),
                    retry_after: 1.0,
                    retry_limit: Some(1),
                    ..OnAction::default()
                },
                OnAction {
                    name: "end-never-reached".to_string(),
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                },
            ],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    spec.arazzo = "1.1.0".to_string();
    spec.source_descriptions[0].url = server.base_url.clone();

    let observer = Arc::new(RetryCancellationObserver::default());
    let engine = match EngineBuilder::new(spec)
        .trace(true)
        .observer(Arc::clone(&observer) as Arc<dyn ExecutionObserver>)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building retry cancellation engine: {err}"),
    };
    let handle = engine.execute("retry-delay-cancellation-fallthrough", BTreeMap::new());

    if tokio::time::timeout(
        Duration::from_secs(1),
        observer.failed_step_completed.notified(),
    )
    .await
    .is_err()
    {
        panic!("initial failed step did not complete before the cancellation deadline");
    }
    handle.cancel_token().cancel();

    let result = match tokio::time::timeout(Duration::from_secs(2), handle.collect()).await {
        Ok(result) => result,
        Err(_) => panic!("cancelled retry delay did not stop within the deadline"),
    };
    let err = match &result.outputs {
        Ok(_) => panic!("cancellation during the later retry delay must fail"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(result.trace_steps().len(), 1);
    assert_eq!(
        result.trace_steps()[0].decision.path,
        TraceDecisionPath::Error,
        "the later catch-all end action must not execute"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(observer.retry_scheduled.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn execute_retry_with_delay() {
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
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_after: 1.0,
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let started = Instant::now();
    let result = engine
        .execute_collect("retry-delay", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = result {
        panic!("expected success with retry delay, got: {err}");
    }
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn execute_retry_delay_honors_execution_timeout() {
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
            target: Some(StepTarget::OperationPath("/flaky".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Retry),
                retry_after: 2.0,
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let started = Instant::now();
    let result = engine
        .execute_with_timeout(
            "retry-delay-timeout",
            BTreeMap::new(),
            Duration::from_millis(120),
        )
        .collect()
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected execution timeout"),
        Err(err) => err,
    };
    assert_eq!(err.message, "execution timeout exceeded");
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionTimeout);
    assert!(started.elapsed() < Duration::from_millis(900));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

/// Failure Action Object: "If a stepId or workflowId are specified, then the
/// reference is executed and the context is returned, after which the current
/// step is retried." A retry action with `workflowId` runs the referenced
/// workflow before the retried attempt — the recovery call flips server state
/// so the retried request succeeds — and the returned context registers for
/// `$workflows.<id>.*`. The reference also lands in the trace decision.
#[tokio::test]
async fn execute_retry_workflow_reference_recovers_then_retries() {
    let recovered = Arc::new(AtomicUsize::new(0));
    let flaky_calls = Arc::new(AtomicUsize::new(0));
    let recover_calls = Arc::new(AtomicUsize::new(0));
    let recovered_ref = Arc::clone(&recovered);
    let flaky_ref = Arc::clone(&flaky_calls);
    let recover_ref = Arc::clone(&recover_calls);
    let server = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/flaky" => {
            flaky_ref.fetch_add(1, Ordering::Relaxed);
            if recovered_ref.load(Ordering::Relaxed) == 0 {
                MockHttpResponse::empty(503)
            } else {
                MockHttpResponse::json(200, r#"{"ok":true}"#)
            }
        }
        "/recover" => {
            recover_ref.fetch_add(1, Ordering::Relaxed);
            recovered_ref.store(1, Ordering::Relaxed);
            MockHttpResponse::json(200, r#"{"token":"tok123"}"#)
        }
        _ => MockHttpResponse::empty(404),
    });

    let mut spec = make_spec(vec![
        Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    workflow_id: "recovery".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "recoveredToken".to_string(),
                "$workflows.recovery.outputs.token".to_string().into(),
            )]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "recovery".to_string(),
            steps: vec![Step {
                step_id: "r1".to_string(),
                target: Some(StepTarget::OperationPath("/recover".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "token".to_string(),
                    "$response.body.token".to_string().into(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "token".to_string(),
                "$steps.r1.outputs.token".to_string().into(),
            )]),
            ..Workflow::default()
        },
    ]);
    if let Some(source) = spec.source_descriptions.get_mut(0) {
        source.url = server.base_url.clone();
    }

    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    let outputs = match &exec_result.outputs {
        Ok(outputs) => outputs.clone(),
        Err(err) => panic!("expected success after recovery, got: {err}"),
    };

    // Recovery ran exactly once, before the single retried attempt.
    assert_eq!(recover_calls.load(Ordering::Relaxed), 1);
    assert_eq!(flaky_calls.load(Ordering::Relaxed), 2);
    // "the context is returned": the reference's outputs resolve afterward.
    assert_eq!(outputs.get("recoveredToken"), Some(&json!("tok123")));

    // The reference appears in the retry trace decision.
    let trace = exec_result.trace_steps();
    let retry_record = trace
        .iter()
        .find(|record| record.decision.path == TraceDecisionPath::Retry)
        .unwrap_or_else(|| panic!("no retry decision in trace"));
    assert_eq!(retry_record.decision.target_workflow_id, "recovery");
    assert_eq!(retry_record.decision.target_step_id, "");
}

/// Retry-action `parameters` map to the referenced workflow's inputs exactly
/// as goto-workflow parameters do (Success/Failure Action Object: parameters
/// "MUST be passed to a workflow as referenced by workflowId").
#[tokio::test]
async fn execute_retry_workflow_reference_parameters_become_callee_inputs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/flaky" => {
            let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current < 2 {
                MockHttpResponse::json(500, r#"{"reason":"overload"}"#)
            } else {
                MockHttpResponse::json(200, r#"{"ok":true}"#)
            }
        }
        "/recover" => MockHttpResponse::json(200, "{}"),
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    workflow_id: "recovery".to_string(),
                    parameters: vec![
                        Parameter {
                            name: "fixed".to_string(),
                            value: serde_yaml_ng::Value::String("literal-value".to_string()).into(),
                            ..Parameter::default()
                        },
                        Parameter {
                            name: "uid".to_string(),
                            value: serde_yaml_ng::Value::String("$inputs.uid".to_string()).into(),
                            ..Parameter::default()
                        },
                        Parameter {
                            name: "reason".to_string(),
                            value: ValueSource::Selector(selector(
                                "$response.body",
                                "/reason",
                                SelectorType::Name("jsonpointer".to_string()),
                            )),
                            ..Parameter::default()
                        },
                    ],
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                (
                    "fixed".to_string(),
                    "$workflows.recovery.outputs.fixed".to_string().into(),
                ),
                (
                    "uid".to_string(),
                    "$workflows.recovery.outputs.uid".to_string().into(),
                ),
                (
                    "reason".to_string(),
                    "$workflows.recovery.outputs.reason".to_string().into(),
                ),
            ]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "recovery".to_string(),
            steps: vec![Step {
                step_id: "r1".to_string(),
                target: Some(StepTarget::OperationPath("/recover".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                ("fixed".to_string(), "$inputs.fixed".to_string().into()),
                ("uid".to_string(), "$inputs.uid".to_string().into()),
                ("reason".to_string(), "$inputs.reason".to_string().into()),
            ]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("uid".to_string(), json!(42))]);
    let outputs = match engine.execute_collect("wf", inputs).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };

    assert_eq!(outputs.get("fixed"), Some(&json!("literal-value")));
    assert_eq!(outputs.get("uid"), Some(&json!(42)));
    assert_eq!(outputs.get("reason"), Some(&json!("overload")));
}

/// A retry action with `stepId` executes the referenced step of the current
/// workflow once, call-and-return: its outputs persist for later expressions,
/// the referenced step's own routing does not run, and the current step is
/// then retried.
#[tokio::test]
async fn execute_retry_step_reference_executes_with_outputs_visible() {
    let calls = Arc::new(AtomicUsize::new(0));
    let recover_calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let recover_ref = Arc::clone(&recover_calls);
    let server = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/flaky" => {
            let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current < 2 {
                MockHttpResponse::empty(503)
            } else {
                MockHttpResponse::json(200, r#"{"ok":true}"#)
            }
        }
        "/recover" => {
            recover_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::json(200, r#"{"token":"tok123"}"#)
        }
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    step_id: "recover".to_string(),
                    ..OnAction::default()
                }],
                // End on success so the referenced step below only ever runs
                // via the retry reference, never as the next sequential step.
                on_success: vec![OnAction {
                    type_: Some(ActionType::End),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "recover".to_string(),
                target: Some(StepTarget::OperationPath("/recover".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "token".to_string(),
                    "$response.body.token".to_string().into(),
                )]),
                ..Step::default()
            },
        ],
        outputs: BTreeMap::from([(
            "token".to_string(),
            "$steps.recover.outputs.token".to_string().into(),
        )]),
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let outputs = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success after step-reference recovery, got: {err}"),
    };

    assert_eq!(recover_calls.load(Ordering::Relaxed), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    // The referenced step's outputs are visible to later expressions.
    assert_eq!(outputs.get("token"), Some(&json!("tok123")));
}

/// The recovery reference executes once per retried attempt the action is
/// selected for — two failures mean two reference executions.
#[tokio::test]
async fn execute_retry_reference_runs_once_per_retried_attempt() {
    let flaky_calls = Arc::new(AtomicUsize::new(0));
    let recover_calls = Arc::new(AtomicUsize::new(0));
    let flaky_ref = Arc::clone(&flaky_calls);
    let recover_ref = Arc::clone(&recover_calls);
    let server = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/flaky" => {
            let current = flaky_ref.fetch_add(1, Ordering::Relaxed) + 1;
            if current < 3 {
                MockHttpResponse::empty(503)
            } else {
                MockHttpResponse::json(200, r#"{"ok":true}"#)
            }
        }
        "/recover" => {
            recover_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::json(200, "{}")
        }
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(2),
                    workflow_id: "recovery".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "recovery".to_string(),
            steps: vec![Step {
                step_id: "r1".to_string(),
                target: Some(StepTarget::OperationPath("/recover".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("wf", BTreeMap::new()).await.outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }

    assert_eq!(flaky_calls.load(Ordering::Relaxed), 3);
    assert_eq!(recover_calls.load(Ordering::Relaxed), 2);
}

/// A failed recovery reference ends the workflow with the structured
/// `RUNTIME_RETRY_REFERENCE_FAILED` error carrying the underlying cause —
/// never a silent retry-anyway.
#[tokio::test]
async fn execute_retry_reference_failure_fails_workflow() {
    let flaky_calls = Arc::new(AtomicUsize::new(0));
    let flaky_ref = Arc::clone(&flaky_calls);
    let server = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/flaky" => {
            flaky_ref.fetch_add(1, Ordering::Relaxed);
            MockHttpResponse::empty(503)
        }
        "/recover" => MockHttpResponse::empty(500),
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    workflow_id: "recovery".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "recovery".to_string(),
            steps: vec![Step {
                step_id: "r1".to_string(),
                target: Some(StepTarget::OperationPath("/recover".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("expected retry reference failure"),
        Err(err) => err,
    };

    assert_eq!(err.kind, RuntimeErrorKind::RetryReferenceFailed);
    assert_eq!(err.code(), "RUNTIME_RETRY_REFERENCE_FAILED");
    assert!(
        err.message
            .contains("retry reference workflow \"recovery\""),
        "message should name the reference: {}",
        err.message
    );
    // The underlying cause survives as the error source.
    assert!(std::error::Error::source(&err).is_some());
    // The failed step ran once; the broken reference must not retry anyway.
    assert_eq!(flaky_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn execute_honors_external_cancel_flag() {
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
            target: Some(StepTarget::OperationPath("/ok".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let handle = engine.execute("cancelled", BTreeMap::new());
    handle.cancel_token().cancel();
    let exec_result = handle.collect().await;
    let result = exec_result.outputs;
    let err = match result {
        Ok(_) => panic!("expected execution cancellation"),
        Err(err) => err,
    };
    assert_eq!(err.message, "execution cancelled");
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn execute_on_failure_criteria_matching() {
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
                target: Some(StepTarget::OperationPath("/main".to_string())),
                success_criteria: success_200(),
                on_failure: vec![
                    OnAction {
                        type_: Some(ActionType::Goto),
                        step_id: "rate-handler".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 429".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: Some(ActionType::Goto),
                        step_id: "server-error-handler".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 500".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: Some(ActionType::End),
                        ..OnAction::default()
                    },
                ],
                ..Step::default()
            },
            Step {
                step_id: "rate-handler".to_string(),
                target: Some(StepTarget::OperationPath("/rate-limit-handler".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
            Step {
                step_id: "server-error-handler".to_string(),
                target: Some(StepTarget::OperationPath("/should-not-reach".to_string())),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("criteria-match", BTreeMap::new())
        .await
        .outputs;
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

#[tokio::test]
async fn execute_on_failure_criteria_none_match() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(418));

    let spec = make_spec(vec![Workflow {
        workflow_id: "no-criteria-match".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/teapot".to_string())),
            success_criteria: success_200(),
            on_failure: vec![
                OnAction {
                    type_: Some(ActionType::Retry),
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 429".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                },
                OnAction {
                    type_: Some(ActionType::Goto),
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

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("no-criteria-match", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected error when no criteria match"),
        Err(err) => err,
    };
    assert!(err
        .message
        .contains("step s1: success criteria not met (status=418"));
}

#[tokio::test]
async fn execute_goto_errors() {
    let server = start_server(|_method, _url, _headers, _body| MockHttpResponse::empty(500));

    let bad_goto_spec = make_spec(vec![Workflow {
        workflow_id: "bad-goto".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Goto),
                step_id: "nonexistent".to_string(),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let bad_goto_engine = new_test_engine(&server.base_url, bad_goto_spec);
    let bad_goto_result = bad_goto_engine
        .execute_collect("bad-goto", BTreeMap::new())
        .await
        .outputs;
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
            target: Some(StepTarget::OperationPath("/fail".to_string())),
            success_criteria: success_200(),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Goto),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let empty_goto_engine = new_test_engine(&server.base_url, empty_goto_spec);
    let empty_goto_result = empty_goto_engine
        .execute_collect("goto-no-target", BTreeMap::new())
        .await
        .outputs;
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

#[tokio::test]
async fn execute_workflow_not_found() {
    let spec = make_spec(Vec::new());
    let engine = new_test_engine("http://localhost", spec);
    let result = engine
        .execute_collect("nonexistent", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected workflow-not-found error"),
        Err(err) => err,
    };
    assert_eq!(err.message, r#"workflow "nonexistent" not found"#);
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowNotFound);
}

#[tokio::test]
async fn execute_default_sequential_without_on_success() {
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
            Step {
                step_id: "s3".to_string(),
                target: Some(StepTarget::OperationPath("/c".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("default-seq", BTreeMap::new())
        .await
        .outputs;
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

// ── Sub-workflow tests ────────────────────────────────────────────

#[tokio::test]
async fn execute_sub_workflow_step() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"token":"xyz-789"}"#)
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "parent".to_string(),
            steps: vec![Step {
                step_id: "call-child".to_string(),
                target: Some(StepTarget::WorkflowId("child".to_string())),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "token".to_string(),
                "$steps.call-child.outputs.token".to_string().into(),
            )]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "get-token".to_string(),
                target: Some(StepTarget::OperationPath("/auth".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "token".to_string(),
                    "$response.body.token".to_string().into(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "token".to_string(),
                "$steps.get-token.outputs.token".to_string().into(),
            )]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs;
    let outputs = match result {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(outputs.get("token"), Some(&json!("xyz-789")));
}

#[tokio::test]
async fn execute_sub_workflow_with_inputs() {
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
                target: Some(StepTarget::WorkflowId("child".to_string())),
                parameters: vec![Parameter {
                    name: "userId".to_string(),
                    value: serde_yaml_ng::Value::String("$inputs.uid".to_string()).into(),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "get-user".to_string(),
                target: Some(StepTarget::OperationPath("/users/{userId}".to_string())),
                parameters: vec![Parameter {
                    name: "userId".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yaml_ng::Value::String("$inputs.userId".to_string()).into(),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("uid".to_string(), json!("42"))]);
    let exec_result = engine.execute_collect("parent", inputs).await;
    let result = exec_result.outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }
    let observed = match got_path.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading captured path"),
    };
    assert_eq!(observed, "/users/42");
}

#[tokio::test]
async fn execute_sub_workflow_failure() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(500, r#"{"error":"fail"}"#)
    });

    let spec = make_spec(vec![
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
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected child workflow failure"),
        Err(err) => err,
    };
    assert!(err.message.contains("sub-workflow child"));
}

#[tokio::test]
async fn execute_goto_workflow() {
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
                target: Some(StepTarget::OperationPath("/main".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Goto),
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
                target: Some(StepTarget::OperationPath("/fallback".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "ok".to_string(),
                    "$response.body.fallback".to_string().into(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "ok".to_string(),
                "$steps.fb.outputs.ok".to_string().into(),
            )]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("main-wf", BTreeMap::new())
        .await
        .outputs;
    let outputs = match result {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(outputs.get("ok"), Some(&json!(true)));
}

/// Success/Failure Action Object (Arazzo 1.1.0): action `parameters` "MUST be
/// passed to a workflow as referenced by workflowId". A literal, a bare
/// runtime expression, an embedded `{$...}` interpolation, and a Selector
/// Object all resolve through the shared value resolver and arrive as the
/// callee workflow's `$inputs` — the bare expression with its type intact,
/// the selector against the failed step's response.
#[tokio::test]
async fn goto_workflow_action_parameters_become_callee_inputs() {
    let server = start_server(|_method, url, _headers, _body| match url.as_str() {
        "/main" => MockHttpResponse::json(500, r#"{"reason":"overload"}"#),
        "/fallback" => MockHttpResponse::json(200, "{}"),
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "main-wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/main".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Goto),
                    workflow_id: "fallback-wf".to_string(),
                    parameters: vec![
                        Parameter {
                            name: "fixed".to_string(),
                            value: serde_yaml_ng::Value::String("literal-value".to_string()).into(),
                            ..Parameter::default()
                        },
                        Parameter {
                            name: "uid".to_string(),
                            value: serde_yaml_ng::Value::String("$inputs.uid".to_string()).into(),
                            ..Parameter::default()
                        },
                        Parameter {
                            name: "greeting".to_string(),
                            value: serde_yaml_ng::Value::String("uid {$inputs.uid}".to_string())
                                .into(),
                            ..Parameter::default()
                        },
                        Parameter {
                            name: "reason".to_string(),
                            value: ValueSource::Selector(selector(
                                "$response.body",
                                "/reason",
                                SelectorType::Name("jsonpointer".to_string()),
                            )),
                            ..Parameter::default()
                        },
                    ],
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
                target: Some(StepTarget::OperationPath("/fallback".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                ("fixed".to_string(), "$inputs.fixed".to_string().into()),
                ("uid".to_string(), "$inputs.uid".to_string().into()),
                (
                    "greeting".to_string(),
                    "$inputs.greeting".to_string().into(),
                ),
                ("reason".to_string(), "$inputs.reason".to_string().into()),
            ]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("uid".to_string(), json!(42))]);
    let outputs = match engine.execute_collect("main-wf", inputs).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };

    assert_eq!(outputs.get("fixed"), Some(&json!("literal-value")));
    assert_eq!(outputs.get("uid"), Some(&json!(42)));
    assert_eq!(outputs.get("greeting"), Some(&json!("uid 42")));
    assert_eq!(outputs.get("reason"), Some(&json!("overload")));
}

/// A goto-workflow action without `parameters` keeps the pre-1.1 behavior:
/// the caller's own inputs transfer to the callee unchanged.
#[tokio::test]
async fn goto_workflow_without_parameters_forwards_caller_inputs() {
    let server = start_server(|_method, url, _headers, _body| match url.as_str() {
        "/main" => MockHttpResponse::json(500, "{}"),
        "/fallback" => MockHttpResponse::json(200, "{}"),
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "main-wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/main".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Goto),
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
                target: Some(StepTarget::OperationPath("/fallback".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("echoed".to_string(), "$inputs.uid".to_string().into())]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("uid".to_string(), json!(7))]);
    let outputs = match engine.execute_collect("main-wf", inputs).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };

    assert_eq!(outputs.get("echoed"), Some(&json!(7)));
}

#[tokio::test]
async fn execute_recursion_guard() {
    let spec = make_spec(vec![
        Workflow {
            workflow_id: "wf-a".to_string(),
            steps: vec![Step {
                step_id: "call-b".to_string(),
                target: Some(StepTarget::WorkflowId("wf-b".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "wf-b".to_string(),
            steps: vec![Step {
                step_id: "call-a".to_string(),
                target: Some(StepTarget::WorkflowId("wf-a".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine("http://localhost", spec);
    let result = engine
        .execute_collect("wf-a", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected recursion guard error"),
        Err(err) => err,
    };
    assert!(err.message.contains("max call depth"));
}

#[tokio::test]
async fn execute_sub_workflow_not_found() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "parent".to_string(),
        steps: vec![Step {
            step_id: "call-missing".to_string(),
            target: Some(StepTarget::WorkflowId("nonexistent".to_string())),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine("http://localhost", spec);
    let result = engine
        .execute_collect("parent", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected missing sub-workflow error"),
        Err(err) => err,
    };
    assert!(err.message.contains(r#"workflow "nonexistent" not found"#));
}

#[tokio::test]
async fn load_openapi_spec_and_resolve_operation_ids() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        ..Workflow::default()
    }]);
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

    let engine = match EngineBuilder::new(spec)
        .openapi_spec(openapi.to_vec(), None)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

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

#[tokio::test]
async fn load_openapi_spec_not_found_and_skips_non_http_fields() {
    let spec = make_spec(Vec::new());
    let openapi = br#"
openapi: "3.0.0"
paths:
  /items:
    parameters:
      - name: format
    get:
      operationId: listItems
"#;

    let engine = match EngineBuilder::new(spec)
        .openapi_spec(openapi.to_vec(), None)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let list = match engine.resolve_operation_id("listItems") {
        Ok(v) => v,
        Err(err) => panic!("resolving listItems: {err}"),
    };
    assert_eq!(list, ("GET".to_string(), "/items".to_string()));

    let missing = engine.resolve_operation_id("nonexistent");
    assert!(missing.is_err());
}

#[tokio::test]
async fn execute_operation_id_and_path_params() {
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

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationId("getUser".to_string())),
                parameters: vec![Parameter {
                    name: "id".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yaml_ng::Value::String("$inputs.userId".to_string()).into(),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let engine = match EngineBuilder::new(spec)
        .openapi_spec(
            br#"{"openapi":"3.0.0","paths":{"/users/{id}":{"get":{"operationId":"getUser"}}}}"#
                .to_vec(),
            None,
        )
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let inputs = BTreeMap::from([("userId".to_string(), json!("42"))]);
    let exec_result = engine.execute_collect("wf", inputs).await;
    let result = exec_result.outputs;
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

#[tokio::test]
async fn execute_operation_id_not_loaded() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationId("listUsers".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let engine = new_test_engine("http://localhost", spec);
    let result = engine.execute_collect("wf", BTreeMap::new()).await.outputs;
    let err = match result {
        Ok(_) => panic!("expected unresolved operationId error"),
        Err(err) => err,
    };
    assert!(err.message.contains("operationId"));
}

// ── Dry-run tests ─────────────────────────────────────────────────

fn request_body_with_replacements(
    payload: serde_json::Value,
    replacements: Vec<Replacement>,
) -> RequestBody {
    RequestBody {
        content_type: "application/json".to_string(),
        payload: Some(to_yaml(payload).into()),
        replacements,
        ..RequestBody::default()
    }
}

fn replacement(target: &str, value: serde_yaml_ng::Value) -> Replacement {
    Replacement {
        target: target.to_string(),
        value: value.into(),
        ..Replacement::default()
    }
}

fn selector(context: &str, selector: &str, type_: SelectorType) -> SelectorObject {
    SelectorObject {
        context: context.to_string(),
        selector: selector.to_string(),
        type_,
        extensions: BTreeMap::new(),
    }
}

#[tokio::test]
async fn structured_selectors_share_one_runtime_across_callers() {
    let captured_url = Arc::new(Mutex::new(String::new()));
    let captured_headers = Arc::new(Mutex::new(BTreeMap::new()));
    let captured_body = Arc::new(Mutex::new(String::new()));
    let url_ref = Arc::clone(&captured_url);
    let headers_ref = Arc::clone(&captured_headers);
    let body_ref = Arc::clone(&captured_body);
    let server = start_server(move |_method, url, headers, body| {
        *url_ref.lock().unwrap_or_else(|err| err.into_inner()) = url;
        *headers_ref.lock().unwrap_or_else(|err| err.into_inner()) = headers;
        *body_ref.lock().unwrap_or_else(|err| err.into_inner()) = body;
        MockHttpResponse::json(
            200,
            r#"{"items":[{"id":2,"enabled":true},{"id":1,"enabled":true}]}"#,
        )
    });

    let jsonpath = SelectorType::Name("jsonpath".to_string());
    let jsonpointer = SelectorType::Name("jsonpointer".to_string());
    let payload = to_yaml(json!({
        "selected": {
            "context": "$inputs.document",
            "selector": "$.items[*].id",
            "type": "jsonpath"
        },
        "first": null,
        "literalMap": {
            "context": "literal-context",
            "selector": "literal-selector"
        }
    }));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "selectors".to_string(),
            steps: vec![Step {
                step_id: "select".to_string(),
                target: Some(StepTarget::OperationPath("POST /select".to_string())),
                parameters: vec![
                    Parameter {
                        name: "selected".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: ValueSource::Selector(selector(
                            "$inputs.document",
                            "$.items[*].id",
                            jsonpath.clone(),
                        )),
                        ..Parameter::default()
                    },
                    Parameter {
                        name: "X-First".to_string(),
                        in_: Some(ParamLocation::Header),
                        value: ValueSource::Selector(selector(
                            "$inputs.document",
                            "/items/0/id",
                            jsonpointer.clone(),
                        )),
                        ..Parameter::default()
                    },
                ],
                request_body: Some(RequestBody {
                    content_type: "application/json".to_string(),
                    payload: Some(payload.into()),
                    replacements: vec![Replacement {
                        target: "/first".to_string(),
                        value: ValueSource::Selector(selector(
                            "$inputs.document",
                            "/items/0/id",
                            jsonpointer.clone(),
                        )),
                        ..Replacement::default()
                    }],
                    ..RequestBody::default()
                }),
                success_criteria: vec![SuccessCriterion {
                    context: "$response.body".to_string(),
                    condition: "$.items[*].id".to_string(),
                    type_: Some(CriterionType::Name("jsonpath".to_string())),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([(
                    "ids".to_string(),
                    OutputValue::Selector(selector(
                        "$response.body",
                        "$.items[*].id",
                        SelectorType::ExpressionType(CriterionExpressionType {
                            type_: "jsonpath".to_string(),
                            version: "rfc9535".to_string(),
                            ..CriterionExpressionType::default()
                        }),
                    )),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "first".to_string(),
                OutputValue::Selector(selector("$steps.select.outputs.ids", "/0", jsonpointer)),
            )]),
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building selector engine: {error}"),
    };
    let inputs = BTreeMap::from([(
        "document".to_string(),
        json!({"items": [{"id": 2}, {"id": 1}]}),
    )]);
    let result = engine.execute_collect("selectors", inputs).await;
    let outputs = match &result.outputs {
        Ok(outputs) => outputs,
        Err(error) => panic!("executing selector workflow: {error}"),
    };

    assert_eq!(outputs.get("first"), Some(&json!(2)));
    assert_eq!(
        captured_url
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_str(),
        "/select?selected=2&selected=1"
    );
    assert_eq!(
        captured_headers
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("X-First"))
            .map(|(_, value)| value.as_str()),
        Some("2")
    );
    assert_eq!(
        parse_json_body(
            captured_body
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .as_str()
        ),
        json!({
            "selected": [2, 1],
            "first": 2,
            "literalMap": {
                "context": "literal-context",
                "selector": "literal-selector"
            }
        })
    );
    assert_eq!(
        result.trace_steps()[0].outputs.get("ids"),
        Some(&json!([2, 1]))
    );
    assert!(result.trace_steps()[0].warnings.is_empty());
}

#[tokio::test]
async fn selector_failures_return_null_and_visible_trace_diagnostics() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"items":[]}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "selector-diagnostics".to_string(),
            steps: vec![Step {
                step_id: "select".to_string(),
                target: Some(StepTarget::OperationPath("/select".to_string())),
                outputs: BTreeMap::from([
                    (
                        "zero".to_string(),
                        OutputValue::Selector(selector(
                            "$response.body",
                            "$.items[*].id",
                            SelectorType::Name("jsonpath".to_string()),
                        )),
                    ),
                    (
                        "invalid".to_string(),
                        OutputValue::Selector(selector(
                            "$response.body",
                            "$.items[0:1]",
                            SelectorType::Name("jsonpath".to_string()),
                        )),
                    ),
                    (
                        "unsupportedVersion".to_string(),
                        OutputValue::Selector(selector(
                            "$response.body",
                            "//item",
                            SelectorType::ExpressionType(CriterionExpressionType {
                                type_: "xpath".to_string(),
                                version: "20".to_string(),
                                ..CriterionExpressionType::default()
                            }),
                        )),
                    ),
                ]),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building diagnostic engine: {error}"),
    };
    let result = engine
        .execute_collect("selector-diagnostics", BTreeMap::new())
        .await;
    if let Err(error) = &result.outputs {
        panic!("executing diagnostic workflow: {error}");
    }

    let trace = result.trace_steps()[0];
    assert_eq!(trace.outputs.get("zero"), Some(&serde_json::Value::Null));
    assert_eq!(trace.outputs.get("invalid"), Some(&serde_json::Value::Null));
    assert_eq!(
        trace.outputs.get("unsupportedVersion"),
        Some(&serde_json::Value::Null)
    );
    assert!(trace
        .warnings
        .iter()
        .any(|warning| warning.contains("selector matched no values")));
    assert!(trace
        .warnings
        .iter()
        .any(|warning| warning.contains("array slices are not supported")));
    // Decision 1 (ac-bd441): bare-number XPath version tokens are not in the
    // §5.8.12.1 table and are invalid metadata, not a capability gap.
    assert!(trace
        .warnings
        .iter()
        .any(|warning| warning.contains("unsupported XPath version \"20\"")));
}

/// H1: a delimiter run in a JSONPath filter predicate used to panic the
/// execution task ("execution task completed without sending result"), which
/// is a process abort under the release profile's `panic = "abort"`.
#[tokio::test]
async fn jsonpath_delimiter_run_fails_criteria_instead_of_panicking() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"a":true,"b":true}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "delimiter-run".to_string(),
            steps: vec![Step {
                step_id: "check".to_string(),
                target: Some(StepTarget::OperationPath("/check".to_string())),
                success_criteria: vec![SuccessCriterion {
                    context: "$response.body".to_string(),
                    condition: "$[?(@.a &&& @.b)]".to_string(),
                    type_: Some(CriterionType::Name("jsonpath".to_string())),
                    ..SuccessCriterion::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building delimiter-run engine: {error}"),
    };
    let result = engine
        .execute_collect("delimiter-run", BTreeMap::new())
        .await;
    let err = match &result.outputs {
        Ok(_) => panic!("a malformed predicate must not satisfy the criterion"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed, "{err}");
}

fn captured_string(captured: &Arc<Mutex<String>>) -> String {
    match captured.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("captured request body lock poisoned"),
    }
}

fn parse_json_body(body: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => value,
        Err(err) => panic!("parsing captured body as JSON: {err}; body={body}"),
    }
}

#[tokio::test]
async fn replacements_overlay_json_payload_before_send() {
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_ref = Arc::clone(&captured);
    let server = start_server(move |_method, url, _headers, body| {
        if url == "/items" {
            match captured_ref.lock() {
                Ok(mut guard) => *guard = body,
                Err(_) => panic!("capturing request body"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        } else {
            MockHttpResponse::empty(404)
        }
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "replace".to_string(),
            target: Some(StepTarget::OperationPath("POST /items".to_string())),
            request_body: Some(request_body_with_replacements(
                json!({"a": 1, "b": [10, 20]}),
                vec![
                    replacement("/a", to_yaml(json!(99))),
                    replacement("/b/1", to_yaml(json!(21))),
                ],
            )),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("wf", BTreeMap::new()).await.outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }

    assert_eq!(
        parse_json_body(&captured_string(&captured)),
        json!({"a": 99, "b": [10, 21]})
    );
}

#[tokio::test]
async fn replacements_with_expression_value_resolves_against_inputs() {
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_ref = Arc::clone(&captured);
    let server = start_server(move |_method, url, _headers, body| {
        if url == "/items" {
            match captured_ref.lock() {
                Ok(mut guard) => *guard = body,
                Err(_) => panic!("capturing request body"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        } else {
            MockHttpResponse::empty(404)
        }
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "replace".to_string(),
            target: Some(StepTarget::OperationPath("POST /items".to_string())),
            request_body: Some(request_body_with_replacements(
                json!({"userId": null}),
                vec![replacement(
                    "/userId",
                    serde_yaml_ng::Value::String("$inputs.userId".to_string()),
                )],
            )),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let mut inputs = BTreeMap::new();
    inputs.insert("userId".to_string(), json!("U-7"));
    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("wf", inputs).await.outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }

    assert_eq!(
        parse_json_body(&captured_string(&captured)),
        json!({"userId": "U-7"})
    );
}

#[tokio::test]
async fn replacements_with_dependent_step_outputs_resolves_in_order() {
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_ref = Arc::clone(&captured);
    let server = start_server(move |_method, url, _headers, body| match url.as_str() {
        "/create" => MockHttpResponse::json(200, r#"{"id":"S-1"}"#),
        "/update" => {
            match captured_ref.lock() {
                Ok(mut guard) => *guard = body,
                Err(_) => panic!("capturing request body"),
            }
            MockHttpResponse::json(200, r#"{"ok":true}"#)
        }
        _ => MockHttpResponse::empty(404),
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "create".to_string(),
                target: Some(StepTarget::OperationPath("POST /create".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "id".to_string(),
                    "$response.body.id".to_string().into(),
                )]),
                ..Step::default()
            },
            Step {
                step_id: "update".to_string(),
                target: Some(StepTarget::OperationPath("POST /update".to_string())),
                request_body: Some(request_body_with_replacements(
                    json!({"id": null}),
                    vec![replacement(
                        "/id",
                        serde_yaml_ng::Value::String("$steps.create.outputs.id".to_string()),
                    )],
                )),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("wf", BTreeMap::new()).await.outputs;
    if let Err(err) = result {
        panic!("expected success, got: {err}");
    }

    assert_eq!(
        parse_json_body(&captured_string(&captured)),
        json!({"id": "S-1"})
    );
}

#[tokio::test]
#[allow(deprecated)]
async fn replacements_dry_run_emits_merged_body() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "replace".to_string(),
            target: Some(StepTarget::OperationPath("POST /items".to_string())),
            request_body: Some(request_body_with_replacements(
                json!({"a": 1, "b": {"keep": true}}),
                vec![
                    replacement("/a", to_yaml(json!(99))),
                    replacement("/missing/path", to_yaml(json!("x"))),
                ],
            )),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = match EngineBuilder::new(spec).dry_run(true).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].body, Some(json!({"a": 99, "b": {"keep": true}})));

    let traces = exec_result.trace_steps();
    assert_eq!(traces.len(), 1);
    assert!(traces[0]
        .warnings
        .iter()
        .any(|warning| warning.contains("requestBody.replacements[1]:")));
}

#[tokio::test]
async fn dry_run_resolves_explicit_null_and_empty_parameter_and_replacement_values() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "request".to_string(),
            target: Some(StepTarget::OperationPath("POST /items".to_string())),
            parameters: vec![
                Parameter {
                    name: "X-Null".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::Null.into(),
                    ..Parameter::default()
                },
                Parameter {
                    name: "X-Empty".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String(String::new()).into(),
                    ..Parameter::default()
                },
            ],
            request_body: Some(request_body_with_replacements(
                json!({"nullValue": "old", "emptyValue": "old"}),
                vec![
                    replacement("/nullValue", serde_yaml_ng::Value::Null),
                    replacement("/emptyValue", serde_yaml_ng::Value::String(String::new())),
                ],
            )),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building dry-run engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        panic!("executing dry-run workflow: {err}");
    }

    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers.get("X-Null"), Some(&String::new()));
    assert_eq!(requests[0].headers.get("X-Empty"), Some(&String::new()));
    assert_eq!(
        requests[0].body,
        Some(json!({"nullValue": null, "emptyValue": ""}))
    );
}

#[tokio::test]
async fn replacements_warnings_propagate_to_step_trace() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "replace".to_string(),
            target: Some(StepTarget::OperationPath("POST /items".to_string())),
            request_body: Some(request_body_with_replacements(
                json!({"a": 1}),
                vec![replacement("/missing/path", to_yaml(json!("x")))],
            )),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let mut spec = spec;
    if let Some(source) = spec.source_descriptions.get_mut(0) {
        source.url = server.base_url.clone();
    }
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    let traces = exec_result.trace_steps();
    assert_eq!(traces.len(), 1);
    assert!(traces[0]
        .warnings
        .iter()
        .any(|warning| warning.starts_with("requestBody.replacements[0]:")));
}

#[tokio::test]
#[allow(deprecated)]
async fn dry_run_captures_requests_and_headers() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("GET /users".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("POST /items".to_string())),
                request_body: Some(RequestBody {
                    payload: Some(to_yaml(json!({"name":"test"})).into()),
                    ..RequestBody::default()
                }),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].method, "GET");
    assert!(reqs[0].url.ends_with("/users"));
    assert_eq!(reqs[0].step_id, "s1");

    assert_eq!(reqs[1].method, "POST");
    assert!(reqs[1].url.ends_with("/items"));
    assert_eq!(reqs[1].step_id, "s2");
    assert_eq!(reqs[1].body, Some(json!({"name":"test"})));
}

#[tokio::test]
#[allow(deprecated)]
async fn dry_run_resolves_expressions_and_skips_http_calls() {
    let hit_count = Arc::new(AtomicUsize::new(0));
    let hit_count_ref = Arc::clone(&hit_count);
    let _server = start_server(move |_method, _url, _headers, _body| {
        hit_count_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("GET /users/{id}".to_string())),
            parameters: vec![
                Parameter {
                    name: "id".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yaml_ng::Value::String("$inputs.userId".to_string()).into(),
                    ..Parameter::default()
                },
                Parameter {
                    name: "Authorization".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String("$inputs.token".to_string()).into(),
                    ..Parameter::default()
                },
                Parameter {
                    name: "format".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("json".to_string()).into(),
                    ..Parameter::default()
                },
            ],
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
    let inputs = BTreeMap::from([
        ("userId".to_string(), json!("42")),
        ("token".to_string(), json!("Bearer secret")),
    ]);
    let exec_result = engine.execute_collect("wf", inputs).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    assert_eq!(hit_count.load(Ordering::Relaxed), 0);
    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].url.contains("/users/42"));
    assert!(reqs[0].url.contains("format=json"));
    assert_eq!(
        reqs[0].headers.get("Authorization"),
        Some(&"Bearer secret".to_string())
    );
}

#[tokio::test]
#[allow(deprecated)]
async fn dry_run_multi_step_and_custom_headers() {
    let spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/create".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "id".to_string(),
                    "$response.body.id".to_string().into(),
                )]),
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/get/{id}".to_string())),
                parameters: vec![Parameter {
                    name: "id".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yaml_ng::Value::String("$steps.s1.outputs.id".to_string()).into(),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                ..Step::default()
            },
            Step {
                step_id: "s3".to_string(),
                target: Some(StepTarget::OperationPath("PUT /data".to_string())),
                parameters: vec![Parameter {
                    name: "X-Custom".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String("custom-value".to_string()).into(),
                    ..Parameter::default()
                }],
                request_body: Some(RequestBody {
                    content_type: "application/xml".to_string(),
                    payload: Some(to_yaml(json!({"key":"val"})).into()),
                    ..RequestBody::default()
                }),
                success_criteria: success_200(),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    let reqs = exec_result.dry_run_requests();
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

// ── execute_step tests ────────────────────────────────────────────

#[tokio::test]
async fn execute_step_standalone_no_deps() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"v":42}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([(
                        "v".to_string(),
                        "$response.body.v".to_string().into(),
                    )]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([(
                        "v".to_string(),
                        "$response.body.v".to_string().into(),
                    )]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    // Execute only s1 — no deps, should succeed
    let exec_result = engine
        .execute_step("wf", "s1", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    let outputs = match result {
        Ok(o) => o,
        Err(e) => panic!("standalone step should execute: {e}"),
    };
    assert_eq!(outputs.get("v"), Some(&json!(42)));
}

#[tokio::test]
async fn execute_step_with_transitive_deps() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&call_count);
    let server = start_server(move |_m, url, _h, _b| {
        counter.fetch_add(1, Ordering::SeqCst);
        if url.contains("/a") {
            MockHttpResponse::json(200, r#"{"id":"abc"}"#)
        } else {
            MockHttpResponse::json(200, r#"{"result":"ok"}"#)
        }
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
                    outputs: BTreeMap::from([(
                        "id".to_string(),
                        "$response.body.id".to_string().into(),
                    )]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    parameters: vec![Parameter {
                        name: "ref_id".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String("$steps.s1.outputs.id".to_string())
                            .into(),
                        ..Parameter::default()
                    }],
                    outputs: BTreeMap::from([(
                        "result".to_string(),
                        "$response.body.result".to_string().into(),
                    )]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("wf", "s2", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    let outputs = match result {
        Ok(o) => o,
        Err(e) => panic!("step with deps should execute: {e}"),
    };
    assert_eq!(outputs.get("result"), Some(&json!("ok")));
    // Both s1 and s2 should have been executed
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn execute_step_goto_into_filtered_gap_resumes_at_or_after_target() {
    // Filtered set is [s0, s2, s5] (s5 declares dependsOn s0 and s2). A goto
    // from s0 targeting s3 — inside the gap — must resume at s5, the first
    // in-scope step at or after the target, and never execute s2, which
    // precedes the target.
    let hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let hits_ref = Arc::clone(&hits);
    let server = start_server(move |_m, url, _h, _b| {
        if let Ok(mut log) = hits_ref.lock() {
            log.push(url.clone());
        }
        MockHttpResponse::json(200, r#"{"v":"ok"}"#)
    });

    let mut steps: Vec<Step> = (0..6)
        .map(|i| Step {
            step_id: format!("s{i}"),
            target: Some(StepTarget::OperationPath(format!("/s{i}"))),
            success_criteria: success_200(),
            ..Step::default()
        })
        .collect();
    steps[0].on_success = vec![OnAction {
        name: "jump-into-gap".to_string(),
        type_: Some(ActionType::Goto),
        step_id: "s3".to_string(),
        ..OnAction::default()
    }];
    steps[5].depends_on = vec!["s0".to_string(), "s2".to_string()];

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps,
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("wf", "s5", BTreeMap::new(), false)
        .collect()
        .await;
    if let Err(err) = exec_result.outputs {
        panic!("goto into filtered gap should not error: {err}");
    }

    let observed = match hits.lock() {
        Ok(log) => log.clone(),
        Err(err) => panic!("hit log poisoned: {err}"),
    };
    assert_eq!(
        observed,
        vec!["/s0".to_string(), "/s5".to_string()],
        "goto targeting s3 must resume at s5 and never execute s2"
    );
}

#[tokio::test]
async fn execute_step_no_deps_flag_standalone_succeeds() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"v":1}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([("v".to_string(), "$response.body.v".to_string().into())]),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("wf", "s1", BTreeMap::new(), true)
        .collect()
        .await;
    let result = exec_result.outputs;
    let outputs = match result {
        Ok(o) => o,
        Err(e) => panic!("no_deps standalone should succeed: {e}"),
    };
    assert_eq!(outputs.get("v"), Some(&json!(1)));
}

#[tokio::test]
async fn execute_step_no_deps_flag_with_refs_fails() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    outputs: BTreeMap::from([(
                        "val".to_string(),
                        "$steps.s1.outputs.id".to_string().into(),
                    )]),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("wf", "s2", BTreeMap::new(), true)
        .collect()
        .await;
    let result = exec_result.outputs;
    let err = match result {
        Ok(_) => panic!("no_deps with refs should fail"),
        Err(e) => e,
    };
    assert_eq!(err.kind, RuntimeErrorKind::StepMissingDependency);
    assert!(
        err.message.contains("s1"),
        "error should mention the missing dep"
    );
}

#[tokio::test]
async fn execute_step_unknown_step_errors() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("wf", "missing", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    let err = match result {
        Ok(_) => panic!("unknown step should fail"),
        Err(e) => e,
    };
    assert_eq!(err.kind, RuntimeErrorKind::StepNotFound);
}

#[tokio::test]
async fn execute_step_unknown_workflow_errors() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = new_test_engine(&server.base_url, spec);

    let exec_result = engine
        .execute_step("bad", "s1", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    let err = match result {
        Ok(_) => panic!("unknown workflow should fail"),
        Err(e) => e,
    };
    assert_eq!(err.kind, RuntimeErrorKind::WorkflowNotFound);
}

// ── execute_step flow decision tests (Bug #14) ──────────────────

#[tokio::test]
async fn execute_step_retries_on_failure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_m, _u, _h, _b| {
        let current = calls_ref.fetch_add(1, Ordering::Relaxed) + 1;
        if current < 3 {
            return MockHttpResponse::empty(500);
        }
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/flaky".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(2),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let engine = new_test_engine(&server.base_url, spec);
    let exec_result = engine
        .execute_step("wf", "s1", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    if let Err(err) = result {
        panic!("expected success after retries in execute_step, got: {err}");
    }
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn execute_step_retry_limit_exceeded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_m, _u, _h, _b| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
        MockHttpResponse::empty(500)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: success_200(),
                on_failure: vec![OnAction {
                    type_: Some(ActionType::Retry),
                    retry_limit: Some(1),
                    ..OnAction::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let engine = new_test_engine(&server.base_url, spec);
    let exec_result = engine
        .execute_step("wf", "s1", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    let err = match result {
        Ok(_) => panic!("expected retry limit exceeded error"),
        Err(e) => e,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RetryLimitExceeded);
    // 1 initial call + 1 retry = 2 total calls
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn execute_step_on_success_end_stops_early() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_ref = Arc::clone(&calls);
    let server = start_server(move |_m, _u, _h, _b| {
        calls_ref.fetch_add(1, Ordering::Relaxed);
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
                    on_success: vec![OnAction {
                        type_: Some(ActionType::End),
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

    let engine = new_test_engine(&server.base_url, spec);
    // Target s2 — deps resolve s1 first. s1's onSuccess:End should stop execution.
    let exec_result = engine
        .execute_step("wf", "s2", BTreeMap::new(), false)
        .collect()
        .await;
    let result = exec_result.outputs;
    // onSuccess:End should cause a clean exit — s2 never runs
    assert!(result.is_ok(), "expected success (Done), got: {result:?}");
    // Only s1 should have been called, s2 should be skipped
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

// ── RuntimeError tests ────────────────────────────────────────────

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

#[test]
fn runtime_error_chain_preserved() {
    use std::error::Error;
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let runtime_err =
        RuntimeError::with_source(RuntimeErrorKind::HttpRequest, "request failed", io_err);
    match runtime_err.source() {
        Some(source) => assert!(source.to_string().contains("file missing")),
        None => panic!("expected source error in chain"),
    }
}

#[test]
fn internal_runtime_api_version_is_v1() {
    assert_eq!(arazzo_runtime::INTERNAL_RUNTIME_API_VERSION, "v1");
}

// ── Response size limit tests ─────────────────────────────────────

#[tokio::test]
async fn response_exceeding_size_limit_produces_error() {
    // Serve a response body larger than the configured limit.
    let large_body = "x".repeat(1024); // 1 KiB
    let server = start_server(move |_method, _url, _headers, _body| {
        MockHttpResponse::json(200, &large_body)
    });

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "size-limit".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/big".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    // Set a very small limit (512 bytes) so the 1 KiB response exceeds it.
    let engine = match EngineBuilder::new(spec).max_response_bytes(512).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("size-limit", BTreeMap::new()).await;
    let err = match result.outputs {
        Err(err) => err,
        Ok(_) => panic!("expected ResponseTooLarge error, got success"),
    };
    assert_eq!(err.kind, RuntimeErrorKind::ResponseTooLarge);
    assert_eq!(err.code(), "RUNTIME_RESPONSE_TOO_LARGE");
}

#[tokio::test]
async fn response_within_size_limit_succeeds() {
    let small_body = r#"{"ok":true}"#;
    let server =
        start_server(move |_method, _url, _headers, _body| MockHttpResponse::json(200, small_body));

    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "size-ok".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/small".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );

    let engine = match EngineBuilder::new(spec)
        .max_response_bytes(1_048_576)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };

    let result = engine.execute_collect("size-ok", BTreeMap::new()).await;
    match result.outputs {
        Ok(_) => {} // success as expected
        Err(err) => panic!("expected success, got: {err}"),
    }
}

// ── Bug #4: iteration limit exceeded on circular goto ────────────

#[tokio::test]
async fn execute_circular_goto_returns_iteration_limit_exceeded() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    // Two steps that goto each other on success — infinite loop.
    let spec = make_spec(vec![Workflow {
        workflow_id: "loop".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/ping".to_string())),
                success_criteria: success_200(),
                on_success: vec![OnAction {
                    type_: Some(ActionType::Goto),
                    step_id: "b".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/pong".to_string())),
                success_criteria: success_200(),
                on_success: vec![OnAction {
                    type_: Some(ActionType::Goto),
                    step_id: "a".to_string(),
                    ..OnAction::default()
                }],
                ..Step::default()
            },
        ],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("loop", BTreeMap::new())
        .await
        .outputs;
    let err = match result {
        Ok(_) => panic!("expected IterationLimitExceeded, got success"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::IterationLimitExceeded);
    assert!(err.message.contains("exceeded iteration limit"));
}

// ── Bug #11: path parameter values are percent-encoded ───────────

#[tokio::test]
async fn execute_path_param_with_special_chars_is_percent_encoded() {
    let received_url = Arc::new(Mutex::new(String::new()));
    let url_capture = Arc::clone(&received_url);
    let server = start_server(move |_method, url, _headers, _body| {
        *url_capture.lock().unwrap_or_else(|e| panic!("lock: {e}")) = url.clone();
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "enc".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/items/{name}".to_string())),
            parameters: vec![Parameter {
                name: "name".to_string(),
                in_: Some(ParamLocation::Path),
                value: serde_yaml_ng::Value::String("hello world/foo#bar".to_string()).into(),
                ..Parameter::default()
            }],
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine.execute_collect("enc", BTreeMap::new()).await.outputs;
    if let Err(err) = &result {
        panic!("expected success, got: {err}");
    }

    let url = received_url
        .lock()
        .unwrap_or_else(|e| panic!("lock: {e}"))
        .clone();
    // Spaces, slashes, and '#' must be percent-encoded in path segments
    assert!(
        url.contains("hello%20world%2Ffoo%23bar"),
        "expected percent-encoded path param, got: {url}"
    );
}

// ── Bug #10: sub-workflow param interpolation preserves types ─────

#[tokio::test]
async fn sub_workflow_interpolated_param_preserves_number_type() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    // Parent passes {$inputs.count} to child — the braces trigger interpolation.
    // Child exposes the received input as a workflow output.
    let spec = make_spec(vec![
        Workflow {
            workflow_id: "parent".to_string(),
            steps: vec![Step {
                step_id: "call-child".to_string(),
                target: Some(StepTarget::WorkflowId("child".to_string())),
                parameters: vec![Parameter {
                    name: "count".to_string(),
                    value: serde_yaml_ng::Value::String("{$inputs.count}".to_string()).into(),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "result".to_string(),
                "$steps.call-child.outputs.received".to_string().into(),
            )]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "noop".to_string(),
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("received".to_string(), "$inputs.count".to_string().into())]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("count".to_string(), json!(42))]);
    let outputs = match engine.execute_collect("parent", inputs).await.outputs {
        Ok(o) => o,
        Err(err) => panic!("expected success, got: {err}"),
    };

    // The value should be a number, not a string
    assert_eq!(
        outputs.get("result"),
        Some(&json!(42)),
        "interpolated param should preserve numeric type, got: {:?}",
        outputs.get("result")
    );
}

#[tokio::test]
async fn sub_workflow_selector_param_preserves_number_type() {
    let server = start_server(|_method, _url, _headers, _body| {
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let spec = make_spec(vec![
        Workflow {
            workflow_id: "parent".to_string(),
            steps: vec![Step {
                step_id: "call-child".to_string(),
                target: Some(StepTarget::WorkflowId("child".to_string())),
                parameters: vec![Parameter {
                    name: "count".to_string(),
                    value: ValueSource::Selector(selector(
                        "$inputs.document",
                        "/count",
                        SelectorType::Name("jsonpointer".to_string()),
                    )),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "result".to_string(),
                "$steps.call-child.outputs.received".to_string().into(),
            )]),
            ..Workflow::default()
        },
        Workflow {
            workflow_id: "child".to_string(),
            steps: vec![Step {
                step_id: "noop".to_string(),
                target: Some(StepTarget::OperationPath("/ok".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("received".to_string(), "$inputs.count".to_string().into())]),
            ..Workflow::default()
        },
    ]);

    let engine = new_test_engine(&server.base_url, spec);
    let inputs = BTreeMap::from([("document".to_string(), json!({"count": 42}))]);
    let outputs = match engine.execute_collect("parent", inputs).await.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };

    assert_eq!(outputs.get("result"), Some(&json!(42)));
}

// ── sourceDescriptions document loading (relative urls) ─────────────

fn testdata_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

/// The path a `./<name>` source url under `testdata_dir()` resolves to once
/// RFC 3986 §5.2.4 has removed its dot segments — the file the runtime reads
/// and the name its diagnostics carry.
fn resolved_testdata_path(name: &str) -> std::path::PathBuf {
    let mut resolved = std::path::PathBuf::new();
    for component in testdata_dir().components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved.join(name)
}

fn relative_source_spec(url: &str, workflows: Vec<Workflow>) -> ArazzoSpec {
    let mut spec = make_spec(workflows);
    spec.source_descriptions = vec![SourceDescription {
        name: "petstore".to_string(),
        url: url.to_string(),
        type_: SourceType::OpenApi,
        ..SourceDescription::default()
    }];
    spec
}

fn list_pets_workflow() -> Vec<Workflow> {
    vec![Workflow {
        workflow_id: "list-pets".to_string(),
        steps: vec![Step {
            step_id: "list".to_string(),
            target: Some(StepTarget::OperationId("listPets".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        outputs: BTreeMap::from([(
            "sourceUrl".to_string(),
            "$sourceDescriptions.petstore.url".to_string().into(),
        )]),
        ..Workflow::default()
    }]
}

#[tokio::test]
async fn relative_source_dry_run_derives_base_from_servers() {
    let spec = relative_source_spec("./petstore.openapi.yaml", list_pets_workflow());
    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .source_base_dir(testdata_dir())
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let exec_result = engine.execute_collect("list-pets", BTreeMap::new()).await;
    let outputs = match &exec_result.outputs {
        Ok(outputs) => outputs.clone(),
        Err(err) => panic!("expected success, got: {err}"),
    };

    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "GET");
    // Base derives from the loaded document's servers[0].url, joined with the
    // operation path — no openapi_spec() input involved.
    assert_eq!(reqs[0].url, "https://petstore.example.com/v1/pets");
    // The literal document url stays visible to expressions.
    assert_eq!(
        outputs.get("sourceUrl"),
        Some(&json!("./petstore.openapi.yaml"))
    );
}

#[tokio::test]
async fn relative_source_missing_file_fails_build() {
    let spec = relative_source_spec("./no-such-file.openapi.yaml", list_pets_workflow());
    let err = match EngineBuilder::new(spec)
        .dry_run(true)
        .source_base_dir(testdata_dir())
        .build()
    {
        Ok(_) => panic!("build should fail for a missing source document"),
        Err(err) => err,
    };

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_LOAD");
    let expected_path = resolved_testdata_path("no-such-file.openapi.yaml");
    assert!(
        err.message.contains("petstore"),
        "error should name the source: {}",
        err.message
    );
    assert!(
        err.message.contains(&expected_path.display().to_string()),
        "error should name the resolved path: {}",
        err.message
    );
}

#[tokio::test]
async fn relative_source_unparseable_file_fails_build() {
    let spec = relative_source_spec("./unparseable.openapi.yaml", list_pets_workflow());
    let err = match EngineBuilder::new(spec)
        .dry_run(true)
        .source_base_dir(testdata_dir())
        .build()
    {
        Ok(_) => panic!("build should fail for an unparseable source document"),
        Err(err) => err,
    };

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse);
    let expected_path = resolved_testdata_path("unparseable.openapi.yaml");
    assert!(
        err.message.contains("petstore"),
        "error should name the source: {}",
        err.message
    );
    assert!(
        err.message.contains(&expected_path.display().to_string()),
        "error should name the resolved path: {}",
        err.message
    );
}

#[tokio::test]
async fn relative_source_without_base_dir_fails_build() {
    let spec = relative_source_spec("./petstore.openapi.yaml", list_pets_workflow());
    let err = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(_) => panic!("build should fail when no source base dir is provided"),
        Err(err) => err,
    };

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert!(
        err.message.contains("petstore"),
        "error should name the source: {}",
        err.message
    );
}

#[tokio::test]
async fn explicit_openapi_spec_overrides_source_operation() {
    let override_bytes = match std::fs::read(testdata_dir().join("petstore-override.openapi.yaml"))
    {
        Ok(bytes) => bytes,
        Err(err) => panic!("reading override fixture: {err}"),
    };

    let spec = relative_source_spec("./petstore.openapi.yaml", list_pets_workflow());
    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .source_base_dir(testdata_dir())
        .openapi_spec(override_bytes, None)
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let exec_result = engine.execute_collect("list-pets", BTreeMap::new()).await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    // The explicitly provided spec wins the duplicate operationId, so the
    // resolved path is its /pets-v2 (joined to the first source's base).
    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://petstore.example.com/v1/pets-v2");
}

#[tokio::test]
async fn legacy_absolute_source_url_stays_literal_in_expressions() {
    let mut spec = make_spec(vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/get".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        outputs: BTreeMap::from([(
            "sourceUrl".to_string(),
            "$sourceDescriptions.test.url".to_string().into(),
        )]),
        ..Workflow::default()
    }]);
    spec.source_descriptions[0].url = "https://api.example.com".to_string();

    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    let outputs = match &exec_result.outputs {
        Ok(outputs) => outputs.clone(),
        Err(err) => panic!("expected success, got: {err}"),
    };

    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].url, "https://api.example.com/get");
    assert_eq!(
        outputs.get("sourceUrl"),
        Some(&json!("https://api.example.com"))
    );
}

#[tokio::test]
async fn relative_source_openapi_32_json_document_indexes_and_derives_base() {
    // Regression for GitHub issue #3's run scenario: a relative OpenAPI 3.2
    // JSON source (root $self, 2020-12 type arrays, numeric exclusiveMinimum,
    // examples arrays, description-less responses) indexes through the untyped
    // walker and derives its request base from servers[0].url.
    let mut spec = relative_source_spec(
        "./bank-32.openapi.json",
        vec![Workflow {
            workflow_id: "get-all-banks".to_string(),
            steps: vec![Step {
                step_id: "getAllBanks".to_string(),
                target: Some(StepTarget::OperationId("GetAllBanks".to_string())),
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    spec.source_descriptions[0].name = "bank".to_string();

    let engine = match EngineBuilder::new(spec)
        .dry_run(true)
        .source_base_dir(testdata_dir())
        .build()
    {
        Ok(e) => e,
        Err(err) => panic!("building engine: {err}"),
    };

    let exec_result = engine
        .execute_collect("get-all-banks", BTreeMap::new())
        .await;
    if let Err(err) = &exec_result.outputs {
        panic!("expected success, got: {err}");
    }

    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "GET");
    assert_eq!(reqs[0].url, "https://localhost:5201/v1/banks");
}
