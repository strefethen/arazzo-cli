#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;

fn cli_bin() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        Ok(bin) => PathBuf::from(bin),
        Err(_) => {
            let mut path = repo_root();
            path.push("target/debug/arazzo-cli");
            if path.exists() {
                path
            } else {
                panic!(
                    "CLI binary not found at {}; CARGO_BIN_EXE_arazzo-cli missing",
                    path.display()
                );
            }
        }
    }
}

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../..");
    path
}

fn schemas_dir() -> PathBuf {
    repo_root().join("docs/schemas")
}

fn assert_schema_matches_file(command: &str, file_name: &str) {
    let output = match Command::new(cli_bin()).args(["schema", command]).output() {
        Ok(output) => output,
        Err(err) => panic!("failed to run arazzo-cli schema {command}: {err}"),
    };

    assert!(
        output.status.success(),
        "arazzo-cli schema {command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let generated = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(err) => panic!("non-UTF-8 schema output for {command}: {err}"),
    };

    let path = schemas_dir().join(file_name);
    let on_disk = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => panic!(
            "failed to read {}: {err}\nRegenerate with: cargo run -p arazzo-cli -- schema {command} > docs/schemas/{file_name}",
            path.display()
        ),
    };

    assert_eq!(
        on_disk, generated,
        "schema drift detected for {file_name}\n\
         Regenerate with: cargo run -p arazzo-cli -- schema {command} > docs/schemas/{file_name}"
    );
}

#[test]
fn schema_validate_matches_checked_in_file() {
    assert_schema_matches_file("validate", "validate.schema.json");
}

#[test]
fn schema_list_matches_checked_in_file() {
    assert_schema_matches_file("list", "list.schema.json");
}

#[test]
fn schema_catalog_matches_checked_in_file() {
    assert_schema_matches_file("catalog", "catalog.schema.json");
}

#[test]
fn schema_show_matches_checked_in_file() {
    assert_schema_matches_file("show", "show.schema.json");
}

#[test]
fn schema_run_matches_checked_in_file() {
    assert_schema_matches_file("run", "run.schema.json");
}
