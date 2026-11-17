//! Server state: loaded Arazzo specs and workflow lookup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arazzo_runtime::{RuntimeError, RuntimeErrorKind, SourceDocumentLoader};
use arazzo_spec::{ArazzoSpec, Workflow};

use crate::filesystem::{
    AcquiredDocument, FileAccess, FileAccessError, SharedSourceReadBudget, SourceReadBudget,
};
use crate::limits::McpLimits;
use crate::timeout_policy::TimeoutPolicy;

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
    pub(crate) limits: McpLimits,
}

impl ServerState {
    pub(crate) fn timeouts(&self) -> TimeoutPolicy {
        TimeoutPolicy::from(&self.limits)
    }

    pub(crate) fn limits(&self) -> McpLimits {
        self.limits
    }

    /// Parse and load specs from the given file paths.
    pub fn load(paths: &[String], allowed_dirs: Option<Vec<String>>) -> Result<Self, String> {
        Self::load_configured(paths, allowed_dirs, McpLimits::default())
    }

    pub fn load_configured(
        paths: &[String],
        allowed_dirs: Option<Vec<String>>,
        limits: McpLimits,
    ) -> Result<Self, String> {
        limits.validate()?;
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
            limits,
        })
    }

    /// Create an empty state (for testing).
    pub fn empty() -> Self {
        Self {
            specs: Vec::new(),
            allowed_dirs: Some(Vec::new()),
            file_access: Arc::new(FileAccess::deny_all()),
            limits: McpLimits::default(),
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
            limits: McpLimits::default(),
        }
    }

    pub(crate) fn file_notice(&self) -> String {
        self.file_access.notice()
    }

    pub(crate) fn acquire_document(&self, path: &str) -> Result<AcquiredDocument, FileAccessError> {
        self.file_access.acquire_shared(path, &self.source_budget())
    }

    pub(crate) fn authorize_document(&self, path: &str) -> Result<(), FileAccessError> {
        self.file_access.authorize(path)
    }

    pub(crate) fn source_document_loader(&self) -> Arc<dyn SourceDocumentLoader> {
        Arc::new(RootedSourceLoader {
            file_access: Arc::clone(&self.file_access),
            budget: self.source_budget(),
        })
    }

    fn source_budget(&self) -> SharedSourceReadBudget {
        SourceReadBudget::shared(self.limits.max_document_bytes)
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

struct RootedSourceLoader {
    file_access: Arc<FileAccess>,
    budget: SharedSourceReadBudget,
}

impl SourceDocumentLoader for RootedSourceLoader {
    fn load(&self, source_name: &str, resolved_path: &Path) -> Result<Vec<u8>, RuntimeError> {
        resolved_path
            .to_str()
            .ok_or(FileAccessError::Denied)
            .and_then(|path| self.file_access.acquire_shared(path, &self.budget))
            .map(|document| document.bytes)
            .map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorKind::SourceDescriptionLoad,
                    format!("sourceDescription \"{source_name}\": {}", error.message()),
                )
            })
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
