use super::*;

pub(super) fn validate_replay_request(
    expected: &TraceRequest,
    actual: &RequestConfig,
    logical_body: Option<&Value>,
    seq: u64,
    attempt: u32,
) -> Result<(), RuntimeError> {
    let mut expected = expected.clone();
    let mut actual = TraceRequest {
        method: actual.method.clone(),
        url: actual.url.clone(),
        headers: actual.headers.clone(),
        body: logical_body.cloned(),
        redirects: Vec::new(),
    };
    redact_trace_request(&mut expected);
    redact_trace_request(&mut actual);
    let category = if !expected.method.eq_ignore_ascii_case(&actual.method) {
        Some("method")
    } else if expected.url != actual.url {
        Some("url")
    } else if expected.headers != actual.headers {
        Some("headers")
    } else if expected.body != actual.body {
        Some("body")
    } else {
        None
    };
    match category {
        Some(category) => Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!("replay request drift at seq {seq} attempt {attempt}: {category} mismatch"),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RequestConfig {
        RequestConfig {
            workflow_id: "wf".into(),
            step_id: "s1".into(),
            method: "GET".into(),
            url: "https://example.invalid/path?token=actual-secret&safe=actual-canary".into(),
            headers: BTreeMap::from([
                ("Authorization".into(), "Bearer actual-secret".into()),
                ("ordinary-actual-name".into(), "actual-header-canary".into()),
            ]),
            body: Some(b"ignored wire bytes".to_vec()),
        }
    }

    fn expected(cfg: &RequestConfig, body: Option<Value>) -> TraceRequest {
        TraceRequest {
            method: cfg.method.clone(),
            url: cfg.url.clone(),
            headers: cfg.headers.clone(),
            body,
            redirects: Vec::new(),
        }
    }

    #[test]
    fn method_ascii_case_and_sensitive_values() {
        let cfg = config();
        let body = json!({"nested":[{"password":"actual-secret","safe":42}]});
        let mut trace = expected(&cfg, Some(body.clone()));
        trace.method = "get".into();
        trace.url = trace.url.replace("actual-secret", "different-secret");
        trace
            .headers
            .insert("Authorization".into(), "[REDACTED]".into());
        trace.body = Some(json!({"nested":[{"password":"[REDACTED]","safe":42}]}));
        assert!(validate_replay_request(&trace, &cfg, Some(&body), 3, 2).is_ok());
        trace.method = "POST".into();
        assert_eq!(
            validate_replay_request(&trace, &cfg, Some(&body), 3, 2)
                .err()
                .unwrap_or_else(|| panic!("expected replay mismatch"))
                .message,
            "replay request drift at seq 3 attempt 2: method mismatch"
        );
    }

    #[test]
    fn logical_body_comparison_does_not_collapse_types_or_presence() {
        let cfg = config();
        for (a, b) in [
            (None, Some(Value::Null)),
            (Some(json!({})), Some(json!({"a":null}))),
            (Some(json!({})), Some(json!({"a":[]}))),
            (Some(json!({"a":42})), Some(json!({"a":"42"}))),
            (Some(json!({"a":["x",null]})), Some(json!({"a":["x"]}))),
            (Some(json!({"password":"hidden"})), Some(json!({}))),
            (Some(json!({"a":"[REDACTED]"})), Some(json!({"a":"real"}))),
            (Some(json!("42")), Some(json!(42))),
            (Some(json!("a+b")), Some(json!("a%20b"))),
            (Some(json!("password=one")), Some(json!("password=two"))),
            (Some(json!("Bearer one")), Some(json!("Bearer two"))),
        ] {
            for (left, right) in [(&a, &b), (&b, &a)] {
                assert!(
                    validate_replay_request(
                        &expected(&cfg, left.clone()),
                        &cfg,
                        right.as_ref(),
                        1,
                        1
                    )
                    .is_err(),
                    "{left:?} versus {right:?}"
                );
            }
        }
    }

    #[test]
    fn diagnostics_contain_only_fixed_categories_and_position() {
        let cfg = config();
        for category in ["method", "url", "headers", "body"] {
            let mut trace = expected(
                &cfg,
                Some(json!({"expected-body-name":"expected-body-canary"})),
            );
            let mut actual = cfg.clone();
            let body = json!({"actual-body-name":"actual-body-canary"});
            match category {
                "method" => {
                    trace.method = "expected-method-canary".into();
                    actual.method = "actual-method-canary".into();
                }
                "url" => {
                    trace.url =
                        "https://expected-path-canary.invalid/?safe=expected-query-canary".into()
                }
                "headers" => {
                    trace.headers = BTreeMap::from([(
                        "expected-header-name".into(),
                        "expected-header-canary".into(),
                    )]);
                }
                _ => {}
            }
            let err = validate_replay_request(&trace, &actual, Some(&body), 99, 7)
                .err()
                .unwrap_or_else(|| panic!("expected replay mismatch"));
            assert_eq!(err.kind, RuntimeErrorKind::ReplayRequestMismatch);
            assert_eq!(
                err.message,
                format!("replay request drift at seq 99 attempt 7: {category} mismatch")
            );
            let displayed = err.to_string();
            for canary in ["expected-", "actual-", "Bearer", "secret", "password"] {
                assert!(!displayed.contains(canary), "{displayed}");
            }
        }
    }

    #[test]
    fn sensitive_and_ordinary_header_and_query_presence_is_exact() {
        let cfg = config();
        for name in ["Authorization", "ordinary-actual-name"] {
            let mut trace = expected(&cfg, None);
            trace.headers.remove(name);
            assert!(validate_replay_request(&trace, &cfg, None, 1, 1).is_err());
        }
        for url in [
            "https://example.invalid/path?safe=actual-canary",
            "https://example.invalid/path?token=actual-secret&safe=[REDACTED]",
            "https://example.invalid/other?token=actual-secret&safe=actual-canary",
            "https://example.invalid/path?token=actual-secret&safe=actual%2Dcanary",
        ] {
            let mut trace = expected(&cfg, None);
            trace.url = url.into();
            assert!(validate_replay_request(&trace, &cfg, None, 1, 1).is_err());
        }
    }
}

#[cfg(test)]
mod persisted_body_tests {
    use super::*;

    #[test]
    fn persisted_null_and_missing_body_remain_distinct() {
        for body in [
            None,
            Some(Value::Null),
            Some(json!("null")),
            Some(json!({})),
        ] {
            let trace = TraceRequest {
                method: "POST".into(),
                url: "https://example.invalid".into(),
                body: body.clone(),
                ..TraceRequest::default()
            };
            let serialized = serde_json::to_value(&trace).unwrap_or_else(|err| panic!("{err}"));
            assert_eq!(serialized.get("body").is_some(), body.is_some());
            let loaded: TraceRequest =
                serde_json::from_value(serialized).unwrap_or_else(|err| panic!("{err}"));
            assert_eq!(loaded.body, body);
        }
    }
}
