use super::catalog::{CaseSpec, HarnessError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceReceipt {
    pub valid: bool,
    pub code: Option<String>,
    pub failure: Option<HarnessError>,
    pub files: usize,
    pub operations: usize,
}

pub(crate) fn endpoint_check(endpoint: &str) -> Result<(), HarnessError> {
    let url = url::Url::parse(endpoint).map_err(|e| HarnessError::new("source", "endpoint", e))?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    };
    if url.scheme() != "http"
        || !loopback
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(HarnessError::new(
            "source",
            "endpoint",
            "expected an explicit HTTP loopback IP and port",
        ));
    }
    Ok(())
}

pub(crate) fn check_sources(
    case: &CaseSpec,
    dir: &Path,
    endpoint: &str,
) -> Result<SourceReceipt, HarnessError> {
    endpoint_check(endpoint)?;
    let mut total = 0;
    for source in 0..case.dimensions.source_files {
        let path = dir.join(format!("source_{source}.json"));
        let metadata = fs::symlink_metadata(&path).map_err(|e| {
            HarnessError::new(
                "source",
                "missing_source",
                format!("{}: {e}", path.display()),
            )
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(HarnessError::new("source", "source_type", path.display()));
        }
        total += metadata.len();
        if total > 4 * 1024 * 1024 {
            return Err(HarnessError::new("source", "source_cap", total));
        }
        let bytes = fs::read(&path).map_err(|e| HarnessError::new("source", "source_read", e))?;
        let actual: Value = serde_json::from_slice(&bytes)
            .map_err(|e| HarnessError::new("source", "malformed_source", e))?;
        let mut paths = Map::new();
        for op in case
            .expected
            .operations
            .iter()
            .filter(|op| op.source == source)
        {
            paths.insert(op.path.clone(), json!({"get": {"operationId": op.operation_id,
                "responses": {"200": {"description": "Successful workload response"}, "503": {"description": "Predetermined retry response"}}}}));
        }
        let expected = json!({"openapi": "3.2.0", "info": {"title": format!("source_{source}"), "version": "1"},
            "servers": [{"url": endpoint}], "paths": paths});
        if actual != expected {
            return Err(HarnessError::new(
                "source",
                "source_shape",
                format!("{} differs from complete catalog shape", path.display()),
            ));
        }
    }
    Ok(SourceReceipt {
        valid: true,
        code: None,
        failure: None,
        files: case.dimensions.source_files,
        operations: case.expected.operations.len(),
    })
}
