//! Characterization tests pinning the redirect behavior the runtime
//! inherited from reqwest 0.12 before transport policy became explicit
//! (ticket ac-fd376). These landed green against the pre-policy engine;
//! the manual follow loop must keep matching this baseline wherever
//! policy does not deliberately diverge.
//!
//! Hermetic: every server binds 127.0.0.1 only.

mod common;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{ParamLocation, Parameter, RequestBody, Step, StepTarget, ValueSource, Workflow};
use common::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn header_param(name: &str, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Header),
        value: ValueSource::Literal(to_yaml(json!(value))),
        ..Parameter::default()
    }
}

fn cookie_param(name: &str, value: &str) -> Parameter {
    Parameter {
        name: name.to_string(),
        in_: Some(ParamLocation::Cookie),
        value: ValueSource::Literal(to_yaml(json!(value))),
        ..Parameter::default()
    }
}

fn one_step_workflow(target: &str, parameters: Vec<Parameter>, body: Option<Value>) -> Workflow {
    Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath(target.to_string())),
            parameters,
            request_body: body.map(|payload| RequestBody {
                content_type: "application/json".to_string(),
                payload: Some(ValueSource::Literal(to_yaml(payload))),
                ..RequestBody::default()
            }),
            success_criteria: success_200(),
            ..Step::default()
        }],
        ..Workflow::default()
    }
}

async fn run_one_step(
    base_url: &str,
    target: &str,
    parameters: Vec<Parameter>,
    body: Option<Value>,
) -> Result<BTreeMap<String, Value>, arazzo_runtime::RuntimeError> {
    let spec = make_spec_with_base(base_url, vec![one_step_workflow(target, parameters, body)]);
    let engine = match EngineBuilder::new(spec).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    engine.execute_collect("wf", BTreeMap::new()).await.outputs
}

// ── Same-host chains ────────────────────────────────────────────────

#[tokio::test]
async fn charac_same_host_302_chain_followed_silently() {
    let log = new_request_log();
    let log_ref = std::sync::Arc::clone(&log);
    // Chain covers both relative and absolute Location resolution.
    let server = start_server_with_base(move |base, method, url, headers, body| {
        record_request(&log_ref, &method, &url, &headers, &body);
        match url.as_str() {
            "/a" => MockHttpResponse::redirect(302, "/b"),
            "/b" => MockHttpResponse::redirect(302, &format!("{base}/c")),
            "/c" => MockHttpResponse::json(200, r#"{"ok":true}"#),
            _ => MockHttpResponse::empty(404),
        }
    });

    let outputs = run_one_step(&server.base_url, "/a", Vec::new(), None).await;
    if let Err(err) = outputs {
        panic!("chain should succeed: {err}");
    }
    let seen: Vec<String> = logged_requests(&log)
        .iter()
        .map(|r| r.url.clone())
        .collect();
    assert_eq!(seen, vec!["/a", "/b", "/c"], "silent follow of both hops");
}

#[tokio::test]
async fn charac_301_302_303_convert_post_to_get_and_drop_body() {
    for status in [301_u16, 302, 303] {
        let log = new_request_log();
        let log_ref = std::sync::Arc::clone(&log);
        let server = start_server(move |method, url, headers, body| {
            record_request(&log_ref, &method, &url, &headers, &body);
            match url.as_str() {
                "/a" => MockHttpResponse::redirect(status, "/b"),
                "/b" => MockHttpResponse::json(200, r#"{"ok":true}"#),
                _ => MockHttpResponse::empty(404),
            }
        });

        let outputs = run_one_step(
            &server.base_url,
            "POST /a",
            Vec::new(),
            Some(json!({"k":"v"})),
        )
        .await;
        if let Err(err) = outputs {
            panic!("{status} chain should succeed: {err}");
        }
        let requests = logged_requests(&log);
        assert_eq!(requests.len(), 2, "{status}: one hop expected");
        assert_eq!(requests[0].method, "POST", "{status}: original method");
        assert_eq!(
            requests[1].method, "GET",
            "{status}: method converts to GET"
        );
        assert_eq!(requests[1].body, "", "{status}: body dropped on hop");
        assert_eq!(
            header_value(&requests[1].headers, "Content-Type"),
            None,
            "{status}: content-type dropped on hop"
        );
    }
}

#[tokio::test]
async fn charac_307_308_preserve_method_and_body() {
    for status in [307_u16, 308] {
        let log = new_request_log();
        let log_ref = std::sync::Arc::clone(&log);
        let server = start_server(move |method, url, headers, body| {
            record_request(&log_ref, &method, &url, &headers, &body);
            match url.as_str() {
                "/a" => MockHttpResponse::redirect(status, "/b"),
                "/b" => MockHttpResponse::json(200, r#"{"ok":true}"#),
                _ => MockHttpResponse::empty(404),
            }
        });

        let outputs = run_one_step(
            &server.base_url,
            "POST /a",
            Vec::new(),
            Some(json!({"k":"v"})),
        )
        .await;
        if let Err(err) = outputs {
            panic!("{status} chain should succeed: {err}");
        }
        let requests = logged_requests(&log);
        assert_eq!(requests.len(), 2, "{status}: one hop expected");
        assert_eq!(requests[1].method, "POST", "{status}: method preserved");
        assert_eq!(requests[1].body, r#"{"k":"v"}"#, "{status}: body preserved");
        assert_eq!(
            header_value(&requests[1].headers, "Content-Type").as_deref(),
            Some("application/json"),
            "{status}: content-type preserved"
        );
    }
}

// ── Sensitive header propagation ────────────────────────────────────

#[tokio::test]
async fn charac_auth_cookie_kept_on_same_host_hop() {
    let log = new_request_log();
    let log_ref = std::sync::Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log_ref, &method, &url, &headers, &body);
        match url.as_str() {
            "/a" => MockHttpResponse::redirect(302, "/b"),
            "/b" => MockHttpResponse::json(200, r#"{"ok":true}"#),
            _ => MockHttpResponse::empty(404),
        }
    });

    let params = vec![
        header_param("Authorization", "Bearer secret-token"),
        cookie_param("sid", "abc123"),
    ];
    let outputs = run_one_step(&server.base_url, "/a", params, None).await;
    if let Err(err) = outputs {
        panic!("chain should succeed: {err}");
    }
    let requests = logged_requests(&log);
    assert_eq!(requests.len(), 2);
    assert_eq!(
        header_value(&requests[1].headers, "Authorization").as_deref(),
        Some("Bearer secret-token"),
        "authorization kept on same-host hop"
    );
    assert_eq!(
        header_value(&requests[1].headers, "Cookie").as_deref(),
        Some("sid=abc123"),
        "cookie kept on same-host hop"
    );
}

#[tokio::test]
async fn charac_auth_cookie_stripped_on_cross_port_hop() {
    // Same IP, different port: reqwest treats a port change as cross-host
    // and strips credential headers while keeping custom headers.
    let target_log = new_request_log();
    let target_log_ref = std::sync::Arc::clone(&target_log);
    let target = start_server(move |method, url, headers, body| {
        record_request(&target_log_ref, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let target_hop_url = format!("{}/b", target.base_url);
    let origin = start_server(move |_method, url, _headers, _body| match url.as_str() {
        "/a" => MockHttpResponse::redirect(302, &target_hop_url),
        _ => MockHttpResponse::empty(404),
    });

    let params = vec![
        header_param("Authorization", "Bearer secret-token"),
        header_param("X-Custom", "keep-me"),
        cookie_param("sid", "abc123"),
    ];
    let outputs = run_one_step(&origin.base_url, "/a", params, None).await;
    if let Err(err) = outputs {
        panic!("cross-port chain should succeed: {err}");
    }
    let requests = logged_requests(&target_log);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header_value(&requests[0].headers, "Authorization"),
        None,
        "authorization stripped on cross-port hop"
    );
    assert_eq!(
        header_value(&requests[0].headers, "Cookie"),
        None,
        "cookie stripped on cross-port hop"
    );
    assert_eq!(
        header_value(&requests[0].headers, "X-Custom").as_deref(),
        Some("keep-me"),
        "custom header kept on cross-port hop"
    );
}

#[tokio::test]
async fn charac_referer_set_on_same_scheme_hop() {
    let log = new_request_log();
    let log_ref = std::sync::Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&log_ref, &method, &url, &headers, &body);
        match url.as_str() {
            "/a" => MockHttpResponse::redirect(302, "/b"),
            "/b" => MockHttpResponse::json(200, r#"{"ok":true}"#),
            _ => MockHttpResponse::empty(404),
        }
    });

    let outputs = run_one_step(&server.base_url, "/a", Vec::new(), None).await;
    if let Err(err) = outputs {
        panic!("chain should succeed: {err}");
    }
    let requests = logged_requests(&log);
    assert_eq!(requests.len(), 2);
    let expected_referer = format!("{}/a", server.base_url);
    assert_eq!(
        header_value(&requests[1].headers, "Referer").as_deref(),
        Some(expected_referer.as_str()),
        "reqwest default referer(true) stamps the previous hop URL"
    );
}

// ── Redirect limit ──────────────────────────────────────────────────

#[tokio::test]
async fn charac_default_redirect_limit_is_ten() {
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

    let err = match run_one_step(&server.base_url, "/hop/0", Vec::new(), None).await {
        Ok(_) => panic!("endless chain must exceed the redirect limit"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::HttpRequest);
    assert!(
        err.message.contains("redirect"),
        "error should mention redirects: {}",
        err.message
    );
    // Original request + 10 followed redirects; the 11th redirect is refused.
    assert_eq!(logged_requests(&log).len(), 11, "default limit is 10 hops");
}

// ── TLS downgrade (raw reqwest — the pre-policy transport layer) ────

/// Evidence item for ac-fd376: pins reqwest 0.12's sensitive-header
/// behavior on a same-host https→http downgrade redirect. The pre-policy
/// engine cannot reach a hermetic TLS origin (self-signed certs hard-fail
/// with no override), so this characterizes reqwest itself — the exact
/// client the engine delegated redirect handling to.
#[tokio::test]
async fn charac_reqwest_tls_downgrade_followed_and_credentials_stripped() {
    let target_log = new_request_log();
    let target_log_ref = std::sync::Arc::clone(&target_log);
    let target = start_server(move |method, url, headers, body| {
        record_request(&target_log_ref, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"ok":true}"#)
    });
    let downgrade_url = format!("{}/b", target.base_url);
    let tls = start_tls_server(move |_method, url, _headers, _body| match url.as_str() {
        "/a" => MockHttpResponse::redirect(302, &downgrade_url),
        _ => MockHttpResponse::empty(404),
    });

    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(client) => client,
        Err(err) => panic!("building raw reqwest client: {err}"),
    };
    let response = match client
        .get(format!("{}/a", tls.base_url))
        .header("Authorization", "Bearer secret-token")
        .header("Cookie", "sid=abc123")
        .header("X-Custom", "keep-me")
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => panic!("downgrade chain should be silently followed today: {err}"),
    };
    assert_eq!(response.status().as_u16(), 200);

    let requests = logged_requests(&target_log);
    assert_eq!(requests.len(), 1, "downgrade hop reached the http target");
    assert_eq!(
        header_value(&requests[0].headers, "Authorization"),
        None,
        "same-host (different-port) https→http downgrade strips Authorization"
    );
    assert_eq!(
        header_value(&requests[0].headers, "Cookie"),
        None,
        "same-host (different-port) https→http downgrade strips Cookie"
    );
    assert_eq!(
        header_value(&requests[0].headers, "X-Custom").as_deref(),
        Some("keep-me"),
        "custom header survives the downgrade hop"
    );
    assert_eq!(
        header_value(&requests[0].headers, "Referer"),
        None,
        "no Referer is stamped on an https→http downgrade hop"
    );
}
