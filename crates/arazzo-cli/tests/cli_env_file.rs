#![forbid(unsafe_code)]

//! A `.env` in the working directory is loaded before argument parsing, so a
//! line `std::env::set_var` cannot accept, or a lone quote, must be skipped
//! rather than abort the process (ac-342bd, audit I6/H3).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Every line shape that panicked before the fix, plus a valid line.
const HOSTILE_ENV: &[u8] =
    b"=empty-key\n  =spaced-empty-key\nNUL\0KEY=v\nNUL_VALUE=a\0b\nLONE_DQ=\"\nLONE_SQ='\nOK=1\n";

#[test]
fn hostile_env_file_does_not_crash_the_cli() {
    let temp = TempDir::new("arazzo-cli-env-file");
    fs::write(temp.path().join(".env"), HOSTILE_ENV)
        .unwrap_or_else(|err| panic!("writing .env: {err}"));

    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .arg("--version")
        .current_dir(temp.path())
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status {:?}: {stderr}",
        output.status
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
