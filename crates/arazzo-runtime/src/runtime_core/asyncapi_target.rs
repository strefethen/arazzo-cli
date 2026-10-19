//! Local AsyncAPI channel projection for a future execution integration.
//! This module does not create a transport or participate in engine build.

use std::path::Path;

use arazzo_spec::{
    classify_operation_path, ArazzoSpec, OperationPathTarget, SourceType, Step, StepAction,
    StepTarget,
};
use serde_yaml_ng::Value as Yaml;

use super::document_set::{DocumentSet, LocalSourceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolveErrorKind {
    InvalidTarget,
    InvalidDocument,
    MissingSource,
    MissingBase,
    MissingFile,
    MissingChannel,
    UnsupportedCapability,
}

#[derive(Debug)]
pub(super) struct ResolveError {
    pub(super) kind: ResolveErrorKind,
    pub(super) target: String,
    pub(super) source_name: Option<String>,
    pub(super) document: Option<String>,
    pub(super) detail: String,
}

impl ResolveError {
    fn new(
        kind: ResolveErrorKind,
        target: &str,
        source_name: Option<&str>,
        document: Option<&str>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            target: target.to_string(),
            source_name: source_name.map(str::to_string),
            document: document.map(str::to_string),
            detail: detail.into(),
        }
    }
}

/// Selected description data, with the address exactly as authored. `None`
/// means the address is unknown, whether its field was absent or null.
#[derive(Debug)]
pub(super) struct ResolvedChannelTarget {
    pub(super) source_name: String,
    pub(super) document: String,
    pub(super) channel_key: String,
    pub(super) channel_pointer: String,
    pub(super) action: StepAction,
    pub(super) address: Option<String>,
    pub(super) channel: Yaml,
}

/// Resolve one canonical channelPath against its explicitly named local file.
pub(super) fn resolve_local_channel(
    spec: &ArazzoSpec,
    step: &Step,
    retrieval_dir: Option<&Path>,
) -> Result<ResolvedChannelTarget, ResolveError> {
    let target = match step.target.as_ref() {
        Some(StepTarget::ChannelPath(target)) => target.as_str(),
        _ => {
            return Err(ResolveError::new(
                ResolveErrorKind::UnsupportedCapability,
                "",
                None,
                None,
                "only channelPath targets are supported by this resolver",
            ));
        }
    };
    let action = step.action.ok_or_else(|| {
        ResolveError::new(
            ResolveErrorKind::InvalidTarget,
            target,
            None,
            None,
            "channelPath requires an explicit send or receive action",
        )
    })?;
    let (source_name, encoded_pointer) = match classify_operation_path(target) {
        OperationPathTarget::SourcePointer {
            source_name,
            encoded_pointer,
        } => (source_name, encoded_pointer),
        OperationPathTarget::Malformed(reason) => {
            return Err(ResolveError::new(
                ResolveErrorKind::InvalidTarget,
                target,
                None,
                None,
                reason.to_string(),
            ));
        }
    };
    let mut sources = spec
        .source_descriptions
        .iter()
        .filter(|sd| sd.name == source_name);
    let source = sources.next().ok_or_else(|| {
        ResolveError::new(
            ResolveErrorKind::MissingSource,
            target,
            Some(source_name),
            None,
            "named source description is absent",
        )
    })?;
    if sources.next().is_some() || source.type_ != SourceType::AsyncApi || source.url.is_empty() {
        return Err(ResolveError::new(
            ResolveErrorKind::InvalidTarget,
            target,
            Some(source_name),
            None,
            "source must be one unambiguous asyncapi declaration with a URL",
        ));
    }

    // The shared classifier checked percent escapes, UTF-8, and RFC 6901
    // escaping. Decode the fragment once, then unescape each pointer token.
    let pointer = percent_encoding::percent_decode_str(encoded_pointer)
        .decode_utf8()
        .map_err(|err| {
            ResolveError::new(
                ResolveErrorKind::InvalidTarget,
                target,
                Some(source_name),
                None,
                err.to_string(),
            )
        })?;
    let tokens: Vec<String> = pointer
        .strip_prefix('/')
        .into_iter()
        .flat_map(|rest| rest.split('/'))
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect();
    let [channels, channel_key] = tokens.as_slice() else {
        return Err(ResolveError::new(
            ResolveErrorKind::UnsupportedCapability,
            target,
            Some(source_name),
            None,
            "only a direct /channels/<key> pointer is supported",
        ));
    };
    if channels != "channels" || channel_key.is_empty() {
        return Err(ResolveError::new(
            ResolveErrorKind::UnsupportedCapability,
            target,
            Some(source_name),
            None,
            "only a direct /channels/<key> pointer is supported",
        ));
    }

    let documents = DocumentSet::new(spec, retrieval_dir, Vec::new()).map_err(|err| {
        ResolveError::new(
            ResolveErrorKind::InvalidTarget,
            target,
            Some(source_name),
            None,
            err.to_string(),
        )
    })?;
    let document = documents.bind_local_source(source).map_err(|err| {
        let (kind, location, detail) = match err {
            LocalSourceError::MissingBase(detail) => (ResolveErrorKind::MissingBase, None, detail),
            LocalSourceError::InvalidUrl(detail) => (ResolveErrorKind::InvalidTarget, None, detail),
            LocalSourceError::UnsupportedUri(uri) => (
                ResolveErrorKind::UnsupportedCapability,
                Some(uri.clone()),
                format!("non-file source URI {uri} is not acquired"),
            ),
            LocalSourceError::MissingFile(path) => (
                ResolveErrorKind::MissingFile,
                Some(path.display().to_string()),
                "local source file is absent".to_string(),
            ),
            LocalSourceError::ReadFile(path, err) => (
                ResolveErrorKind::InvalidDocument,
                Some(path.display().to_string()),
                format!("reading local source file: {err}"),
            ),
        };
        ResolveError::new(kind, target, Some(source_name), location.as_deref(), detail)
    })?;
    let document_label = document.label;
    let root: Yaml = serde_yaml_ng::from_slice(&document.data).map_err(|err| {
        ResolveError::new(
            ResolveErrorKind::InvalidDocument,
            target,
            Some(source_name),
            Some(&document_label),
            format!("malformed JSON/YAML: {err}"),
        )
    })?;
    if !root.is_mapping() {
        return Err(ResolveError::new(
            ResolveErrorKind::InvalidDocument,
            target,
            Some(source_name),
            Some(&document_label),
            "AsyncAPI document root must be a mapping",
        ));
    }
    let version = root.get("asyncapi").and_then(Yaml::as_str).ok_or_else(|| {
        ResolveError::new(
            ResolveErrorKind::InvalidDocument,
            target,
            Some(source_name),
            Some(&document_label),
            "AsyncAPI version string is missing or invalid",
        )
    })?;
    match supported_version(version) {
        None => {
            return Err(ResolveError::new(
                ResolveErrorKind::InvalidDocument,
                target,
                Some(source_name),
                Some(&document_label),
                format!("malformed AsyncAPI version string {version}"),
            ));
        }
        Some(false) => {
            return Err(ResolveError::new(
                ResolveErrorKind::UnsupportedCapability,
                target,
                Some(source_name),
                Some(&document_label),
                format!("AsyncAPI version {version} is outside supported 3.0.x"),
            ));
        }
        Some(true) => {}
    }
    let channels = match root.get("channels") {
        None => {
            return Err(ResolveError::new(
                ResolveErrorKind::MissingChannel,
                target,
                Some(source_name),
                Some(&document_label),
                "document has no channels map",
            ));
        }
        Some(channels) => channels.as_mapping().ok_or_else(|| {
            ResolveError::new(
                ResolveErrorKind::InvalidDocument,
                target,
                Some(source_name),
                Some(&document_label),
                "channels must be a mapping",
            )
        })?,
    };
    let channel = channels
        .get(Yaml::String(channel_key.clone()))
        .ok_or_else(|| {
            ResolveError::new(
                ResolveErrorKind::MissingChannel,
                target,
                Some(source_name),
                Some(&document_label),
                format!("channel key {channel_key:?} is absent"),
            )
        })?;
    let fields = channel.as_mapping().ok_or_else(|| {
        ResolveError::new(
            ResolveErrorKind::InvalidDocument,
            target,
            Some(source_name),
            Some(&document_label),
            "selected channel must be a Channel Object mapping",
        )
    })?;
    if fields.contains_key(Yaml::String("$ref".into())) {
        return Err(ResolveError::new(
            ResolveErrorKind::UnsupportedCapability,
            target,
            Some(source_name),
            Some(&document_label),
            "channel Reference Objects are not resolved",
        ));
    }
    const CHANNEL_FIELDS: &[&str] = &[
        "address",
        "messages",
        "title",
        "summary",
        "description",
        "servers",
        "parameters",
        "tags",
        "externalDocs",
        "bindings",
    ];
    if fields.keys().any(|field| {
        field
            .as_str()
            .is_none_or(|name| !CHANNEL_FIELDS.contains(&name) && !name.starts_with("x-"))
    }) {
        return Err(ResolveError::new(
            ResolveErrorKind::InvalidDocument,
            target,
            Some(source_name),
            Some(&document_label),
            "selected mapping has fields outside a direct Channel Object",
        ));
    }
    let address = match channel.get("address") {
        None | Some(Yaml::Null) => None,
        Some(Yaml::String(address)) => Some(address.clone()),
        Some(_) => {
            return Err(ResolveError::new(
                ResolveErrorKind::InvalidDocument,
                target,
                Some(source_name),
                Some(&document_label),
                "channel address must be a string or null",
            ));
        }
    };
    Ok(ResolvedChannelTarget {
        source_name: source_name.to_string(),
        document: document_label,
        channel_key: channel_key.clone(),
        channel_pointer: pointer.into_owned(),
        action,
        address,
        channel: channel.clone(),
    })
}

/// `major.minor.patch` with an optional alphanumeric prerelease suffix.
/// Patch revisions of the 3.0 specification have the same tooling contract.
fn supported_version(version: &str) -> Option<bool> {
    let mut parts = version.split('.');
    let (major, minor, patch) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || !major.bytes().all(|byte| byte.is_ascii_digit())
        || !minor.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let major: u64 = major.parse().ok()?;
    let minor: u64 = minor.parse().ok()?;
    let (patch, suffix) = patch.split_once('-').unwrap_or((patch, ""));
    if patch.is_empty()
        || !patch.bytes().all(|byte| byte.is_ascii_digit())
        || patch.parse::<u64>().is_err()
        || (!suffix.is_empty() && !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        || version.ends_with('-')
    {
        return None;
    }
    Some(major == 3 && minor == 0)
}
