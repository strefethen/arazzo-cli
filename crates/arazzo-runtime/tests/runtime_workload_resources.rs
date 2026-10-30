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
#[path = "../examples/runtime_workload/service.rs"]
#[allow(dead_code)]
mod service;
#[path = "../examples/runtime_workload/source_check.rs"]
#[allow(dead_code)]
mod source_check;
#[path = "../examples/runtime_workload/worker.rs"]
#[allow(dead_code)]
mod worker;
use catalog::{CaseSpec, Family};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

struct Rig {
    ready: protocol::ServiceReadiness,
    join: Option<std::thread::JoinHandle<Result<(), catalog::HarnessError>>>,
}
impl Rig {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            service::serve_with_readiness(
                service::ServiceOptions {
                    lifetime_budget: Duration::from_secs(120),
                },
                |ready| {
                    tx.send(ready.clone())
                        .map_err(|e| catalog::HarnessError::new("test", "ready", e))
                },
            )
        });
        Self {
            ready: rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            join: Some(join),
        }
    }
    fn client(&self) -> protocol::ControlClient {
        protocol::ControlClient::new(self.ready.control_endpoint)
    }
    fn identity(&self, case: &CaseSpec) -> protocol::Identity {
        protocol::Identity {
            catalog_version: 1,
            case_id: case.case_id.clone(),
            invocation_id: format!(
                "test_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            response_policy: case.response_policy.clone(),
        }
    }
    fn run(&self, case: &CaseSpec) -> (worker::TimedExecution, protocol::Snapshot) {
        let id = self.identity(case);
        self.client().begin(&id, Duration::from_secs(70)).unwrap();
        let dir = temporary();
        std::fs::create_dir(&dir).unwrap();
        for (f, bytes) in fixtures::generated_files(case, &self.ready.data_endpoint).unwrap() {
            std::fs::write(dir.join(f.path), bytes).unwrap();
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let entered = rt.enter();
        let mut client = arazzo_runtime::ClientConfig {
            timeout: Duration::from_millis(case.settings.http_timeout_ms),
            rate_limit: arazzo_runtime::RateLimitConfig {
                requests_per_second: f64::from(case.settings.rate),
                burst: case.settings.burst as usize,
            },
            ..Default::default()
        };
        client.default_headers.insert(
            case.settings.invocation_header.clone(),
            id.invocation_id.clone(),
        );
        let engine = arazzo_runtime::EngineBuilder::new(
            serde_json::from_value(fixtures::root_document(case)).unwrap(),
        )
        .source_base_dir(&dir)
        .client_config(client)
        .parallel(case.settings.parallel)
        .capture_run(case.settings.capture)
        .trace(case.settings.trace)
        .channel_capacity(case.settings.channel_capacity)
        .max_response_bytes(case.settings.response_cap as usize)
        .build()
        .unwrap();
        drop(entered);
        let timed = rt
            .block_on(worker::execute_case(&engine, case, &self.client(), &id))
            .unwrap();
        let snapshot = self.client().snapshot(&id).unwrap();
        assert!(self
            .client()
            .begin(&self.identity(case), Duration::from_secs(5))
            .is_err());
        self.client().release(&id).unwrap();
        assert!(self.client().readiness(&id).is_err());
        drop(engine);
        rt.shutdown_timeout(Duration::from_secs(5));
        std::fs::remove_dir_all(dir).unwrap();
        (timed, snapshot)
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.client().stop().unwrap();
        self.join.take().unwrap().join().unwrap().unwrap();
    }
}
fn temporary() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "runtime_workload_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}
fn clone_actual(actual: &worker::ObservedExecution) -> worker::ObservedExecution {
    worker::ObservedExecution {
        outputs: actual
            .outputs
            .as_ref()
            .map(Clone::clone)
            .map_err(|e| arazzo_runtime::RuntimeError::new(e.kind, e.message.clone())),
        events: actual.events.clone(),
    }
}
fn assert_rejected(report: execution_oracle::OracleReport) {
    assert!(!report.passed, "perturbation escaped oracle");
}

#[test]
fn all_body_variants_and_controls_have_exact_outputs_and_complete_delivery() {
    let rig = Rig::new();
    let cases: Vec<_> = catalog::catalog()
        .into_iter()
        .filter(|c| c.family == Family::Bodies)
        .collect();
    assert_eq!(cases.len(), 35);
    for case in cases {
        let (timed, snapshot) = rig.run(&case);
        let report = response_oracle::check_body(&case, &timed.actual, &snapshot);
        assert!(
            report.passed,
            "{}: {:?}",
            case.case_id,
            report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .collect::<Vec<_>>()
        );
        assert!(serde_json::to_vec(&report).unwrap().len() < 1024 * 1024);
    }
}
#[test]
fn all_retry_variants_preserve_exact_attempts_replay_and_settled_siblings_and_reset() {
    let rig = Rig::new();
    let cases: Vec<_> = catalog::catalog()
        .into_iter()
        .filter(|c| c.family == Family::Retries)
        .collect();
    assert_eq!(cases.len(), 48);
    for case in cases {
        let (timed, snapshot) = rig.run(&case);
        let report = response_oracle::check_retry(&case, &timed.actual, &snapshot);
        assert!(
            report.passed,
            "{}: {:?}",
            case.case_id,
            report
                .checks
                .iter()
                .filter(|c| !c.passed)
                .collect::<Vec<_>>()
        );
        assert!(snapshot
            .requests
            .iter()
            .filter(|r| r.path == case.expected.required_paths[0])
            .any(|r| r.attempt == 1));
    }
}
#[test]
fn body_oracle_rejects_scalar_body_nodes_lengths_paths_and_partial_delivery() {
    let rig = Rig::new();
    let case = catalog::case("bodies_s1_b1024_fullbody_collect_fixed").unwrap();
    let (timed, snapshot) = rig.run(&case);
    for change in 0..8 {
        let mut actual = clone_actual(&timed.actual);
        let mut service = snapshot.clone();
        let mut spec = case.clone();
        match change {
            0 => {
                actual
                    .outputs
                    .as_mut()
                    .unwrap()
                    .insert("step_0_value_0".into(), json!(999));
            }
            1 => {
                actual
                    .outputs
                    .as_mut()
                    .unwrap()
                    .get_mut("step_0_body")
                    .unwrap()["padding"] = json!("bad")
            }
            2 => spec.dimensions.json_nodes += 1,
            3 => spec.dimensions.serialized_body_bytes += 1,
            4 => service.requests[0].path = "/wrong".into(),
            5 => service.requests[0].body_bytes_written -= 1,
            6 => service.requests[0].complete = false,
            _ => service.requests[0].final_response_release_ns = None,
        }
        assert_rejected(response_oracle::check_body(&spec, &actual, &service));
    }
}
#[test]
fn retry_oracle_rejects_attempt_count_status_order_capture_and_lost_settled_sibling() {
    let rig = Rig::new();
    let case = catalog::case("control_retry_exhaustion_s8_l3_ptrue_d0").unwrap();
    let (timed, snapshot) = rig.run(&case);
    for change in 0..7 {
        let mut actual = clone_actual(&timed.actual);
        let mut service = snapshot.clone();
        match change {
            0 => {
                service.requests.pop();
            }
            1 => service.requests[0].attempt += 1,
            2 => service.requests[0].status = 201,
            3 => {
                let indices: Vec<_> = service
                    .requests
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.path == "/s0/op0")
                    .map(|(i, _)| i)
                    .collect();
                service.requests.swap(indices[0], indices[1]);
            }
            4 => {
                actual.events.swap(0, 1);
            }
            5 => {
                actual.events.retain(
                    |e| !matches!(e,arazzo_runtime::EngineEvent::RunStep(r) if r.step_id=="step_7"),
                );
            }
            _ => {
                if let Some(arazzo_runtime::EngineEvent::RunStep(r)) = actual
                    .events
                    .iter_mut()
                    .find(|e| matches!(e, arazzo_runtime::EngineEvent::RunStep(_)))
                {
                    r.attempt = 99;
                }
            }
        }
        assert_rejected(response_oracle::check_retry(&case, &actual, &service));
    }
}
