mod common;

use common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

async fn route(branch: &str, local: Value, inherited: Value, components: Value) -> Vec<String> {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let failure = branch == "onFailure";
    let server = start_server(move |_, path, _, _| {
        captured
            .lock()
            .unwrap_or_else(|e| panic!("ledger: {e}"))
            .push(path.clone());
        MockHttpResponse::json(
            if failure && path == "/start" {
                500
            } else {
                200
            },
            "{}",
        )
    });
    let workflow_branch = if failure {
        "failureActions"
    } else {
        "successActions"
    };
    let mut spec = json!({
        "arazzo":"1.1.0", "info":{"title":"Action inheritance","version":"1"},
        "sourceDescriptions":[{"name":"test","url":server.base_url,"type":"openapi"}],
        "components": components,
        "workflows":[{"workflowId":"main", "steps":[
            {"stepId":"start","operationPath":"{$sourceDescriptions.test.url}#/paths/~1start/get",
             "successCriteria":[{"condition":"$statusCode == 200"}]},
            {"stepId":"never","operationPath":"{$sourceDescriptions.test.url}#/paths/~1never/get"},
            {"stepId":"recovered","operationPath":"{$sourceDescriptions.test.url}#/paths/~1recovered/get",
             "onSuccess":[{"name":"stop","type":"end"}]}
        ]}]
    });
    spec["workflows"][0][workflow_branch] = inherited;
    spec["workflows"][0]["steps"][0][branch] = local;
    let spec = arazzo_validate::parse_bytes(spec.to_string().as_bytes())
        .unwrap_or_else(|e| panic!("parse: {e}"));
    arazzo_validate::validate(&spec).unwrap_or_else(|e| panic!("validate: {e}"));
    let engine = fixture_engine_builder(spec)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));
    assert!(engine
        .execute_collect("main", BTreeMap::new())
        .await
        .outputs
        .is_ok());
    let result = requests
        .lock()
        .unwrap_or_else(|e| panic!("ledger: {e}"))
        .clone();
    result
}

#[tokio::test]
async fn differently_named_defaults_survive_nonmatching_local_actions_on_both_branches() {
    for branch in ["onSuccess", "onFailure"] {
        let requests = route(
            branch,
            json!([{"name":"local","type":"end","criteria":[{"condition":"false"}]}]),
            json!([{"name":"inherited","type":"goto","stepId":"recovered"},
                   {"name":"last","type":"end"}]),
            json!({}),
        )
        .await;
        assert_eq!(requests, ["/start", "/recovered"]);
    }
}

#[tokio::test]
async fn same_name_replaces_even_when_local_criteria_do_not_match() {
    for branch in ["onSuccess", "onFailure"] {
        let requests = route(
            branch,
            json!([{"name":"replace","type":"end","criteria":[{"condition":"false"}]}]),
            json!([{"name":"replace","type":"end"},
                   {"name":"fallback","type":"goto","stepId":"recovered"}]),
            json!({}),
        )
        .await;
        assert_eq!(requests, ["/start", "/recovered"]);
    }
}

#[tokio::test]
async fn names_are_case_sensitive_on_both_branches() {
    for branch in ["onSuccess", "onFailure"] {
        let requests = route(
            branch,
            json!([{"name":"Cleanup","type":"end","criteria":[{"condition":"false"}]}]),
            json!([{"name":"cleanup","type":"goto","stepId":"recovered"}]),
            json!({}),
        )
        .await;
        assert_eq!(requests, ["/start", "/recovered"]);
    }
}

#[tokio::test]
async fn first_matching_local_action_precedes_matching_inherited_actions() {
    for branch in ["onSuccess", "onFailure"] {
        let requests = route(
            branch,
            json!([{"name":"skip","type":"end","criteria":[{"condition":"false"}]},
                   {"name":"local","type":"goto","stepId":"recovered"},
                   {"name":"later","type":"end"}]),
            json!([{"name":"inherited","type":"end"}]),
            json!({}),
        )
        .await;
        assert_eq!(requests, ["/start", "/recovered"]);
    }
}

#[tokio::test]
async fn reusable_names_are_resolved_before_replacement_on_both_branches() {
    for branch in ["onSuccess", "onFailure"] {
        let section = if branch == "onSuccess" {
            "successActions"
        } else {
            "failureActions"
        };
        let mut components = json!({});
        components[section] = json!({"local":{"name":"replace","type":"end","criteria":[{"condition":"false"}]},
                                     "default":{"name":"replace","type":"end"},
                                     "fallback":{"name":"fallback","type":"goto","stepId":"recovered"}});
        let requests = route(
            branch,
            json!([{"reference":format!("$components.{section}.local")}]),
            json!([{"reference":format!("$components.{section}.default")},
                   {"reference":format!("$components.{section}.fallback")}]),
            components,
        )
        .await;
        assert_eq!(requests, ["/start", "/recovered"]);
    }
}

#[tokio::test]
async fn exhausted_local_and_inherited_retries_keep_separate_counters_then_cleanup() {
    let requests = route(
        "onFailure",
        json!([{"name":"local-retry","type":"retry","retryLimit":1,"retryAfter":0}]),
        json!([{"name":"default-retry","type":"retry","retryLimit":1,"retryAfter":0},
               {"name":"cleanup","type":"goto","stepId":"recovered"}]),
        json!({}),
    )
    .await;
    assert_eq!(requests, ["/start", "/start", "/start", "/recovered"]);
}
