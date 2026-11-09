mod common;

use arazzo_runtime::{ClientConfig, RuntimeError, RuntimeErrorKind};
use arazzo_spec::{ParamLocation, Parameter, Step, StepTarget, ValueSource, Workflow};
use common::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

const USERINFO_SECRET: &str = "secret";
const QUERY_SECRET: &str = "abc";

fn query_param(name: &str, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Query),
        value: ValueSource::Literal(to_yaml(json!(value))),
        ..Parameter::default()
    }
}

fn spec_for(base_url: &str, path: &str, parameters: Vec<Parameter>) -> arazzo_spec::ArazzoSpec {
    make_spec_with_base(
        base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath(format!(
                    "{{$sourceDescriptions.test.url}}#/paths/{path}/get"
                ))),
                parameters,
                success_criteria: success_200(),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
    )
}

async fn failed_run(spec: arazzo_spec::ArazzoSpec, config: ClientConfig) -> (RuntimeError, String) {
    let engine = common::fixture_engine_builder(spec)
        .client_config(config)
        .trace(true)
        .build()
        .unwrap_or_else(|err| panic!("building engine: {err}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    let trace_error = result
        .trace_steps()
        .first()
        .and_then(|step| step.error.clone())
        .unwrap_or_else(|| panic!("trace step must record the failure"));
    match result.outputs {
        Ok(_) => panic!("run must fail"),
        Err(err) => (err, trace_error),
    }
}

fn assert_no_secret(err: &RuntimeError, trace_error: &str) {
    for text in [err.message.as_str(), trace_error, &format!("{err:?}")] {
        assert!(!text.contains(USERINFO_SECRET), "userinfo leaked: {text}");
        assert!(!text.contains(QUERY_SECRET), "query secret leaked: {text}");
    }
}

fn insecure_for(base_url: &str) -> ClientConfig {
    let host_port = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .trim_end_matches('/')
        .to_string();
    ClientConfig {
        insecure_hosts: BTreeSet::from([host_port]),
        ..ClientConfig::default()
    }
}

#[tokio::test]
async fn connection_failure_redacts_userinfo_and_sensitive_query_value() {
    let spec = spec_for(
        "https://user:secret@127.0.0.1:1",
        "~1a",
        vec![query_param("api_key", QUERY_SECRET)],
    );
    let (err, trace_error) = failed_run(spec, ClientConfig::default()).await;

    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    assert_eq!(err.code(), "RUNTIME_HTTP_REQUEST");
    assert_no_secret(&err, &trace_error);
    let expected = "executing request: error sending request for url \
                    (https://[REDACTED]@127.0.0.1:1/a?api_key=[REDACTED])";
    for text in [&err.message, &trace_error] {
        assert_eq!(text, &format!("step s1: {expected}"));
    }
}

#[tokio::test]
async fn connection_failure_keeps_an_ordinary_url_verbatim() {
    let spec = spec_for("https://127.0.0.1:1", "~1a", vec![query_param("page", "2")]);
    let (err, trace_error) = failed_run(spec, ClientConfig::default()).await;

    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    let expected =
        "executing request: error sending request for url (https://127.0.0.1:1/a?page=2)";
    for text in [&err.message, &trace_error] {
        assert_eq!(text, &format!("step s1: {expected}"));
    }
}

#[tokio::test]
async fn redirect_limit_message_redacts_the_refused_url() {
    let base: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let base_for_server = Arc::clone(&base);
    let server = start_server(move |_method, url, _headers, _body| {
        let hop: usize = url
            .strip_prefix("/hop/")
            .and_then(|rest| rest.split('?').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let origin = base_for_server.get().cloned().unwrap_or_default();
        MockHttpResponse::redirect(
            302,
            &format!("{origin}/hop/{}?api_key={QUERY_SECRET}", hop + 1),
        )
    });
    let credentialed = server
        .base_url
        .replacen("http://", "http://user:secret@", 1);
    base.set(credentialed.clone())
        .unwrap_or_else(|_| panic!("redirect origin set once"));
    let spec = spec_for(&credentialed, "~1hop~10", Vec::new());
    let config = ClientConfig {
        max_redirects: 2,
        ..ClientConfig::default()
    };
    let (err, trace_error) = failed_run(spec, config).await;

    assert_eq!(err.kind, RuntimeErrorKind::RedirectLimitExceeded);
    assert_eq!(err.code(), "RUNTIME_REDIRECT_LIMIT_EXCEEDED");
    assert_no_secret(&err, &trace_error);
    let redacted_origin = server.base_url.replacen("http://", "http://[REDACTED]@", 1);
    assert_eq!(
        err.message,
        format!(
            "step s1: redirect limit of 2 exceeded at \"{redacted_origin}/hop/3?api_key=[REDACTED]\" \
             (raise --max-redirects to follow further)"
        )
    );
}

#[tokio::test]
async fn downgrade_refusal_redacts_both_urls() {
    let target = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let downgrade = format!(
        "{}/b?token={QUERY_SECRET}",
        target
            .base_url
            .replacen("http://", "http://user:secret@", 1)
    );
    let tls = start_tls_server(move |_m, url, _h, _b| {
        if url.starts_with("/a") {
            MockHttpResponse::redirect(302, &downgrade)
        } else {
            MockHttpResponse::empty(404)
        }
    });
    let credentialed = tls.base_url.replacen("https://", "https://user:secret@", 1);
    let spec = spec_for(
        &credentialed,
        "~1a",
        vec![query_param("api_key", QUERY_SECRET)],
    );
    let (err, trace_error) = failed_run(spec, insecure_for(&tls.base_url)).await;

    assert_eq!(err.kind, RuntimeErrorKind::RedirectDowngradeRefused);
    assert_eq!(err.code(), "RUNTIME_REDIRECT_DOWNGRADE_REFUSED");
    assert_no_secret(&err, &trace_error);
    let from = format!(
        "{}/a?api_key=[REDACTED]",
        tls.base_url.replacen("https://", "https://[REDACTED]@", 1)
    );
    let to = format!(
        "{}/b?token=[REDACTED]",
        target.base_url.replacen("http://", "http://[REDACTED]@", 1)
    );
    assert_eq!(
        err.message,
        format!(
            "step s1: refusing https→http downgrade redirect from \"{from}\" to \"{to}\" \
             (pass --allow-downgrade-redirects to follow it)"
        )
    );
}

#[tokio::test]
async fn downgrade_refusal_keeps_ordinary_urls_verbatim() {
    let target = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let downgrade = format!("{}/b?page=2", target.base_url);
    let downgrade_for_server = downgrade.clone();
    let tls = start_tls_server(move |_m, url, _h, _b| {
        if url.starts_with("/a") {
            MockHttpResponse::redirect(302, &downgrade_for_server)
        } else {
            MockHttpResponse::empty(404)
        }
    });
    let spec = spec_for(&tls.base_url, "~1a", Vec::new());
    let (err, _) = failed_run(spec, insecure_for(&tls.base_url)).await;

    assert_eq!(err.kind, RuntimeErrorKind::RedirectDowngradeRefused);
    assert!(
        err.message
            .contains(&format!("from \"{}/a\" to \"{downgrade}\"", tls.base_url)),
        "{}",
        err.message
    );
}
