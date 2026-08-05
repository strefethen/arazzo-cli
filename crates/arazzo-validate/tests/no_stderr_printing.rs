#![forbid(unsafe_code)]

//! Guard: `arazzo-validate` reports findings, it never prints them.
//!
//! Warnings used to reach the user through bare `eprintln!` calls inside the
//! library, which made them invisible to `--json`, to `arazzo-mcp`, and to the
//! debug adapter. Diagnostics now travel back to the caller, and the caller
//! owns the output stream. This test lives outside `src/` so the crate's own
//! sources stay free of the very token it looks for.

use std::fs;
use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => panic!("reading {}: {err}", dir.display()),
    };

    let mut files = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(err) => panic!("reading entry in {}: {err}", dir.display()),
        };
        if path.is_dir() {
            files.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn crate_sources_never_write_to_stderr() {
    let files = rust_sources(&src_dir());
    assert!(!files.is_empty(), "no Rust sources found under src/");

    let mut offenders = Vec::new();
    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(err) => panic!("reading {}: {err}", file.display()),
        };
        for (idx, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains("eprint!") || trimmed.contains("eprintln!") {
                offenders.push(format!("{}:{}: {trimmed}", file.display(), idx + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "arazzo-validate must return diagnostics, not print them: {offenders:?}"
    );
}
