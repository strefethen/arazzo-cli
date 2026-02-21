use std::time::Duration;

/// Global CLI output and verbosity options shared by all commands.
#[derive(Debug, Clone, Copy)]
pub struct GlobalOptions {
    pub json: bool,
    pub verbose: bool,
}

/// Reserved debug toggles for upcoming replay/debugger features.
#[derive(Debug, Clone, Copy, Default)]
pub struct DebugFlags {
    pub capture_runtime_trace: bool,
    pub capture_execution_events: bool,
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

/// Centralized run context with global options and future debug controls.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub global: GlobalOptions,
    pub run: RunOptions,
    pub debug: DebugFlags,
}

impl RunContext {
    pub fn new(global: GlobalOptions, run: RunOptions) -> Self {
        Self {
            global,
            run,
            debug: DebugFlags::default(),
        }
    }
}
