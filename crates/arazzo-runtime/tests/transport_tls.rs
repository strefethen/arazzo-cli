//! Transport-trust policy tests for ac-fd376: scoped TLS exceptions,
//! downgrade refusal, redirect limits, trace chains, and transport
//! warning events. Hermetic: every server binds 127.0.0.1; the one
//! intentionally unreachable host uses TEST-NET-1 (RFC 5737) with a
//! sub-second whole-chain timeout.

mod common;

use arazzo_runtime::{
    ClientConfig, EngineBuilder, EngineEvent, RuntimeErrorKind, TraceStepRecord,
    TransportWarningKind,
};
use arazzo_spec::{
    ActionType, OnAction, ParamLocation, Parameter, Step, StepTarget, ValueSource, Workflow,
};
use common::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

fn host_port(base_url: &str) -> String {
    match base_url.split_once("://") {
        Some((_, rest)) => rest.trim_end_matches('/').to_string(),
        None => base_url.to_string(),
    }
}

fn insecure_config(entries: &[&str]) -> ClientConfig {
    ClientConfig {
        insecure_hosts: entries
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>(),
        ..ClientConfig::default()
    }
}

fn get_step(step_id: &str, target: &str) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath(target.to_string())),
        success_criteria: success_200(),
        ..Step::default()
    }
}

fn single_step_spec(base_url: &str, step: Step) -> arazzo_spec::ArazzoSpec {
    make_spec_with_base(
        base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![step],
            ..Workflow::default()
        }],
    )
}

// ── Scoped TLS exceptions ───────────────────────────────────────────

#[tokio::test]
async fn tls_self_signed_fails_by_default_with_actionable_error() {
    let server = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = single_step_spec(&server.base_url, get_step("s1", "/ok"));
    let engine = match EngineBuilder::new(spec).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("untrusted self-signed cert must fail by default"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    let expected_hint = format!("--insecure-host {}", host_port(&server.base_url));
    assert!(
        err.message.contains("certificate") && err.message.contains(&expected_hint),
        "error should name the certificate failure and the remedy: {}",
        err.message
    );
}

#[tokio::test]
async fn tls_insecure_host_exact_entry_allows_untrusted_cert() {
    let server = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let entry = host_port(&server.base_url);
    let spec = single_step_spec(&server.base_url, get_step("s1", "/ok"));
    let engine = match EngineBuilder::new(spec)
        .client_config(insecure_config(&[entry.as_str()]))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("--insecure-host {entry} should allow the untrusted cert: {err}");
    }
    assert!(
        engine.unused_insecure_hosts().is_empty(),
        "the exception was targeted, none should be unused"
    );
}

#[tokio::test]
async fn tls_bare_host_entry_matches_any_port() {
    let server = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = single_step_spec(&server.base_url, get_step("s1", "/ok"));
    let engine = match EngineBuilder::new(spec)
        .client_config(insecure_config(&["127.0.0.1"]))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    if let Err(err) = engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        panic!("bare-host exception should match any port: {err}");
    }
}

#[tokio::test]
async fn tls_blanket_insecure_all_allows_untrusted_cert() {
    let server = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let spec = single_step_spec(&server.base_url, get_step("s1", "/ok"));
    let engine = match EngineBuilder::new(spec)
        .client_config(ClientConfig {
            insecure_all: true,
            ..ClientConfig::default()
        })
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    if let Err(err) = engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        panic!("blanket --insecure should allow the untrusted cert: {err}");
    }
    assert!(
        engine.unused_insecure_hosts().is_empty(),
        "blanket exception reports no per-entry usage"
    );
}

#[tokio::test]
async fn tls_second_host_still_verified_in_same_run() {
    let excepted = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let verified = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let entry = host_port(&excepted.base_url);

    let spec = make_spec_with_base(
        &excepted.base_url,
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                get_step("excepted", "/ok"),
                get_step("verified", &format!("GET {}/ok", verified.base_url)),
            ],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec)
        .client_config(insecure_config(&[entry.as_str()]))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("the second, unlisted host must still enforce verification"),
        Err(err) => err,
    };
    assert!(
        err.message.contains("step verified") && err.message.contains("certificate"),
        "failure should come from the verified host's certificate: {}",
        err.message
    );
}

// ── Redirect policy ─────────────────────────────────────────────────

#[tokio::test]
async fn downgrade_refused_by_default_names_both_urls() {
    // Pre-policy baseline (charac_reqwest_tls_downgrade_followed_and_
    // credentials_stripped) followed this silently; the policy change
    // flips it to an explicit refusal.
    let target_log = new_request_log();
    let target_log_ref = std::sync::Arc::clone(&target_log);
    let target = start_server(move |method, url, headers, body| {
        record_request(&target_log_ref, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let downgrade_url = format!("{}/b", target.base_url);
    let downgrade_url_for_server = downgrade_url.clone();
    let tls = start_tls_server(move |_m, url, _h, _b| match url.as_str() {
        "/a" => MockHttpResponse::redirect(302, &downgrade_url_for_server),
        _ => MockHttpResponse::empty(404),
    });
    let entry = host_port(&tls.base_url);

    let spec = single_step_spec(&tls.base_url, get_step("s1", "/a"));
    let engine = match EngineBuilder::new(spec)
        .client_config(insecure_config(&[entry.as_str()]))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("https→http downgrade must be refused by default"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RedirectDowngradeRefused);
    assert!(
        err.message.contains(&format!("{}/a", tls.base_url))
            && err.message.contains(&downgrade_url),
        "refusal should name both URLs: {}",
        err.message
    );
    assert!(
        logged_requests(&target_log).is_empty(),
        "the http target must never be contacted on refusal"
    );
}

#[tokio::test]
async fn downgrade_followed_with_allow_flag_matches_stripping_baseline() {
    let target_log = new_request_log();
    let target_log_ref = std::sync::Arc::clone(&target_log);
    let target = start_server(move |method, url, headers, body| {
        record_request(&target_log_ref, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let downgrade_url = format!("{}/b", target.base_url);
    let downgrade_url_for_server = downgrade_url.clone();
    let tls = start_tls_server(move |_m, url, _h, _b| match url.as_str() {
        "/a" => MockHttpResponse::redirect(302, &downgrade_url_for_server),
        _ => MockHttpResponse::empty(404),
    });
    let entry = host_port(&tls.base_url);

    let step = Step {
        step_id: "s1".to_string(),
        target: Some(StepTarget::OperationPath("/a".to_string())),
        parameters: vec![
            Parameter {
                name: "Authorization".to_string(),
                in_: Some(ParamLocation::Header),
                value: ValueSource::Literal(to_yaml(json!("Bearer secret-token"))),
                ..Parameter::default()
            },
            Parameter {
                name: "X-Custom".to_string(),
                in_: Some(ParamLocation::Header),
                value: ValueSource::Literal(to_yaml(json!("keep-me"))),
                ..Parameter::default()
            },
        ],
        success_criteria: success_200(),
        ..Step::default()
    };
    let spec = single_step_spec(&tls.base_url, step);
    let engine = match EngineBuilder::new(spec)
        .client_config(ClientConfig {
            allow_downgrade_redirects: true,
            ..insecure_config(&[entry.as_str()])
        })
        .trace(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("--allow-downgrade-redirects should follow the hop: {err}");
    }

    let requests = logged_requests(&target_log);
    assert_eq!(requests.len(), 1, "downgrade hop reached the http target");
    // Same-host different-port downgrade strips credentials — the
    // empirically recorded reqwest baseline (Evidence item).
    assert_eq!(header_value(&requests[0].headers, "Authorization"), None);
    assert_eq!(
        header_value(&requests[0].headers, "X-Custom").as_deref(),
        Some("keep-me")
    );

    let steps: Vec<&TraceStepRecord> = result.trace_steps();
    let request = match steps.first().and_then(|s| s.request.as_ref()) {
        Some(request) => request,
        None => panic!("trace should record the step request"),
    };
    assert_eq!(request.redirects.len(), 1, "hop chain recorded in trace");
    assert_eq!(request.redirects[0].status_code, 302);
    assert!(request.redirects[0].from.contains("/a"));
    assert_eq!(request.redirects[0].to, downgrade_url);
}

#[tokio::test]
async fn redirect_chain_recorded_in_trace_for_plain_chain() {
    let server = start_server(|_m, url, _h, _b| match url.as_str() {
        "/a" => MockHttpResponse::redirect(302, "/b"),
        "/b" => MockHttpResponse::redirect(301, "/c"),
        "/c" => MockHttpResponse::json(200, r#"{"ok":true}"#),
        _ => MockHttpResponse::empty(404),
    });
    let spec = single_step_spec(&server.base_url, get_step("s1", "/a"));
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("chain should succeed: {err}");
    }
    let steps = result.trace_steps();
    let request = match steps.first().and_then(|s| s.request.as_ref()) {
        Some(request) => request,
        None => panic!("trace should record the step request"),
    };
    let summary: Vec<(i64, &str, &str)> = request
        .redirects
        .iter()
        .map(|hop| (hop.status_code, hop.from.as_str(), hop.to.as_str()))
        .collect();
    assert_eq!(request.redirects.len(), 2, "two hops recorded: {summary:?}");
    assert_eq!(request.redirects[0].status_code, 302);
    assert!(request.redirects[0].from.ends_with("/a"));
    assert!(request.redirects[0].to.ends_with("/b"));
    assert_eq!(request.redirects[1].status_code, 301);
    assert!(request.redirects[1].to.ends_with("/c"));
    // The recorded request URL stays the original target.
    assert!(request.url.ends_with("/a"));
}

#[tokio::test]
async fn max_redirects_config_enforced_with_named_hop() {
    let log = new_request_log();
    let log_ref = std::sync::Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log_ref, &method, &url, &headers, &body);
        let n: usize = url
            .strip_prefix("/hop/")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        MockHttpResponse::redirect(302, &format!("/hop/{}", n + 1))
    });
    let spec = single_step_spec(&server.base_url, get_step("s1", "/hop/0"));
    let engine = match EngineBuilder::new(spec)
        .client_config(ClientConfig {
            max_redirects: 2,
            ..ClientConfig::default()
        })
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let err = match engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        Ok(_) => panic!("chain must exceed max_redirects=2"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::RedirectLimitExceeded);
    assert!(
        err.message.contains("redirect limit of 2") && err.message.contains("/hop/3"),
        "error should name the limit and the refused hop: {}",
        err.message
    );
    assert_eq!(
        logged_requests(&log).len(),
        3,
        "original request plus two followed hops"
    );
}

// ── Transport warning events ────────────────────────────────────────

fn transport_warning_events(events: &[EngineEvent]) -> Vec<&arazzo_runtime::TransportWarning> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::TransportWarning(warning) => Some(warning),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn cleartext_credentials_warning_event_once_per_host() {
    // TEST-NET-1 (RFC 5737): guaranteed non-loopback and unroutable.
    // The requests fail; the warning fires before send.
    let step1 = Step {
        step_id: "s1".to_string(),
        target: Some(StepTarget::OperationPath("http://192.0.2.1/a".to_string())),
        parameters: vec![Parameter {
            name: "Authorization".to_string(),
            in_: Some(ParamLocation::Header),
            value: ValueSource::Literal(to_yaml(json!("Bearer secret-token"))),
            ..Parameter::default()
        }],
        success_criteria: success_200(),
        on_failure: vec![OnAction {
            name: "next".to_string(),
            type_: Some(ActionType::Goto),
            step_id: "s2".to_string(),
            ..OnAction::default()
        }],
        ..Step::default()
    };
    let step2 = Step {
        step_id: "s2".to_string(),
        target: Some(StepTarget::OperationPath("http://192.0.2.1/b".to_string())),
        parameters: vec![Parameter {
            name: "Authorization".to_string(),
            in_: Some(ParamLocation::Header),
            value: ValueSource::Literal(to_yaml(json!("Bearer secret-token"))),
            ..Parameter::default()
        }],
        success_criteria: success_200(),
        ..Step::default()
    };
    let spec = make_spec_with_base(
        "http://192.0.2.1",
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![step1, step2],
            ..Workflow::default()
        }],
    );
    let engine = match EngineBuilder::new(spec)
        .client_config(ClientConfig {
            timeout: Duration::from_millis(250),
            ..ClientConfig::default()
        })
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(result.outputs.is_err(), "TEST-NET requests must fail");

    let warnings = transport_warning_events(&result.events);
    assert_eq!(
        warnings.len(),
        1,
        "exactly one cleartext warning per host per run: {warnings:?}"
    );
    assert_eq!(warnings[0].kind, TransportWarningKind::CleartextCredentials);
    assert_eq!(warnings[0].hosts, vec!["192.0.2.1".to_string()]);
}

#[tokio::test]
async fn loopback_cleartext_credentials_exempt() {
    let server = start_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let step = Step {
        step_id: "s1".to_string(),
        target: Some(StepTarget::OperationPath("/ok".to_string())),
        parameters: vec![Parameter {
            name: "Authorization".to_string(),
            in_: Some(ParamLocation::Header),
            value: ValueSource::Literal(to_yaml(json!("Bearer secret-token"))),
            ..Parameter::default()
        }],
        success_criteria: success_200(),
        ..Step::default()
    };
    let spec = single_step_spec(&server.base_url, step);
    let engine = match EngineBuilder::new(spec).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("loopback request should succeed: {err}");
    }
    assert!(
        transport_warning_events(&result.events).is_empty(),
        "loopback hosts are exempt from the cleartext warning"
    );
}

#[tokio::test]
async fn replay_engine_emits_no_transport_warnings() {
    use arazzo_runtime::{TraceRequest, TraceResponse};

    let mut headers = BTreeMap::new();
    headers.insert("Authorization".to_string(), "Bearer secret".to_string());
    let record = TraceStepRecord {
        seq: 1,
        workflow_id: "wf".to_string(),
        step_id: "s1".to_string(),
        attempt: 1,
        kind: "http".to_string(),
        operation_path: "http://192.0.2.1/a".to_string(),
        duration_ms: 0,
        request: Some(TraceRequest {
            method: "GET".to_string(),
            url: "http://192.0.2.1/a".to_string(),
            headers,
            body: None,
            redirects: Vec::new(),
        }),
        response: Some(TraceResponse {
            status_code: 200,
            content_type: arazzo_runtime::ContentType::Json,
            headers: BTreeMap::new(),
            body_bytes: 11,
            body_preview: Some(r#"{"ok":true}"#.to_string()),
            body: Some(r#"{"ok":true}"#.to_string()),
            body_lossy: false,
        }),
        ..TraceStepRecord::default()
    };

    let step = Step {
        step_id: "s1".to_string(),
        target: Some(StepTarget::OperationPath("http://192.0.2.1/a".to_string())),
        parameters: vec![Parameter {
            name: "Authorization".to_string(),
            in_: Some(ParamLocation::Header),
            value: ValueSource::Literal(to_yaml(json!("Bearer secret"))),
            ..Parameter::default()
        }],
        success_criteria: success_200(),
        ..Step::default()
    };
    let spec = single_step_spec("http://192.0.2.1", step);
    let engine = match EngineBuilder::new(spec)
        .replay_trace_steps(vec![record])
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building replay engine: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("replay should succeed offline: {err}");
    }
    assert!(
        transport_warning_events(&result.events).is_empty(),
        "replay runs emit no transport warnings"
    );
}

#[tokio::test]
async fn unused_insecure_entries_reported_after_run() {
    let server = start_tls_server(|_m, _u, _h, _b| MockHttpResponse::json(200, r#"{"ok":true}"#));
    let used_entry = host_port(&server.base_url);
    let spec = single_step_spec(&server.base_url, get_step("s1", "/ok"));
    let engine = match EngineBuilder::new(spec)
        .client_config(insecure_config(&[
            used_entry.as_str(),
            "stale.example:9443",
        ]))
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    if let Err(err) = engine.execute_collect("wf", BTreeMap::new()).await.outputs {
        panic!("run should succeed: {err}");
    }
    assert_eq!(
        engine.unused_insecure_hosts(),
        vec!["stale.example:9443".to_string()],
        "only the untargeted entry is reported unused"
    );
}
