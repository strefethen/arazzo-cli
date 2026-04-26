#![forbid(unsafe_code)]

//! CLI for executing and debugging Arazzo 1.0.1 API workflow specifications.
//!
//! Commands: `run`, `replay`, `validate`, `list`, `steps`, `catalog`, `show`,
//! `generate`, `schema`, `serve`.

mod cli;
mod generate;
mod handlers;
mod output;
mod run_context;
mod test_runner;
mod trace;

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;

use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::run_context::{GlobalOptions, RunContext, RunOptions};

fn main() {
    // Load .env before starting the tokio runtime so that std::env::set_var
    // is called from a single-threaded context (safe per Rust docs).
    load_env_file(".env");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| panic!("failed to build tokio runtime: {err}"))
        .block_on(async_main());
}

async fn async_main() {
    let cli = Cli::parse_from(normalize_args(std::env::args_os()));
    if let Err(err) = run(cli).await {
        if !err.is_empty() {
            eprintln!("{err}");
        }
        std::process::exit(1);
    }
}

fn normalize_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = args.into_iter().collect();
    normalize_generate_positional_spec(&mut args);
    args
}

fn normalize_generate_positional_spec(args: &mut Vec<OsString>) {
    let Some(generate_index) = args.iter().position(|arg| arg == "generate") else {
        return;
    };
    let tail = &args[generate_index + 1..];
    if tail
        .iter()
        .any(|arg| arg == "--spec" || arg.to_string_lossy().starts_with("--spec="))
    {
        return;
    }

    let mut skip_next = false;
    for (offset, current) in tail.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }

        let arg = current.to_string_lossy();
        if matches!(arg.as_ref(), "--scenario" | "--output" | "-o") {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--scenario=") || arg.starts_with("--output=") {
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }

        args.insert(generate_index + 1 + offset, OsString::from("--spec"));
        return;
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let global = GlobalOptions {
        json: cli.json,
        verbose: cli.verbose,
    };

    match cli.command {
        Commands::Run {
            spec,
            workflow_id,
            step,
            no_deps,
            input,
            input_json,
            http_timeout,
            execution_timeout,
            header,
            openapi,
            expr_diagnostics,
            parallel,
            dry_run,
            strict_inputs,
            trace,
            trace_max_body_bytes,
            max_response_size,
        } => {
            let context = RunContext::new(
                global,
                RunOptions {
                    spec_path: spec,
                    workflow_id,
                    step_id: step,
                    no_deps,
                    input_flags: input,
                    input_json_flags: input_json,
                    http_timeout,
                    execution_timeout,
                    header_flags: header,
                    openapi_flags: openapi,
                    expr_diagnostics,
                    parallel,
                    dry_run,
                    strict_inputs,
                    trace,
                    trace_max_body_bytes,
                    max_response_size,
                },
            );
            handlers::run_workflow(context).await
        }
        Commands::Replay {
            trace,
            spec,
            workflow_id,
            execution_timeout,
            openapi,
        } => {
            handlers::replay_trace(
                &trace,
                spec.as_deref(),
                workflow_id.as_deref(),
                &openapi,
                execution_timeout,
                global,
            )
            .await
        }
        Commands::Validate { spec } => handlers::validate_spec(&spec, global),
        Commands::List { spec } => handlers::list_workflows(&spec, global),
        Commands::Catalog { dir } => handlers::catalog_workflows(&dir, global),
        Commands::Show { workflow_id, dir } => handlers::show_workflow(&workflow_id, &dir, global),
        Commands::Steps { spec, workflow_id } => handlers::list_steps(&spec, &workflow_id, global),
        Commands::Generate {
            spec,
            scenario,
            output,
        } => handlers::generate_workflow(&spec, &scenario, output.as_deref(), global),
        Commands::Schema { command } => handlers::schema(command.as_deref()),
        Commands::Test {
            paths,
            format,
            input,
            input_json,
            http_timeout,
            execution_timeout,
            header,
            openapi,
            expr_diagnostics,
            parallel,
            strict_inputs,
            max_response_size,
            fail_fast,
            filter,
        } => {
            handlers::run_tests(
                paths,
                format,
                input,
                input_json,
                http_timeout,
                execution_timeout,
                header,
                openapi,
                expr_diagnostics,
                parallel,
                strict_inputs,
                max_response_size,
                fail_fast,
                filter,
                global,
            )
            .await
        }
        Commands::Serve {
            specs,
            dir,
            allowed_dir,
        } => {
            let mut paths = specs;
            if let Some(d) = dir {
                let discovered = arazzo_mcp::state::discover_specs(&d)
                    .map_err(|err| format!("discovering specs: {err}"))?;
                paths.extend(discovered);
            }
            if paths.is_empty() {
                return Err(
                    "no spec files provided. Pass file paths or use --dir <directory>".to_string(),
                );
            }
            let allowed = if allowed_dir.is_empty() {
                None
            } else {
                Some(allowed_dir)
            };
            // Escape the existing tokio runtime before calling run_mcp_stdio,
            // which creates its own runtime internally.
            tokio::task::spawn_blocking(move || {
                let reader = io::BufReader::new(io::stdin());
                let mut writer = io::BufWriter::new(io::stdout().lock());
                arazzo_mcp::run_mcp_stdio(reader, &mut writer, &paths, allowed)
            })
            .await
            .map_err(|err| format!("serve task panicked: {err}"))?
        }
    }
}

fn load_env_file(path: impl AsRef<Path>) {
    let file = match fs::File::open(path.as_ref()) {
        Ok(file) => file,
        Err(_) => return,
    };

    let reader = io::BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(v) => v,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let trimmed = value.trim();
        let value = if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            trimmed[1..trimmed.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\\\", "\\")
        } else {
            trimmed.to_string()
        };
        std::env::set_var(key, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn normalize_generate_positional_spec_inserts_spec_flag() {
        let args = normalize_args([
            OsString::from("arazzo"),
            OsString::from("generate"),
            OsString::from("petstore.json"),
            OsString::from("--scenario"),
            OsString::from("crud"),
        ]);

        assert_eq!(
            strings(&args),
            vec![
                "arazzo",
                "generate",
                "--spec",
                "petstore.json",
                "--scenario",
                "crud"
            ]
        );
    }

    #[test]
    fn normalize_generate_positional_spec_preserves_explicit_spec_flag() {
        let args = normalize_args([
            OsString::from("arazzo"),
            OsString::from("generate"),
            OsString::from("--spec"),
            OsString::from("petstore.json"),
            OsString::from("--scenario"),
            OsString::from("crud"),
        ]);

        assert_eq!(
            strings(&args),
            vec![
                "arazzo",
                "generate",
                "--spec",
                "petstore.json",
                "--scenario",
                "crud"
            ]
        );
    }
}
