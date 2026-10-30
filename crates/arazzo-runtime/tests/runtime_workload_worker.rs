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
use catalog::ControlKind;
use records::{InvocationRole, RawRecord};
use serde_json::json;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

fn temporary(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "workload-worker-{}-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
use std::time::SystemTime;
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn empty() -> RawRecord {
    let dir = temporary("empty");
    let record = worker::run_empty_worker(worker::EmptyWorkerOptions {
        invocation_id: "empty_test".into(),
        evidence_dir: dir.clone(),
    })
    .unwrap();
    assert!(record.failure.is_none(), "{:?}", record.failure);
    std::fs::remove_dir_all(dir).unwrap();
    record
}
#[test]
fn empty_worker_has_same_runtime_record_path_and_no_service() {
    let record = empty();
    assert_eq!(records::classify_exit(&record), 0);
    assert_eq!(record.role, InvocationRole::Empty);
    assert_eq!(record.outcome, "empty");
    assert!(!record.sample_eligible);
    assert!(record.service.is_none());
    assert!(record.elapsed_ns.is_some());
    assert!(record.actual.event_file.is_none());
    assert!(record.actual.output_file.is_none());
}
#[test]
fn raw_record_is_one_exact_frame_and_rejects_truncation_multiple_unknown_fields() {
    let record = empty();
    let mut out = Vec::new();
    records::write_record(&record, &mut out).unwrap();
    assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 1);
    assert_eq!(
        records::classify_exit(&records::read_record(&out).unwrap()),
        0
    );
    assert!(records::read_record(&out[..out.len() - 1]).is_err());
    let double = [out.clone(), out.clone()].concat();
    assert!(records::read_record(&double).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    value["unknown"] = json!(1);
    let mut unknown = serde_json::to_vec(&value).unwrap();
    unknown.push(b'\n');
    assert!(records::read_record(&unknown).is_err());
}
#[test]
fn classification_never_promotes_missing_timing_failed_oracle_cleanup_or_negative_role() {
    for change in 0..7 {
        let mut r = empty();
        match change {
            0 => r.elapsed_ns = None,
            1 => r.oracle.passed = false,
            2 => r.cleanup.release_ok = false,
            3 => r.cleanup.runtime_shutdown_bounded = false,
            4 => r.sample_eligible = true,
            5 => r.failure = Some(catalog::HarnessError::new("test", "failure", "rejected")),
            _ => {
                r.outcome = "verified_negative".into();
                r.case_kind = "negative_control".into();
                r.role = InvocationRole::Measured;
            }
        }
        assert_ne!(records::classify_exit(&r), 0);
    }
}
struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("injected write failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn output_evidence_identity_unknown_case_and_settings_fail_explicitly() {
    assert_eq!(
        records::write_record(&empty(), FailingWriter)
            .unwrap_err()
            .code,
        "write"
    );
    let path = temporary("file");
    std::fs::write(&path, b"occupied").unwrap();
    let record = worker::run_empty_worker(worker::EmptyWorkerOptions {
        invocation_id: "test".into(),
        evidence_dir: path.clone(),
    })
    .unwrap();
    assert_ne!(records::classify_exit(&record), 0);
    assert_eq!(record.failure.unwrap().stage, "evidence");
    std::fs::remove_file(path).unwrap();
    let invalid = worker::run_empty_worker(worker::EmptyWorkerOptions {
        invocation_id: "../escape".into(),
        evidence_dir: temporary("invalid"),
    })
    .unwrap();
    assert_eq!(invalid.failure.unwrap().code, "invocation_id");
    for (id, mut settings) in [
        ("unknown", catalog::Settings::default()),
        (
            "expressions_s8_c1",
            catalog::case("expressions_s8_c1").unwrap().settings,
        ),
    ] {
        settings.rate = 99;
        let record = worker::run_worker(worker::WorkerOptions {
            case_id: id.into(),
            fixture_dir: PathBuf::new(),
            data_endpoint: "http://127.0.0.1:9".into(),
            control_endpoint: "127.0.0.1:9".parse().unwrap(),
            invocation_id: "invalid".into(),
            role: InvocationRole::Verification,
            settings,
            evidence_dir: temporary("config"),
        })
        .unwrap();
        assert!(record.failure.is_some());
        assert!(record.elapsed_ns.is_none());
        assert!(!record.sample_eligible);
    }
}
#[test]
fn worker_budgets_are_explicit_and_empty_worker_completes_within_setup_budget() {
    assert_eq!(worker::SETUP_BUDGET.as_secs(), 5);
    assert_eq!(worker::EXECUTION_BUDGET.as_secs(), 60);
    assert_eq!(worker::CLEANUP_BUDGET.as_secs(), 5);
    assert_eq!(worker::WHOLE_BUDGET.as_secs(), 75);
    let start = Instant::now();
    let _ = empty();
    assert!(start.elapsed() < worker::SETUP_BUDGET);
}

use fixtures::{Preflight, PreparedManifest, ValidationReceipt};
use std::path::Path;
fn write_prepared(dir: &Path, case: &catalog::CaseSpec, endpoint: &str) {
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

fn fake_control(
    fault: &'static str,
    delay: std::time::Duration,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let join = std::thread::spawn(move || {
        let deadline = Instant::now() + std::time::Duration::from_secs(4);
        let mut calls = Vec::new();
        while Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(std::time::Duration::from_millis(1));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            let request: protocol::Request =
                protocol::decode_frame(&bytes, protocol::REQUEST_CAP).unwrap();
            let op = request.op().to_owned();
            calls.push(op.clone());
            let (mut identity, payload) = match request {
                protocol::Request::Begin {
                    identity,
                    session_budget_ms,
                } => (identity, json!({"session_budget_ms":session_budget_ms})),
                protocol::Request::Ready { identity } => (
                    identity,
                    json!({"ready":fault!="readiness","arrived_paths":[],"stalled_paths":[]}),
                ),
                protocol::Request::Snapshot { identity } => (
                    identity,
                    json!({"requests":[],"active_connections":0,"rejected_requests":0}),
                ),
                protocol::Request::Release { identity } => {
                    (identity, json!({"active_connections":0,"joined":true}))
                }
                _ => panic!("unexpected stop"),
            };
            if fault == "identity" && op == "ready" {
                identity.invocation_id = "wrong".into();
            }
            std::thread::park_timeout(delay);
            let response = if fault == "teardown" && op == "release" {
                json!({"op":op,"ok":false,"code":"release_failed","message":"injected release failure"})
            } else {
                json!({"op":op,"ok":true,"identity":identity,"payload":payload})
            };
            let mut reply = serde_json::to_vec(&response).unwrap();
            reply.push(b'\n');
            stream.write_all(&reply).unwrap();
            if op == "release" {
                break;
            }
        }
        calls
    });
    (endpoint, join)
}
fn source_options(dir: &Path, control_endpoint: std::net::SocketAddr) -> worker::WorkerOptions {
    let case = catalog::case("control_missing_source").unwrap();
    worker::WorkerOptions {
        case_id: case.case_id,
        fixture_dir: dir.join("fixtures"),
        data_endpoint: "http://127.0.0.1:32123".into(),
        control_endpoint,
        invocation_id: "source_test".into(),
        role: InvocationRole::Verification,
        settings: case.settings,
        evidence_dir: dir.join("evidence"),
    }
}
fn source_directory() -> PathBuf {
    let dir = temporary("source");
    std::fs::create_dir_all(dir.join("fixtures")).unwrap();
    write_prepared(
        &dir.join("fixtures"),
        &catalog::case("control_missing_source").unwrap(),
        "http://127.0.0.1:32123",
    );
    dir
}
#[test]
fn untimed_control_delays_do_not_enter_source_build_interval_and_preflight_is_retained() {
    let dir = source_directory();
    let (endpoint, join) = fake_control("none", std::time::Duration::from_millis(80));
    let start = Instant::now();
    let record = worker::run_worker(source_options(&dir, endpoint)).unwrap();
    let wall = start.elapsed();
    assert_eq!(records::classify_exit(&record), 0, "{:?}", record.failure);
    assert!(wall >= std::time::Duration::from_millis(300));
    assert!(u128::from(record.elapsed_ns.unwrap()) < wall.as_nanos() / 2);
    assert_eq!(record.actual.terminal_kind, "SourceDescriptionLoad");
    assert_eq!(
        record.actual.runtime_code.as_deref(),
        Some("RUNTIME_SOURCE_DESCRIPTION_LOAD")
    );
    assert!(record
        .evidence_files
        .iter()
        .any(|e| e.path.ends_with("preflight.json")));
    assert_eq!(
        join.join().unwrap(),
        ["begin", "ready", "snapshot", "release"]
    );
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn wrong_identity_readiness_unavailable_service_mutated_fixture_and_teardown_are_invalid() {
    for fault in ["identity", "readiness", "teardown"] {
        let dir = source_directory();
        let (endpoint, join) = fake_control(fault, std::time::Duration::ZERO);
        let record = worker::run_worker(source_options(&dir, endpoint)).unwrap();
        assert_ne!(records::classify_exit(&record), 0, "{fault}");
        assert!(record.failure.is_some());
        assert!(!record.sample_eligible);
        assert!(join.join().unwrap().contains(&"release".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }
    let dir = source_directory();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let unavailable = worker::run_worker(source_options(&dir, endpoint)).unwrap();
    assert_eq!(unavailable.failure.unwrap().code, "connect");
    std::fs::write(dir.join("fixtures/control_missing_source/root.json"), b"{}").unwrap();
    let mutated = worker::run_worker(source_options(&dir, endpoint)).unwrap();
    assert_eq!(mutated.failure.unwrap().code, "byte_identity");
    assert!(mutated.elapsed_ns.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn watchdog_child() {
    if let Ok(stage) = std::env::var("ARAZZO_WORKER_WATCHDOG_STAGE") {
        worker::exercise_watchdog(&stage);
        panic!("watchdog did not exit");
    }
}
#[test]
fn independent_watchdog_bounds_blocking_setup_execution_cleanup_and_whole_process() {
    for stage in ["setup", "execution", "cleanup", "whole"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "watchdog_child", "--nocapture"])
            .env("ARAZZO_WORKER_WATCHDOG_STAGE", stage)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(124), "{stage}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("watchdog child exceeded driver deadline: {stage}");
            }
            std::thread::park_timeout(std::time::Duration::from_millis(1));
        }
    }
}
#[test]
fn execution_deadline_drops_handle_and_never_returns_a_fake_duration_or_result() {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", server.server_addr());
    let case = catalog::case("expressions_s8_c1").unwrap();
    let dir = temporary("deadline");
    std::fs::create_dir(&dir).unwrap();
    for (file, bytes) in fixtures::generated_files(&case, &endpoint).unwrap() {
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
    let response = std::thread::spawn(move || {
        let request = server
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        std::thread::park_timeout(std::time::Duration::from_millis(100));
        let _ = request.respond(tiny_http::Response::from_string("{}"));
    });
    let error = rt
        .block_on(worker::execute_with_budget(
            &engine,
            &case,
            std::time::Duration::from_millis(25),
        ))
        .err()
        .unwrap();
    assert_eq!(error.code, "deadline");
    drop(engine);
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    response.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn settings_admission_rejects_unknown_fields_large_files_and_directories() {
    let dir = temporary("settings");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("settings.json");
    let settings = catalog::Settings::default();
    std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    assert_eq!(worker::read_settings(&path).unwrap(), settings);
    let mut value = serde_json::to_value(&settings).unwrap();
    value["unknown"] = json!(true);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        worker::read_settings(&path).unwrap_err().code,
        "settings_parse"
    );
    std::fs::write(&path, vec![b'x'; 16385]).unwrap();
    assert_eq!(
        worker::read_settings(&path).unwrap_err().code,
        "settings_file"
    );
    assert_eq!(
        worker::read_settings(&dir).unwrap_err().code,
        "settings_file"
    );
    #[cfg(unix)]
    {
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(worker::read_settings(&path).unwrap(), settings);
        let link = dir.join("settings-link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            worker::read_settings(&link).unwrap_err().code,
            "settings_file"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn completion_modes_preserve_outputs_and_result_only_has_no_fabricated_events() {
    for mode in [
        catalog::HandleMode::Collect,
        catalog::HandleMode::ResultOnly,
    ] {
        let case = catalog::case("expressions_s8_c64").unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", server.server_addr());
        let dir = temporary("completion");
        std::fs::create_dir(&dir).unwrap();
        for (file, bytes) in fixtures::generated_files(&case, &endpoint).unwrap() {
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
        .trace(true)
        .build()
        .unwrap();
        drop(entered);
        let response = std::thread::spawn(move || {
            for i in 0..8 {
                let request = server
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                    .unwrap();
                assert_eq!(request.url(), format!("/s0/op{i}"));
                std::thread::park_timeout(std::time::Duration::from_millis(5));
                request
                    .respond(tiny_http::Response::from_data(
                        fixtures::payload_bytes(i, 1024, 12).unwrap(),
                    ))
                    .unwrap();
            }
        });
        let start = Instant::now();
        let observed = rt.block_on(async {
            let handle = engine.execute("workload", std::collections::BTreeMap::new());
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                worker::complete(handle, mode),
            )
            .await
            .unwrap()
        });
        assert!(start.elapsed() >= std::time::Duration::from_millis(40));
        let expected: std::collections::BTreeMap<_, _> = case
            .expected
            .outputs
            .iter()
            .map(|(name, value)| (name.clone(), fixtures::materialize_expected(value).unwrap()))
            .collect();
        assert_eq!(observed.outputs.unwrap(), expected);
        if mode == catalog::HandleMode::ResultOnly {
            assert!(observed.events.is_empty());
        } else {
            assert!(observed
                .events
                .iter()
                .any(|event| matches!(event, arazzo_runtime::EngineEvent::TraceStep(_))));
        }
        drop(engine);
        rt.shutdown_timeout(std::time::Duration::from_secs(1));
        response.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
