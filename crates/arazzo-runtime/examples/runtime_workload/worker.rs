use super::catalog::{self, CaseSpec, Family, HandleMode, HarnessError, Settings};
use super::execution_oracle::{self, OracleReport};
use super::fixtures::{self, PreparedCase};
use super::protocol::{ControlClient, Identity};
use super::records::{EvidenceFile, InvocationRole, RawRecord};
use arazzo_runtime::{
    ClientConfig, Engine, EngineBuilder, EngineEvent, ExecutionHandle, RateLimitConfig,
    RuntimeError,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const SETUP_BUDGET: Duration = Duration::from_secs(5);
pub(crate) const EXECUTION_BUDGET: Duration = Duration::from_secs(60);
pub(crate) const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
pub(crate) const WHOLE_BUDGET: Duration = Duration::from_secs(75);

pub(crate) struct WorkerOptions {
    pub case_id: String,
    pub fixture_dir: PathBuf,
    pub data_endpoint: String,
    pub control_endpoint: SocketAddr,
    pub invocation_id: String,
    pub role: InvocationRole,
    pub settings: Settings,
    pub evidence_dir: PathBuf,
}
pub(crate) struct EmptyWorkerOptions {
    pub invocation_id: String,
    pub evidence_dir: PathBuf,
}
pub(crate) struct ObservedExecution {
    pub outputs: Result<BTreeMap<String, Value>, RuntimeError>,
    pub events: Vec<EngineEvent>,
}
pub(crate) struct TimedExecution {
    pub elapsed_ns: u64,
    pub actual: ObservedExecution,
}

enum WatchdogMessage {
    Phase(Duration, Instant),
    Done,
}
pub(crate) struct Watchdog {
    tx: mpsc::Sender<WatchdogMessage>,
    join: Option<thread::JoinHandle<()>>,
}
impl Watchdog {
    fn arm() -> Result<Self, HarnessError> {
        Self::arm_with(SETUP_BUDGET, WHOLE_BUDGET)
    }
    fn arm_with(setup: Duration, whole_budget: Duration) -> Result<Self, HarnessError> {
        let (tx, rx) = mpsc::channel();
        let start = Instant::now();
        let join = thread::Builder::new()
            .name("workload-watchdog".into())
            .spawn(move || {
                let whole = start + whole_budget;
                let mut deadline = start + setup;
                loop {
                    let message = rx.recv_timeout(
                        deadline
                            .min(whole)
                            .saturating_duration_since(Instant::now()),
                    );
                    if Instant::now() >= deadline.min(whole) {
                        std::process::exit(124);
                    }
                    match message {
                        Ok(WatchdogMessage::Done) => break,
                        Ok(WatchdogMessage::Phase(budget, requested)) => {
                            deadline = requested + budget;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            std::process::exit(124);
                        }
                    }
                }
            })
            .map_err(|e| HarnessError::new("setup", "watchdog", e))?;
        Ok(Self {
            tx,
            join: Some(join),
        })
    }
    fn phase(&self, duration: Duration) -> Result<(), HarnessError> {
        self.tx
            .send(WatchdogMessage::Phase(duration, Instant::now()))
            .map_err(|e| HarnessError::new("worker", "watchdog_channel", e))
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.tx.send(WatchdogMessage::Done);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn process_watchdog() -> Result<Watchdog, HarnessError> {
    Watchdog::arm()
}

pub(crate) fn read_settings(path: &Path) -> Result<Settings, HarnessError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| HarnessError::new("arguments", "settings_metadata", e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        return Err(HarnessError::new(
            "arguments",
            "settings_file",
            "bounded regular settings file required",
        ));
    }
    let bytes = fs::read(path).map_err(|e| HarnessError::new("arguments", "settings_read", e))?;
    serde_json::from_slice(&bytes).map_err(|e| HarnessError::new("arguments", "settings_parse", e))
}

pub(crate) async fn complete(handle: ExecutionHandle, mode: HandleMode) -> ObservedExecution {
    match mode {
        HandleMode::Collect => {
            let result = handle.collect().await;
            ObservedExecution {
                outputs: result.outputs,
                events: result.events,
            }
        }
        HandleMode::ResultOnly => ObservedExecution {
            outputs: handle.result_only().await,
            events: Vec::new(),
        },
    }
}

pub(crate) fn configured_builder(
    prepared: &PreparedCase,
    settings: &Settings,
    invocation_id: &str,
) -> EngineBuilder {
    let mut config = ClientConfig {
        timeout: Duration::from_millis(settings.http_timeout_ms),
        rate_limit: RateLimitConfig {
            requests_per_second: f64::from(settings.rate),
            burst: settings.burst as usize,
        },
        ..ClientConfig::default()
    };
    config
        .default_headers
        .insert(settings.invocation_header.clone(), invocation_id.into());
    EngineBuilder::new(prepared.root.clone())
        .source_base_dir(&prepared.source_base_dir)
        .client_config(config)
        .parallel(settings.parallel)
        .trace(settings.trace)
        .capture_run(settings.capture)
        .channel_capacity(settings.channel_capacity)
        .max_response_bytes(settings.response_cap as usize)
}

fn elapsed(start: Instant) -> Result<u64, HarnessError> {
    u64::try_from(start.elapsed().as_nanos()).map_err(|e| HarnessError::new("timing", "range", e))
}

pub(crate) async fn execute_case(
    engine: &Engine,
    case: &CaseSpec,
    _control: &ControlClient,
    _identity: &Identity,
) -> Result<TimedExecution, HarnessError> {
    execute_with_budget(engine, case, EXECUTION_BUDGET).await
}

pub(crate) async fn execute_with_budget(
    engine: &Engine,
    case: &CaseSpec,
    budget: Duration,
) -> Result<TimedExecution, HarnessError> {
    let start = Instant::now();
    let handle = engine.execute(&case.workflow_id, BTreeMap::new());
    let actual = tokio::time::timeout(budget, complete(handle, case.settings.handle_mode))
        .await
        .map_err(|e| HarnessError::new("execution", "deadline", e))?;
    let elapsed_ns = elapsed(start)?;
    Ok(TimedExecution { elapsed_ns, actual })
}

fn validate_invocation(id: &str) -> Result<(), HarnessError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(HarnessError::new(
            "setup",
            "invocation_id",
            "expected 1..128 ASCII letters, digits, underscores or hyphens",
        ));
    }
    Ok(())
}
fn evidence_directory(base: &Path, id: &str) -> Result<PathBuf, HarnessError> {
    validate_invocation(id)?;
    fs::create_dir_all(base).map_err(|e| HarnessError::new("evidence", "directory", e))?;
    let dir = base.join(id);
    fs::create_dir(&dir).map_err(|e| HarnessError::new("evidence", "fresh_invocation", e))?;
    Ok(dir)
}
fn save(
    record: &mut RawRecord,
    dir: &Path,
    name: &str,
    value: &Value,
) -> Result<String, HarnessError> {
    let path = dir.join(name);
    let bytes =
        serde_json::to_vec(value).map_err(|e| HarnessError::new("evidence", "encode", e))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|e| HarnessError::new("evidence", "open", e))?;
    file.write_all(&bytes)
        .and_then(|()| file.flush())
        .map_err(|e| HarnessError::new("evidence", "write", e))?;
    let path = path.to_string_lossy().into_owned();
    record.evidence_files.push(EvidenceFile {
        path: path.clone(),
        bytes: bytes.len() as u64,
    });
    Ok(path)
}
fn record_actual(
    record: &mut RawRecord,
    dir: &Path,
    actual: &ObservedExecution,
) -> Result<(), HarnessError> {
    record.actual.terminal_kind = execution_oracle::terminal(actual);
    if let Err(error) = &actual.outputs {
        record.actual.runtime_code = Some(error.code().into());
        record.actual.error_message = Some(error.message.clone());
    }
    record.actual.output_file = Some(save(
        record,
        dir,
        "outputs.json",
        &actual.outputs.as_ref().map_or(Value::Null, |o| json!(o)),
    )?);
    record.actual.event_file = Some(save(
        record,
        dir,
        "events.json",
        &json!(actual
            .events
            .iter()
            .map(execution_oracle::event_projection)
            .collect::<Vec<_>>()),
    )?);
    Ok(())
}
fn runtime() -> Result<tokio::runtime::Runtime, HarnessError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| HarnessError::new("setup", "runtime", e))
}

#[allow(dead_code)]
pub(crate) fn run_worker(options: WorkerOptions) -> Result<RawRecord, HarnessError> {
    let watchdog = Watchdog::arm()?;
    run_worker_with_watchdog(options, &watchdog)
}

pub(crate) fn run_worker_with_watchdog(
    options: WorkerOptions,
    watchdog: &Watchdog,
) -> Result<RawRecord, HarnessError> {
    let mut record = RawRecord::invalid(options.invocation_id.clone(), options.role);
    record.case_id = Some(options.case_id.clone());
    let mut session = false;
    let control = ControlClient::new(options.control_endpoint);
    let mut identity = None;
    let mut rt = None;
    let result = (|| -> Result<(), HarnessError> {
        validate_invocation(&options.invocation_id)?;
        let case = catalog::case(&options.case_id)?;
        if !matches!(
            case.family,
            Family::Sources | Family::Expressions | Family::Dag
        ) {
            return Err(HarnessError::new(
                "setup",
                "unimplemented_slice",
                &case.case_id,
            ));
        }
        record.metric = if case.family == Family::Sources {
            "source_build"
        } else {
            "workflow_completion"
        }
        .into();
        record.case_kind = if case.control.is_some() {
            "negative_control"
        } else {
            "positive"
        }
        .into();
        record.settings = Some(options.settings.clone());
        if options.settings != case.settings
            || options.role == InvocationRole::Empty
            || (case.control.is_some() && options.role != InvocationRole::Verification)
        {
            return Err(HarnessError::new(
                "setup",
                "configuration",
                "catalog settings and invocation role must match",
            ));
        }
        if !options.control_endpoint.ip().is_loopback() {
            return Err(HarnessError::new(
                "setup",
                "control_endpoint",
                "loopback required",
            ));
        }
        super::source_check::endpoint_check(&options.data_endpoint)?;
        let prepared = fixtures::load_prepared(&options.fixture_dir, &options.case_id)?;
        if prepared.manifest.endpoint != options.data_endpoint {
            return Err(HarnessError::new(
                "setup",
                "endpoint_identity",
                "prepared endpoint mismatch",
            ));
        }
        record.manifest_id = Some(prepared.manifest.manifest_id.clone());
        let dir = evidence_directory(&options.evidence_dir, &options.invocation_id)?;
        save(
            &mut record,
            &dir,
            "preflight.json",
            &json!(prepared.manifest),
        )?;
        identity = Some(Identity {
            catalog_version: catalog::CATALOG_VERSION,
            case_id: case.case_id.clone(),
            invocation_id: options.invocation_id.clone(),
            response_policy: case.response_policy.clone(),
        });
        let id = identity
            .as_ref()
            .ok_or_else(|| HarnessError::new("setup", "identity", "missing identity"))?;
        session = true;
        control.begin(id, Duration::from_secs(70))?;
        let ready = control.readiness(id)?;
        if !ready.ready || !ready.arrived_paths.is_empty() || !ready.stalled_paths.is_empty() {
            return Err(HarnessError::new("setup", "readiness", json!(ready)));
        }
        rt = Some(runtime()?);
        let runtime = rt
            .as_ref()
            .ok_or_else(|| HarnessError::new("setup", "runtime", "missing runtime"))?;
        let _entered = runtime.enter();
        let builder = configured_builder(&prepared, &options.settings, &options.invocation_id);
        if case.family == Family::Sources {
            watchdog.phase(EXECUTION_BUDGET)?;
            let start = Instant::now();
            let built = builder.build();
            record.elapsed_ns = Some(elapsed(start)?);
            match built {
                Ok(engine) => {
                    let timed = runtime.block_on(execute_case(&engine, &case, &control, id))?;
                    record_actual(&mut record, &dir, &timed.actual)?;
                    let snapshot = control.snapshot(id)?;
                    record.oracle =
                        execution_oracle::check_source(&engine, &case, &timed.actual, &snapshot);
                    record.service = Some(snapshot);
                }
                Err(error) => {
                    let actual = ObservedExecution {
                        outputs: Err(error),
                        events: Vec::new(),
                    };
                    record_actual(&mut record, &dir, &actual)?;
                    let snapshot = control.snapshot(id)?;
                    record.oracle = execution_oracle::check_execution(&case, &actual, &snapshot);
                    record.service = Some(snapshot);
                }
            }
        } else {
            let engine = match builder.build() {
                Ok(engine) => engine,
                Err(error) => {
                    let message = format!("{}: {}", error.code(), error);
                    record_actual(
                        &mut record,
                        &dir,
                        &ObservedExecution {
                            outputs: Err(error),
                            events: Vec::new(),
                        },
                    )?;
                    return Err(HarnessError::new("setup", "engine_build", message));
                }
            };
            watchdog.phase(EXECUTION_BUDGET)?;
            let timed = runtime.block_on(execute_case(&engine, &case, &control, id))?;
            record.elapsed_ns = Some(timed.elapsed_ns);
            record_actual(&mut record, &dir, &timed.actual)?;
            let snapshot = control.snapshot(id)?;
            record.oracle = execution_oracle::check_execution(&case, &timed.actual, &snapshot);
            record.service = Some(snapshot);
        }
        if !record.oracle.passed {
            return Err(HarnessError::new(
                "oracle",
                "mismatch",
                "independent execution oracle rejected invocation",
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        record.failure = Some(error);
    }
    watchdog.phase(CLEANUP_BUDGET)?;
    if session {
        if let Some(id) = &identity {
            if record.service.is_none() {
                match control.snapshot(id) {
                    Ok(snapshot) => record.service = Some(snapshot),
                    Err(e) => {
                        record.failure.get_or_insert(e);
                    }
                }
            }
            match control.release(id) {
                Ok(receipt) => {
                    record.cleanup.release_ok = true;
                    record.cleanup.active_connections =
                        u32::try_from(receipt.active_connections).ok();
                }
                Err(error) => {
                    record.failure.get_or_insert(error);
                }
            }
        }
    }
    if let Some(runtime) = rt {
        runtime.shutdown_timeout(CLEANUP_BUDGET);
    }
    record.cleanup.runtime_shutdown_bounded = true;
    if record.failure.is_none() && record.oracle.passed && record.cleanup.release_ok {
        record.outcome = if record.case_kind == "negative_control" {
            "verified_negative"
        } else {
            "valid"
        }
        .into();
        record.sample_eligible =
            record.outcome == "valid" && record.role == InvocationRole::Measured;
    }
    Ok(record)
}

#[allow(dead_code)]
pub(crate) fn run_empty_worker(options: EmptyWorkerOptions) -> Result<RawRecord, HarnessError> {
    let watchdog = Watchdog::arm()?;
    run_empty_worker_with_watchdog(options, &watchdog)
}

pub(crate) fn run_empty_worker_with_watchdog(
    options: EmptyWorkerOptions,
    watchdog: &Watchdog,
) -> Result<RawRecord, HarnessError> {
    let mut record = RawRecord::invalid(options.invocation_id.clone(), InvocationRole::Empty);
    let result = (|| -> Result<(), HarnessError> {
        let dir = evidence_directory(&options.evidence_dir, &options.invocation_id)?;
        let rt = runtime()?;
        watchdog.phase(EXECUTION_BUDGET)?;
        let start = Instant::now();
        rt.block_on(async {});
        record.elapsed_ns = Some(elapsed(start)?);
        record.actual.terminal_kind = "empty".into();
        record.oracle = OracleReport::default();
        record
            .oracle
            .compare("empty/no_runtime_calls", json!(true), json!(true));
        save(
            &mut record,
            &dir,
            "empty.json",
            &json!({"runtime_calls":0,"service_calls":0}),
        )?;
        watchdog.phase(CLEANUP_BUDGET)?;
        rt.shutdown_timeout(CLEANUP_BUDGET);
        record.cleanup.release_ok = true;
        record.cleanup.runtime_shutdown_bounded = true;
        record.outcome = "empty".into();
        Ok(())
    })();
    if let Err(error) = result {
        record.failure = Some(error);
    }
    Ok(record)
}

#[cfg(test)]
pub(crate) fn exercise_watchdog(stage: &str) {
    let short = Duration::from_millis(40);
    let watchdog = Watchdog::arm_with(
        if stage == "setup" {
            short
        } else {
            Duration::from_secs(1)
        },
        if stage == "whole" {
            short
        } else {
            Duration::from_secs(2)
        },
    )
    .unwrap();
    if stage == "execution" {
        watchdog.phase(short).unwrap();
    }
    if stage == "cleanup" {
        watchdog.phase(short).unwrap();
    }
    if stage == "whole" {
        watchdog.phase(Duration::from_secs(1)).unwrap();
    }
    thread::park_timeout(Duration::from_secs(3));
}
