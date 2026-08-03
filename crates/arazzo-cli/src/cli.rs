use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use crate::trace::{parse_trace_max_body_bytes, TRACE_BODY_PREVIEW_DEFAULT_BYTES};

#[derive(Parser, Debug)]
#[command(name = "arazzo")]
#[command(
    version,
    about = "Execute Arazzo 1.0 workflows",
    long_about = "Execute, validate, inspect, generate, and test Arazzo 1.0 workflows.\n\nFor agents: use --json for machine-readable output, use schema <command> to discover the JSON contract, and use run --dry-run --json before a live run.",
    after_help = "For agents:\n  1. Discover workflows: arazzo-cli catalog <dir> --json or arazzo-cli list <spec> --json\n  2. Inspect one workflow: arazzo-cli show <workflow-id> --dir <dir> --json\n  3. Inspect steps: arazzo-cli steps <spec> <workflow-id> --json\n  4. Check output contracts: arazzo-cli schema <command>\n  5. Plan execution: arazzo-cli run <spec> <workflow-id> --dry-run --json\n  6. Capture replayable evidence: add --trace <trace.json> and replay it with arazzo-cli replay <trace.json> --json\n  7. Expose workflows to agents: arazzo-cli serve --dir <dir> --allowed-dir <dir>\n\nAll commands support global --json for stable stdout where a JSON contract exists. Diagnostics go to stderr."
)]
pub struct Cli {
    /// Emit machine-readable JSON on stdout when the command has a JSON contract.
    #[arg(long, global = true)]
    pub json: bool,

    /// Emit diagnostic progress to stderr without changing JSON stdout.
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Execute a workflow from an Arazzo spec.
    #[command(
        after_help = "For agents:\n  - Prefer --json for parseable success/error envelopes.\n  - Use --dry-run --json to preview requests without sending HTTP traffic.\n  - Use --trace <trace.json> to capture redacted replay evidence.\n  - Use schema run to inspect the JSON output contract."
    )]
    Run {
        /// Path to an Arazzo YAML spec file.
        spec: String,

        /// workflowId to execute from the spec.
        workflow_id: String,

        /// Execute a single step within the workflow (auto-resolves dependencies)
        #[arg(long = "step")]
        step: Option<String>,

        /// Skip dependency resolution when using --step (isolated execution)
        #[arg(long = "no-deps", requires = "step")]
        no_deps: bool,

        /// String input as key=value. Environment variables in values are expanded.
        #[arg(short = 'i', long = "input")]
        input: Vec<String>,

        /// JSON-typed input as key=<json-value>, preserving numbers, booleans, arrays, and objects.
        #[arg(long = "input-json")]
        input_json: Vec<String>,

        /// Per-request HTTP timeout, for example 500ms, 30s, or 2m.
        #[arg(
            short = 't',
            long = "http-timeout",
            default_value = "30s",
            value_parser = parse_duration_value
        )]
        http_timeout: Duration,

        /// Per-workflow execution timeout, for example 30s, 5m, or 1h.
        #[arg(
            long = "execution-timeout",
            default_value = "5m",
            value_parser = parse_duration_value
        )]
        execution_timeout: Duration,

        /// Custom HTTP header for every request, as 'Name: value' or Name=value. Repeatable.
        #[arg(short = 'H', long = "header")]
        header: Vec<String>,

        /// OpenAPI file used to resolve operationId targets. Repeatable.
        #[arg(long = "openapi")]
        openapi: Vec<String>,

        /// Expression evaluation diagnostics: off, warn, or error.
        #[arg(
            long = "expr-diagnostics",
            value_enum,
            default_value_t = ExpressionDiagnosticsMode::Off
        )]
        expr_diagnostics: ExpressionDiagnosticsMode,

        /// Enable parallel step execution where dependencies allow it.
        #[arg(long)]
        parallel: bool,

        /// Build and print the request plan without sending HTTP requests.
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Make input validation errors fatal (missing required fields, type mismatches)
        #[arg(long = "strict-inputs")]
        strict_inputs: bool,

        /// Write a redacted trace.v1 JSON file for replay and debugging.
        #[arg(long = "trace")]
        trace: Option<String>,

        /// Maximum response body preview bytes stored per trace step.
        #[arg(
            long = "trace-max-body-bytes",
            default_value_t = TRACE_BODY_PREVIEW_DEFAULT_BYTES,
            value_parser = parse_trace_max_body_bytes
        )]
        trace_max_body_bytes: usize,

        /// Maximum response body size in bytes (default: 10485760 = 10 MiB)
        #[arg(long = "max-response-size")]
        max_response_size: Option<usize>,
    },
    /// Replay a recorded trace.v1 file with deterministic response injection
    #[command(
        after_help = "For agents:\n  - Replay is deterministic and does not need live upstream responses recorded in the trace.\n  - Use --json for parseable replay success/error envelopes.\n  - Use schema replay to inspect the JSON output contract."
    )]
    Replay {
        /// Path to trace.v1 JSON file
        trace: String,

        /// Override spec path from trace.run.specPath
        #[arg(long = "spec")]
        spec: Option<String>,

        /// Override workflow id from trace.run.workflowId
        #[arg(long = "workflow-id")]
        workflow_id: Option<String>,

        #[arg(
            long = "execution-timeout",
            default_value = "5m",
            value_parser = parse_duration_value
        )]
        execution_timeout: Duration,

        /// OpenAPI file used to resolve operationId targets (repeatable)
        #[arg(long = "openapi")]
        openapi: Vec<String>,
    },
    /// Validate an Arazzo spec and report structural errors.
    #[command(
        after_help = "For agents:\n  - Use --json for structured valid/error output.\n  - Use schema validate to inspect the JSON output contract."
    )]
    Validate {
        /// Path to an Arazzo YAML spec file.
        spec: String,
    },
    /// List workflow IDs and summaries from one Arazzo spec.
    #[command(
        after_help = "For agents:\n  - Use --json to receive an array of workflows.\n  - Follow with show or steps for deeper inspection.\n  - Use schema list to inspect the JSON output contract."
    )]
    List {
        /// Path to an Arazzo YAML spec file.
        spec: String,
    },
    /// Scan a directory for Arazzo specs and summarize available workflows.
    #[command(
        after_help = "For agents:\n  - Use --json to receive an array of catalog entries.\n  - Follow with show <workflow-id> --dir <dir> --json.\n  - Use schema catalog to inspect the JSON output contract."
    )]
    Catalog {
        /// Directory containing .yaml or .yml Arazzo specs.
        dir: String,
    },
    /// Show one workflow's inputs, steps, and outputs from a spec directory.
    #[command(
        after_help = "For agents:\n  - Use --json to inspect inputs, outputs, and step summaries.\n  - Use steps <spec> <workflow-id> --json when you only need step rows.\n  - Use schema show to inspect the JSON output contract."
    )]
    Show {
        /// workflowId to find in the catalog directory.
        workflow_id: String,
        /// Directory containing Arazzo spec files.
        #[arg(long = "dir", default_value = ".")]
        dir: String,
    },
    /// List steps within a workflow
    #[command(
        after_help = "For agents:\n  - Use --json to receive an array of step rows.\n  - Use show <workflow-id> --dir <dir> --json when you need workflow inputs and outputs too.\n  - Use schema steps to inspect the JSON output contract."
    )]
    Steps {
        /// Path to an Arazzo YAML spec file.
        spec: String,

        /// workflowId whose steps should be listed.
        workflow_id: String,
    },
    /// Generate Arazzo workflows from an OpenAPI specification
    #[command(
        after_help = "For agents:\n  - Generated YAML is written to stdout unless --output is set.\n  - Use --json to receive generation metadata instead of YAML.\n  - Use schema generate to inspect the JSON output contract."
    )]
    Generate {
        /// Path to the OpenAPI 3.x spec (YAML or JSON)
        #[arg(long = "spec")]
        spec: String,

        /// Generation scenario
        #[arg(long = "scenario", default_value = "crud")]
        scenario: String,

        /// Write YAML to file instead of stdout
        #[arg(short = 'o', long = "output")]
        output: Option<String>,
    },
    /// Print JSON Schema for a command's --json output
    Schema {
        /// Command name (validate, list, catalog, show, steps, run, replay, generate). Omit to list available commands.
        command: Option<String>,
    },
    /// Run Arazzo specs as tests and report results
    #[command(
        after_help = "For agents:\n  - Use --json for a structured test report regardless of --format.\n  - Directories are scanned recursively for .arazzo.yaml and .arazzo.yml.\n  - Use schema test to inspect the JSON output contract."
    )]
    Test {
        /// Spec files or directories to test (directories scanned recursively
        /// for .arazzo.yaml / .arazzo.yml)
        #[arg(required = true)]
        paths: Vec<String>,

        /// Output format (overridden to json when --json is set)
        #[arg(long, value_enum, default_value_t = TestFormat::Tap)]
        format: TestFormat,

        /// Key=value inputs for all workflows
        #[arg(short = 'i', long = "input")]
        input: Vec<String>,

        /// JSON-typed inputs (key=<json-value>)
        #[arg(long = "input-json")]
        input_json: Vec<String>,

        /// Per-request HTTP timeout
        #[arg(
            short = 't',
            long = "http-timeout",
            default_value = "30s",
            value_parser = parse_duration_value
        )]
        http_timeout: Duration,

        /// Per-workflow execution timeout
        #[arg(
            long = "execution-timeout",
            default_value = "5m",
            value_parser = parse_duration_value
        )]
        execution_timeout: Duration,

        /// Custom HTTP headers
        #[arg(short = 'H', long = "header")]
        header: Vec<String>,

        /// Additional OpenAPI spec files for operationId resolution
        #[arg(long = "openapi")]
        openapi: Vec<String>,

        /// Expression evaluation diagnostics
        #[arg(
            long = "expr-diagnostics",
            value_enum,
            default_value_t = ExpressionDiagnosticsMode::Off
        )]
        expr_diagnostics: ExpressionDiagnosticsMode,

        /// Parallel step execution within each workflow
        #[arg(long)]
        parallel: bool,

        /// Make input validation errors fatal
        #[arg(long = "strict-inputs")]
        strict_inputs: bool,

        /// Maximum response body size in bytes
        #[arg(long = "max-response-size")]
        max_response_size: Option<usize>,

        /// Stop on first failure
        #[arg(long)]
        fail_fast: bool,

        /// Regex filter on workflow IDs
        #[arg(long)]
        filter: Option<String>,
    },
    /// Start an MCP server for AI agent integration
    #[command(
        after_help = "For agents:\n  - Load specs with positional files, --dir, or both.\n  - Use --allowed-dir to constrain file access exposed through MCP tools.\n  - This command speaks MCP over stdio; stdout is reserved for protocol messages."
    )]
    Serve {
        /// Arazzo spec files to load
        specs: Vec<String>,

        /// Directory containing .arazzo.yaml files to load
        #[arg(long = "dir")]
        dir: Option<String>,

        /// Restrict validate_spec file access to these directories (repeatable)
        #[arg(long = "allowed-dir")]
        allowed_dir: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum TestFormat {
    Json,
    Junit,
    Tap,
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
