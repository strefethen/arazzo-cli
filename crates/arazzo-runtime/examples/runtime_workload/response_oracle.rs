use super::catalog::{self, CaseSpec, ControlKind, ExpectedValue, HandleMode, Transfer};
use super::execution_oracle::{terminal, OracleReport};
use super::fixtures::{materialize_expected, payload_bytes};
use super::protocol::Snapshot;
use super::worker::ObservedExecution;
use arazzo_runtime::{EngineEvent, ObserverEvent};
use serde_json::{json, Value};
use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::hash::Hasher;

pub(crate) fn node_count(value: &Value) -> usize {
    1 + match value {
        Value::Object(values) => values.values().map(node_count).sum(),
        Value::Array(values) => values.iter().map(node_count).sum(),
        _ => 0,
    }
}

fn summary(value: &Value) -> Value {
    let bytes = value.to_string().into_bytes();
    if bytes.len() <= 1024 {
        return value.clone();
    }
    let mut hasher = DefaultHasher::new();
    hasher.write(&bytes);
    json!({"serialized_bytes":bytes.len(),"json_nodes":node_count(value),"fingerprint":format!("{:016x}",hasher.finish())})
}

fn difference(expected: &Value, actual: &Value, path: &str) -> Option<Value> {
    if expected == actual {
        return None;
    }
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()) {
                let p = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (e.get(key), a.get(key)) {
                    (Some(e), Some(a)) => {
                        if let Some(diff) = difference(e, a, &p) {
                            return Some(diff);
                        }
                    }
                    (e, a) => {
                        return Some(
                            json!({"path":p,"expected":e.map(summary),"actual":a.map(summary)}),
                        )
                    }
                }
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (e, a)) in e.iter().zip(a).enumerate() {
                if let Some(diff) = difference(e, a, &format!("{path}/{i}")) {
                    return Some(diff);
                }
            }
        }
        _ => {}
    }
    Some(json!({"path":path,"expected":summary(expected),"actual":summary(actual)}))
}

pub(crate) fn exact(
    report: &mut OracleReport,
    name: impl Into<String>,
    expected: Value,
    actual: Value,
) {
    let equal = expected == actual;
    let expected_summary = summary(&expected);
    let actual_summary = if equal {
        summary(&actual)
    } else {
        json!({"value":summary(&actual),"first_difference":difference(&expected,&actual,"")})
    };
    report.compare(name, expected_summary, actual_summary);
    if let Some(check) = report.checks.last_mut() {
        check.passed = equal;
    }
    report.passed = report.checks.iter().all(|c| c.passed);
}

fn values(expected: &BTreeMap<String, ExpectedValue>) -> Value {
    json!(expected
        .iter()
        .map(|(k, v)| (
            k.clone(),
            materialize_expected(v).unwrap_or_else(|e| json!({"invalid_expected":e}))
        ))
        .collect::<BTreeMap<_, _>>())
}

fn url_path(url: &str) -> Value {
    match url::Url::parse(url) {
        Ok(url) if url.query().is_none() && url.fragment().is_none() => json!(url.path()),
        _ => json!({"invalid_url":url}),
    }
}

fn projection(event: &EngineEvent) -> Value {
    match event {
        EngineEvent::Execution(e) => json!([
            "execution",
            e.kind,
            e.workflow_id,
            e.step_id,
            e.seq,
            e.status_code,
            e.outputs,
            e.err.is_some(),
            e.operation_path,
            e.workflow_id_ref
        ]),
        EngineEvent::RunStep(e) => json!([
            "capture",
            e.workflow_id,
            e.step_id,
            e.seq,
            e.attempt,
            e.outcome,
            e.outputs,
            e.request
                .as_ref()
                .map(|r| json!([r.method, url_path(&r.url), r.headers])),
            e.response.as_ref().map(|r| json!([
                r.status_code,
                r.body_bytes,
                r.content_type,
                r.headers
            ])),
            e.decision,
            e.error
        ]),
        EngineEvent::Observer(e) => match e {
            ObserverEvent::StepStarted {
                workflow_id,
                step_id,
                operation_path,
                workflow_id_ref,
            } => json!([
                "started",
                workflow_id,
                step_id,
                operation_path,
                workflow_id_ref
            ]),
            ObserverEvent::RequestPrepared {
                workflow_id,
                step_id,
                method,
                url,
                headers,
                has_body,
            } => json!([
                "prepared",
                workflow_id,
                step_id,
                method,
                url_path(url),
                headers,
                has_body
            ]),
            ObserverEvent::RequestSent {
                workflow_id,
                step_id,
                method,
                url,
            } => json!(["sent", workflow_id, step_id, method, url_path(url)]),
            ObserverEvent::CriterionEvaluated {
                workflow_id,
                step_id,
                index,
                condition,
                passed,
            } => json!(["criterion", workflow_id, step_id, index, condition, passed]),
            ObserverEvent::StepCompleted {
                workflow_id,
                step_id,
                status_code,
                outputs,
                error,
                criteria_passed,
                ..
            } => json!([
                "completed",
                workflow_id,
                step_id,
                status_code,
                outputs,
                error.is_some(),
                criteria_passed
            ]),
            ObserverEvent::WorkflowCompleted {
                workflow_id,
                outputs,
                error,
                ..
            } => json!(["workflow_completed", workflow_id, outputs, error.is_some()]),
            ObserverEvent::RetryScheduled {
                workflow_id,
                step_id,
                attempt,
                max_attempts,
                delay_seconds,
            } => json!([
                "retry",
                workflow_id,
                step_id,
                attempt,
                max_attempts,
                delay_seconds
            ]),
            other => json!(["unexpected_observer", format!("{other:?}")]),
        },
        other => json!([
            "unexpected",
            super::execution_oracle::event_projection(other)
        ]),
    }
}

fn announce(case: &CaseSpec, i: usize, seq: &mut usize, events: &mut Vec<Value>) {
    let op = catalog::step_operation(case, i);
    let id = format!("step_{i}");
    let operation = format!(
        "$sourceDescriptions.source_{}.{}",
        op.source, op.operation_id
    );
    *seq += 1;
    events.push(json!([
        "execution",
        "beforeStep",
        case.workflow_id,
        id,
        *seq,
        0,
        {},
        false,
        operation,
        ""
    ]));
    events.push(json!(["started", case.workflow_id, id, operation, ""]));
}

fn expected_events(case: &CaseSpec) -> Vec<Value> {
    if case.settings.handle_mode == HandleMode::ResultOnly {
        return Vec::new();
    }
    let mut events = Vec::new();
    let mut seq = 0;
    let mut capture_seq = 0;
    let count = case.expected.lifecycle_steps.len();
    if case.settings.parallel {
        for i in 0..count {
            announce(case, i, &mut seq, &mut events)
        }
    }
    for i in 0..count {
        let op = catalog::step_operation(case, i);
        let id = format!("step_{i}");
        let operation = format!(
            "$sourceDescriptions.source_{}.{}",
            op.source, op.operation_id
        );
        let attempts = case.expected.attempts[&op.path];
        for attempt in 1..=attempts {
            if !case.settings.parallel || attempt > 1 {
                announce(case, i, &mut seq, &mut events)
            }
            let over_cap = case.control == Some(ControlKind::OverCap);
            let status = if over_cap {
                0
            } else if attempt <= case.response_policy.failure_attempts
                || case.response_policy.exhaust_path.as_deref() == Some(&op.path)
            {
                503
            } else {
                200
            };
            let succeeded = status == 200;
            let out = if succeeded {
                case.expected
                    .settled_step_outputs
                    .get(&id)
                    .map(values)
                    .unwrap_or_else(|| json!({}))
            } else {
                json!({})
            };
            events.push(json!([
                "prepared",
                case.workflow_id,
                id,
                "GET",
                op.path,
                {},
                false
            ]));
            events.push(json!(["sent", case.workflow_id, id, "GET", op.path]));
            if !over_cap {
                events.push(json!([
                    "criterion",
                    case.workflow_id,
                    id,
                    0,
                    "$statusCode == 200",
                    succeeded
                ]));
            }
            seq += 1;
            events.push(json!([
                "execution",
                "afterStep",
                case.workflow_id,
                id,
                seq,
                status,
                out,
                over_cap,
                operation,
                ""
            ]));
            events.push(json!([
                "completed",
                case.workflow_id,
                id,
                status,
                out,
                over_cap,
                succeeded
            ]));
            let retried = attempt < attempts;
            if case.settings.capture {
                capture_seq += 1;
                let decision = if status == 200 {
                    json!({"path":"next"})
                } else {
                    json!({"path":if retried{"retry"}else{"error"},"actionType":"retry","retryAfterSeconds":case.dimensions.retry_after_ms as f64/1000.0,"retryLimit":case.dimensions.retry_limit})
                };
                let error = if status != 200 && !retried {
                    json!(format!(
                        "step {id}: max retries ({}) exceeded",
                        case.dimensions.retry_limit
                    ))
                } else {
                    Value::Null
                };
                let mut headers = json!({"connection":"close","content-type":"application/json"});
                match case.response_policy.transfer {
                    Transfer::Fixed => {
                        headers["content-length"] =
                            json!(case.dimensions.serialized_body_bytes.to_string())
                    }
                    Transfer::Chunked => headers["transfer-encoding"] = json!("chunked"),
                }
                events.push(json!([
                    "capture",
                    case.workflow_id,
                    id,
                    capture_seq,
                    attempt,
                    if succeeded { "success" } else { "failure" },
                    out,
                    ["GET", op.path, {}],
                    [
                        status,
                        case.dimensions.serialized_body_bytes,
                        "json",
                        headers
                    ],
                    decision,
                    error
                ]));
            }
            if retried {
                events.push(json!([
                    "retry",
                    case.workflow_id,
                    id,
                    attempt,
                    case.dimensions.retry_limit,
                    case.dimensions.retry_after_ms as f64 / 1000.0
                ]));
            }
        }
    }
    events.push(json!([
        "workflow_completed",
        case.workflow_id,
        values(&case.expected.outputs),
        case.expected.terminal_kind != "success"
    ]));
    events
}

fn base(case: &CaseSpec, actual: &ObservedExecution, snapshot: &Snapshot) -> OracleReport {
    let mut report = OracleReport::default();
    report.compare(
        "terminal",
        json!(case.expected.terminal_kind),
        json!(terminal(actual)),
    );
    exact(
        &mut report,
        "outputs",
        values(&case.expected.outputs),
        actual
            .outputs
            .as_ref()
            .map_or_else(|_| json!({}), |o| json!(o)),
    );
    report.compare(
        "service/rejected",
        json!(0),
        json!(snapshot.rejected_requests),
    );
    let mut expected = Vec::new();
    for path in &case.expected.required_paths {
        for attempt in 1..=case.expected.attempts[path] {
            let status = if attempt <= case.response_policy.failure_attempts
                || case.response_policy.exhaust_path.as_deref() == Some(path)
            {
                503
            } else {
                200
            };
            expected.push(json!([path, attempt, status]));
        }
    }
    let mut observed: Vec<_> = snapshot
        .requests
        .iter()
        .map(|r| json!([r.path, r.attempt, r.status]))
        .collect();
    if case.settings.parallel {
        expected.sort_by_key(Value::to_string);
        observed.sort_by_key(Value::to_string);
    }
    report.compare("service/attempts", json!(expected), json!(observed));
    for path in &case.expected.required_paths {
        report.compare(
            format!("service/attempt_order/{path}"),
            json!((1..=case.expected.attempts[path]).collect::<Vec<_>>()),
            json!(snapshot
                .requests
                .iter()
                .filter(|r| r.path == *path)
                .map(|r| r.attempt)
                .collect::<Vec<_>>()),
        );
    }
    for (i, r) in snapshot.requests.iter().enumerate() {
        if case.control == Some(ControlKind::OverCap) {
            report.compare(
                format!("service/delivery/{i}"),
                json!(true),
                json!(
                    r.body_bytes_written <= case.response_policy.body_bytes
                        && (!r.complete
                            || (r.body_bytes_written == case.response_policy.body_bytes
                                && r.final_response_release_ns.is_some()))
                ),
            );
        } else {
            report.compare(
                format!("service/delivery/{i}"),
                json!([case.dimensions.serialized_body_bytes, true, true]),
                json!([
                    r.body_bytes_written,
                    r.complete,
                    r.final_response_release_ns.is_some()
                ]),
            );
        }
    }
    let expected = expected_events(case);
    report.compare(
        "events/count",
        json!(expected.len()),
        json!(actual.events.len()),
    );
    for (i, (expected, actual)) in expected.into_iter().zip(&actual.events).enumerate() {
        exact(
            &mut report,
            format!("events/{i}"),
            expected,
            projection(actual),
        );
    }
    report
}

pub(crate) fn check_body(
    case: &CaseSpec,
    actual: &ObservedExecution,
    snapshot: &Snapshot,
) -> OracleReport {
    let mut report = base(case, actual, snapshot);
    for i in 0..case.dimensions.steps {
        let op = catalog::step_operation(case, i);
        let bytes = payload_bytes(
            op.operation,
            case.dimensions.serialized_body_bytes,
            case.dimensions.pointer_depth,
        );
        match bytes.and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes)
                .map(|value| (bytes, value))
                .map_err(|e| catalog::HarnessError::new("oracle", "payload_parse", e))
        }) {
            Ok((bytes, parsed)) => {
                report.compare(
                    format!("body/{i}/serialized_bytes"),
                    json!(case.dimensions.serialized_body_bytes),
                    json!(bytes.len()),
                );
                report.compare(
                    format!("body/{i}/json_nodes"),
                    json!(case.dimensions.json_nodes),
                    json!(node_count(&parsed)),
                );
                match materialize_expected(&ExpectedValue::FullBody {
                    payload_index: op.operation,
                    bytes: case.dimensions.serialized_body_bytes,
                    pointer_depth: case.dimensions.pointer_depth,
                }) {
                    Ok(expected) => exact(
                        &mut report,
                        format!("body/{i}/independent_tree"),
                        expected,
                        parsed,
                    ),
                    Err(error) => {
                        report.compare("body/materialization", json!("valid"), json!(error))
                    }
                }
            }
            Err(error) => report.compare("body/payload", json!("valid"), json!(error)),
        }
    }
    report
}

pub(crate) fn check_retry(
    case: &CaseSpec,
    actual: &ObservedExecution,
    snapshot: &Snapshot,
) -> OracleReport {
    base(case, actual, snapshot)
}
