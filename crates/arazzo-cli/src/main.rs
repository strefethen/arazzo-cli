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
    if let Some(report) = load_env_file(".env") {
        eprint_env_load_report(".env", &report);
    }
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

// `.env` loading. This and the copy in `arazzo-mcp/src/main.rs` are kept in lockstep: the
// same file must load and report identically from either binary (audit F20
// owns deduplicating them).

/// What `load_env_file` did with one `.env` file.
#[derive(Debug, Default, PartialEq, Eq)]
struct EnvLoadReport {
    /// Names the file set because the environment did not have them.
    set: usize,
    /// Names the file defined that were kept from the environment (ac-51491).
    kept: usize,
    /// 1-based line numbers of lines that could not be used, with the reason.
    ignored: Vec<(usize, IgnoredEnvLine)>,
    /// The 1-based line number and error where a hard I/O error ended
    /// reading — `.env` is a directory, say (ac-d11dc).
    stopped: Option<(usize, String)>,
}

/// Why a `.env` line was ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IgnoredEnvLine {
    Unreadable,
    NoSeparator,
    EmptyName,
    NulByte,
}

impl IgnoredEnvLine {
    fn reason(self) -> &'static str {
        match self {
            Self::Unreadable => "line could not be read",
            Self::NoSeparator => "no `=` separator",
            Self::EmptyName => "empty name",
            Self::NulByte => "NUL byte in name or value",
        }
    }
}

/// One parsed `.env` line.
#[derive(Debug, PartialEq, Eq)]
enum EnvLine {
    /// Blank or `#` comment: not a setting, never reported.
    Blank,
    Setting(String, String),
    Ignored(IgnoredEnvLine),
}

/// Loads `path` into the process environment, or returns `None` when there is
/// no file to read. The environment wins: a `.env` value is only a default for
/// a name that is not already set, even to the empty string (ac-51491).
fn load_env_file(path: impl AsRef<Path>) -> Option<EnvLoadReport> {
    let file = fs::File::open(path.as_ref()).ok()?;

    let mut report = EnvLoadReport::default();
    let reader = io::BufReader::new(file);
    for (index, line) in reader.lines().enumerate() {
        let parsed = match line {
            Ok(line) => parse_env_line(&line),
            // Invalid UTF-8: the line was consumed, so the next read moves on.
            Err(err) if err.kind() == io::ErrorKind::InvalidData => {
                EnvLine::Ignored(IgnoredEnvLine::Unreadable)
            }
            // Any other error would repeat on every read; stop here.
            Err(err) => {
                report.stopped = Some((index + 1, err.to_string()));
                break;
            }
        };
        match parsed {
            EnvLine::Blank => {}
            EnvLine::Setting(key, value) => {
                if std::env::var_os(&key).is_none() {
                    std::env::set_var(key, value);
                    report.set += 1;
                } else {
                    report.kept += 1;
                }
            }
            EnvLine::Ignored(why) => report.ignored.push((index + 1, why)),
        }
    }
    Some(report)
}

/// Writes `report` to stderr: one warning per ignored line, then one summary
/// line. Counts only — a `.env` commonly holds secrets, so no name or value
/// is ever printed.
fn eprint_env_load_report(path: &str, report: &EnvLoadReport) {
    for (line, why) in &report.ignored {
        eprintln!("warning: {path}:{line}: ignored line: {}", why.reason());
    }
    if let Some((line, err)) = &report.stopped {
        eprintln!("warning: {path}:{line}: stopped reading: {err}");
    }
    eprintln!(
        "loaded {path}: set {}, kept {} already in the environment, ignored {}",
        report.set,
        report.kept,
        report.ignored.len()
    );
}

/// Parses one `.env` line. A line `std::env::set_var` would panic on — an
/// empty name, or a NUL byte in the name or value (ac-342bd) — is ignored,
/// like one with no `=`. A value wrapped in matching quotes is unwrapped and
/// unescaped; a lone quote is a literal one-character value.
fn parse_env_line(line: &str) -> EnvLine {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return EnvLine::Blank;
    }
    let Some((key, value)) = line.split_once('=') else {
        return EnvLine::Ignored(IgnoredEnvLine::NoSeparator);
    };
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
    if key.is_empty() {
        return EnvLine::Ignored(IgnoredEnvLine::EmptyName);
    }
    if key.contains('\0') || value.contains('\0') {
        return EnvLine::Ignored(IgnoredEnvLine::NulByte);
    }
    EnvLine::Setting(key.to_string(), value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_line(line: &str) -> Option<(String, String)> {
        match parse_env_line(line) {
            EnvLine::Setting(key, value) => Some((key, value)),
            EnvLine::Blank | EnvLine::Ignored(_) => None,
        }
    }

    #[test]
    fn ignored_env_lines_carry_their_reason() {
        for (line, why) in [
            ("NO_SEPARATOR", IgnoredEnvLine::NoSeparator),
            ("=v", IgnoredEnvLine::EmptyName),
            ("  =v", IgnoredEnvLine::EmptyName),
            ("K\0=v", IgnoredEnvLine::NulByte),
            ("K=a\0b", IgnoredEnvLine::NulByte),
            ("K=\"a\0b\"", IgnoredEnvLine::NulByte),
        ] {
            assert_eq!(parse_env_line(line), EnvLine::Ignored(why), "{line:?}");
        }
        for line in ["", "   ", "# comment", "  # indented"] {
            assert_eq!(parse_env_line(line), EnvLine::Blank, "{line:?}");
        }
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

    /// Writes `contents` to a fresh `.env` in a per-test temp directory, loads
    /// it, and returns what the process environment holds for `key`. Each
    /// test uses its own variable name, so parallel tests cannot interfere.
    fn load_and_read(test: &str, key: &str, contents: &str) -> io::Result<Option<String>> {
        let dir =
            std::env::temp_dir().join(format!("arazzo-cli-env-{test}-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let path = dir.join(".env");
        fs::write(&path, contents)?;
        load_env_file(&path);
        fs::remove_dir_all(&dir)?;
        Ok(std::env::var(key).ok())
    }

    #[test]
    fn env_file_does_not_override_an_exported_variable() -> io::Result<()> {
        let key = "ARAZZO_CLI_TEST_EXPORTED";
        std::env::set_var(key, "from-env");
        let seen = load_and_read("exported", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some("from-env"));
        Ok(())
    }

    #[test]
    fn env_file_sets_an_absent_variable() -> io::Result<()> {
        let key = "ARAZZO_CLI_TEST_ABSENT";
        std::env::remove_var(key);
        let seen = load_and_read("absent", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some("from-file"));
        Ok(())
    }

    #[test]
    fn env_file_does_not_override_an_empty_exported_variable() -> io::Result<()> {
        let key = "ARAZZO_CLI_TEST_EMPTY";
        std::env::set_var(key, "");
        let seen = load_and_read("empty", key, &format!("{key}=from-file\n"))?;
        assert_eq!(seen.as_deref(), Some(""));
        Ok(())
    }
}
