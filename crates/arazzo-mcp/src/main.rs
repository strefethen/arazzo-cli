#![forbid(unsafe_code)]

//! Standalone MCP server binary for Arazzo workflows.

use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter};
use std::path::Path;

use clap::Parser;

#[derive(Parser)]
#[command(name = "arazzo-mcp")]
#[command(about = "MCP server exposing Arazzo workflows as AI-agent tools")]
struct Args {
    /// Arazzo spec files to load
    specs: Vec<String>,

    /// Directory containing .arazzo.yaml files to load
    #[arg(long = "dir")]
    dir: Option<String>,

    /// Restrict validate_spec file access to these directories (repeatable)
    #[arg(long = "allowed-dir")]
    allowed_dir: Vec<String>,
}

fn main() {
    load_env_file(".env");

    let args = Args::parse();
    let spec_paths = match resolve_spec_paths(&args) {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    if spec_paths.is_empty() {
        eprintln!("error: no spec files provided. Pass file paths or use --dir <directory>");
        std::process::exit(1);
    }

    let reader = BufReader::new(io::stdin());
    let mut writer = BufWriter::new(io::stdout().lock());

    let allowed = if args.allowed_dir.is_empty() {
        None
    } else {
        Some(args.allowed_dir)
    };

    if let Err(err) = arazzo_mcp::run_mcp_stdio(reader, &mut writer, &spec_paths, allowed) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn resolve_spec_paths(args: &Args) -> Result<Vec<String>, String> {
    let mut paths = args.specs.clone();
    if let Some(dir) = &args.dir {
        let discovered = arazzo_mcp::state::discover_specs(dir)?;
        paths.extend(discovered);
    }
    Ok(paths)
}

/// Loads environment variables from a `.env` file if present.
///
/// Adapted from `arazzo-cli/src/main.rs`.
fn load_env_file(path: impl AsRef<Path>) {
    let file = match fs::File::open(path.as_ref()) {
        Ok(file) => file,
        Err(_) => return,
    };

    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(v) => v,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let trimmed = value.trim();
        let value = if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            trimmed[1..trimmed.len() - 1]
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\\\", "\\")
        } else {
            trimmed.to_string()
        };
        std::env::set_var(key, &value);
    }
}
