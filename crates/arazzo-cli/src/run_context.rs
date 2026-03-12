use std::time::Duration;

use crate::cli::ExpressionDiagnosticsMode;

/// Global CLI output and verbosity options shared by all commands.
#[derive(Debug, Clone, Copy)]
pub struct GlobalOptions {
    pub json: bool,
    pub verbose: bool,
}

/// Run-command options parsed from CLI flags.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub spec_path: String,
    pub workflow_id: String,
    pub step_id: Option<String>,
    pub no_deps: bool,
    pub input_flags: Vec<String>,
    pub input_json_flags: Vec<String>,
    pub http_timeout: Duration,
    pub execution_timeout: Duration,
    pub header_flags: Vec<String>,
    pub openapi_flags: Vec<String>,
    pub expr_diagnostics: ExpressionDiagnosticsMode,
    pub parallel: bool,
    pub dry_run: bool,
    pub strict_inputs: bool,
    pub trace: Option<String>,
    pub trace_max_body_bytes: usize,
    pub max_response_size: Option<usize>,
}

/// Centralized run context with global options.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub global: GlobalOptions,
    pub run: RunOptions,
}

impl RunContext {
    pub fn new(global: GlobalOptions, run: RunOptions) -> Self {
        Self { global, run }
    }
}
