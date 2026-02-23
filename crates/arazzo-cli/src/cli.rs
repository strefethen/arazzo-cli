use std::time::Duration;

use clap::{Parser, Subcommand};

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

        #[arg(
            short = 't',
            long = "timeout",
            default_value = "30s",
            value_parser = parse_duration_value
        )]
        timeout: Duration,

        #[arg(short = 'H', long = "header")]
        header: Vec<String>,

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
}

pub fn parse_duration_value(raw: &str) -> Result<Duration, String> {
    if let Ok(seconds) = raw.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    humantime::parse_duration(raw).map_err(|err| format!("invalid timeout \"{raw}\": {err}"))
}
