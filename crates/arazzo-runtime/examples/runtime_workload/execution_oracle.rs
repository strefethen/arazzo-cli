use super::catalog::{self, CaseSpec, ControlKind, ExpectedValue};
use super::fixtures::materialize_expected;
use super::protocol::Snapshot;
use super::worker::ObservedExecution;
use arazzo_runtime::{Engine, EngineEvent, ObserverEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OracleCheck {
    pub name: String,
    pub passed: bool,
    pub expected: Value,
    pub actual: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OracleReport {
    pub passed: bool,
    pub checks: Vec<OracleCheck>,
}

impl OracleReport {
    pub fn compare(&mut self, name: impl Into<String>, expected: Value, actual: Value) {
        self.checks.push(OracleCheck {
            name: name.into(),
            passed: expected == actual,
            expected,
            actual,
        });
        self.passed = self.checks.iter().all(|c| c.passed);
    }
}

fn values(expected: &BTreeMap<String, ExpectedValue>) -> Result<Value, catalog::HarnessError> {
    expected
        .iter()
        .map(|(key, value)| materialize_expected(value).map(|value| (key.clone(), value)))
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map(|values| json!(values))
}

pub(crate) fn terminal(actual: &ObservedExecution) -> String {
    actual
        .outputs
        .as_ref()
        .map(|_| "success".into())
        .unwrap_or_else(|e| format!("{:?}", e.kind))
}

fn request_path(url: &str) -> Value {
    match url::Url::parse(url) {
        Ok(url) if url.query().is_none() && url.fragment().is_none() => json!(url.path()),
        _ => json!({"invalid_url":url}),
    }
}

pub(crate) fn dependency_satisfied(
    dependent: &super::protocol::RequestRecord,
    prerequisite: &super::protocol::RequestRecord,
) -> bool {
    prerequisite
        .final_response_release_ns
        .is_some_and(|release| dependent.header_arrival_ns >= release)
}

fn observer_projection(event: &ObserverEvent) -> Value {
    match event {
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
            request_path(url),
            headers,
            has_body
        ]),
        ObserverEvent::RequestSent {
            workflow_id,
            step_id,
            method,
            url,
        } => json!(["sent", workflow_id, step_id, method, request_path(url)]),
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
            error,
            criteria_passed
        ]),
        ObserverEvent::WorkflowCompleted {
            workflow_id,
            outputs,
            error,
            ..
        } => json!(["workflow_completed", workflow_id, outputs, error]),
        other => json!(["unexpected_observer", format!("{other:?}")]),
    }
}

fn levels(case: &CaseSpec) -> Vec<Vec<usize>> {
    let count = case.expected.lifecycle_steps.len();
    if case.settings.parallel && case.control != Some(ControlKind::SuccessEnd) && count > 0 {
        if case.dimensions.diamond {
            vec![vec![0], vec![1, 2], vec![3]]
        } else {
            vec![
                (0..case.dimensions.width).collect(),
                (case.dimensions.width..count).collect(),
            ]
        }
    } else {
        (0..count).map(|i| vec![i]).collect()
    }
}

fn event_order(event: &EngineEvent) -> Value {
    match event {
        EngineEvent::Execution(event) => json!(["execution", event.kind, event.step_id]),
        EngineEvent::TraceStep(event) => json!(["trace", event.step_id]),
        EngineEvent::RunStep(event) => json!(["capture", event.step_id]),
        EngineEvent::SequentialFallback(_) => json!(["fallback"]),
        EngineEvent::Observer(ObserverEvent::CriterionEvaluated { step_id, index, .. }) => {
            json!(["observer", "criterion", step_id, index])
        }
        EngineEvent::Observer(ObserverEvent::WorkflowCompleted { workflow_id, .. }) => {
            json!(["observer", "workflow_completed", workflow_id])
        }
        EngineEvent::Observer(event) => {
            let projection = observer_projection(event);
            json!(["observer", projection[0], projection[2]])
        }
        _ => json!(["unexpected"]),
    }
}

pub(crate) fn event_projection(event: &EngineEvent) -> Value {
    let (kind, mut value) = match event {
        EngineEvent::Execution(e) => ("execution", json!(e)),
        EngineEvent::RunStep(e) => ("capture", json!(e)),
        EngineEvent::TraceStep(e) => ("trace", json!(e)),
        EngineEvent::Observer(e) => {
            return json!({"kind":"observer","value":observer_projection(e)})
        }
        EngineEvent::SequentialFallback(e) => ("fallback", json!(e)),
        EngineEvent::TransportWarning(e) => ("transport_warning", json!(e)),
        EngineEvent::WorkflowOutputWarning(e) => ("output_warning", json!(e)),
        other => return json!({"kind":"unexpected","value":format!("{other:?}")}),
    };
    if let Some(object) = value.as_object_mut() {
        object.remove("durationNs");
        object.remove("durationMs");
    }
    json!({"kind":kind,"value":value})
}

pub(crate) fn check_source(
    engine: &Engine,
    case: &CaseSpec,
    actual: &ObservedExecution,
    snapshot: &Snapshot,
) -> OracleReport {
    let mut report = check_execution(case, actual, snapshot);
    report.compare(
        "workflow_ids",
        json!(case.expected.workflow_ids),
        json!(engine.workflows()),
    );
    for operation in &case.expected.operations {
        let qualified = format!(
            "$sourceDescriptions.source_{}.{}",
            operation.source, operation.operation_id
        );
        report.compare(
            format!("operation/{qualified}"),
            json!([operation.method, operation.path]),
            match engine.resolve_operation_id(&qualified) {
                Ok(pair) => json!(pair),
                Err(e) => json!({"error":e.code()}),
            },
        );
    }
    report
}

pub(crate) fn check_execution(
    case: &CaseSpec,
    actual: &ObservedExecution,
    snapshot: &Snapshot,
) -> OracleReport {
    let mut report = OracleReport::default();
    let expected_values = match values(&case.expected.outputs) {
        Ok(values) => values,
        Err(error) => {
            report.compare("expected_values", json!("valid"), json!(error));
            return report;
        }
    };
    let expected_step_values = match case
        .expected
        .settled_step_outputs
        .iter()
        .map(|(step, expected)| values(expected).map(|values| (step.clone(), values)))
        .collect::<Result<BTreeMap<_, _>, _>>()
    {
        Ok(values) => values,
        Err(error) => {
            report.compare("expected_step_values", json!("valid"), json!(error));
            return report;
        }
    };
    report.compare(
        "terminal",
        json!(case.expected.terminal_kind),
        json!(terminal(actual)),
    );
    report.compare(
        "outputs",
        expected_values.clone(),
        actual.outputs.as_ref().map_or(json!({}), |o| json!(o)),
    );
    report.compare(
        "service/rejected",
        json!(0),
        json!(snapshot.rejected_requests),
    );
    let mut requests: Vec<_> = snapshot
        .requests
        .iter()
        .map(|r| {
            json!([
                r.path,
                r.attempt,
                r.status,
                r.body_bytes_written,
                r.complete,
                r.final_response_release_ns.is_some()
            ])
        })
        .collect();
    requests.sort_by_key(Value::to_string);
    let mut expected: Vec<_> = case
        .expected
        .required_paths
        .iter()
        .map(|p| json!([p, 1, 200, case.dimensions.serialized_body_bytes, true, true]))
        .collect();
    expected.sort_by_key(Value::to_string);
    report.compare("service/requests", json!(expected), json!(requests));
    if !case.settings.parallel {
        report.compare(
            "service/order",
            json!(case.expected.required_paths),
            json!(snapshot
                .requests
                .iter()
                .map(|r| &r.path)
                .collect::<Vec<_>>()),
        );
    }
    let by_path: BTreeMap<_, _> = snapshot
        .requests
        .iter()
        .map(|r| (r.path.as_str(), r))
        .collect();
    for i in 0..case.dimensions.steps {
        let target = catalog::step_operation(case, i);
        if !case.expected.required_paths.contains(&target.path) {
            continue;
        }
        for predecessor in catalog::predecessors(case, i) {
            let source = catalog::step_operation(case, predecessor);
            let causal = by_path
                .get(target.path.as_str())
                .zip(by_path.get(source.path.as_str()))
                .is_some_and(|(dependent, prerequisite)| {
                    dependency_satisfied(dependent, prerequisite)
                });
            report.compare(
                format!("dependency/{predecessor}/{i}"),
                json!(true),
                json!(causal),
            );
        }
    }
    let traces: Vec<_> = actual
        .events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::TraceStep(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    let captures: Vec<_> = actual
        .events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::RunStep(t) = e {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    let expected_steps = &case.expected.lifecycle_steps;
    report.compare(
        "trace/steps",
        json!(if case.settings.trace {
            expected_steps.clone()
        } else {
            Vec::new()
        }),
        json!(traces.iter().map(|t| &t.step_id).collect::<Vec<_>>()),
    );
    report.compare(
        "capture/steps",
        json!(if case.settings.capture {
            expected_steps.clone()
        } else {
            Vec::new()
        }),
        json!(captures.iter().map(|t| &t.step_id).collect::<Vec<_>>()),
    );
    let criteria: Vec<_> = traces.iter().flat_map(|t| t.criteria.iter().map(move |c| json!({"step_id":t.step_id,"index":c.index,"attempt":t.attempt,"passed":c.result,"criterion_type":c.type_,"condition":c.condition,"context":c.context}))).collect();
    report.compare(
        "trace/criteria",
        if case.settings.trace {
            json!(case.expected.criteria)
        } else {
            json!([])
        },
        json!(criteria),
    );
    for (index, t) in traces.iter().enumerate() {
        let op = catalog::step_operation(case, index);
        report.compare(
            format!("record/{index}/seq"),
            json!(index + 1),
            json!(t.seq),
        );
        let decision = if case.control == Some(ControlKind::SuccessEnd) {
            json!({"path":"done","actionType":"end"})
        } else if case.control == Some(ControlKind::FalseCriterion) {
            json!({"path":"error"})
        } else {
            json!({"path":"next"})
        };
        report.compare(
            format!("record/{index}/decision"),
            decision,
            json!(t.decision),
        );
        report.compare(
            format!("trace/{index}/identity"),
            json!([
                case.workflow_id,
                format!("step_{index}"),
                1,
                "http",
                format!(
                    "$sourceDescriptions.source_{}.{}",
                    op.source, op.operation_id
                ),
                ""
            ]),
            json!([
                t.workflow_id,
                t.step_id,
                t.attempt,
                t.kind,
                t.operation_path,
                t.workflow_id_ref
            ]),
        );
        report.compare(
            format!("trace/{index}/request"),
            json!([op.method, op.path]),
            json!([
                t.request.as_ref().map(|r| r.method.as_str()),
                t.request.as_ref().map(|r| request_path(&r.url))
            ]),
        );
        report.compare(
            format!("trace/{index}/response"),
            json!([200, case.dimensions.serialized_body_bytes]),
            json!([
                t.response.as_ref().map(|r| r.status_code),
                t.response.as_ref().map(|r| r.body_bytes)
            ]),
        );
        report.compare(
            format!("trace/{index}/outputs"),
            expected_step_values
                .get(&t.step_id)
                .cloned()
                .unwrap_or_else(|| json!({})),
            json!(t.outputs),
        );
        report.compare(
            format!("trace/{index}/error"),
            json!(actual.outputs.as_ref().err().map(|e| &e.message)),
            json!(t.error),
        );
        report.compare(
            format!("trace/{index}/warnings"),
            json!([]),
            json!(t.warnings),
        );
        report.compare(
            format!("trace/{index}/criterion_warnings"),
            json!(true),
            json!(t.criteria.iter().all(|c| c.warnings.is_empty())),
        );
    }
    for (index, t) in captures.iter().enumerate() {
        let op = catalog::step_operation(case, index);
        report.compare(
            format!("record/{index}/seq"),
            json!(index + 1),
            json!(t.seq),
        );
        let decision = if case.control == Some(ControlKind::SuccessEnd) {
            json!({"path":"done","actionType":"end"})
        } else if case.control == Some(ControlKind::FalseCriterion) {
            json!({"path":"error"})
        } else {
            json!({"path":"next"})
        };
        report.compare(
            format!("record/{index}/decision"),
            decision,
            json!(t.decision),
        );
        report.compare(
            format!("capture/{index}/identity"),
            json!([case.workflow_id, format!("step_{index}"), 1, "success"]),
            json!([t.workflow_id, t.step_id, t.attempt, t.outcome]),
        );
        report.compare(
            format!("capture/{index}/request"),
            json!([op.method, op.path]),
            json!([
                t.request.as_ref().map(|r| r.method.as_str()),
                t.request.as_ref().map(|r| request_path(&r.url))
            ]),
        );
        report.compare(
            format!("capture/{index}/response"),
            json!([200, case.dimensions.serialized_body_bytes]),
            json!([
                t.response.as_ref().map(|r| r.status_code),
                t.response.as_ref().map(|r| r.body_bytes)
            ]),
        );
        report.compare(
            format!("capture/{index}/outputs"),
            expected_step_values
                .get(&t.step_id)
                .cloned()
                .unwrap_or_else(|| json!({})),
            json!(t.outputs),
        );
        report.compare(
            format!("capture/{index}/error"),
            Value::Null,
            json!(t.error),
        );
    }
    let lifecycle: Vec<_> = actual
        .events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::Execution(e) = e {
                Some(json!([
                    e.kind,
                    e.workflow_id,
                    e.step_id,
                    e.status_code,
                    e.outputs,
                    e.err.is_some(),
                    e.operation_path,
                    e.workflow_id_ref
                ]))
            } else {
                None
            }
        })
        .collect();
    let mut expected_lifecycle = Vec::new();
    let mut expected_observers = Vec::new();
    let mut expected_event_order = Vec::new();
    if case.control == Some(ControlKind::SuccessEnd) {
        expected_event_order.push(json!(["fallback"]));
    }
    for level in levels(case) {
        for i in &level {
            let id = &expected_steps[*i];
            let op = catalog::step_operation(case, *i);
            let operation = format!(
                "$sourceDescriptions.source_{}.{}",
                op.source, op.operation_id
            );
            expected_event_order.push(json!(["execution", "beforeStep", id]));
            expected_event_order.push(json!(["observer", "started", id]));
            expected_lifecycle.push(json!([
                "beforeStep",
                case.workflow_id,
                id,
                0,
                {},
                false,
                operation,
                ""
            ]));
            expected_observers.push(json!(["started", case.workflow_id, id, operation, ""]));
        }
        for i in level {
            let id = &expected_steps[i];
            let op = catalog::step_operation(case, i);
            let operation = format!(
                "$sourceDescriptions.source_{}.{}",
                op.source, op.operation_id
            );
            let step_outputs = expected_step_values
                .get(id)
                .cloned()
                .unwrap_or_else(|| json!({}));
            expected_lifecycle.push(json!([
                "afterStep",
                case.workflow_id,
                id,
                200,
                step_outputs,
                false,
                operation,
                ""
            ]));
            expected_event_order.push(json!(["observer", "prepared", id]));
            expected_event_order.push(json!(["observer", "sent", id]));
            expected_observers.push(json!([
                "prepared",
                case.workflow_id,
                id,
                "GET",
                op.path,
                {},
                false
            ]));
            expected_observers.push(json!(["sent", case.workflow_id, id, "GET", op.path]));
            for criterion in case.expected.criteria.iter().filter(|c| c.step_id == *id) {
                expected_event_order.push(json!(["observer", "criterion", id, criterion.index]));
                expected_observers.push(json!([
                    "criterion",
                    case.workflow_id,
                    id,
                    criterion.index,
                    criterion.condition,
                    criterion.passed
                ]));
            }
            expected_event_order.push(json!(["execution", "afterStep", id]));
            expected_event_order.push(json!(["observer", "completed", id]));
            if case.settings.trace {
                expected_event_order.push(json!(["trace", id]));
            }
            if case.settings.capture {
                expected_event_order.push(json!(["capture", id]));
            }
            expected_observers.push(json!([
                "completed",
                case.workflow_id,
                id,
                200,
                step_outputs,
                null,
                case.control != Some(ControlKind::FalseCriterion)
            ]));
        }
    }
    if !expected_steps.is_empty() {
        expected_event_order.push(json!(["observer", "workflow_completed", case.workflow_id]));
        expected_observers.push(json!([
            "workflow_completed",
            case.workflow_id,
            expected_values.clone(),
            actual.outputs.as_ref().err().map(|e| &e.message)
        ]));
    }
    report.compare(
        "event_replay",
        json!(expected_event_order),
        json!(actual.events.iter().map(event_order).collect::<Vec<_>>()),
    );
    report.compare("lifecycle", json!(expected_lifecycle), json!(lifecycle));
    let actual_seq: Vec<_> = actual
        .events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::Execution(e) = e {
                Some(e.seq)
            } else {
                None
            }
        })
        .collect();
    report.compare(
        "lifecycle/sequence",
        json!((1..=expected_steps.len() * 2).collect::<Vec<_>>()),
        json!(actual_seq),
    );
    report.compare(
        "observer/replay",
        json!(expected_observers),
        json!(actual
            .events
            .iter()
            .filter_map(|e| if let EngineEvent::Observer(e) = e {
                Some(observer_projection(e))
            } else {
                None
            })
            .collect::<Vec<_>>()),
    );
    let fallbacks: Vec<_> = actual
        .events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::SequentialFallback(e) = e {
                Some(json!(e))
            } else {
                None
            }
        })
        .collect();
    let expected_fallback = if case.control == Some(ControlKind::SuccessEnd) {
        json!([{"workflowId":case.workflow_id,"reason":"controlFlowAction","stepId":"step_0","message":format!("workflow \"{}\" runs sequentially because step \"step_0\" has an onSuccess action of type \"end\", which redirects control flow",case.workflow_id)}])
    } else {
        json!([])
    };
    report.compare("fallbacks", expected_fallback, json!(fallbacks));
    report.compare(
        "unexpected_events",
        json!([]),
        json!(actual
            .events
            .iter()
            .filter(|e| !matches!(
                e,
                EngineEvent::Observer(_)
                    | EngineEvent::Execution(_)
                    | EngineEvent::TraceStep(_)
                    | EngineEvent::RunStep(_)
                    | EngineEvent::SequentialFallback(_)
            ))
            .map(event_projection)
            .collect::<Vec<_>>()),
    );
    report
}
