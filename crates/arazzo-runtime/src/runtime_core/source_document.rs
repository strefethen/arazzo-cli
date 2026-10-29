//! Byte acquisition for OpenAPI Source Descriptions resolved to local paths.

use std::path::Path;

use super::{RuntimeError, RuntimeErrorKind};

/// Acquires an OpenAPI Source Description's bytes during engine construction.
///
/// The engine resolves document identities and URIs before calling this loader.
/// Provided documents and refused non-file URIs never reach it. The path is the
/// resolved file path, without additional canonicalization or filesystem checks.
/// Returned bytes follow the engine's normal parsing and indexing pipeline;
/// errors propagate unchanged, without a native retry. External `$ref` documents
/// and AsyncAPI sources are outside this loader's scope.
pub trait SourceDocumentLoader: Send + Sync {
    /// Loads the named source from its resolved path.
    fn load(&self, source_name: &str, resolved_path: &Path) -> Result<Vec<u8>, RuntimeError>;
}

/// Native acquisition retains the URI context used by existing diagnostics.
/// This context belongs to resolution, not to the public byte-loading contract.
pub(super) struct NativeSourceDocumentLoader<'a> {
    pub(super) authored_url: &'a str,
    pub(super) base_uri: &'a str,
    pub(super) resolved_uri: &'a str,
}

impl SourceDocumentLoader for NativeSourceDocumentLoader<'_> {
    fn load(&self, source_name: &str, resolved_path: &Path) -> Result<Vec<u8>, RuntimeError> {
        std::fs::read(resolved_path).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionLoad,
                format!(
                    "sourceDescription \"{source_name}\": url \"{url}\" resolved against base URI \
                     \"{base}\" to \"{resolved}\"; reading OpenAPI document \"{path}\": {err}",
                    url = self.authored_url,
                    base = self.base_uri,
                    resolved = self.resolved_uri,
                    path = resolved_path.display(),
                ),
            )
        })
    }
}
