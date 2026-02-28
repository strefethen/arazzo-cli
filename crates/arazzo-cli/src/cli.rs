use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use crate::trace::{parse_trace_max_body_bytes, TRACE_BODY_PREVIEW_DEFAULT_BYTES};

#[derive(Parser, Debug)]
#[command(name = "arazzo")]
#[command(about = "Execute Arazzo 1.0 workflows")]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Run {
        spec: String,
        workflow_id: String,

        #[arg(short = 'i', long = "input")]
        input: Vec<String>,

        #[arg(long = "input-json")]
        input_json: Vec<String>,

        #[arg(
            short = 't',
            long = "http-timeout",
            default_value = "30s",
            value_parser = parse_duration_value
        )]
        http_timeout: Duration,

        #[arg(
            long = "execution-timeout",
            default_value = "5m",
            value_parser = parse_duration_value
        )]
        execution_timeout: Duration,

        #[arg(short = 'H', long = "header")]
        header: Vec<String>,

        #[arg(long = "openapi")]
        openapi: Vec<String>,

        #[arg(
            long = "expr-diagnostics",
            value_enum,
            default_value_t = ExpressionDiagnosticsMode::Off
        )]
        expr_diagnostics: ExpressionDiagnosticsMode,

        #[arg(long)]
        parallel: bool,

        #[arg(long = "dry-run")]
        dry_run: bool,

        #[arg(long = "trace")]
        trace: Option<String>,

        #[arg(
            long = "trace-max-body-bytes",
            default_value_t = TRACE_BODY_PREVIEW_DEFAULT_BYTES,
            value_parser = parse_trace_max_body_bytes
        )]
        trace_max_body_bytes: usize,
    },
    Validate {
        spec: String,
    },
    List {
        spec: String,
    },
    Catalog {
        dir: String,
    },
    Show {
        workflow_id: String,
        #[arg(long = "dir", default_value = ".")]
        dir: String,
    },
    /// Print JSON Schema for a command's --json output
    Schema {
        /// Command name (validate, list, catalog, show, run). Omit to list available commands.
        command: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ExpressionDiagnosticsMode {
    Off,
    Warn,
    Error,
}

pub fn parse_duration_value(raw: &str) -> Result<Duration, String> {
    if let Ok(seconds) = raw.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    humantime::parse_duration(raw).map_err(|err| format!("invalid timeout \"{raw}\": {err}"))
}
