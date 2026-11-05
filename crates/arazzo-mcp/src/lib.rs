#![forbid(unsafe_code)]

//! MCP (Model Context Protocol) server for Arazzo workflow execution.
//!
//! Exposes Arazzo workflows as discoverable, invocable tools for AI agents
//! over JSON-RPC 2.0 stdio transport with Content-Length framing.

pub mod handlers;
pub mod protocol;
pub mod state;
mod timeout_policy;
pub mod tools;

#[derive(Default)]
pub struct McpServerConfig {
    pub allowed_dirs: Option<Vec<String>>,
    pub(crate) timeouts: timeout_policy::TimeoutPolicy,
}

use std::io::{BufRead, Read, Write};

/// Runs the MCP server over stdio.
///
/// Parses specs from `spec_paths`, then enters the Content-Length framed
/// JSON-RPC server loop reading from `reader` and writing to `writer`.
///
/// This function creates its own tokio runtime internally for async workflow
/// execution. Do NOT call from within an existing tokio runtime — use
/// `tokio::task::spawn_blocking` to escape first.
pub fn run_mcp_stdio<R, W>(
    reader: R,
    writer: &mut W,
    spec_paths: &[String],
    allowed_dirs: Option<Vec<String>>,
) -> Result<(), String>
where
    R: BufRead + Read + Send + 'static,
    W: Write,
{
    run_mcp_stdio_configured(
        reader,
        writer,
        spec_paths,
        McpServerConfig {
            allowed_dirs,
            ..McpServerConfig::default()
        },
    )
}

pub fn run_mcp_stdio_configured<R, W>(
    reader: R,
    writer: &mut W,
    spec_paths: &[String],
    config: McpServerConfig,
) -> Result<(), String>
where
    R: BufRead + Read + Send + 'static,
    W: Write,
{
    let server_state = state::ServerState::load(spec_paths, config.allowed_dirs)?;
    let _ = config.timeouts;

    // Diagnostics go to stderr only when specs are loaded (not in bare MCP mode).
    if !spec_paths.is_empty() {
        eprintln!(
            "arazzo-mcp: loaded {} spec(s) with {} workflow(s)",
            server_state.specs.len(),
            server_state.all_workflows().len(),
        );
    }

    protocol::serve(reader, writer, &server_state)
}
