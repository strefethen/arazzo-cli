#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, ExecutionResult};
use arazzo_spec::{
    ActionType, CriterionType, OnAction, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::*;

fn criterion(condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        condition: condition.to_owned(),
        ..SuccessCriterion::default()
    }
}

fn step(id: &str) -> Step {
    Step {
        step_id: id.to_owned(),
        target: Some(StepTarget::OperationPath(format!("GET /{id}"))),
        ..Step::default()
    }
}

fn goto(id: &str, condition: &str) -> OnAction {
    OnAction {
        type_: Some(ActionType::Goto),
        step_id: id.to_owned(),
        criteria: vec![criterion(condition)],
        ..OnAction::default()
    }
}

async fn execute(steps: Vec<Step>) -> (ExecutionResult, Vec<String>) {
    let log = new_request_log();
    let recorder = log.clone();
    let server = start_server(move |method, url, headers, body| {
        record_request(&recorder, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"items":[{"id":7}],"nil":null,"number":42}"#)
    });
    let spec = make_spec_with_base(
        &server.base_url,
        vec![Workflow {
            workflow_id: "wf".to_owned(),
            steps,
            ..Workflow::default()
        }],
    );
    let engine = EngineBuilder::new(spec)
        .trace(true)
        .build()
        .expect("hermetic engine");
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    let paths = logged_requests(&log)
        .into_iter()
        .map(|request| request.url)
        .collect();
    (result, paths)
}

#[tokio::test]
async fn invalid_step_success_condition_allows_failure_routing() {
    let excessive = format!("{}true{}", "(".repeat(33), ")".repeat(33));
    for condition in [
        "true || (",
        excessive.as_str(),
        "1",
        "$response.body.nil.x == null",
        "!($response.body.number.x == 1)",
    ] {
        let mut original = step("original");
        original.success_criteria = vec![criterion(condition)];
        original.on_success = vec![goto("forbidden", "true")];
        original.on_failure = vec![goto("recovery", "$response.body.items[0].id == 7")];
        let mut recovery = step("recovery");
        recovery.on_success = vec![OnAction {
            type_: Some(ActionType::End),
            ..OnAction::default()
        }];
        let (result, paths) = execute(vec![original, step("forbidden"), recovery]).await;
        assert!(
            result.outputs.is_ok(),
            "failure recovery remains eligible: {condition}"
        );
        assert_eq!(paths, ["/original", "/recovery"], "{condition}");
        assert!(
            !result.trace_steps()[0].criteria[0].result,
            "invalid success criterion fails"
        );
    }
}

#[tokio::test]
async fn invalid_action_condition_skips_only_owning_action() {
    let excessive = format!("{}true{}", "(".repeat(33), ")".repeat(33));
    for condition in [
        "true || (",
        excessive.as_str(),
        "1",
        "$response.body.nil.x == null",
        "false",
    ] {
        for action_type in [ActionType::Goto, ActionType::Retry, ActionType::End] {
            let mut original = step("original");
            original.success_criteria = vec![criterion("true")];
            original.on_success = vec![
                OnAction {
                    type_: Some(action_type),
                    step_id: "forbidden".to_owned(),
                    retry_limit: Some(1),
                    criteria: vec![criterion(condition)],
                    ..OnAction::default()
                },
                goto("later", "$response.body.items[0].id == 7"),
            ];
            let mut later = step("later");
            later.on_success = vec![OnAction {
                type_: Some(ActionType::End),
                ..OnAction::default()
            }];
            let (result, paths) = execute(vec![original, step("forbidden"), later]).await;
            assert!(
                result.outputs.is_ok(),
                "later action remains eligible: {condition}"
            );
            assert_eq!(
                paths,
                ["/original", "/later"],
                "no owning-invalid-action side effect: {condition}"
            );
        }
    }
}

#[tokio::test]
async fn valid_true_false_omitted_simple_and_warning_controls() {
    for (condition, target) in [
        ("true", "success"),
        ("false", "failure"),
        (
            "$inputs.absent == null && $response.body.items[0].id == 7",
            "success",
        ),
    ] {
        for type_ in [None, Some(CriterionType::Name("simple".to_owned()))] {
            let mut original = step("original");
            let mut check = criterion(condition);
            check.type_ = type_;
            original.success_criteria = vec![check];
            original.on_success = vec![goto("success", "true")];
            original.on_failure = vec![goto("failure", "true")];
            let mut success = step("success");
            success.on_success = vec![OnAction {
                type_: Some(ActionType::End),
                ..OnAction::default()
            }];
            let (result, paths) = execute(vec![original, success, step("failure")]).await;
            assert!(result.outputs.is_ok());
            assert_eq!(paths, ["/original", &format!("/{target}")]);
            if condition.contains("absent") {
                assert!(result.trace_steps()[0]
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("input \"absent\" not found")));
            }
        }
    }
}
