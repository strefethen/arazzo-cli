#![forbid(unsafe_code)]

//! CLI for executing and debugging Arazzo 1.0.1 API workflow specifications.
//!
//! Commands: `run`, `validate`, `list`, `catalog`, `show`, `schema`.
//! All commands support `--json` for structured output.

mod cli;
mod handlers;
mod output;
mod run_context;
mod trace;

use std::fs;
use std::io::{self, BufRead};
use std::path::Path;

use clap::Parser;

use crate::cli::{Cli, Commands};
use crate::run_context::{GlobalOptions, RunContext, RunOptions};

fn main() {
    load_env_file(".env");
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        if !err.is_empty() {
            eprintln!("{err}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
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
            trace,
            trace_max_body_bytes,
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
                    trace,
                    trace_max_body_bytes,
                },
            );
            handlers::run_workflow(context)
        }
        Commands::Validate { spec } => handlers::validate_spec(&spec, global),
        Commands::List { spec } => handlers::list_workflows(&spec, global),
        Commands::Catalog { dir } => handlers::catalog_workflows(&dir, global),
        Commands::Show { workflow_id, dir } => handlers::show_workflow(&workflow_id, &dir, global),
        Commands::Steps { spec, workflow_id } => handlers::list_steps(&spec, &workflow_id, global),
        Commands::Schema { command } => handlers::schema(command.as_deref()),
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
        let value = value.trim().trim_matches('"').trim_matches('\'');
        std::env::set_var(key, value);
    }
}
