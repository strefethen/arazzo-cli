use std::time::Duration;

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
    pub input_flags: Vec<String>,
    pub timeout: Duration,
    pub header_flags: Vec<String>,
    pub parallel: bool,
    pub dry_run: bool,
    pub trace: Option<String>,
    pub trace_max_body_bytes: usize,
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
