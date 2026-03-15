mod common;

use arazzo_runtime::{Engine, EngineBuilder};
use arazzo_spec::{
    ActionType, Info, OnAction, ParamLocation, Parameter, RequestBody, SourceDescription,
    SourceType, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_spec::ArazzoSpec;
use std::sync::Mutex;

// -----------------------------------------------------------------------
// Expression/config tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn execute_response_header_expression() {
    let server = start_server(|_method, _url, _headers, _body| {
        let mut response = MockHttpResponse::json(200, r#"{"ok":true}"#);
        response
            .headers
            .insert("X-Request-Id".to_string(), "abc-123".to_string());
        response
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "header-extract".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/test".to_string())),
            success_criteria: success_200(),
            outputs: BTreeMap::from([(
                "request_id".to_string(),
                "$response.header.X-Request-Id".to_string(),
            )]),
            ..Step::default()
        }],
        outputs: BTreeMap::from([(
            "request_id".to_string(),
            "$steps.s1.outputs.request_id".to_string(),
        )]),
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("header-extract", BTreeMap::new())
        .await
        .outputs;
    let outputs = match result {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(outputs.get("request_id"), Some(&json!("abc-123")));
}

#[tokio::test]
async fn execute_env_expression() {
    std::env::set_var("ARAZZO_RUNTIME_TEST_TOKEN", "secret-42");
    let server = start_server(|_method, _url, headers, _body| {
        let auth = header_value(&headers, "Authorization").unwrap_or_default();
        MockHttpResponse::json(200, &format!(r#"{{"auth":"{auth}"}}"#))
    });

    let spec = make_spec(vec![Workflow {
        workflow_id: "env-test".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/protected".to_string())),
            parameters: vec![arazzo_spec::Parameter {
                name: "Authorization".to_string(),
                in_: Some(ParamLocation::Header),
                value: serde_yaml_ng::Value::String("$env.ARAZZO_RUNTIME_TEST_TOKEN".to_string()),
                ..arazzo_spec::Parameter::default()
            }],
            success_criteria: success_200(),
            outputs: BTreeMap::from([("auth".to_string(), "$response.body.auth".to_string())]),
            ..Step::default()
        }],
        outputs: BTreeMap::from([("auth".to_string(), "$steps.s1.outputs.auth".to_string())]),
        ..Workflow::default()
    }]);

    let engine = new_test_engine(&server.base_url, spec);
    let result = engine
        .execute_collect("env-test", BTreeMap::new())
        .await
        .outputs;
    let outputs = match result {
        Ok(outputs) => outputs,
        Err(err) => panic!("expected success, got: {err}"),
    };
    assert_eq!(outputs.get("auth"), Some(&json!("secret-42")));
}

#[tokio::test]
async fn execute_request_body_content_type_and_method_selection() {
    let put_method = Arc::new(Mutex::new(String::new()));
    let put_method_ref = Arc::clone(&put_method);
    let put_content_type = Arc::new(Mutex::new(String::new()));
    let put_content_type_ref = Arc::clone(&put_content_type);
    let put_server = start_server(move |method, _url, headers, _body| {
        let content_type = header_value(&headers, "Content-Type").unwrap_or_default();
        match put_content_type_ref.lock() {
            Ok(mut guard) => *guard = content_type,
            Err(_) => panic!("capturing content type"),
        }
        match put_method_ref.lock() {
            Ok(mut guard) => *guard = method,
            Err(_) => panic!("capturing HTTP method"),
        }
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });

    let put_spec = make_spec(vec![Workflow {
        workflow_id: "put".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("PUT /users/123".to_string())),
            request_body: Some(RequestBody {
                content_type: "application/x-www-form-urlencoded".to_string(),
                payload: Some(to_yaml(json!({"key":"val"}))),
                ..RequestBody::default()
            }),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let put_engine = new_test_engine(&put_server.base_url, put_spec);
    let put_result = put_engine
        .execute_collect("put", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = put_result {
        panic!("expected PUT workflow success, got: {err}");
    }
    let captured_put = match put_method.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading PUT method"),
    };
    assert_eq!(captured_put, "PUT");
    let captured_put_content_type = match put_content_type.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading PUT content type"),
    };
    assert_eq!(
        captured_put_content_type,
        "application/x-www-form-urlencoded"
    );

    let delete_method = Arc::new(Mutex::new(String::new()));
    let delete_method_ref = Arc::clone(&delete_method);
    let delete_server = start_server(move |method, _url, _headers, _body| {
        match delete_method_ref.lock() {
            Ok(mut guard) => *guard = method,
            Err(_) => panic!("capturing DELETE method"),
        }
        MockHttpResponse::json(204, "{}")
    });
    let delete_spec = make_spec(vec![Workflow {
        workflow_id: "delete".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("DELETE /users/123".to_string())),
            success_criteria: vec![SuccessCriterion {
                condition: "$statusCode == 204".to_string(),
                ..SuccessCriterion::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let delete_engine = new_test_engine(&delete_server.base_url, delete_spec);
    let delete_result = delete_engine
        .execute_collect("delete", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = delete_result {
        panic!("expected DELETE workflow success, got: {err}");
    }
    let captured_delete = match delete_method.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading DELETE method"),
    };
    assert_eq!(captured_delete, "DELETE");

    let patch_method = Arc::new(Mutex::new(String::new()));
    let patch_method_ref = Arc::clone(&patch_method);
    let patch_server = start_server(move |method, _url, _headers, _body| {
        match patch_method_ref.lock() {
            Ok(mut guard) => *guard = method,
            Err(_) => panic!("capturing PATCH method"),
        }
        MockHttpResponse::json(200, r#"{"patched":true}"#)
    });
    let patch_spec = make_spec(vec![Workflow {
        workflow_id: "patch".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("PATCH /items/42".to_string())),
            request_body: Some(RequestBody {
                payload: Some(to_yaml(json!({"status":"active"}))),
                ..RequestBody::default()
            }),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let patch_engine = new_test_engine(&patch_server.base_url, patch_spec);
    let patch_result = patch_engine
        .execute_collect("patch", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = patch_result {
        panic!("expected PATCH workflow success, got: {err}");
    }
    let captured_patch = match patch_method.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading PATCH method"),
    };
    assert_eq!(captured_patch, "PATCH");

    let get_method = Arc::new(Mutex::new(String::new()));
    let get_method_ref = Arc::clone(&get_method);
    let get_server = start_server(move |method, _url, _headers, _body| {
        match get_method_ref.lock() {
            Ok(mut guard) => *guard = method,
            Err(_) => panic!("capturing GET method"),
        }
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let get_spec = make_spec(vec![Workflow {
        workflow_id: "fallback-get".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/health".to_string())),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }]);
    let get_engine = new_test_engine(&get_server.base_url, get_spec);
    let get_result = get_engine
        .execute_collect("fallback-get", BTreeMap::new())
        .await
        .outputs;
    if let Err(err) = get_result {
        panic!("expected fallback GET success, got: {err}");
    }
    let captured_get = match get_method.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => panic!("reading GET method"),
    };
    assert_eq!(captured_get, "GET");
}

// -----------------------------------------------------------------------
// Workflow behavior tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn workflow_level_success_actions_as_default() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            success_actions: vec![OnAction {
                type_: ActionType::End,
                ..OnAction::default()
            }],
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 200".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    // Workflow-level end action should stop after s1, so s2 never runs
    let result = engine.execute_collect("wf", BTreeMap::new()).await.outputs;
    assert!(result.is_ok());
}

#[tokio::test]
async fn workflow_level_actions_ignored_when_step_has_own() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            success_actions: vec![OnAction {
                type_: ActionType::End,
                ..OnAction::default()
            }],
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    success_criteria: vec![SuccessCriterion {
                        condition: "$statusCode == 200".to_string(),
                        ..SuccessCriterion::default()
                    }],
                    // Step has its own on_success (goto s2), so workflow-level "end" is ignored
                    on_success: vec![OnAction {
                        type_: ActionType::Goto,
                        step_id: "s2".to_string(),
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("/b".to_string())),
                    outputs: BTreeMap::from([("reached".to_string(), "$statusCode".to_string())]),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::from([(
                "s2_reached".to_string(),
                "$steps.s2.outputs.reached".to_string(),
            )]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    // s2 should execute because s1 has its own on_success, bypassing workflow-level end
    assert_eq!(result.get("s2_reached"), Some(&json!(200)));
}

#[tokio::test]
async fn workflow_level_failure_actions_as_default() {
    let server =
        start_server(|_m, _u, _h, _b| MockHttpResponse::json(500, r#"{"error":"internal"}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            failure_actions: vec![OnAction {
                type_: ActionType::End,
                ..OnAction::default()
            }],
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/fail".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    // Workflow-level "end" on failure should produce an error (end on failure path = error)
    match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("expected workflow to fail"),
        Err(err) => assert!(
            err.message.contains("workflow ended by onFailure action"),
            "unexpected error: {err}"
        ),
    }
}

#[tokio::test]
async fn workflow_level_parameters_merge_into_steps() {
    let spec = make_spec_with_base(
        "http://localhost",
        vec![Workflow {
            workflow_id: "wf".to_string(),
            parameters: vec![Parameter {
                name: "X-Workflow-Header".to_string(),
                in_: Some(ParamLocation::Header),
                value: serde_yaml_ng::Value::String("workflow-value".to_string()),
                ..Parameter::default()
            }],
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if exec_result.outputs.is_err() {
        panic!("expected dry-run execution to succeed");
    }
    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].headers.get("X-Workflow-Header"),
        Some(&"workflow-value".to_string())
    );
}

#[tokio::test]
async fn step_params_override_workflow_params() {
    let spec = make_spec_with_base(
        "http://localhost",
        vec![Workflow {
            workflow_id: "wf".to_string(),
            parameters: vec![Parameter {
                name: "X-Auth".to_string(),
                in_: Some(ParamLocation::Header),
                value: serde_yaml_ng::Value::String("default-token".to_string()),
                ..Parameter::default()
            }],
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                parameters: vec![Parameter {
                    name: "X-Auth".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String("step-token".to_string()),
                    ..Parameter::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let exec_result = engine.execute_collect("wf", BTreeMap::new()).await;
    if exec_result.outputs.is_err() {
        panic!("expected dry-run execution to succeed");
    }
    let reqs = exec_result.dry_run_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].headers.get("X-Auth"),
        Some(&"step-token".to_string())
    );
}

#[tokio::test]
async fn workflow_params_not_merged_into_subworkflow_steps() {
    let spec = make_spec_with_base(
        "http://localhost",
        vec![
            Workflow {
                workflow_id: "parent".to_string(),
                parameters: vec![Parameter {
                    name: "X-Parent".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String("parent-val".to_string()),
                    ..Parameter::default()
                }],
                steps: vec![Step {
                    step_id: "call-child".to_string(),
                    target: Some(StepTarget::WorkflowId("child".to_string())),
                    parameters: vec![Parameter {
                        name: "input_val".to_string(),
                        value: serde_yaml_ng::Value::String("hello".to_string()),
                        ..Parameter::default()
                    }],
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "child-step".to_string(),
                    target: Some(StepTarget::OperationPath("/a".to_string())),
                    ..Step::default()
                }],
                ..Workflow::default()
            },
        ],
    );
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let exec_result = engine.execute_collect("parent", BTreeMap::new()).await;
    if exec_result.outputs.is_err() {
        panic!("expected parent workflow dry-run execution to succeed");
    }
    let reqs = exec_result.dry_run_requests();
    // child-step should NOT have the parent's X-Parent header
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].headers.get("X-Parent"), None);
}

#[tokio::test]
async fn build_outputs_with_interpolation_and_outputs_ref() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"total":42}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                outputs: BTreeMap::from([("sum".to_string(), "total".to_string())]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                ("amount".to_string(), "$steps.s1.outputs.sum".to_string()),
                (
                    "summary".to_string(),
                    "Total is {$outputs.amount}".to_string(),
                ),
            ]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("amount"), Some(&json!(42)));
    assert_eq!(result.get("summary"), Some(&json!("Total is 42")));
}

// --- Phase 3: Request Introspection + Multiple Source Descriptions ---

#[tokio::test]
async fn url_expression_in_outputs() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/api/test".to_string())),
                success_criteria: success_200(),
                outputs: BTreeMap::from([
                    ("captured_url".to_string(), "$url".to_string()),
                    ("captured_method".to_string(), "$method".to_string()),
                ]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "url".to_string(),
                "$steps.s1.outputs.captured_url".to_string(),
            )]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    let url_val = result
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        url_val.contains("/api/test"),
        "expected url to contain /api/test, got: {url_val}"
    );
}

#[tokio::test]
async fn request_header_expression_in_outputs() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/api/test".to_string())),
                parameters: vec![Parameter {
                    name: "X-Auth".to_string(),
                    in_: Some(ParamLocation::Header),
                    value: serde_yaml_ng::Value::String("Bearer token123".to_string()),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "auth".to_string(),
                    "$request.header.X-Auth".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("auth".to_string(), "$steps.s1.outputs.auth".to_string())]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("auth"), Some(&json!("Bearer token123")));
}

#[tokio::test]
async fn request_query_expression_in_outputs() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/api/test".to_string())),
                parameters: vec![Parameter {
                    name: "page".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("5".to_string()),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                outputs: BTreeMap::from([("page".to_string(), "$request.query.page".to_string())]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([("page".to_string(), "$steps.s1.outputs.page".to_string())]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("page"), Some(&json!("5")));
}

#[tokio::test]
async fn request_path_expression_in_outputs() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath(
                    "/users/{userId}/profile".to_string(),
                )),
                parameters: vec![Parameter {
                    name: "userId".to_string(),
                    in_: Some(ParamLocation::Path),
                    value: serde_yaml_ng::Value::String("42".to_string()),
                    ..Parameter::default()
                }],
                success_criteria: success_200(),
                outputs: BTreeMap::from([(
                    "user_id".to_string(),
                    "$request.path.userId".to_string(),
                )]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([(
                "user_id".to_string(),
                "$steps.s1.outputs.user_id".to_string(),
            )]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("user_id"), Some(&json!("42")));
}

#[tokio::test]
async fn request_body_expression_in_outputs() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"created":true}"#));
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("POST /api/items".to_string())),
                request_body: Some(RequestBody {
                    content_type: "application/json".to_string(),
                    payload: Some(to_yaml(json!({"name": "widget", "count": 3}))),
                    ..RequestBody::default()
                }),
                success_criteria: success_200(),
                outputs: BTreeMap::from([
                    ("full_body".to_string(), "$request.body".to_string()),
                    ("body_name".to_string(), "$request.body.name".to_string()),
                    (
                        "body_count_ptr".to_string(),
                        "$request.body#/count".to_string(),
                    ),
                ]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                (
                    "full_body".to_string(),
                    "$steps.s1.outputs.full_body".to_string(),
                ),
                (
                    "body_name".to_string(),
                    "$steps.s1.outputs.body_name".to_string(),
                ),
                (
                    "body_count_ptr".to_string(),
                    "$steps.s1.outputs.body_count_ptr".to_string(),
                ),
            ]),
            ..Workflow::default()
        }],
    );
    let engine = match Engine::new(spec) {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(
        result.get("full_body"),
        Some(&json!({"name": "widget", "count": 3}))
    );
    assert_eq!(result.get("body_name"), Some(&json!("widget")));
    assert_eq!(result.get("body_count_ptr"), Some(&json!(3)));
}

// -----------------------------------------------------------------------
// Source description tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn source_descriptions_url_expression() {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "test".to_string(),
            summary: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![
            SourceDescription {
                name: "primary".to_string(),
                url: "http://localhost".to_string(),
                type_: SourceType::OpenApi,
            },
            SourceDescription {
                name: "secondary".to_string(),
                url: "https://api.example.com".to_string(),
                type_: SourceType::OpenApi,
            },
        ],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/status".to_string())),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                (
                    "primary_url".to_string(),
                    "$sourceDescriptions.primary.url".to_string(),
                ),
                (
                    "secondary_url".to_string(),
                    "$sourceDescriptions.secondary.url".to_string(),
                ),
                (
                    "missing_url".to_string(),
                    "$sourceDescriptions.nope.url".to_string(),
                ),
            ]),
            ..Workflow::default()
        }],
        components: None,
    };
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("primary_url"), Some(&json!("http://localhost")));
    assert_eq!(
        result.get("secondary_url"),
        Some(&json!("https://api.example.com"))
    );
    assert_eq!(result.get("missing_url"), Some(&Value::Null));
}

#[tokio::test]
async fn multiple_source_descriptions_routing() {
    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "test".to_string(),
            summary: String::new(),
            version: "1.0.0".to_string(),
            description: String::new(),
        },
        source_descriptions: vec![
            SourceDescription {
                name: "api1".to_string(),
                url: "https://api1.example.com".to_string(),
                type_: SourceType::OpenApi,
            },
            SourceDescription {
                name: "api2".to_string(),
                url: "https://api2.example.com".to_string(),
                type_: SourceType::OpenApi,
            },
        ],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "s1".to_string(),
                    target: Some(StepTarget::OperationPath("{api1}./v1/users".to_string())),
                    outputs: BTreeMap::from([("url".to_string(), "$url".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s2".to_string(),
                    target: Some(StepTarget::OperationPath("{api2}./v2/items".to_string())),
                    outputs: BTreeMap::from([("url".to_string(), "$url".to_string())]),
                    ..Step::default()
                },
                Step {
                    step_id: "s3".to_string(),
                    target: Some(StepTarget::OperationPath("/v1/default".to_string())),
                    outputs: BTreeMap::from([("url".to_string(), "$url".to_string())]),
                    ..Step::default()
                },
            ],
            outputs: BTreeMap::from([
                ("url1".to_string(), "$steps.s1.outputs.url".to_string()),
                ("url2".to_string(), "$steps.s2.outputs.url".to_string()),
                ("url3".to_string(), "$steps.s3.outputs.url".to_string()),
            ]),
            ..Workflow::default()
        }],
        components: None,
    };
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(
        result.get("url1"),
        Some(&json!("https://api1.example.com/v1/users"))
    );
    assert_eq!(
        result.get("url2"),
        Some(&json!("https://api2.example.com/v2/items"))
    );
    assert_eq!(
        result.get("url3"),
        Some(&json!("https://api1.example.com/v1/default"))
    );
}

#[tokio::test]
async fn evaluate_output_expression_routes_dollar_expressions_correctly() {
    let spec = make_spec_with_base(
        "http://localhost",
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/test".to_string())),
                outputs: BTreeMap::from([
                    ("method_out".to_string(), "$method".to_string()),
                    ("input_out".to_string(), "$inputs.name".to_string()),
                ]),
                ..Step::default()
            }],
            outputs: BTreeMap::from([
                (
                    "method".to_string(),
                    "$steps.s1.outputs.method_out".to_string(),
                ),
                (
                    "input".to_string(),
                    "$steps.s1.outputs.input_out".to_string(),
                ),
            ]),
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec).dry_run(true).build() {
        Ok(e) => e,
        Err(err) => panic!("creating engine: {err}"),
    };
    let result = match engine
        .execute_collect("wf", BTreeMap::from([("name".to_string(), json!("Alice"))]))
        .await
        .outputs
    {
        Ok(r) => r,
        Err(err) => panic!("executing workflow: {err}"),
    };
    assert_eq!(result.get("method"), Some(&json!("GET")));
    assert_eq!(result.get("input"), Some(&json!("Alice")));
}
