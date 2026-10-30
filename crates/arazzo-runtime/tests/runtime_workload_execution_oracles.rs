#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "../examples/runtime_workload/cancellation.rs"]
#[allow(dead_code)]
mod cancellation;
#[path = "../examples/runtime_workload/catalog.rs"]
#[allow(dead_code)]
mod catalog;
#[path = "../examples/runtime_workload/execution_oracle.rs"]
#[allow(dead_code)]
mod execution_oracle;
#[path = "../examples/runtime_workload/fixtures.rs"]
#[allow(dead_code)]
mod fixtures;
#[path = "../examples/runtime_workload/protocol.rs"]
#[allow(dead_code)]
mod protocol;
#[path = "../examples/runtime_workload/records.rs"]
#[allow(dead_code)]
mod records;
#[path = "../examples/runtime_workload/response_oracle.rs"]
#[allow(dead_code)]
mod response_oracle;
#[path = "../examples/runtime_workload/source_check.rs"]
#[allow(dead_code)]
mod source_check;
#[path = "../examples/runtime_workload/worker.rs"]
#[allow(dead_code)]
mod worker;
use arazzo_runtime::{
    EngineEvent, ExecutionEvent, RunStepRecord, RuntimeError, RuntimeErrorKind, TraceStepRecord,
};
use catalog::{CaseSpec, ControlKind, ExpectedValue};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn outputs(values: &BTreeMap<String, ExpectedValue>) -> BTreeMap<String, Value> {
    values
        .iter()
        .map(|(k, v)| (k.clone(), fixtures::materialize_expected(v).unwrap()))
        .collect()
}
fn observation(case: &CaseSpec) -> (worker::ObservedExecution, protocol::Snapshot) {
    let mut events = Vec::new();
    let mut requests = Vec::new();
    for (i, step) in case.expected.lifecycle_steps.iter().enumerate() {
        let op = catalog::step_operation(case, i);
        let operation = format!(
            "$sourceDescriptions.source_{}.{}",
            op.source, op.operation_id
        );
        let out = case
            .expected
            .settled_step_outputs
            .get(step)
            .map(outputs)
            .unwrap_or_default();
        let failed = case.control == Some(ControlKind::FalseCriterion);
        for (offset, kind, status, map) in [
            (1, "beforeStep", 0, BTreeMap::new()),
            (2, "afterStep", 200, out.clone()),
        ] {
            events.push(EngineEvent::Execution(serde_json::from_value::<ExecutionEvent>(json!({"seq":i*2+offset,"kind":kind,"workflowId":case.workflow_id,"stepId":step,"operationPath":operation,"workflowIdRef":"","statusCode":status,"outputs":map,"err":if failed && offset==2{Some("criterion failed")}else{None},"durationNs":0})).unwrap()));
        }
        let decision = json!({"path":"next"});
        if case.settings.capture {
            events.push(EngineEvent::RunStep(serde_json::from_value::<RunStepRecord>(json!({"seq":i+1,"workflowId":case.workflow_id,"stepId":step,"attempt":1,"outcome":"success","durationMs":0,"outputs":out,"request":{"method":"GET","url":format!("http://127.0.0.1:12345{}",op.path),"headers":{}},"response":{"statusCode":200,"headers":{},"contentType":"json","bodyBytes":1024},"decision":decision,"error":null})).unwrap()));
        }
        if case.settings.trace {
            let criteria:Vec<_>=case.expected.criteria.iter().filter(|c|c.step_id==*step).map(|c|json!({"index":c.index,"type":c.criterion_type,"condition":c.condition,"context":c.context,"result":c.passed})).collect();
            events.push(EngineEvent::TraceStep(serde_json::from_value::<TraceStepRecord>(json!({"seq":i+1,"workflowId":case.workflow_id,"stepId":step,"attempt":1,"kind":"http","operationPath":operation,"workflowIdRef":"","durationMs":0,"request":{"method":"GET","url":format!("http://127.0.0.1:12345{}",op.path),"headers":{}},"response":{"statusCode":200,"contentType":"json","bodyBytes":1024},"criteria":criteria,"warnings":[],"decision":decision,"outputs":out,"error":null})).unwrap()));
        }
        requests.push(protocol::RequestRecord {
            path: op.path,
            attempt: 1,
            status: 200,
            header_arrival_ns: (i * 10) as u64,
            final_response_release_ns: Some((i * 10 + 1) as u64),
            body_bytes_written: 1024,
            complete: true,
        });
    }
    if case.control == Some(ControlKind::SuccessEnd) {
        events.insert(0,EngineEvent::SequentialFallback(serde_json::from_value(json!({"workflowId":"workload","reason":"controlFlowAction","stepId":"step_0","message":"workflow \"workload\" runs sequentially because step \"step_0\" has an onSuccess action of type \"end\", which redirects control flow"})).unwrap()));
    }
    let old_events = events;
    let mut events = Vec::new();
    let levels = if case.settings.parallel
        && case.control != Some(ControlKind::SuccessEnd)
        && !case.expected.lifecycle_steps.is_empty()
    {
        if case.dimensions.diamond {
            vec![vec![0], vec![1, 2], vec![3]]
        } else {
            vec![
                (0..case.dimensions.width).collect(),
                (case.dimensions.width..case.expected.lifecycle_steps.len()).collect(),
            ]
        }
    } else {
        (0..case.expected.lifecycle_steps.len())
            .map(|i| vec![i])
            .collect()
    };
    let mut sequence = 0;
    for level in levels {
        for i in &level {
            let id = format!("step_{i}");
            let op = catalog::step_operation(case, *i);
            let mut before = old_events
                .iter()
                .find_map(|e| {
                    if let EngineEvent::Execution(e) = e {
                        if e.step_id == id && e.status_code == 0 {
                            Some(e.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .unwrap();
            sequence += 1;
            before.seq = sequence;
            events.push(EngineEvent::Execution(before));
            events.push(EngineEvent::Observer(
                arazzo_runtime::ObserverEvent::StepStarted {
                    workflow_id: case.workflow_id.clone(),
                    step_id: id,
                    operation_path: format!(
                        "$sourceDescriptions.source_{}.{}",
                        op.source, op.operation_id
                    ),
                    workflow_id_ref: String::new(),
                },
            ));
        }
        for i in level {
            let id = format!("step_{i}");
            let op = catalog::step_operation(case, i);
            let url = format!("http://127.0.0.1:12345{}", op.path);
            events.push(EngineEvent::Observer(
                arazzo_runtime::ObserverEvent::RequestPrepared {
                    workflow_id: case.workflow_id.clone(),
                    step_id: id.clone(),
                    method: "GET".into(),
                    url: url.clone(),
                    headers: BTreeMap::new(),
                    has_body: false,
                },
            ));
            events.push(EngineEvent::Observer(
                arazzo_runtime::ObserverEvent::RequestSent {
                    workflow_id: case.workflow_id.clone(),
                    step_id: id.clone(),
                    method: "GET".into(),
                    url,
                },
            ));
            for c in case.expected.criteria.iter().filter(|c| c.step_id == id) {
                events.push(EngineEvent::Observer(
                    arazzo_runtime::ObserverEvent::CriterionEvaluated {
                        workflow_id: case.workflow_id.clone(),
                        step_id: id.clone(),
                        index: c.index,
                        condition: c.condition.clone(),
                        passed: c.passed,
                    },
                ));
            }
            let out = case
                .expected
                .settled_step_outputs
                .get(&id)
                .map(outputs)
                .unwrap_or_default();
            let mut after = old_events
                .iter()
                .find_map(|e| {
                    if let EngineEvent::Execution(e) = e {
                        if e.step_id == id && e.status_code == 200 {
                            Some(e.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .unwrap();
            sequence += 1;
            after.seq = sequence;
            after.err = None;
            events.push(EngineEvent::Execution(after));
            events.push(EngineEvent::Observer(
                arazzo_runtime::ObserverEvent::StepCompleted {
                    workflow_id: case.workflow_id.clone(),
                    step_id: id.clone(),
                    status_code: 200,
                    duration: std::time::Duration::ZERO,
                    outputs: out,
                    error: None,
                    criteria_passed: case.control != Some(ControlKind::FalseCriterion),
                },
            ));
            for e in &old_events {
                match e {
                    EngineEvent::RunStep(t) if t.step_id == id => {
                        let mut t = t.clone();
                        if case.control == Some(ControlKind::SuccessEnd) {
                            t.decision =
                                serde_json::from_value(json!({"path":"done","actionType":"end"}))
                                    .unwrap();
                        }
                        events.push(EngineEvent::RunStep(t));
                    }
                    EngineEvent::TraceStep(t) if t.step_id == id => {
                        let mut t = t.clone();
                        if case.control == Some(ControlKind::FalseCriterion) {
                            t.decision = serde_json::from_value(json!({"path":"error"})).unwrap();
                            t.error = Some("expected negative".into());
                        }
                        events.push(EngineEvent::TraceStep(t));
                    }
                    _ => {}
                }
            }
        }
    }
    for e in old_events {
        if matches!(e, EngineEvent::SequentialFallback(_)) {
            events.insert(0, e);
        }
    }
    if !case.expected.lifecycle_steps.is_empty() {
        events.push(EngineEvent::Observer(
            arazzo_runtime::ObserverEvent::WorkflowCompleted {
                workflow_id: case.workflow_id.clone(),
                outputs: outputs(&case.expected.outputs),
                duration: std::time::Duration::ZERO,
                error: if case.expected.terminal_kind == "success" {
                    None
                } else {
                    Some("expected negative".into())
                },
            },
        ));
    }
    let result = if case.expected.terminal_kind == "success" {
        Ok(outputs(&case.expected.outputs))
    } else {
        Err(RuntimeError::new(
            match case.control.unwrap() {
                ControlKind::Cycle => RuntimeErrorKind::DependencyCycle,
                ControlKind::MissingSource => RuntimeErrorKind::SourceDescriptionLoad,
                ControlKind::MalformedSource => RuntimeErrorKind::SourceDescriptionParse,
                _ => RuntimeErrorKind::SuccessCriteriaFailed,
            },
            "expected negative",
        ))
    };
    (
        worker::ObservedExecution {
            outputs: result,
            events,
        },
        protocol::Snapshot {
            requests,
            active_connections: 0,
            rejected_requests: 0,
        },
    )
}
fn rejects(
    case: &CaseSpec,
    mutate: impl FnOnce(&mut worker::ObservedExecution, &mut protocol::Snapshot),
) {
    let (mut observed, mut snapshot) = observation(case);
    assert!(execution_oracle::check_execution(case, &observed, &snapshot).passed);
    mutate(&mut observed, &mut snapshot);
    assert!(!execution_oracle::check_execution(case, &observed, &snapshot).passed);
}
#[test]
fn entire_expression_matrix_and_controls_have_independent_exact_values() {
    for id in [
        "expressions_s8_c1",
        "expressions_s8_c16",
        "expressions_s8_c64",
        "expressions_s64_c16",
        "control_false_criterion",
        "control_cycle",
        "control_missing_source",
        "control_malformed_source",
        "control_success_end",
    ] {
        let case = catalog::case(id).unwrap();
        let (actual, snapshot) = observation(&case);
        let report = execution_oracle::check_execution(&case, &actual, &snapshot);
        assert!(
            report.passed,
            "{id}: {:?}",
            report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .collect::<Vec<_>>()
        );
    }
}
#[test]
fn changed_values_omissions_additions_and_terminal_errors_reject() {
    let case = catalog::case("expressions_s8_c16").unwrap();
    rejects(&case, |a, _| {
        a.outputs
            .as_mut()
            .unwrap()
            .insert("step_7_value_7".into(), json!(0));
    });
    rejects(&case, |a, _| {
        a.outputs.as_mut().unwrap().remove("step_0_value_0");
    });
    rejects(&case, |a, _| {
        a.outputs = Err(RuntimeError::new(RuntimeErrorKind::InternalError, "wrong"));
    });
    rejects(&case, |a, _| {
        a.events.pop();
    });
    rejects(&case, |a, _| {
        a.events.push(a.events[0].clone());
    });
    rejects(&case, |_, s| {
        s.requests.pop();
    });
    rejects(&case, |_, s| {
        s.requests.push(s.requests[0].clone());
    });
    rejects(&case, |_, s| {
        s.rejected_requests = 1;
    });
}
#[test]
fn every_criterion_identity_result_and_request_projection_is_checked() {
    let case = catalog::case("expressions_s8_c64").unwrap();
    for change in 0..10 {
        rejects(&case, |a, _| {
            let t = a
                .events
                .iter_mut()
                .find_map(|e| {
                    if let EngineEvent::TraceStep(t) = e {
                        Some(t)
                    } else {
                        None
                    }
                })
                .unwrap();
            match change {
                0 => {
                    t.criteria.pop();
                }
                1 => t.criteria.push(t.criteria[0].clone()),
                2 => t.criteria[0].index = 9,
                3 => t.criteria[0].type_ = "regex".into(),
                4 => t.criteria[0].condition = "true".into(),
                5 => t.criteria[0].context = "changed".into(),
                6 => t.criteria[0].result = false,
                7 => t.criteria[0].warnings.push("warning".into()),
                8 => t.request.as_mut().unwrap().method = "POST".into(),
                _ => t.request.as_mut().unwrap().url = "http://127.0.0.1:12345/wrong".into(),
            }
        });
    }
}
#[test]
fn every_wide_dag_edge_and_diamond_predecessor_is_checked() {
    for id in [
        "dag_w1_ptrue_r0",
        "dag_w8_ptrue_r0",
        "dag_w40_ptrue_r0",
        "dag_w128_ptrue_r0",
        "dag_diamond_ptrue_r0",
    ] {
        let case = catalog::case(id).unwrap();
        let (baseline, snapshot) = observation(&case);
        let report = execution_oracle::check_execution(&case, &baseline, &snapshot);
        assert!(report.passed);
        let expected_edges: std::collections::BTreeSet<_> = (0..case.dimensions.steps)
            .flat_map(|i| {
                catalog::predecessors(&case, i)
                    .into_iter()
                    .map(move |p| format!("dependency/{p}/{i}"))
            })
            .collect();
        assert_eq!(
            report
                .checks
                .iter()
                .filter(|check| check.name.starts_with("dependency/"))
                .map(|check| check.name.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected_edges
        );
        for i in 0..case.dimensions.steps {
            for p in catalog::predecessors(&case, i) {
                assert!(execution_oracle::dependency_satisfied(
                    &snapshot.requests[i],
                    &snapshot.requests[p]
                ));
                let mut dependent = snapshot.requests[i].clone();
                dependent.header_arrival_ns =
                    snapshot.requests[p].final_response_release_ns.unwrap() - 1;
                assert!(!execution_oracle::dependency_satisfied(
                    &dependent,
                    &snapshot.requests[p]
                ));
            }
        }
        rejects(&case, |a, _| {
            let mut captured: Vec<_> = a
                .events
                .iter()
                .filter_map(|e| {
                    if let EngineEvent::RunStep(t) = e {
                        Some(t.clone())
                    } else {
                        None
                    }
                })
                .collect();
            captured.swap(0, 1);
            let mut iter = captured.into_iter();
            for e in &mut a.events {
                if matches!(e, EngineEvent::RunStep(_)) {
                    *e = EngineEvent::RunStep(iter.next().unwrap());
                }
            }
        });
        for change in 0..5 {
            rejects(&case, |a, _| {
                let t = a
                    .events
                    .iter_mut()
                    .find_map(|e| {
                        if let EngineEvent::RunStep(t) = e {
                            Some(t)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                match change {
                    0 => t.attempt = 2,
                    1 => t.response.as_mut().unwrap().status_code = 201,
                    2 => t.request.as_mut().unwrap().method = "POST".into(),
                    3 => t.outputs.clear(),
                    _ => t.error = Some("wrong".into()),
                }
            });
        }
    }
}

#[test]
fn source_lookup_checks_all_files_operations_methods_paths_and_last_target() {
    for id in [
        "sources_f1_o10",
        "sources_f1_o100",
        "sources_f8_o10",
        "sources_f8_o100",
    ] {
        let case = catalog::case(id).unwrap();
        let endpoint = "http://127.0.0.1:32123";
        let dir = std::env::temp_dir().join(format!("source-oracle-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        for (file, bytes) in fixtures::generated_files(&case, endpoint).unwrap() {
            std::fs::write(dir.join(file.path), bytes).unwrap();
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let entered = rt.enter();
        let engine = arazzo_runtime::EngineBuilder::new(
            serde_json::from_value(fixtures::root_document(&case)).unwrap(),
        )
        .source_base_dir(&dir)
        .build()
        .unwrap();
        drop(entered);
        let (actual, snapshot) = observation(&case);
        let report = execution_oracle::check_source(&engine, &case, &actual, &snapshot);
        assert!(report.passed);
        assert_eq!(
            report
                .checks
                .iter()
                .filter(|c| c.name.starts_with("operation/"))
                .count(),
            case.dimensions.source_files * case.dimensions.operations_per_file
        );
        for change in 0..4 {
            let mut changed = case.clone();
            let last = changed.expected.operations.last_mut().unwrap();
            match change {
                0 => last.method = "POST".into(),
                1 => last.path = "/wrong_last_target".into(),
                2 => last.operation_id = "missing".into(),
                _ => changed.expected.workflow_ids.push("extra".into()),
            };
            assert!(!execution_oracle::check_source(&engine, &changed, &actual, &snapshot).passed);
        }
        let mut changed = snapshot.clone();
        changed.requests.last_mut().unwrap().path = "/wrong_last_target".into();
        assert!(!execution_oracle::check_source(&engine, &case, &actual, &changed).passed);
        drop(engine);
        rt.shutdown_timeout(std::time::Duration::from_secs(1));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn diamond_outputs_and_terminal_controls_reject_wrong_predecessors_and_errors() {
    let case = catalog::case("dag_diamond_ptrue_r0").unwrap();
    for name in case
        .expected
        .outputs
        .keys()
        .filter(|key| key.contains("predecessor"))
    {
        rejects(&case, |a, _| {
            a.outputs.as_mut().unwrap().insert(name.clone(), json!(999));
        });
    }
    for id in [
        "control_false_criterion",
        "control_cycle",
        "control_missing_source",
        "control_malformed_source",
    ] {
        let case = catalog::case(id).unwrap();
        rejects(&case, |a, _| {
            a.outputs = Err(RuntimeError::new(
                RuntimeErrorKind::InternalError,
                "wrong terminal",
            ));
        });
        rejects(&case, |_, s| {
            s.requests.push(protocol::RequestRecord {
                path: "/unexpected".into(),
                attempt: 1,
                status: 200,
                header_arrival_ns: 0,
                final_response_release_ns: Some(1),
                body_bytes_written: 1024,
                complete: true,
            });
        });
    }
}

#[test]
fn cross_event_replay_order_and_trace_errors_cannot_be_reshuffled() {
    let case = catalog::case("expressions_s8_c16").unwrap();
    rejects(&case, |a, _| {
        a.events.swap(0, 1);
    });
    rejects(&case, |a, _| {
        let capture = a
            .events
            .iter()
            .position(|e| matches!(e, EngineEvent::TraceStep(_)))
            .unwrap();
        a.events.swap(capture, capture - 1);
    });
    let negative = catalog::case("control_false_criterion").unwrap();
    rejects(&negative, |a, _| {
        a.events
            .iter_mut()
            .find_map(|e| {
                if let EngineEvent::TraceStep(t) = e {
                    Some(t)
                } else {
                    None
                }
            })
            .unwrap()
            .error = None;
    });
}
