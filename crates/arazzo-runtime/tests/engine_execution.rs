mod common;

use arazzo_runtime::{EngineBuilder, RuntimeError, RuntimeErrorKind};
use arazzo_spec::{
    ActionType, OnAction, ParamLocation, Parameter, RequestBody, Step, StepTarget,
    SuccessCriterion, Workflow,
};
use common::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Basic execution tests ─────────────────────────────────────────

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
                    type_: ActionType::End,
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
                    type_: ActionType::End,
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
                    type_: ActionType::Goto,
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
                    type_: ActionType::Goto,
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
                type_: ActionType::Retry,
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
                type_: ActionType::Retry,
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
    assert_eq!(err.message, "step s1: max retries (3) exceeded");
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
                type_: ActionType::Retry,
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
                type_: ActionType::Retry,
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
                type_: ActionType::Retry,
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
                type_: ActionType::Retry,
                retry_after: 1,
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
                type_: ActionType::Retry,
                retry_after: 2,
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
                        type_: ActionType::Goto,
                        step_id: "rate-handler".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 429".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: ActionType::Goto,
                        step_id: "server-error-handler".to_string(),
                        criteria: vec![SuccessCriterion {
                            condition: "$statusCode == 500".to_string(),
                            ..SuccessCriterion::default()
                        }],
                        ..OnAction::default()
                    },
                    OnAction {
                        type_: ActionType::End,
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
                    type_: ActionType::Retry,
                    criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 429".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..OnAction::default()
                },
                OnAction {
                    type_: ActionType::Goto,
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
                type_: ActionType::Goto,
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
                type_: ActionType::Goto,
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
                "$steps.call-child.outputs.token".to_string(),
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
                    value: serde_yml::Value::String("$inputs.uid".to_string()),
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
                    value: serde_yml::Value::String("$inputs.userId".to_string()),
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
                    type_: ActionType::Goto,
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
                    "$response.body.fallback".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("ok".to_string(), "$steps.fb.outputs.ok".to_string())]),
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
        .openapi_spec(openapi.to_vec())
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
        .openapi_spec(openapi.to_vec())
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
                    value: serde_yml::Value::String("$inputs.userId".to_string()),
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
                    payload: Some(to_yaml(json!({"name":"test"}))),
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
                    value: serde_yml::Value::String("$inputs.userId".to_string()),
                    ..Parameter::default()
                },
                Parameter {
                    name: "Authorization".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yml::Value::String("$inputs.token".to_string()),
                    ..Parameter::default()
                },
                Parameter {
                    name: "format".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yml::Value::String("json".to_string()),
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
                outputs: BTreeMap::from([("id".to_string(), "$response.body.id".to_string())]),
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/get/{id}".to_string())),
                parameters: vec![Parameter {
                    name: "id".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yml::Value::String("$steps.s1.outputs.id".to_string()),
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
                    value: serde_yml::Value::String("custom-value".to_string()),
                    ..Parameter::default()
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
                    outputs: BTreeMap::from([("v".to_string(), "$response.body.v".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    outputs: BTreeMap::from([("v".to_string(), "$response.body.v".to_string())]),
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
                    outputs: BTreeMap::from([("id".to_string(), "$response.body.id".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    success_criteria: success_200(),
                    parameters: vec![Parameter {
                        name: "ref_id".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yml::Value::String("$steps.s1.outputs.id".to_string()),
                        ..Parameter::default()
                    }],
                    outputs: BTreeMap::from([(
                        "result".to_string(),
                        "$response.body.result".to_string(),
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
                outputs: BTreeMap::from([("v".to_string(), "$response.body.v".to_string())]),
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
                        "$steps.s1.outputs.id".to_string(),
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
                    type_: ActionType::Goto,
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
                    type_: ActionType::Goto,
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
                value: serde_yml::Value::String("hello world/foo#bar".to_string()),
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
                    value: serde_yml::Value::String("{$inputs.count}".to_string()),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "result".to_string(),
                "$steps.call-child.outputs.received".to_string(),
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
            outputs: BTreeMap::from([("received".to_string(), "$inputs.count".to_string())]),
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
