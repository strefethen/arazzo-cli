//! `serve`: CLI startup of the MCP stdio server.

use std::io;

use arazzo_mcp::{McpLimits, McpServerConfig};

use crate::cli::ServeLimitArgs;
use crate::cli_error::CliError;

pub(crate) async fn serve(
    specs: Vec<String>,
    dir: Option<String>,
    allowed_dir: Vec<String>,
    limits: ServeLimitArgs,
) -> Result<(), CliError> {
    arazzo_mcp::print_operator_trust_notice();

    let mut paths = specs;
    if let Some(d) = dir {
        let discovered = arazzo_mcp::state::discover_specs(&d).map_err(|err| {
            CliError::new("SERVE_SPEC_DISCOVERY", format!("discovering specs: {err}"))
        })?;
        paths.extend(discovered);
    }
    if paths.is_empty() {
        return Err(CliError::new(
            "SERVE_NO_SPECS",
            "no spec files provided. Pass file paths or use --dir <directory>",
        ));
    }
    let config = McpServerConfig {
        allowed_dirs: (!allowed_dir.is_empty()).then_some(allowed_dir),
        limits: mcp_limits(&limits),
    };
    tokio::task::spawn_blocking(move || {
        let reader = io::BufReader::new(io::stdin());
        let mut writer = io::BufWriter::new(io::stdout().lock());
        arazzo_mcp::run_mcp_stdio_configured(reader, &mut writer, &paths, config)
    })
    .await
    .map_err(|err| CliError::new("SERVE_TASK_PANIC", format!("serve task panicked: {err}")))?
    .map_err(|err| CliError::new("SERVE_MCP", err))
}

fn mcp_limits(args: &ServeLimitArgs) -> McpLimits {
    let defaults = McpLimits::default();
    McpLimits {
        max_request_bytes: args.max_request_bytes.unwrap_or(defaults.max_request_bytes),
        max_pending_requests: args
            .max_pending_requests
            .unwrap_or(defaults.max_pending_requests),
        max_result_bytes: args.max_result_bytes.unwrap_or(defaults.max_result_bytes),
        max_response_body_bytes: args
            .max_response_body_bytes
            .unwrap_or(defaults.max_response_body_bytes),
        max_document_bytes: args
            .max_document_bytes
            .unwrap_or(defaults.max_document_bytes),
        max_http_timeout: args.max_http_timeout.unwrap_or(defaults.max_http_timeout),
        max_execution_timeout: args
            .max_execution_timeout
            .unwrap_or(defaults.max_execution_timeout),
    }
}
