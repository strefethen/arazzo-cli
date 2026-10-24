//! `serve`: CLI startup of the MCP stdio server.

use std::io;

use crate::cli_error::CliError;

/// Fixed MCP risk disclosure printed to stderr at every `serve` startup
/// (ac-62861). Constant text only: it never names paths, roots, inputs, or
/// environment values, and it claims no access control that is not delivered
/// (hardening is owned by ac-950a8).
const MCP_RISK_NOTICE: &str = concat!(
    "warning: MCP server access carries security risks. Agent-requested tools can read files, ",
    "make network requests, and execute workflows that use credentials or modify remote systems.\n",
    "warning: Run only with trusted agents and reviewed workflows, isolate the process, and grant ",
    "the least filesystem, network, and credential access needed. Use MCP at your own risk; ",
    "review README.md (MCP Server > Security) and SECURITY.md before enabling it.\n",
    "warning: --allowed-dir limits selected file reads; it is not a process sandbox or a network ",
    "boundary. Do not assume default file, hidden-file, DNS, or redirect protections are present ",
    "unless verified for your installed version.\n",
);

pub(crate) async fn serve(
    specs: Vec<String>,
    dir: Option<String>,
    allowed_dir: Vec<String>,
) -> Result<(), CliError> {
    // Before discovery, loading, or any request: stderr only, never stdout,
    // which carries MCP frames. No prompt and no TTY check.
    eprint!("{MCP_RISK_NOTICE}");

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
    .map_err(|err| CliError::new("SERVE_TASK_PANIC", format!("serve task panicked: {err}")))?
    .map_err(|err| CliError::new("SERVE_MCP", err))
}
