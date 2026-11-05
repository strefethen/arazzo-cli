//! Server state: loaded Arazzo specs and workflow lookup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arazzo_spec::{ArazzoSpec, Workflow};

use crate::filesystem::{AcquiredDocument, FileAccess, FileAccessError, SourceReadBudget};

/// A parsed spec with its source file path.
pub struct LoadedSpec {
    pub file_path: String,
    pub spec: ArazzoSpec,
}

/// Holds all loaded specs for the MCP server session.
pub struct ServerState {
    pub specs: Vec<LoadedSpec>,
    pub allowed_dirs: Option<Vec<PathBuf>>,
    pub(crate) file_access: Arc<FileAccess>,
}

impl ServerState {
    pub(crate) fn timeouts(&self) -> crate::timeout_policy::TimeoutPolicy {
        Default::default()
    }

    /// Parse and load specs from the given file paths.
    pub fn load(paths: &[String], allowed_dirs: Option<Vec<String>>) -> Result<Self, String> {
        let mut specs = Vec::with_capacity(paths.len());
        for path in paths {
            let spec =
                arazzo_validate::parse(path).map_err(|err| format!("loading {path}: {err}"))?;
            specs.push(LoadedSpec {
                file_path: path.clone(),
                spec,
            });
        }
        let file_access = Arc::new(FileAccess::configure(paths, allowed_dirs)?);
        let allowed = Some(file_access.selectors());
        Ok(Self {
            specs,
            allowed_dirs: allowed,
            file_access,
        })
    }

    /// Create an empty state (for testing).
    pub fn empty() -> Self {
        Self {
            specs: Vec::new(),
            allowed_dirs: Some(Vec::new()),
            file_access: Arc::new(FileAccess::deny_all()),
        }
    }

    /// Create state from a pre-parsed spec (for testing without file I/O).
    pub fn from_spec(file_path: impl Into<String>, spec: ArazzoSpec) -> Self {
        Self {
            specs: vec![LoadedSpec {
                file_path: file_path.into(),
                spec,
            }],
            allowed_dirs: Some(Vec::new()),
            file_access: Arc::new(FileAccess::deny_all()),
        }
    }

    pub(crate) fn file_notice(&self) -> String {
        self.file_access.notice()
    }

    pub(crate) fn acquire_document(&self, path: &str) -> Result<AcquiredDocument, FileAccessError> {
        self.file_access
            .acquire_shared(path, &SourceReadBudget::shared())
    }

    pub fn check_path_allowed(&self, file_path: &str) -> Result<(), String> {
        if !cfg!(target_os = "linux") {
            return Err("File access denied by server policy".to_string());
        }
        let Some(allowed) = &self.allowed_dirs else {
            return Err("File access denied by server policy".to_string());
        };

        let path = Path::new(file_path);

        // Try to canonicalize the file path. If the file doesn't exist,
        // try the parent directory to prevent existence probing.
        let canonical = std::fs::canonicalize(path).or_else(|_| {
            path.parent()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no parent directory")
                })
                .and_then(std::fs::canonicalize)
        });

        let canonical = canonical.map_err(|_| format!("path not allowed: {file_path}"))?;

        for dir in allowed {
            if canonical.starts_with(dir) {
                return Ok(());
            }
        }

        Err(format!("path not allowed: {file_path}"))
    }

    /// Find a workflow by ID across all loaded specs.
    ///
    /// Returns an error if the workflow ID is ambiguous (exists in multiple specs).
    pub fn find_workflow(&self, workflow_id: &str) -> Result<(&LoadedSpec, &Workflow), String> {
        let mut matches: Vec<(&LoadedSpec, &Workflow)> = Vec::new();
        for loaded in &self.specs {
            for wf in &loaded.spec.workflows {
                if wf.workflow_id == workflow_id {
                    matches.push((loaded, wf));
                }
            }
        }

        match matches.len() {
            0 => Err(format!("workflow not found: {workflow_id}")),
            1 => Ok(matches[0]),
            n => Err(format!(
                "ambiguous workflow id \"{workflow_id}\": found in {n} specs ({})",
                matches
                    .iter()
                    .map(|(s, _)| s.file_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Enumerate all workflows with their source spec.
    pub fn all_workflows(&self) -> Vec<(&LoadedSpec, &Workflow)> {
        let mut result = Vec::new();
        for loaded in &self.specs {
            for wf in &loaded.spec.workflows {
                result.push((loaded, wf));
            }
        }
        result
    }
}

/// Discover `.arazzo.json`, `.arazzo.yaml`, and `.arazzo.yml` files in a directory tree.
pub fn discover_specs(dir: &str) -> Result<Vec<String>, String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    let mut paths = Vec::new();
    collect_arazzo_files(root, &mut paths).map_err(|err| format!("scanning {dir}: {err}"))?;
    paths.sort();
    Ok(paths)
}

fn collect_arazzo_files(dir: &Path, out: &mut Vec<String>) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_arazzo_files(&entry.path(), out)?;
        } else if ft.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".arazzo.json")
                || name.ends_with(".arazzo.yaml")
                || name.ends_with(".arazzo.yml")
            {
                if let Some(path) = entry.path().to_str() {
                    out.push(path.to_string());
                }
            }
        }
    }
    Ok(())
}
