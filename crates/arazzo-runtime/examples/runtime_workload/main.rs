#![forbid(unsafe_code)]

#[allow(dead_code)]
mod catalog;
#[allow(dead_code)]
mod fixtures;
#[allow(dead_code)]
mod protocol;
mod service;
mod source_check;

use catalog::HarnessError;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

fn run() -> Result<(), HarnessError> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| {
        HarnessError::new("arguments", "command", "prepare or service is required")
    })?;
    if command == "worker" || command == "empty-worker" {
        return Err(HarnessError::new(
            "arguments",
            "unimplemented_slice",
            "worker commands are owned by the next implementation slice",
        ));
    }
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
    if let Err(error) = run() {
        eprintln!("{}", serde_json::json!({"error":error}));
        std::process::exit(1);
    }
}
