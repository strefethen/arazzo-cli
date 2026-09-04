mod common;

use arazzo_runtime::{redact_text_patterns, EngineBuilder, ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{Step, StepTarget, SuccessCriterion, Workflow};
use common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const JSON_NESTED_SENTINEL: &str = "json-nested-secret-7zK";
const JSON_ARRAY_SENTINEL: &str = "json-array-secret-3mQ";
const MISLABELED_JSON_SENTINEL: &str = "mislabeled-json-secret-5qB";
const BEARER_SENTINEL: &str = "bearer-secret-8xT";
const BASIC_SENTINEL: &str = "basic-secret-4rV";
const KEY_VALUE_SENTINEL: &str = "key-value-secret-1pN";
const BOUNDARY_SENTINEL: &str = "boundary-secret-9dL";

fn workflow(expected_status: i64, outputs: BTreeMap<String, String>) -> Workflow {
    let workflow_outputs = outputs
        .keys()
        .map(|name| {
            (
                name.clone(),
                format!("$steps.credential-response.outputs.{name}").into(),
            )
        })
        .collect();
    Workflow {
        workflow_id: "failure-redaction".to_string(),
        steps: vec![Step {
            step_id: "credential-response".to_string(),
            target: Some(StepTarget::OperationPath("/credential".to_string())),
            success_criteria: vec![SuccessCriterion {
                condition: format!("$statusCode == {expected_status}"),
                ..SuccessCriterion::default()
            }],
            outputs: outputs
                .into_iter()
                .map(|(name, expression)| (name, expression.into()))
                .collect(),
            ..Step::default()
        }],
        outputs: workflow_outputs,
        ..Workflow::default()
    }
}

async fn execute_response(
    response: MockHttpResponse,
    expected_status: i64,
    outputs: BTreeMap<String, String>,
) -> ExecutionResult {
    let server = start_server(move |_method, _url, _headers, _body| response.clone());
    let spec = make_spec_with_base(&server.base_url, vec![workflow(expected_status, outputs)]);
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    engine
        .execute_collect("failure-redaction", BTreeMap::new())
        .await
}

fn assert_redacted_failure(result: &ExecutionResult, sentinels: &[&str]) {
    let err = match &result.outputs {
        Ok(_) => panic!("expected failed success criterion"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed);
    assert_eq!(err.code(), "RUNTIME_SUCCESS_CRITERIA_FAILED");

    let trace = result.trace_steps();
    assert_eq!(trace.len(), 1);
    let trace_error = match &trace[0].error {
        Some(error) => error,
        None => panic!("missing failure trace error"),
    };

    for text in [&err.to_string(), trace_error] {
        for sentinel in sentinels {
            assert!(
                !text
                    .as_bytes()
                    .windows(sentinel.len())
                    .any(|bytes| bytes == sentinel.as_bytes()),
                "sentinel leaked in complete error bytes: {text}"
            );
            for fragment in sentinel.as_bytes().windows(8) {
                let fragment = match std::str::from_utf8(fragment) {
                    Ok(fragment) => fragment,
                    Err(err) => panic!("ASCII test sentinel must be valid UTF-8: {err}"),
                };
                assert!(
                    !text.contains(fragment),
                    "partial sentinel leaked in complete error bytes: {text}"
                );
            }
        }
    }
}

#[tokio::test]
async fn failed_json_criterion_redacts_nested_fields_in_terminal_and_trace_errors() {
    let body = json!({
        "safe": "naïve café",
        "nested": { "accessToken": JSON_NESTED_SENTINEL },
        "items": [{ "password": JSON_ARRAY_SENTINEL }]
    })
    .to_string();
    let result = execute_response(MockHttpResponse::json(200, &body), 201, BTreeMap::new()).await;

    assert_redacted_failure(&result, &[JSON_NESTED_SENTINEL, JSON_ARRAY_SENTINEL]);
    let err = match &result.outputs {
        Ok(_) => panic!("expected failed success criterion"),
        Err(err) => err,
    };
    assert!(err.message.contains("status=200"), "error: {err}");
    assert!(err.message.contains("naïve café"), "error: {err}");
    assert!(err.message.is_char_boundary(err.message.len()));
    assert!(err.message.contains("[REDACTED]"), "error: {err}");
}

#[tokio::test]
async fn failed_mislabeled_json_criterion_redacts_structured_fields_in_errors() {
    let body = json!({ "token": MISLABELED_JSON_SENTINEL }).to_string();
    let result = execute_response(
        MockHttpResponse {
            status: 200,
            headers: BTreeMap::from([(
                "Content-Type".to_string(),
                "application/rss+xml".to_string(),
            )]),
            body,
        },
        201,
        BTreeMap::new(),
    )
    .await;

    assert_redacted_failure(&result, &[MISLABELED_JSON_SENTINEL]);
}

#[tokio::test]
async fn failed_non_json_criterion_redacts_patterns_before_preview_truncation() {
    let prefix = format!(
        "Bearer {BEARER_SENTINEL} Basic {BASIC_SENTINEL} api_key={KEY_VALUE_SENTINEL} safe="
    );
    let raw_padding_len = 500 - prefix.len() - " token=".len() - BOUNDARY_SENTINEL.len() / 2;
    let before_multibyte = format!(
        "{prefix}{} token={BOUNDARY_SENTINEL}",
        "x".repeat(raw_padding_len)
    );
    assert!(before_multibyte
        .find(BOUNDARY_SENTINEL)
        .is_some_and(|index| index < 500));
    assert!(before_multibyte
        .find(BOUNDARY_SENTINEL)
        .is_some_and(|index| index + BOUNDARY_SENTINEL.len() > 500));
    let before_multibyte = format!("{before_multibyte} safe-tail=");
    let redacted_before_multibyte = redact_text_patterns(&before_multibyte);
    let multibyte_padding_len = 499 - redacted_before_multibyte.len();
    let body = format!(
        "{before_multibyte}{}é trailing",
        "x".repeat(multibyte_padding_len)
    );
    let redacted_body = redact_text_patterns(&body);
    assert!(redacted_body.len() > 500);
    assert!(redacted_body.is_char_boundary(499));
    assert!(!redacted_body.is_char_boundary(500));
    let expected_preview = format!("{}...", &redacted_body[..499]);

    let result = execute_response(
        MockHttpResponse {
            status: 200,
            headers: BTreeMap::new(),
            body,
        },
        201,
        BTreeMap::new(),
    )
    .await;

    assert_redacted_failure(
        &result,
        &[
            BEARER_SENTINEL,
            BASIC_SENTINEL,
            KEY_VALUE_SENTINEL,
            BOUNDARY_SENTINEL,
        ],
    );
    let err = match &result.outputs {
        Ok(_) => panic!("expected failed success criterion"),
        Err(err) => err,
    };
    assert_eq!(
        err.message,
        format!(
            "step credential-response: success criteria not met (status=200, body={expected_preview})"
        )
    );
}

#[tokio::test]
async fn successful_outputs_keep_the_original_response_value() {
    let sentinel = "successful-output-secret-2wH";
    let result = execute_response(
        MockHttpResponse::json(200, &json!({ "token": sentinel }).to_string()),
        200,
        BTreeMap::from([("token".to_string(), "$response.body.token".to_string())]),
    )
    .await;

    let outputs = match result.outputs {
        Ok(outputs) => outputs,
        Err(err) => panic!("successful workflow failed: {err}"),
    };
    assert_eq!(
        outputs.get("token"),
        Some(&Value::String(sentinel.to_string()))
    );
}
