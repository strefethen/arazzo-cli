#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use arazzo_mcp::state::{discover_specs, ServerState};
use serde_json::json;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-mcp-json-discovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap_or_else(|err| panic!("create fixture directory: {err}"));
        Self(path)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap_or_else(|| panic!("fixture parent")))
            .unwrap_or_else(|err| panic!("create fixture parent: {err}"));
        fs::write(&path, content).unwrap_or_else(|err| panic!("write fixture: {err}"));
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap_or_else(|err| panic!("remove fixture directory: {err}"));
    }
}

#[test]
fn recursive_discovery_keeps_sorted_exact_suffixes() {
    let dir = TempDir::new();
    let yaml = dir.write("b.arazzo.yaml", "fixture");
    let json = dir.write("nested/a.arazzo.json", "fixture");
    let yml = dir.write("nested/c.arazzo.yml", "fixture");
    for excluded in [
        "plain.json",
        "plain.yaml",
        "nested/plain.yml",
        "nested/d.arazzo.JSON",
        "nested/e.ARAZZO.json",
        "nested/f.arazzo.Yaml",
    ] {
        dir.write(excluded, "fixture");
    }

    let found = discover_specs(
        dir.path()
            .to_str()
            .unwrap_or_else(|| panic!("UTF-8 fixture path")),
    )
    .unwrap_or_else(|err| panic!("discover fixture files: {err}"));
    let mut expected = vec![yaml, json, yml];
    expected.sort();
    let expected: Vec<String> = expected
        .into_iter()
        .map(|path| {
            path.to_str()
                .unwrap_or_else(|| panic!("UTF-8 fixture path"))
                .to_owned()
        })
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn discovered_json_loads_workflows_without_execution() {
    let dir = TempDir::new();
    let source = dir.write(
        "source.openapi.json",
        &json!({
            "openapi": "3.0.3",
            "info": {"title": "Local source", "version": "1.0.0"},
            "paths": {"/ping": {"get": {"operationId": "ping", "responses": {"200": {"description": "OK"}}}}}
        })
        .to_string(),
    );
    let spec = json!({
        "arazzo": "1.1.0",
        "info": {"title": "Local workflow", "version": "1.0.0"},
        "sourceDescriptions": [{"name": "api", "url": source, "type": "openapi"}],
        "workflows": [{"workflowId": "json-workflow", "steps": [{"stepId": "ping-step", "operationId": "ping"}]}]
    });
    let spec_path = dir.write("nested/workflow.arazzo.json", &spec.to_string());

    let paths = discover_specs(
        dir.path()
            .to_str()
            .unwrap_or_else(|| panic!("UTF-8 fixture path")),
    )
    .unwrap_or_else(|err| panic!("discover JSON spec: {err}"));
    assert_eq!(
        paths,
        vec![spec_path
            .to_str()
            .unwrap_or_else(|| panic!("UTF-8 fixture path"))]
    );
    let state =
        ServerState::load(&paths, None).unwrap_or_else(|err| panic!("load JSON spec: {err}"));
    let workflows = state.all_workflows();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].0.file_path, paths[0]);
    assert_eq!(workflows[0].1.workflow_id, "json-workflow");
}

#[test]
fn invalid_root_keeps_directory_error() {
    let dir = TempDir::new();
    let file = dir.write("not-a-directory", "fixture");
    let path = file
        .to_str()
        .unwrap_or_else(|| panic!("UTF-8 fixture path"));
    assert_eq!(
        discover_specs(path),
        Err(format!("not a directory: {path}"))
    );
}

#[test]
fn standalone_directory_help_names_all_suffixes() {
    let output = Command::new(env!("CARGO_BIN_EXE_arazzo-mcp"))
        .arg("--help")
        .output()
        .unwrap_or_else(|err| panic!("run MCP help: {err}"));
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap_or_else(|err| panic!("UTF-8 help: {err}"));
    for suffix in [".arazzo.json", ".arazzo.yaml", ".arazzo.yml"] {
        assert!(help.contains(suffix), "missing {suffix}: {help}");
    }
}
