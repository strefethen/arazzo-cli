#![forbid(unsafe_code)]

#[allow(dead_code)]
mod catalog;
mod execution_oracle;
#[allow(dead_code)]
mod fixtures;
#[allow(dead_code)]
mod protocol;
mod records;
mod service;
mod source_check;
mod worker;

use catalog::HarnessError;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

fn run(watchdog: Option<&worker::Watchdog>) -> Result<(), HarnessError> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| {
        HarnessError::new("arguments", "command", "prepare or service is required")
    })?;
    let mut flags = BTreeMap::new();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| HarnessError::new("arguments", "value", &flag))?;
        if flags.insert(flag.clone(), value).is_some() {
            return Err(HarnessError::new("arguments", "duplicate", flag));
        }
    }
    let mut required = |name: &str| {
        flags
            .remove(name)
            .ok_or_else(|| HarnessError::new("arguments", "missing", name))
    };
    match command.as_str() {
        "worker" => {
            let case_id = required("--case")?;
            let fixture_dir = PathBuf::from(required("--fixtures")?);
            let data_endpoint = required("--data-endpoint")?;
            let control_endpoint = required("--control-endpoint")?
                .parse()
                .map_err(|e| HarnessError::new("arguments", "control_endpoint", e))?;
            let invocation_id = required("--invocation-id")?;
            let role = match required("--role")?.as_str() {
                "verification" => records::InvocationRole::Verification,
                "warmup" => records::InvocationRole::Warmup,
                "measured" => records::InvocationRole::Measured,
                other => return Err(HarnessError::new("arguments", "role", other)),
            };
            let settings_path = PathBuf::from(required("--settings")?);
            let evidence_dir = PathBuf::from(required("--evidence-dir")?);
            if !flags.is_empty() {
                return Err(HarnessError::new(
                    "arguments",
                    "unknown",
                    format!("{flags:?}"),
                ));
            }
            let settings = worker::read_settings(&settings_path)?;
            let record = worker::run_worker_with_watchdog(
                worker::WorkerOptions {
                    case_id,
                    fixture_dir,
                    data_endpoint,
                    control_endpoint,
                    invocation_id,
                    role,
                    settings,
                    evidence_dir,
                },
                watchdog.ok_or_else(|| {
                    HarnessError::new("worker", "watchdog", "missing process watchdog")
                })?,
            )?;
            records::write_record(&record, std::io::stdout().lock())?;
            if records::classify_exit(&record) != 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        "empty-worker" => {
            let invocation_id = required("--invocation-id")?;
            let evidence_dir = PathBuf::from(required("--evidence-dir")?);
            if !flags.is_empty() {
                return Err(HarnessError::new(
                    "arguments",
                    "unknown",
                    format!("{flags:?}"),
                ));
            }
            let record = worker::run_empty_worker_with_watchdog(
                worker::EmptyWorkerOptions {
                    invocation_id,
                    evidence_dir,
                },
                watchdog.ok_or_else(|| {
                    HarnessError::new("worker", "watchdog", "missing process watchdog")
                })?,
            )?;
            records::write_record(&record, std::io::stdout().lock())?;
            if records::classify_exit(&record) != 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        "prepare" => {
            let output_dir = PathBuf::from(required("--output-dir")?);
            let data_endpoint = required("--data-endpoint")?;
            let cli_path = PathBuf::from(required("--cli")?);
            if !flags.is_empty() {
                return Err(HarnessError::new(
                    "arguments",
                    "unknown",
                    format!("{flags:?}"),
                ));
            }
            let manifests = fixtures::prepare(&fixtures::PrepareOptions {
                output_dir,
                data_endpoint,
                cli_path,
            })?;
            println!(
                "{}",
                serde_json::json!({"prepared":manifests.len(),"catalog_version":catalog::CATALOG_VERSION})
            );
            Ok(())
        }
        "service" => {
            let budget = required("--lifetime-budget-ms")?
                .parse::<u64>()
                .map_err(|e| HarnessError::new("arguments", "budget", e))?;
            if budget == 0 || !flags.is_empty() {
                return Err(HarnessError::new(
                    "arguments",
                    "budget_or_flags",
                    "positive budget and known flags required",
                ));
            }
            service::serve(service::ServiceOptions {
                lifetime_budget: Duration::from_millis(budget),
            })
        }
        _ => Err(HarnessError::new("arguments", "unknown_command", command)),
    }
}

fn main() {
    let worker_command = std::env::args()
        .nth(1)
        .is_some_and(|command| command == "worker" || command == "empty-worker");
    let _process_deadline = if worker_command {
        match worker::process_watchdog() {
            Ok(watchdog) => Some(watchdog),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    if let Err(error) = run(_process_deadline.as_ref()) {
        eprintln!("{}", serde_json::json!({"error":error}));
        let args: Vec<_> = std::env::args().collect();
        if args
            .get(1)
            .is_some_and(|command| command == "worker" || command == "empty-worker")
            && error.stage != "record"
        {
            let field = |flag: &str| {
                args.windows(2)
                    .find(|pair| pair[0] == flag)
                    .map(|pair| pair[1].clone())
            };
            let role = if args.get(1).is_some_and(|command| command == "empty-worker") {
                records::InvocationRole::Empty
            } else {
                match field("--role").as_deref() {
                    Some("measured") => records::InvocationRole::Measured,
                    Some("warmup") => records::InvocationRole::Warmup,
                    _ => records::InvocationRole::Verification,
                }
            };
            let mut record =
                records::RawRecord::invalid(field("--invocation-id").unwrap_or_default(), role);
            record.case_id = field("--case");
            record.failure = Some(error);
            if let Err(write_error) = records::write_record(&record, std::io::stdout().lock()) {
                eprintln!("{write_error}");
            }
        }
        std::process::exit(1);
    }
}
