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
use std::time::{Duration, Instant};

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
fn both_handles_both_stalls_both_widths_and_watchdog_preserve_exact_terminal_and_held_sockets() {
    let rig = Rig::new();
    let cases: Vec<_> = catalog::catalog()
        .into_iter()
        .filter(|c| c.family == Family::Cancellation)
        .collect();
    assert_eq!(cases.len(), 9);
    for case in cases {
        let (timed, snapshot) = rig.run(&case);
        let report = cancellation::check_cancelled(&case, &timed, &snapshot);
        assert!(report.passed, "{}: {:?}", case.case_id, report.checks);
        assert!(timed.elapsed_ns <= 900_000_000);
    }
}

#[test]
fn completion_is_polled_during_barrier_control_io_for_both_handle_modes() {
    let rig = Rig::new();
    for name in [
        "cancellation_w8_headers_collect",
        "cancellation_w8_headers_resultonly",
    ] {
        let mut case = catalog::case(name).unwrap();
        case.settings.channel_capacity = 1;
        let (timed, snapshot) = rig.run(&case);
        assert!(cancellation::check_cancelled(&case, &timed, &snapshot).passed);
    }
}

#[test]
fn cancellation_oracle_rejects_terminal_next_level_missing_participant_delivery_and_late_api() {
    let rig = Rig::new();
    let case = catalog::case("cancellation_w8_betweenchunks_collect").unwrap();
    let (timed, snapshot) = rig.run(&case);
    for change in 0..6 {
        let mut actual = worker::TimedExecution {
            elapsed_ns: timed.elapsed_ns,
            actual: clone_actual(&timed.actual),
        };
        let mut service = snapshot.clone();
        match change {
            0 => {
                actual.actual.outputs = Err(arazzo_runtime::RuntimeError::new(
                    arazzo_runtime::RuntimeErrorKind::ExecutionTimeout,
                    "wrong terminal",
                ))
            }
            1 => service.requests[0].path = "/s0/op8".into(),
            2 => {
                service.requests.pop();
            }
            3 => service.requests[0].complete = true,
            4 => service.active_connections = 0,
            _ => actual.elapsed_ns = 900_000_001,
        }
        assert_rejected(cancellation::check_cancelled(&case, &actual, &service));
    }
}

fn proxy(
    endpoint: std::net::SocketAddr,
    fault: &'static str,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let join = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut calls = Vec::new();
        let mut readiness = 0;
        while Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(Duration::from_millis(1));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: serde_json::Value =
                protocol::decode_frame(&bytes, protocol::REQUEST_CAP).unwrap();
            let op = request["op"].as_str().unwrap();
            calls.push(op.into());
            if op == "ready" {
                readiness += 1;
            }
            let mut upstream =
                std::net::TcpStream::connect_timeout(&endpoint, Duration::from_secs(1)).unwrap();
            upstream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            upstream.write_all(&bytes).unwrap();
            upstream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = Vec::new();
            upstream.read_to_end(&mut reply).unwrap();
            let mut value: serde_json::Value =
                protocol::decode_frame(&reply, protocol::RESPONSE_CAP).unwrap();
            if op == "ready" && readiness > 1 {
                match fault {
                    "identity" => value["identity"]["invocation_id"] = json!("stale"),
                    "omitted" => {
                        value["payload"]["ready"] = json!(true);
                        value["payload"]["arrived_paths"] = json!([]);
                        value["payload"]["stalled_paths"] = json!([]);
                    }
                    "missing_barrier" => value["payload"]["ready"] = json!(false),
                    "malformed" => {
                        stream.write_all(b"malformed\n").unwrap();
                        continue;
                    }
                    "lost" => continue,
                    _ => {}
                }
            }
            if op == "release" {
                match fault {
                    "release_refused" => {
                        value = json!({"op":"release","ok":false,"code":"release_failed","message":"injected release refusal"})
                    }
                    "release_unjoined" => value["payload"]["joined"] = json!(false),
                    _ => {}
                }
            }
            let mut output = serde_json::to_vec(&value).unwrap();
            output.push(b'\n');
            let _ = stream.write_all(&output);
            if op == "release" {
                break;
            }
        }
        calls
    });
    (address, join)
}

#[test]
fn wrong_stale_malformed_unavailable_omitted_missing_barrier_and_release_failures_invalidate_records(
) {
    let rig = Rig::new();
    for fault in [
        "identity",
        "omitted",
        "missing_barrier",
        "malformed",
        "lost",
        "release_refused",
        "release_unjoined",
    ] {
        let case = catalog::case("cancellation_w1_headers_collect").unwrap();
        let dir = temporary();
        std::fs::create_dir_all(dir.join("fixtures")).unwrap();
        write_prepared(&dir.join("fixtures"), &case, &rig.ready.data_endpoint);
        let (endpoint, join) = proxy(rig.ready.control_endpoint, fault);
        let options = worker::WorkerOptions {
            case_id: case.case_id.clone(),
            fixture_dir: dir.join("fixtures"),
            data_endpoint: rig.ready.data_endpoint.clone(),
            control_endpoint: endpoint,
            invocation_id: format!("fault_{fault}"),
            role: records::InvocationRole::Verification,
            settings: case.settings.clone(),
            evidence_dir: dir.join("evidence"),
        };
        let start = Instant::now();
        let record = worker::run_worker(options).unwrap();
        assert!(start.elapsed() < Duration::from_secs(12));
        assert_ne!(records::classify_exit(&record), 0, "{fault}");
        assert!(record.failure.is_some(), "{fault}");
        assert!(!record.sample_eligible);
        assert!(record.service.is_some(), "{fault}");
        if !fault.starts_with("release") {
            assert!(record.elapsed_ns.is_none(), "{fault}");
        }
        assert!(join.join().unwrap().contains(&"release".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let case = catalog::case("cancellation_w1_headers_collect").unwrap();
    let dir = temporary();
    std::fs::create_dir_all(dir.join("fixtures")).unwrap();
    write_prepared(&dir.join("fixtures"), &case, &rig.ready.data_endpoint);
    let record = worker::run_worker(worker::WorkerOptions {
        case_id: case.case_id,
        fixture_dir: dir.join("fixtures"),
        data_endpoint: rig.ready.data_endpoint.clone(),
        control_endpoint: endpoint,
        invocation_id: "unavailable".into(),
        role: records::InvocationRole::Verification,
        settings: case.settings,
        evidence_dir: dir.join("evidence"),
    })
    .unwrap();
    assert_ne!(records::classify_exit(&record), 0);
    assert_eq!(record.failure.unwrap().code, "connect");
    assert!(record.elapsed_ns.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

use catalog::ControlKind;
use fixtures::{Preflight, PreparedManifest, ValidationReceipt};
fn write_prepared(dir: &std::path::Path, case: &catalog::CaseSpec, endpoint: &str) {
    let base = dir.join(&case.case_id);
    std::fs::create_dir(&base).unwrap();
    std::fs::write(dir.join("cli-version.stdout"), b"arazzo 0.9.0\n").unwrap();
    std::fs::write(dir.join("cli-version.stderr"), b"").unwrap();
    std::fs::write(dir.join("cli-version.status"), b"0\n").unwrap();
    let files = fixtures::generated_files(case, endpoint).unwrap();
    for (file, bytes) in &files {
        std::fs::write(base.join(&file.path), bytes).unwrap();
    }
    let cycle = case.control == Some(ControlKind::Cycle);
    let stdout = if cycle {
        json!({"kind":"error","code":"VALIDATE_SPEC_VALIDATION","file":base.join("root.json"),"valid":false,"errors":[{"kind":"dependencyCycle","source":"validation","path":"workflow \"workload\".steps","message":"cycle"}]})
    } else {
        json!({"valid":true,"file":base.join("root.json"),"version":"1.1.0","title":"Runtime workload","workflows":1,"sources":case.dimensions.source_files})
    };
    std::fs::write(
        base.join("validate.stdout"),
        serde_json::to_vec(&stdout).unwrap(),
    )
    .unwrap();
    std::fs::write(base.join("validate.stderr"), b"").unwrap();
    let source = match source_check::check_sources(case, &base, endpoint) {
        Ok(receipt) => receipt,
        Err(e) => source_check::SourceReceipt {
            valid: false,
            code: Some(e.code.clone()),
            failure: Some(e),
            files: 0,
            operations: 0,
        },
    };
    std::fs::write(
        base.join("source-receipt.json"),
        serde_json::to_vec(&source).unwrap(),
    )
    .unwrap();
    let manifest = PreparedManifest {
        format_version: 1,
        catalog_version: 1,
        case_id: case.case_id.clone(),
        manifest_id: format!("catalog-1/{}", case.case_id),
        endpoint: endpoint.into(),
        dimensions: case.dimensions.clone(),
        settings: case.settings.clone(),
        files: files.into_iter().map(|(f, _)| f).collect(),
        expected_preflight: if cycle {
            Preflight::RootRejected
        } else if source.valid {
            Preflight::Valid
        } else {
            Preflight::SourceRejected
        },
        validation: ValidationReceipt {
            cli_path: std::env::current_exe().unwrap(),
            cli_version: "arazzo 0.9.0".into(),
            exit_code: if cycle { 1 } else { 0 },
            stdout_file: "validate.stdout".into(),
            stderr_file: "validate.stderr".into(),
            arazzo_version: if cycle { None } else { Some("1.1.0".into()) },
            workflow_count: if cycle { None } else { Some(1) },
            source_count: if cycle {
                None
            } else {
                Some(case.dimensions.source_files)
            },
            warnings: 0,
        },
        source_receipt_file: "source-receipt.json".into(),
    };
    std::fs::write(
        base.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}
