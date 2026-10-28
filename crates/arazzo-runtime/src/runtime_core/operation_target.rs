//! Source-bound operation projection shared by ID indexing and JSON Pointers.
use super::*;
use arazzo_spec::SourceDescription;
use serde_yaml_ng::Value as Yaml;

/// An `operationId` resolved to the request it names.
pub(crate) struct ResolvedOperation {
    pub method: String,
    pub path: String,
    /// Effective operation/path/document server from its bound source.
    pub base: String,
}

/// Shared final projection for ID and pointer resolution.
pub(super) fn resolved_operation(
    target: &str,
    entry: &OperationEntry,
    document_base: &str,
) -> Result<ResolvedOperation, RuntimeError> {
    Ok(ResolvedOperation {
        method: entry.method.clone(),
        path: entry.path.clone(),
        base: operation_base(target, entry, document_base)?,
    })
}

/// The request base a source-bound operation is sent to: the server it
/// declares for itself, else its source description's document base.
fn operation_base(
    target: &str,
    entry: &OperationEntry,
    document_base: &str,
) -> Result<String, RuntimeError> {
    entry
        .server
        .base(document_base)
        .map_err(|unusable| unusable_operation_server(target, entry, unusable))
}

/// The refusal for an operation whose own `servers` declaration yields no
/// usable request base. Like every refusal at resolution it says whose limit
/// it is: a relative url is this runtime's — OpenAPI allows it, but it
/// resolves against where the document is served from — while a malformed
/// declaration is the document's to correct.
fn unusable_operation_server(
    target: &str,
    entry: &OperationEntry,
    unusable: &UnusableServer,
) -> RuntimeError {
    let named = format!(
        "operation target \"{target}\" names {} {} in {}",
        entry.method,
        entry.path,
        entry.origin.describe()
    );
    let level = unusable.level.object_name();
    let message = match &unusable.issue {
        ServerIssue::Relative(url) => format!(
            "{named}, and its {level} declares servers[0].url \"{url}\", which is relative. \
             OpenAPI resolves a relative server url against the location the document is served \
             from; this runtime reads the document from disk, so it has no HTTP location to \
             resolve it against — declare an absolute server url there"
        ),
        ServerIssue::NotAnArray => format!(
            "{named}, and its {level} declares a servers field that is not an array. OpenAPI \
             requires an array of Server Objects there, so the document must be corrected"
        ),
        ServerIssue::MissingUrl => format!(
            "{named}, and its {level} declares a first Server Object with no url. OpenAPI \
             requires a url on every Server Object, so the document must be corrected"
        ),
    };
    RuntimeError::new(RuntimeErrorKind::SourceDescriptionParse, message)
}

pub(super) struct BoundOpenApi {
    pub(super) root: Yaml,
    pub(super) label: String,
}

const FIXED_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace", "query",
];

/// Project only directly routable Operation Object positions with Path Item
/// context. References require a separate resolver and are explicitly refused.
fn operation_entry(
    root: &Yaml,
    origin: &OperationOrigin,
    tokens: &[&str],
) -> Option<OperationEntry> {
    let (path, method, additional) = match tokens {
        ["paths", path, method] if FIXED_METHODS.contains(method) => (*path, *method, false),
        ["paths", path, "additionalOperations", method] => (*path, *method, true),
        _ => return None,
    };
    if !path.starts_with('/') {
        return None;
    }
    let path_item = root.get("paths")?.get(path)?;
    if !path_item.is_mapping() || path_item.get("$ref").is_some() {
        return None;
    }
    let operation = if additional {
        // Fixed methods have their own fields. Additional keys are HTTP tokens
        // and their case is the method sent on the wire (OAS 3.2).
        if FIXED_METHODS
            .iter()
            .any(|fixed| fixed.to_uppercase() == method)
            || method.is_empty()
            || !method
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return None;
        }
        path_item.get("additionalOperations")?.get(method)?
    } else {
        path_item.get(method)?
    };
    if !operation.is_mapping() || operation.get("$ref").is_some() {
        return None;
    }
    Some(OperationEntry {
        method: if additional {
            method.to_string()
        } else {
            method.to_uppercase()
        },
        path: path.to_string(),
        origin: origin.clone(),
        server: operation_server(path_item, operation),
    })
}

pub(super) fn index_operations(root: &Yaml, origin: &OperationOrigin, index: &mut OperationIndex) {
    let Some(paths) = root.get("paths").and_then(Yaml::as_mapping) else {
        return;
    };
    for (path, path_item) in paths {
        let Some(path) = path.as_str() else {
            continue;
        };
        for method in FIXED_METHODS {
            let tokens = ["paths", path, *method];
            index_operation(root, origin, &tokens, path_item.get(*method), index);
        }
        if let Some(additional) = path_item
            .get("additionalOperations")
            .and_then(Yaml::as_mapping)
        {
            for (method, operation) in additional {
                let Some(method) = method.as_str() else {
                    continue;
                };
                index_operation(
                    root,
                    origin,
                    &["paths", path, "additionalOperations", method],
                    Some(operation),
                    index,
                );
            }
        }
    }
}

fn index_operation(
    root: &Yaml,
    origin: &OperationOrigin,
    tokens: &[&str],
    operation: Option<&Yaml>,
    index: &mut OperationIndex,
) {
    let Some(id) = operation
        .and_then(|op| op.get("operationId"))
        .and_then(Yaml::as_str)
    else {
        return;
    };
    if let Some(entry) = operation_entry(root, origin, tokens) {
        index.push(id.to_string(), entry);
    }
}

impl Engine {
    pub(super) fn resolve_operation_path(
        &self,
        target: &str,
    ) -> Result<ResolvedOperation, RuntimeError> {
        let (source_name, encoded_pointer) = match classify_operation_path(target) {
            OperationPathTarget::SourcePointer { source_name, encoded_pointer } => (source_name, encoded_pointer),
            OperationPathTarget::Malformed(reason) => return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedOperationPathForm,
                format!("operationPath \"{target}\" {reason}; expected {SUPPORTED_OPERATION_PATH_FORMS}"),
            )),
        };
        let source = self.inner.index.spec.source_descriptions.iter().find(|sd| sd.name == source_name)
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::SourceDescriptionNotFound,
                format!("sourceDescription \"{source_name}\" referenced by operationPath \"{target}\" was not found")))?;
        if source.type_ != SourceType::OpenApi {
            return Err(RuntimeError::new(RuntimeErrorKind::UnsupportedSourceDescriptionType,
                format!("operationPath \"{target}\" names sourceDescription \"{source_name}\" of type \"{}\"; this runtime resolves operation pointers only against OpenAPI documents", source.type_)));
        }
        let document = self
            .inner
            .index
            .source_documents
            .get(source_name)
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::SourceDescriptionNotFound,
                    format!("sourceDescription \"{source_name}\" has no bound OpenAPI document"),
                )
            })?;
        // The shared parser already checked percent escapes, UTF-8 and RFC6901
        // token escaping. Decode the fragment before splitting pointer tokens.
        let pointer = percent_encoding::percent_decode_str(encoded_pointer)
            .decode_utf8()
            .map_err(|err| {
                RuntimeError::new(
                    RuntimeErrorKind::UnsupportedOperationPathForm,
                    err.to_string(),
                )
            })?;
        let tokens: Vec<String> = pointer
            .strip_prefix('/')
            .into_iter()
            .flat_map(|rest| rest.split('/'))
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect();
        let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let origin = OperationOrigin::Source {
            name: source_name.to_string(),
            path: document.label.clone(),
        };
        let entry = operation_entry(&document.root, &origin, &tokens).ok_or_else(|| RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!("sourceDescription \"{source_name}\": pointer \"#{encoded_pointer}\" in OpenAPI document \"{}\" does not select a directly routable Operation Object under paths; missing, non-operation, referenced and non-routable locations cannot supply request context", document.label),
        ))?;
        let base = self
            .inner
            .index
            .source_bases
            .get(source_name)
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::SourceDescriptionNotFound,
                    format!("sourceDescription \"{source_name}\" has no document request base"),
                )
            })?;
        resolved_operation(target, &entry, base)
    }
}

/// Derives the document's request base from its OpenAPI Object `servers`
/// under the one Server Object rule, [`read_servers`]. The document level has
/// no level above it, so declaring nothing is as fatal here as declaring
/// something unusable.
pub(super) fn derive_servers_base(
    root: &serde_yaml_ng::Value,
    sd: &SourceDescription,
    document: &str,
) -> Result<String, RuntimeError> {
    match read_servers(root.get("servers")) {
        ServersField::Base(base) => Ok(base),
        ServersField::Unusable(ServerIssue::Relative(url)) => Err(RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionParse,
            format!(
                "sourceDescription \"{}\": server URL \"{url}\" in OpenAPI document \
                 \"{document}\" is relative; an absolute URL is required to derive the request \
                 base",
                sd.name,
            ),
        )),
        ServersField::Undeclared
        | ServersField::Unusable(ServerIssue::NotAnArray | ServerIssue::MissingUrl) => {
            Err(RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionParse,
                format!(
                    "sourceDescription \"{}\": OpenAPI document \"{document}\" declares no \
                     servers[0].url; an absolute server URL is required to derive the request \
                     base",
                    sd.name,
                ),
            ))
        }
    }
}

/// What one `servers` field yields.
enum ServersField {
    /// The field is absent, or an empty array: this level declares nothing.
    Undeclared,
    /// The request base the field yields.
    Base(String),
    /// The field is present but yields no usable server.
    Unusable(ServerIssue),
}

/// The one Server Object rule, applied alike at the document, Path Item, and
/// Operation levels: the first Server Object's `url`, its variables' defaults
/// substituted, a trailing `/` trimmed, and a relative reference refused
/// rather than guessed at.
///
/// `generate` repeats the substitution in its typed generation-time
/// diagnostic, but that diagnostic still treats only a leading `/` as
/// relative.
fn read_servers(field: Option<&serde_yaml_ng::Value>) -> ServersField {
    let Some(field) = field else {
        return ServersField::Undeclared;
    };
    let Some(servers) = field.as_sequence() else {
        return ServersField::Unusable(ServerIssue::NotAnArray);
    };
    let Some(first_server) = servers.first() else {
        return ServersField::Undeclared;
    };
    let mut url = first_server
        .get("url")
        .and_then(serde_yaml_ng::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if url.is_empty() {
        return ServersField::Unusable(ServerIssue::MissingUrl);
    }
    if let Some(vars) = first_server
        .get("variables")
        .and_then(serde_yaml_ng::Value::as_mapping)
    {
        for (name, var) in vars {
            let (Some(name), Some(default)) = (
                name.as_str(),
                var.get("default").and_then(serde_yaml_ng::Value::as_str),
            ) else {
                continue;
            };
            url = url.replace(&format!("{{{name}}}"), default);
        }
    }
    if document_set::is_relative_reference(&url) {
        return ServersField::Unusable(ServerIssue::Relative(url));
    }
    ServersField::Base(url.trim_end_matches('/').to_string())
}

/// The server an operation declares for itself. The Operation Object's
/// `servers` override the Path Item Object's (OpenAPI 3.2 §4.10.1), which
/// override the document's (§4.9.1); a level that declares nothing passes to
/// the one above, and the document level is the caller's.
///
/// This is the one place that precedence lives. A resolver that reaches an
/// operation other than by operationId — the specification's JSON Pointer
/// `operationPath` — must call it too.
fn operation_server(
    path_item: &serde_yaml_ng::Value,
    operation: &serde_yaml_ng::Value,
) -> OperationServer {
    for (level, object) in [
        (ServerLevel::Operation, operation),
        (ServerLevel::PathItem, path_item),
    ] {
        match read_servers(object.get("servers")) {
            ServersField::Undeclared => continue,
            ServersField::Base(base) => return OperationServer::Declared(base),
            ServersField::Unusable(issue) => {
                return OperationServer::Unusable(UnusableServer { level, issue })
            }
        }
    }
    OperationServer::Inherited
}
