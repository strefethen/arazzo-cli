//! Document identity and reference resolution for Arazzo Description
//! references (Arazzo 1.1 §5.5 "Parsing Documents", §5.6 "Relative References
//! in Arazzo Description URIs").
//!
//! Resolution and retrieval are separate concerns and only the first lives
//! here. Nothing in this module can reach the network: a reference either
//! matches the identity of a document already provided to the engine, or names
//! a local file, or is refused. That is the mechanism the specification itself
//! describes — §9.6 presents identity-based referencing as what lets an
//! implementation "locate documents from a provided collection without making
//! network requests" — so the absence of retrieval is a design position, not a
//! missing feature.
//!
//! The four ordered phases:
//!
//! 1. Parse the whole provided set and assign each member its identity: its
//!    `$self` when that is an absolute URI, otherwise its retrieval URI as
//!    `file:///abs/path`. Arazzo 1.1 §5.5.2 and OpenAPI 3.2 §4.1.1 state the
//!    same rule — *"references MUST use the target document's `$self` URI if
//!    the `$self` field is present"* — so one rule covers both document kinds.
//!    §5.5 also forbids treating a reference as unresolvable "before
//!    completely parsing all documents provided to the implementation", which
//!    is why the whole set is parsed before any reference is bound.
//! 2. Establish the base URI per §5.6.1: the Arazzo document's `$self` when
//!    absolute; a relative `$self` resolved against the retrieval URI first;
//!    otherwise the retrieval URI itself.
//! 3. Resolve each `sourceDescriptions[].url` against that base URI, then bind
//!    the resulting absolute URI to a document: an identity match in the
//!    provided set wins, a `file://` URI is read from disk, anything else is
//!    refused.
//! 4. Indexing is the caller's job and is unchanged — this module hands back
//!    bytes and a label.

use std::fs;
use std::path::{Path, PathBuf};

use arazzo_spec::{ArazzoSpec, SourceDescription, SourceType};

use super::url_crate::{ParseError, Url};
use super::{RuntimeError, RuntimeErrorKind};

/// A document handed to the engine by its operator, together with the location
/// it was read from when the caller knows one. The retrieval path is what
/// gives an otherwise anonymous document an identity to be referenced by.
pub(super) struct ProvidedDocument {
    data: Vec<u8>,
    retrieval_path: Option<PathBuf>,
}

impl ProvidedDocument {
    pub(super) fn new(data: Vec<u8>, retrieval_path: Option<PathBuf>) -> Self {
        Self {
            data,
            retrieval_path,
        }
    }
}

/// One parsed member of the provided set.
struct SetMember {
    /// The URI this document answers to. `None` when it declares no absolute
    /// `$self` and was handed over without a retrieval path, which leaves it
    /// unreachable by reference — it can still be indexed as an explicitly
    /// provided spec.
    identity: Option<String>,
    /// 1-based position among the documents handed to the builder, preserved
    /// so diagnostics keep naming the same spec after earlier members are
    /// claimed by a Source Description.
    ordinal: usize,
    data: Vec<u8>,
    claimed: bool,
}

/// A Source Description bound to the document it names.
pub(super) struct BoundDocument {
    pub(super) data: Vec<u8>,
    /// How diagnostics name this document: the local path when it was read
    /// from disk, otherwise the URI it was bound by.
    pub(super) label: String,
}

/// The provided set, parsed, with the base URI every reference resolves
/// against.
pub(super) struct DocumentSet {
    /// `None` when the Arazzo document declares no absolute `$self` and the
    /// caller supplied no directory to resolve against, which leaves relative
    /// references unresolvable.
    base: Option<Url>,
    /// The `$self` the Arazzo document declared, and whether a retrieval URI
    /// was available to resolve it against. Kept only so a refusal can tell
    /// the two causes of a missing base URI apart: a `$self` that cannot be
    /// resolved at all, and a relative one with nothing to resolve against.
    declared_self: Option<String>,
    had_retrieval_base: bool,
    members: Vec<SetMember>,
}

impl DocumentSet {
    /// Phases 1 and 2: parse every provided document to learn its identity,
    /// then establish the base URI relative references resolve against.
    pub(super) fn new(
        spec: &ArazzoSpec,
        base_dir: Option<&Path>,
        provided: Vec<ProvidedDocument>,
    ) -> Result<Self, RuntimeError> {
        let retrieval_base = match base_dir {
            Some(dir) => Some(directory_uri(dir)?),
            None => None,
        };
        let members = provided
            .into_iter()
            .enumerate()
            .map(|(idx, document)| {
                let identity = document_self_uri(&document.data)
                    .or_else(|| document.retrieval_path.as_deref().and_then(file_uri));
                SetMember {
                    identity,
                    ordinal: idx + 1,
                    data: document.data,
                    claimed: false,
                }
            })
            .collect();
        Ok(Self {
            had_retrieval_base: retrieval_base.is_some(),
            base: base_uri(spec.self_uri.as_deref(), retrieval_base),
            declared_self: spec.self_uri.clone(),
            members,
        })
    }

    /// Phase 3: resolve one Source Description's url and bind it to a document.
    ///
    /// `Ok(None)` means the source keeps the url-as-base-URL reading this
    /// runtime has always applied to an absolute non-`file` url: that
    /// compatibility rule is a labeled extension owned elsewhere, and honoring
    /// `$self` does not disturb it.
    pub(super) fn bind(
        &mut self,
        sd: &SourceDescription,
    ) -> Result<Option<BoundDocument>, RuntimeError> {
        if sd.type_ != SourceType::OpenApi || sd.url.is_empty() {
            return Ok(None);
        }
        let authored_is_relative =
            matches!(Url::parse(&sd.url), Err(ParseError::RelativeUrlWithoutBase));
        let Some(resolved) = self.resolve(sd, authored_is_relative)? else {
            return Ok(None);
        };
        let identity = identity_of(&resolved);

        if let Some(member) = self
            .members
            .iter_mut()
            .find(|member| member.identity.as_deref() == Some(identity.as_str()))
        {
            // An identity match binds without touching the filesystem, which
            // is the whole point of §5.5.2: the document is already here.
            member.claimed = true;
            return Ok(Some(BoundDocument {
                data: member.data.clone(),
                label: identity,
            }));
        }

        if resolved.scheme() != "file" {
            if !authored_is_relative {
                return Ok(None);
            }
            return Err(RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionNotFound,
                format!(
                    "sourceDescription \"{name}\": url \"{url}\" resolved against base URI \
                     \"{base}\" to \"{resolved}\", which is not a document provided to this \
                     engine; provide that document so it can be bound by identity — no document \
                     is retrieved over the network",
                    name = sd.name,
                    url = sd.url,
                    base = self.base_display(),
                ),
            ));
        }

        let Some(path) = resolved.to_file_path().ok() else {
            return Err(RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionNotFound,
                format!(
                    "sourceDescription \"{name}\": url \"{url}\" resolved against base URI \
                     \"{base}\" to \"{resolved}\", which is not a usable local file path",
                    name = sd.name,
                    url = sd.url,
                    base = self.base_display(),
                ),
            ));
        };
        let data = fs::read(&path).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionLoad,
                format!(
                    "sourceDescription \"{name}\": url \"{url}\" resolved against base URI \
                     \"{base}\" to \"{resolved}\"; reading OpenAPI document \"{path}\": {err}",
                    name = sd.name,
                    url = sd.url,
                    base = self.base_display(),
                    path = path.display(),
                ),
            )
        })?;
        Ok(Some(BoundDocument {
            data,
            label: path.display().to_string(),
        }))
    }

    /// The provided documents no Source Description claimed, each with the
    /// 1-based position it was supplied in. They keep being indexed as
    /// explicitly provided specs; a claimed document belongs to its source and
    /// must not also be indexed as one, or its operations would be defined
    /// twice under two different origins.
    pub(super) fn unclaimed(self) -> Vec<(usize, Vec<u8>)> {
        self.members
            .into_iter()
            .filter(|member| !member.claimed)
            .map(|member| (member.ordinal, member.data))
            .collect()
    }

    /// Joins an authored url onto the base URI, or refuses when a relative url
    /// has no base to resolve against.
    fn resolve(
        &self,
        sd: &SourceDescription,
        authored_is_relative: bool,
    ) -> Result<Option<Url>, RuntimeError> {
        if !authored_is_relative {
            return Ok(Url::parse(&sd.url).ok());
        }
        let Some(base) = self.base.as_ref() else {
            // Which cause is named decides whether the remedy works. A `$self`
            // that had a retrieval URI to resolve against and still produced no
            // base is itself the fault, and supplying a directory would not
            // help. Every other way to arrive here — no `$self`, or a relative
            // one with nothing to resolve it against — is answered by the
            // directory, so do not blame a `$self` that is merely relative.
            let cause = match self.declared_self.as_deref() {
                Some(declared) if self.had_retrieval_base => format!(
                    "its $self \"{declared}\" is not a URI this runtime can resolve against the \
                     document's retrieval URI"
                ),
                _ => "declare an absolute $self, or provide the Arazzo document's directory via \
                      EngineBuilder::source_base_dir"
                    .to_string(),
            };
            return Err(RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionLoad,
                format!(
                    "sourceDescription \"{}\": relative url \"{}\" has no base URI to resolve \
                     against: {cause}",
                    sd.name, sd.url
                ),
            ));
        };
        base.join(&sd.url).map(Some).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::SourceDescriptionLoad,
                format!(
                    "sourceDescription \"{name}\": url \"{url}\" cannot be resolved against base \
                     URI \"{base}\": {err}",
                    name = sd.name,
                    url = sd.url,
                    base = base.as_str(),
                ),
            )
        })
    }

    fn base_display(&self) -> &str {
        self.base.as_ref().map_or("<none>", Url::as_str)
    }
}

/// The local files a build will read, keyed by source name.
///
/// Callers that gate filesystem access (the MCP server) vet these before
/// building an engine. Identity binding only ever removes a read — a source
/// whose reference matches a provided document reads nothing — so this list
/// covers every read the engine can perform, and covers it exactly whenever no
/// documents are provided, which is the MCP server's case.
pub(super) fn local_source_paths(spec: &ArazzoSpec, base_dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(base) = directory_uri(base_dir) else {
        return Vec::new();
    };
    let base = base_uri(spec.self_uri.as_deref(), Some(base));
    spec.source_descriptions
        .iter()
        .filter(|sd| sd.type_ == SourceType::OpenApi && !sd.url.is_empty())
        .filter_map(|sd| {
            let resolved = match Url::parse(&sd.url) {
                Ok(absolute) => absolute,
                Err(ParseError::RelativeUrlWithoutBase) => base.as_ref()?.join(&sd.url).ok()?,
                Err(_) => return None,
            };
            let path = resolved.to_file_path().ok()?;
            Some((sd.name.clone(), path))
        })
        .collect()
}

/// The base URI relative references resolve against, per §5.6.1: an absolute
/// `$self` wins; a relative `$self` is itself resolved against the retrieval
/// URI before being used as a base; with no `$self` the retrieval URI is the
/// base.
fn base_uri(self_uri: Option<&str>, retrieval: Option<Url>) -> Option<Url> {
    match self_uri {
        Some(declared) => match Url::parse(declared) {
            Ok(absolute) => Some(absolute),
            Err(_) => retrieval.and_then(|base| base.join(declared).ok()),
        },
        None => retrieval,
    }
}

/// A document's self-assigned URI, when it declares an absolute one. A
/// document this runtime cannot parse contributes no identity; the parse
/// failure keeps surfacing where it always has, at indexing.
fn document_self_uri(data: &[u8]) -> Option<String> {
    let root: serde_yaml_ng::Value = serde_yaml_ng::from_slice(data).ok()?;
    let declared = root.get("$self")?.as_str()?;
    Url::parse(declared).ok().map(|uri| identity_of(&uri))
}

/// The URI a document is referenced by, which is the resolved URI without its
/// fragment: §5.6.2 resolves a fragment inside the referenced document, so the
/// fragment never selects which document is meant.
fn identity_of(uri: &Url) -> String {
    let mut identity = uri.clone();
    identity.set_fragment(None);
    identity.into()
}

/// The `file://` URI of a local path, absolute-ized without touching the
/// filesystem so a path that does not exist still has an identity.
fn file_uri(path: &Path) -> Option<String> {
    let absolute = absolute_path(path).ok()?;
    Url::from_file_path(absolute).ok().map(String::from)
}

/// The `file://` URI of a directory, which is the retrieval URI an Arazzo
/// document read from that directory resolves against. The trailing slash
/// `from_directory_path` adds is what makes RFC 3986 §5.2 resolution treat the
/// directory as the reference point rather than stripping its last segment.
fn directory_uri(dir: &Path) -> Result<Url, RuntimeError> {
    let absolute = absolute_path(dir).map_err(|err| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionLoad,
            format!(
                "resolving the Arazzo document's directory \"{}\": {err}",
                dir.display()
            ),
        )
    })?;
    Url::from_directory_path(&absolute).map_err(|()| {
        RuntimeError::new(
            RuntimeErrorKind::SourceDescriptionLoad,
            format!(
                "the Arazzo document's directory \"{}\" is not a usable base URI",
                absolute.display()
            ),
        )
    })
}

/// An absolute path in normal form, computed lexically — no filesystem access,
/// so a path that does not exist still has a URI. An empty path means "where
/// the process is", which is what an Arazzo document named without any
/// directory is retrieved relative to.
fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.as_os_str().is_empty() {
        std::env::current_dir()?
    } else {
        std::path::absolute(path)?
    };
    Ok(remove_dot_segments(&absolute))
}

/// Drops `.` and `..` segments, which RFC 3986 §5.2.4 requires of a resolved
/// URI and §6.2.2.3 requires before two URIs are compared. Without it a
/// document reached through `../` would carry a different identity than the
/// same document named directly, and no reference to it would ever match.
///
/// This is a URI operation, not a filesystem one, and the two disagree in one
/// place: for `/a/link/../b` where `link` is a symlink, the kernel would
/// answer relative to the link's target and this answers `/a/b`. Resolving
/// symlinks instead would be the wrong trade — it needs the path to exist,
/// which a reference being resolved need not, and §5.6.1 asks for RFC 3986
/// resolution, not for whatever the filesystem would have done.
fn remove_dot_segments(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // At the root there is nothing above to ascend to, which is
                // the same floor RFC 3986 §5.2.4 puts on the output buffer.
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}
