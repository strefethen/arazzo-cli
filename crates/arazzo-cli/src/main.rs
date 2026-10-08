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
mod transport;

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
        strict: cli.strict,
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
            insecure_host,
            insecure,
            allow_downgrade_redirects,
            max_redirects,
            no_transport_warnings,
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
                    transport: transport::TransportFlags {
                        insecure_hosts: insecure_host,
                        insecure_all: insecure,
                        allow_downgrade_redirects,
                        max_redirects,
                        no_transport_warnings,
                    },
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
            insecure_host,
            insecure,
            allow_downgrade_redirects,
            max_redirects,
            no_transport_warnings,
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
                transport::TransportFlags {
                    insecure_hosts: insecure_host,
                    insecure_all: insecure,
                    allow_downgrade_redirects,
                    max_redirects,
                    no_transport_warnings,
                },
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
        if let Some((key, value)) = parse_env_line(&line) {
            std::env::set_var(key, value);
        }
    }
}

/// Parses one `.env` line into the name and value to set, or `None` for a line
/// the loader skips: blank, `#` comment, no `=`, or one `std::env::set_var`
/// would panic on — an empty name, or a NUL byte in the name or value
/// (ac-342bd). A value wrapped in matching quotes is unwrapped and unescaped;
/// a lone quote is a literal one-character value.
fn parse_env_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    let trimmed = value.trim();
    let value = if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\'", "'")
            .replace("\\\\", "\\")
    } else {
        trimmed.to_string()
    };
    if key.is_empty() || key.contains('\0') || value.contains('\0') {
        return None;
    }
    Some((key.to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_line(line: &str) -> Option<(String, String)> {
        parse_env_line(line)
    }

    fn pair(key: &str, value: &str) -> Option<(String, String)> {
        Some((key.to_string(), value.to_string()))
    }

    #[test]
    fn env_lines_set_var_would_panic_on_are_skipped() {
        for line in ["=v", "  =v", "K\0=v", "K=a\0b", "K=\"a\0b\""] {
            assert_eq!(env_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn env_lines_without_a_setting_are_skipped() {
        for line in ["", "   ", "# comment", "  # indented", "NO_SEPARATOR"] {
            assert_eq!(env_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn env_line_parsing_is_unchanged_for_valid_lines() {
        assert_eq!(env_line("K=\""), pair("K", "\""));
        assert_eq!(env_line("K='"), pair("K", "'"));
        assert_eq!(env_line("K=\"\""), pair("K", ""));
        assert_eq!(env_line("K=''"), pair("K", ""));
        assert_eq!(env_line(r#"K="a\"b""#), pair("K", "a\"b"));
        assert_eq!(env_line(r"K='it\'s'"), pair("K", "it's"));
        assert_eq!(env_line(r#"K="a\\b""#), pair("K", r"a\b"));
        assert_eq!(env_line("K=a=b"), pair("K", "a=b"));
        assert_eq!(env_line(" K = v "), pair("K", "v"));
        assert_eq!(env_line("K="), pair("K", ""));
        assert_eq!(env_line("K=\"mismatched'"), pair("K", "\"mismatched'"));
    }

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
