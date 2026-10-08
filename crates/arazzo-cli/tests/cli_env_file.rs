#![forbid(unsafe_code)]

//! A `.env` in the working directory is loaded before argument parsing. A
//! line `std::env::set_var` cannot accept, or a lone quote, must be skipped
//! rather than abort the process (ac-342bd, audit I6/H3), and every load is
//! reported on stderr — never stdout, never a name or value (ac-a2fbd).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &[u8]) {
        fs::write(self.path.join(name), contents)
            .unwrap_or_else(|err| panic!("writing {name}: {err}"));
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A value that must never appear in any output.
const SENTINEL: &str = "sentinel-6f1c2a9e-must-not-leak";

const SPEC: &str = "\
arazzo: 1.0.0
info:
  title: env file reporting
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: http://127.0.0.1:9/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: s
        operationPath: /get
        successCriteria:
          - condition: $statusCode == 200
";

/// `SPEC` with the workflow input `v` sent as query parameter `q`.
const ECHO_SPEC: &str = "\
arazzo: 1.0.0
info:
  title: env file reporting
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: http://127.0.0.1:9/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        v:
          type: string
    steps:
      - stepId: s
        operationPath: /get
        parameters:
          - name: q
            in: query
            value: $inputs.v
        successCriteria:
          - condition: $statusCode == 200
";

/// Runs the CLI in `dir` with `args`, removing every name in `unset` from its
/// environment and setting each `set` pair.
fn run_cli(dir: &Path, args: &[&str], unset: &[&str], set: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
    command.args(args).current_dir(dir);
    for name in unset {
        command.env_remove(name);
    }
    for (name, value) in set {
        command.env(name, value);
    }
    command
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli: {err}"))
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_no_leak(stderr: &str) {
    assert!(!stderr.contains(SENTINEL), "value leaked: {stderr}");
}

/// Every line shape that panicked before the fix, plus a valid line.
const HOSTILE_ENV: &[u8] =
    b"=empty-key\n  =spaced-empty-key\nNUL\0KEY=v\nNUL_VALUE=a\0b\nLONE_DQ=\"\nLONE_SQ='\nOK=1\n";

#[test]
fn hostile_env_file_does_not_crash_the_cli() {
    let temp = TempDir::new("arazzo-cli-env-file");
    temp.write(".env", HOSTILE_ENV);

    let output = run_cli(temp.path(), &["--version"], &[], &[]);

    let stderr = stderr_of(&output);
    assert!(
        output.status.success(),
        "status {:?}: {stderr}",
        output.status
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
    for line in 1..=4 {
        assert!(
            stderr.contains(&format!("warning: .env:{line}: ignored line: ")),
            "line {line} not reported: {stderr}"
        );
    }
}

#[test]
fn well_formed_env_reports_a_count_and_no_values() {
    let temp = TempDir::new("arazzo-cli-env-count");
    temp.write(
        ".env",
        format!("# comment\n\nARAZZO_T_COUNT_A={SENTINEL}\nARAZZO_T_COUNT_B=\"{SENTINEL}\"\n")
            .as_bytes(),
    );

    let output = run_cli(
        temp.path(),
        &["--version"],
        &["ARAZZO_T_COUNT_A", "ARAZZO_T_COUNT_B"],
        &[],
    );

    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        stderr,
        "loaded .env: set 2, kept 0 already in the environment, ignored 0\n"
    );
    assert!(!stderr.contains("ARAZZO_T_COUNT"), "name printed: {stderr}");
    assert_no_leak(&stderr);
}

#[test]
fn ignored_lines_are_reported_by_number_and_valid_lines_still_load() {
    let temp = TempDir::new("arazzo-cli-env-mixed");
    temp.write(
        ".env",
        format!(
            "ARAZZO_T_MIXED_A=1\nNO_SEPARATOR_{SENTINEL}\n# comment\n={SENTINEL}\nARAZZO_T_MIXED_B=2\n"
        )
        .as_bytes(),
    );

    let output = run_cli(
        temp.path(),
        &["--version"],
        &["ARAZZO_T_MIXED_A", "ARAZZO_T_MIXED_B"],
        &[],
    );

    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        stderr,
        "warning: .env:2: ignored line: no `=` separator\n\
         warning: .env:4: ignored line: empty name\n\
         loaded .env: set 2, kept 0 already in the environment, ignored 2\n"
    );
    assert_no_leak(&stderr);
}

#[test]
fn exported_name_is_counted_as_kept() {
    let temp = TempDir::new("arazzo-cli-env-kept");
    temp.write(
        ".env",
        format!("ARAZZO_T_KEPT={SENTINEL}\nARAZZO_T_KEPT_NEW=1\n").as_bytes(),
    );

    let output = run_cli(
        temp.path(),
        &["--version"],
        &["ARAZZO_T_KEPT_NEW"],
        &[("ARAZZO_T_KEPT", "from-env")],
    );

    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        stderr,
        "loaded .env: set 1, kept 1 already in the environment, ignored 0\n"
    );
    assert_no_leak(&stderr);
}

#[test]
fn no_env_file_adds_no_output() {
    let temp = TempDir::new("arazzo-cli-env-absent");

    let output = run_cli(temp.path(), &["--version"], &[], &[]);

    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(stderr_of(&output), "");
}

#[test]
fn value_containing_equals_still_splits_on_the_first() {
    let temp = TempDir::new("arazzo-cli-env-equals");
    temp.write(".env", b"ARAZZO_T_EQ=a=b\n");
    temp.write("spec.arazzo.yaml", ECHO_SPEC.as_bytes());

    // `-i v=$NAME` reads the variable back, so the dry-run request shows the
    // value the loader set.
    let output = run_cli(
        temp.path(),
        &[
            "run",
            "spec.arazzo.yaml",
            "wf",
            "--dry-run",
            "--json",
            "-i",
            "v=$ARAZZO_T_EQ",
        ],
        &["ARAZZO_T_EQ"],
        &[],
    );

    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("loaded .env: set 1, kept 0 already in the environment, ignored 0\n"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout not JSON ({err}): {stdout}"));
    let url = parsed["requests"][0]["url"].as_str().unwrap_or_default();
    assert!(url.ends_with("?q=a%3Db"), "{stdout}");
}

#[test]
fn report_goes_to_stderr_and_json_stdout_is_unchanged() {
    let with_env = TempDir::new("arazzo-cli-env-json");
    with_env.write(
        ".env",
        format!("ARAZZO_T_JSON={SENTINEL}\nNO_SEPARATOR\n").as_bytes(),
    );
    with_env.write("spec.arazzo.yaml", SPEC.as_bytes());
    let without_env = TempDir::new("arazzo-cli-env-json-baseline");
    without_env.write("spec.arazzo.yaml", SPEC.as_bytes());

    let args = ["run", "spec.arazzo.yaml", "wf", "--dry-run", "--json"];
    let output = run_cli(with_env.path(), &args, &["ARAZZO_T_JSON"], &[]);
    let baseline = run_cli(without_env.path(), &args, &["ARAZZO_T_JSON"], &[]);

    let stderr = stderr_of(&output);
    assert!(
        stderr.contains("warning: .env:2: ignored line: no `=` separator\n"),
        "{stderr}"
    );
    assert!(stderr.contains("loaded .env: "), "{stderr}");
    assert_no_leak(&stderr);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(".env"), "report on stdout: {stdout}");
    let parsed: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout not JSON ({err}): {stdout}"));
    assert_eq!(parsed["kind"], "dryRun", "{stdout}");
    assert_eq!(output.stdout, baseline.stdout, "stdout changed by .env");
}
