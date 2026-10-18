#![forbid(unsafe_code)]
#![allow(clippy::expect_used)] // Fixture setup and assertions fail the test on errors.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-json-discovery-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path().join(name);
        fs::write(&path, contents).expect("write test document");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove test directory");
    }
}

fn document(id: &str) -> String {
    json!({
        "arazzo": "1.1.0",
        "info": {"title": "Local catalog fixture", "version": "1.0.0"},
        "sourceDescriptions": [{
            "name": "api", "url": "https://example.invalid/openapi.json", "type": "openapi"
        }],
        "workflows": [{
            "workflowId": id,
            "summary": "A local workflow",
            "steps": [{"stepId": "request", "operationId": "getThing"}]
        }]
    })
    .to_string()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arazzo-cli"))
        .args(args)
        .output()
        .expect("run arazzo-cli")
}

fn success_json(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON stdout")
}

fn error_code(args: &[&str]) -> String {
    let output = run(args);
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    let body: Value = serde_json::from_slice(&output.stdout).expect("JSON error stdout");
    assert_eq!(body["kind"], "error");
    body["code"].as_str().expect("error code").to_string()
}

#[test]
fn json_only_catalog_and_show_match_identical_yaml_named_bytes() {
    let json_dir = TempDir::new();
    let yaml_dir = TempDir::new();
    let bytes = document("from-json");
    let json_file = json_dir.write("workflow.json", &bytes);
    yaml_dir.write("workflow.yaml", &bytes);
    let json_path = json_dir.path().to_str().expect("UTF-8 path");
    let yaml_path = yaml_dir.path().to_str().expect("UTF-8 path");

    let direct = json_file.to_str().expect("UTF-8 path");
    assert_eq!(success_json(&["--json", "validate", direct])["valid"], true);
    assert_eq!(
        success_json(&["--json", "list", direct])[0]["id"],
        "from-json"
    );

    let json_catalog = success_json(&["--json", "catalog", json_path]);
    let yaml_catalog = success_json(&["--json", "catalog", yaml_path]);
    assert_eq!(json_catalog.as_array().expect("catalog rows").len(), 1);
    assert_eq!(json_catalog[0]["file"], "workflow.json");
    assert_eq!(json_catalog[0]["workflows"], yaml_catalog[0]["workflows"]);

    let mut json_show = success_json(&["--json", "show", "from-json", "--dir", json_path]);
    let mut yaml_show = success_json(&["--json", "show", "from-json", "--dir", yaml_path]);
    assert_eq!(json_show["id"], "from-json");
    json_show
        .as_object_mut()
        .expect("show object")
        .remove("file");
    yaml_show
        .as_object_mut()
        .expect("show object")
        .remove("file");
    assert_eq!(json_show, yaml_show);
}

#[test]
fn mixed_directory_preserves_selection_order_and_skip_behavior() {
    let dir = TempDir::new();
    dir.write("z.JSON", &document("upper"));
    dir.write("a.yml", &document("yaml"));
    dir.write("b.json", "{invalid json");
    dir.write("c.txt", &document("unrelated"));
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).expect("create nested directory");
    fs::write(nested.join("child.json"), document("nested")).expect("write nested document");

    let path = dir.path().to_str().expect("UTF-8 path");
    let catalog = success_json(&["--json", "catalog", path]);
    let files: Vec<_> = catalog
        .as_array()
        .expect("catalog rows")
        .iter()
        .map(|row| row["file"].as_str().expect("file name"))
        .collect();
    assert_eq!(files, ["a.yml", "z.JSON"]);
    assert_eq!(catalog[0]["workflows"][0]["id"], "yaml");
    assert_eq!(catalog[1]["workflows"][0]["id"], "upper");
    assert_eq!(
        success_json(&["--json", "show", "upper", "--dir", path])["id"],
        "upper"
    );
    for id in ["unrelated", "nested"] {
        assert_eq!(
            error_code(&["--json", "show", id, "--dir", path]),
            "SHOW_WORKFLOW_NOT_FOUND"
        );
    }
}

#[test]
fn mixed_format_duplicates_and_absent_ids_keep_structured_errors() {
    let dir = TempDir::new();
    let bytes = document("duplicate");
    dir.write("one.json", &bytes);
    dir.write("two.yaml", &bytes);
    let path = dir.path().to_str().expect("UTF-8 path");
    assert_eq!(
        error_code(&["--json", "show", "duplicate", "--dir", path]),
        "SHOW_WORKFLOW_AMBIGUOUS"
    );
    assert_eq!(
        error_code(&["--json", "show", "absent", "--dir", path]),
        "SHOW_WORKFLOW_NOT_FOUND"
    );
}

#[test]
fn direct_input_help_names_both_formats_and_generate_stays_yaml_only() {
    for command in ["run", "validate", "list", "steps"] {
        let output = run(&[command, "--help"]);
        assert!(output.status.success(), "{command} help failed");
        let help = String::from_utf8(output.stdout).expect("UTF-8 help");
        assert!(
            help.contains("Arazzo YAML or JSON spec file"),
            "{command} help: {help}"
        );
    }
    let output = run(&["generate", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains("generate writes YAML only"));
    assert!(help.contains("Write YAML to this .yaml or .yml file"));
}
