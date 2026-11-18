use super::*;
use request_body::LogicalBody;

pub(super) fn validate_replay_request(
    expected: &TraceRequest,
    actual: &RequestConfig,
    logical_body: &LogicalBody,
    seq: u64,
    attempt: u32,
) -> Result<(), RuntimeError> {
    let mut expected = expected.clone();
    let mut actual = TraceRequest {
        method: actual.method.clone(),
        url: actual.url.clone(),
        headers: actual.headers.clone(),
        body: logical_body.json().cloned(),
        body_blobs: logical_body.blobs(),
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
    } else if expected.body != actual.body || !same_blobs(&expected.body_blobs, &actual.body_blobs)
    {
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

fn same_blobs(
    expected: &BTreeMap<String, BytesDescriptor>,
    actual: &BTreeMap<String, BytesDescriptor>,
) -> bool {
    expected.len() == actual.len()
        && expected
            .iter()
            .zip(actual)
            .all(|((expected_name, expected), (actual_name, actual))| {
                expected_name == actual_name && same_bytes(expected, actual)
            })
}

fn same_bytes(expected: &BytesDescriptor, actual: &BytesDescriptor) -> bool {
    match (expected, actual) {
        (BytesDescriptor::Content(expected), BytesDescriptor::Content(actual)) => {
            expected.size == actual.size && expected.sha256 == actual.sha256
        }
        (BytesDescriptor::Redacted(_), BytesDescriptor::Redacted(_)) => true,
        (BytesDescriptor::Content(_), BytesDescriptor::Redacted(_))
        | (BytesDescriptor::Redacted(_), BytesDescriptor::Content(_)) => false,
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
            body_blobs: BTreeMap::new(),
            redirects: Vec::new(),
        }
    }

    fn logical(body: Option<Value>) -> LogicalBody {
        body.map_or(LogicalBody::Absent, LogicalBody::Json)
    }

    fn with_blob(cfg: &RequestConfig, descriptor: BytesDescriptor) -> TraceRequest {
        TraceRequest {
            body_blobs: BTreeMap::from([(String::new(), descriptor)]),
            ..expected(cfg, None)
        }
    }

    #[test]
    fn blob_bodies_match_by_size_and_hash_and_redacted_markers_match_each_other() {
        let cfg = config();
        let live = Blob::new(b"jpeg".to_vec()).with_filename("cat.jpg");
        let sent = LogicalBody::Bytes(live.clone());
        let renamed =
            BytesDescriptor::of(b"jpeg", Some("image/jpeg".into()), Some("dog.jpg".into()));
        for recorded in [live.descriptor(), renamed] {
            assert!(validate_replay_request(&with_blob(&cfg, recorded), &cfg, &sent, 1, 1).is_ok());
        }
        let redacted = LogicalBody::Bytes(live.clone().sensitive());
        let marker = with_blob(&cfg, BytesDescriptor::redacted());
        assert!(validate_replay_request(&marker, &cfg, &redacted, 1, 1).is_ok());
        let restored = LogicalBody::Bytes(Blob::from_descriptor(BytesDescriptor::redacted()));
        assert!(validate_replay_request(&marker, &cfg, &restored, 1, 1).is_ok());

        let mismatches = [
            (
                with_blob(&cfg, BytesDescriptor::of(b"jpgg", None, None)),
                &sent,
            ),
            (
                with_blob(&cfg, BytesDescriptor::of(b"jpeg!", None, None)),
                &sent,
            ),
            (marker.clone(), &sent),
            (with_blob(&cfg, live.descriptor()), &redacted),
            (expected(&cfg, None), &sent),
            (with_blob(&cfg, live.descriptor()), &LogicalBody::Absent),
            (
                marker,
                &LogicalBody::Json(json!({"kind": "bytes", "redacted": true})),
            ),
        ];
        for (recorded, body) in mismatches {
            assert_eq!(
                validate_replay_request(&recorded, &cfg, body, 4, 1)
                    .err()
                    .unwrap_or_else(|| panic!("{recorded:?} must not match {body:?}"))
                    .message,
                "replay request drift at seq 4 attempt 1: body mismatch"
            );
        }
        let mut keyed = with_blob(&cfg, live.descriptor());
        keyed.body_blobs = BTreeMap::from([("/photo".to_owned(), live.descriptor())]);
        assert!(validate_replay_request(&keyed, &cfg, &sent, 1, 1).is_err());
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
        assert!(
            validate_replay_request(&trace, &cfg, &LogicalBody::Json(body.clone()), 3, 2).is_ok()
        );
        trace.method = "POST".into();
        assert_eq!(
            validate_replay_request(&trace, &cfg, &LogicalBody::Json(body.clone()), 3, 2)
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
            (
                Some(json!("password=one&scope=a")),
                Some(json!("password=two&scope=b")),
            ),
            (Some(json!("Bearer one ok")), Some(json!("Bearer two no"))),
            (
                Some(json!("username=alice\npassword=x")),
                Some(json!("username=bob\npassword=x")),
            ),
        ] {
            for (left, right) in [(&a, &b), (&b, &a)] {
                assert!(
                    validate_replay_request(
                        &expected(&cfg, left.clone()),
                        &cfg,
                        &logical(right.clone()),
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
    fn string_bodies_differing_only_in_redacted_secrets_replay_alike() {
        let cfg = config();
        for (recorded, live) in [
            ("password=[REDACTED]&scope=a", "password=two&scope=a"),
            ("password=one&scope=a", "password=two&scope=a"),
            ("Authorization: Bearer one", "Authorization: Bearer two"),
        ] {
            assert!(
                validate_replay_request(
                    &expected(&cfg, Some(json!(recorded))),
                    &cfg,
                    &LogicalBody::Json(json!(live)),
                    1,
                    1
                )
                .is_ok(),
                "{recorded} versus {live}"
            );
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
            let err = validate_replay_request(&trace, &actual, &LogicalBody::Json(body), 99, 7)
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
            assert!(validate_replay_request(&trace, &cfg, &LogicalBody::Absent, 1, 1).is_err());
        }
        for url in [
            "https://example.invalid/path?safe=actual-canary",
            "https://example.invalid/path?token=actual-secret&safe=[REDACTED]",
            "https://example.invalid/other?token=actual-secret&safe=actual-canary",
            "https://example.invalid/path?token=actual-secret&safe=actual%2Dcanary",
        ] {
            let mut trace = expected(&cfg, None);
            trace.url = url.into();
            assert!(validate_replay_request(&trace, &cfg, &LogicalBody::Absent, 1, 1).is_err());
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
