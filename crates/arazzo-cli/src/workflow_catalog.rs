//! Shallow workflow catalog discovery and unique-workflow lookup.

use std::fs;
use std::path::{Path, PathBuf};

use arazzo_spec::ArazzoSpec;

use crate::cli_error::{CliError, CLI_OUTPUT_WRITE};
use crate::output::{self, CatalogEntry};
use crate::run_context::GlobalOptions;

pub(super) fn catalog_workflows(dir: &str, global: GlobalOptions) -> Result<(), CliError> {
    let entries = fs::read_dir(dir).map_err(|err| {
        CliError::new(
            "CATALOG_DIR_READ",
            format!("reading directory \"{dir}\": {err}"),
        )
    })?;
    let mut paths = Vec::<PathBuf>::new();
    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(err) => {
                if global.verbose {
                    eprintln!("skipping entry: {err}");
                }
                continue;
            }
        };
        paths.push(entry.path());
    }
    paths.sort_unstable();

    let mut catalog = Vec::<CatalogEntry>::new();
    for path in paths {
        if !is_arazzo_document_path(&path) {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let spec = match arazzo_validate::parse(&path) {
            Ok(spec) => spec,
            Err(err) => {
                if global.verbose {
                    eprintln!("skipping {file_name}: {err}");
                }
                continue;
            }
        };
        let workflows = spec
            .workflows
            .iter()
            .map(output::build_workflow_info)
            .collect::<Vec<_>>();
        catalog.push(CatalogEntry {
            file: file_name,
            title: spec.info.title.clone(),
            description: spec.info.description.clone(),
            version: spec.info.version.clone(),
            sources: output::build_sources(&spec),
            workflows,
        });
    }
    catalog.sort_unstable_by(|a, b| a.file.cmp(&b.file));

    output::emit_catalog(&catalog, global.json).map_err(|err| CliError::new(CLI_OUTPUT_WRITE, err))
}

pub(super) fn show_workflow(
    workflow_id: &str,
    dir: &str,
    global: GlobalOptions,
) -> Result<(), CliError> {
    let (spec, file) = find_workflow(dir, workflow_id)?;
    let workflow = spec
        .workflows
        .iter()
        .find(|wf| wf.workflow_id == workflow_id)
        .ok_or_else(|| show_not_found(workflow_id, dir))?;
    output::emit_workflow_detail(&spec, workflow, file, global.json)
        .map_err(|err| CliError::new(CLI_OUTPUT_WRITE, err))
}

fn show_not_found(workflow_id: &str, dir: &str) -> CliError {
    CliError::new(
        "SHOW_WORKFLOW_NOT_FOUND",
        format!("workflow \"{workflow_id}\" not found in {dir}"),
    )
}

fn find_workflow(dir: &str, workflow_id: &str) -> Result<(ArazzoSpec, String), CliError> {
    let entries = fs::read_dir(dir).map_err(|err| {
        CliError::new(
            "SHOW_DIR_READ",
            format!("reading directory \"{dir}\": {err}"),
        )
    })?;
    let mut paths = Vec::<PathBuf>::new();
    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        paths.push(entry.path());
    }
    paths.sort_unstable();

    let mut matches = Vec::<String>::new();
    let mut match_spec: Option<ArazzoSpec> = None;
    let mut match_file = String::new();

    for path in paths {
        if !is_arazzo_document_path(&path) {
            continue;
        }
        let spec = match arazzo_validate::parse(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for wf in &spec.workflows {
            if wf.workflow_id == workflow_id {
                let file = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                matches.push(file.clone());
                match_spec = Some(spec.clone());
                match_file = file;
            }
        }
    }
    matches.sort_unstable();

    if matches.is_empty() {
        return Err(show_not_found(workflow_id, dir));
    }
    if matches.len() > 1 {
        return Err(CliError::new(
            "SHOW_WORKFLOW_AMBIGUOUS",
            format!("workflow \"{workflow_id}\" found in multiple files: {matches:?}"),
        ));
    }

    match match_spec {
        Some(spec) => Ok((spec, match_file)),
        None => Err(show_not_found(workflow_id, dir)),
    }
}

fn is_arazzo_document_path(path: &Path) -> bool {
    match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            ext == "yaml" || ext == "yml" || ext == "json"
        }
        None => false,
    }
}
