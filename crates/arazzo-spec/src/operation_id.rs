//! Classification of a step's `operationId` value.
//!
//! Arazzo 1.1 Step Object, `operationId`: *"If multiple (non arazzo type)
//! sourceDescriptions are defined, then the operationId MUST be specified
//! using a Runtime Expression (e.g., `$sourceDescriptions.<name>.<operationId>`)
//! to avoid ambiguity or potential clashes."*
//!
//! Two crates read that value — `arazzo-runtime` resolves it against the named
//! source and builds the request URL from that source's server base, and
//! `arazzo-validate` checks the source it names — so both route through
//! [`classify_operation_id`]. `operation_path.rs` is the precedent, and the
//! reason for it: two independently written prefix parsers for one field is
//! exactly how that surface drifted.
//!
//! The qualified form is §5.9's `source-reference` production, which
//! [`crate::source_reference`] owns for this field and for `dependsOn` alike.
//! Its one counter-intuitive rule matters here: `source-reference-id` is
//! `1*CHAR`, annotated *"operationIds have no character restrictions in
//! OpenAPI/AsyncAPI"*, so `$sourceDescriptions.alpha.svc.v1.getPet` names the
//! operation `svc.v1.getPet` and is perfectly legal.

use crate::source_reference::{parse_source_reference, SourceReferenceError};

/// The runtime-expression namespace the source-qualified form names.
///
/// Matched case-sensitively: Arazzo runtime expressions are not case-folded,
/// so `$sourcedescriptions.alpha.getPet` names no source.
const QUALIFIED_NAMESPACE: &str = "$sourceDescriptions";

/// The `operationId` forms this runtime resolves, phrased for error text so
/// every surface points at the same set.
pub const SUPPORTED_OPERATION_ID_FORMS: &str =
    "a bare \"operationId\", or \"$sourceDescriptions.<name>.<operationId>\" where <name> matches \
     [A-Za-z0-9_-]+";

/// Which rule of the `source-reference` production a qualified `operationId`
/// broke.
///
/// Every variant is a refusal, never a fallback to bare lookup: a value that
/// names `$sourceDescriptions` and then breaks the production is a typo, and
/// looking it up as a literal operation name would report the wrong problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MalformedOperationId {
    /// No `.<operationId>` follows the source name — `$sourceDescriptions`,
    /// `$sourceDescriptions.alpha`.
    MissingOperationId,
    /// The source name is empty or is not `identifier-strict` —
    /// `$sourceDescriptions..getPet`, `$sourceDescriptions.a b.getPet`.
    SourceName,
    /// The operation name is empty or leaves the `CHAR` rule, which excludes
    /// braces, quotes, lone backslashes, and controls —
    /// `$sourceDescriptions.alpha.`, `{$sourceDescriptions.alpha.getPet}`.
    OperationId,
}

impl MalformedOperationId {
    /// Phrase naming what is wrong with the value, for message interpolation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingOperationId => "names a source description but no operation",
            Self::SourceName => {
                "names a source description that is empty or uses characters outside [A-Za-z0-9_-]"
            }
            Self::OperationId => {
                "names an operation that is empty or uses characters the reference grammar excludes"
            }
        }
    }
}

impl From<SourceReferenceError> for MalformedOperationId {
    fn from(err: SourceReferenceError) -> Self {
        match err {
            SourceReferenceError::MissingSeparator => Self::MissingOperationId,
            SourceReferenceError::SourceName => Self::SourceName,
            SourceReferenceError::ReferenceId => Self::OperationId,
        }
    }
}

impl std::fmt::Display for MalformedOperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a step's `operationId` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationIdTarget<'a> {
    /// An unqualified operation name, resolvable only when the ambiguity the
    /// specification's MUST guards against cannot arise.
    Bare(&'a str),
    /// `$sourceDescriptions.<name>.<operationId>` — resolved against that one
    /// Source Description and no other.
    SourceQualified {
        source_name: &'a str,
        operation_id: &'a str,
    },
    /// A value reaching for the qualified form without being it.
    Malformed(MalformedOperationId),
}

impl<'a> OperationIdTarget<'a> {
    /// The Source Description this value resolves against, if any.
    pub const fn source_name(&self) -> Option<&'a str> {
        match *self {
            Self::SourceQualified { source_name, .. } => Some(source_name),
            _ => None,
        }
    }

    /// Why this value cannot be resolved, or `None` when it can be.
    pub const fn malformed(&self) -> Option<MalformedOperationId> {
        match *self {
            Self::Malformed(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Classifies a step's `operationId` into the target it names.
pub fn classify_operation_id(operation_id: &str) -> OperationIdTarget<'_> {
    let Some(rest) = operation_id.strip_prefix(QUALIFIED_NAMESPACE) else {
        return OperationIdTarget::Bare(operation_id);
    };
    let Some(reference) = rest.strip_prefix('.') else {
        // `$sourceDescriptions` alone reaches for the qualified form and names
        // nothing. `$sourceDescriptionsAlpha` is an operation whose name merely
        // starts with the namespace text — the separating dot is what makes a
        // value a reference.
        return if rest.is_empty() {
            OperationIdTarget::Malformed(MalformedOperationId::MissingOperationId)
        } else {
            OperationIdTarget::Bare(operation_id)
        };
    };
    match parse_source_reference(reference) {
        Ok(parsed) => OperationIdTarget::SourceQualified {
            source_name: parsed.source_name,
            operation_id: parsed.reference_id,
        },
        Err(err) => OperationIdTarget::Malformed(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per shape `operationId` can take. A new shape means a new row
    /// here before it means new behavior anywhere else.
    const TABLE: &[(&str, OperationIdTarget<'static>)] = &[
        // Bare: the only form a single-source document may use.
        ("getPet", OperationIdTarget::Bare("getPet")),
        ("get.pet", OperationIdTarget::Bare("get.pet")),
        ("", OperationIdTarget::Bare("")),
        // Bare: text that merely starts with the namespace is not the
        // qualified form — the separating dot is required.
        (
            "$sourceDescriptionsAlpha",
            OperationIdTarget::Bare("$sourceDescriptionsAlpha"),
        ),
        // Bare: the namespace is case-sensitive, so this names no source.
        (
            "$sourcedescriptions.alpha.getPet",
            OperationIdTarget::Bare("$sourcedescriptions.alpha.getPet"),
        ),
        // Bare: a different namespace is not this one.
        ("$inputs.getPet", OperationIdTarget::Bare("$inputs.getPet")),
        // Source-qualified: the form the specification's MUST requires.
        (
            "$sourceDescriptions.alpha.getPet",
            OperationIdTarget::SourceQualified {
                source_name: "alpha",
                operation_id: "getPet",
            },
        ),
        (
            "$sourceDescriptions.a.b",
            OperationIdTarget::SourceQualified {
                source_name: "a",
                operation_id: "b",
            },
        ),
        // Source-qualified: `source-reference-id` is `1*CHAR`, annotated in
        // the grammar as "operationIds have no character restrictions", so the
        // split is at the *first* dot and everything after it is the operation
        // — `svc.v1.getPet` is one name, not three segments.
        (
            "$sourceDescriptions.alpha.svc.v1.getPet",
            OperationIdTarget::SourceQualified {
                source_name: "alpha",
                operation_id: "svc.v1.getPet",
            },
        ),
        // Malformed: a source description named, but no operation after it.
        (
            "$sourceDescriptions.getPet",
            OperationIdTarget::Malformed(MalformedOperationId::MissingOperationId),
        ),
        (
            "$sourceDescriptions",
            OperationIdTarget::Malformed(MalformedOperationId::MissingOperationId),
        ),
        (
            "$sourceDescriptions.",
            OperationIdTarget::Malformed(MalformedOperationId::MissingOperationId),
        ),
        // Malformed: `source-name` is `identifier-strict`, so it can be
        // neither empty nor spaced.
        (
            "$sourceDescriptions..getPet",
            OperationIdTarget::Malformed(MalformedOperationId::SourceName),
        ),
        (
            "$sourceDescriptions.al pha.getPet",
            OperationIdTarget::Malformed(MalformedOperationId::SourceName),
        ),
        // Malformed: the operation segment is empty, or leaves `CHAR` — which
        // is where a `{$…}` template lands, since `CHAR` excludes braces.
        (
            "$sourceDescriptions.alpha.",
            OperationIdTarget::Malformed(MalformedOperationId::OperationId),
        ),
        (
            "$sourceDescriptions.alpha.get{Pet}",
            OperationIdTarget::Malformed(MalformedOperationId::OperationId),
        ),
        // Bare: braces before the namespace mean the value never reaches the
        // qualified form at all. It resolves — and fails — as a literal name.
        (
            "{$sourceDescriptions.alpha.getPet}",
            OperationIdTarget::Bare("{$sourceDescriptions.alpha.getPet}"),
        ),
    ];

    #[test]
    fn classification_table() {
        for (input, expected) in TABLE {
            assert_eq!(
                classify_operation_id(input),
                *expected,
                "classification for {input:?}"
            );
        }
    }

    #[test]
    fn source_name_is_reported_only_for_qualified_values() {
        for (input, target) in TABLE {
            let expected = match target {
                OperationIdTarget::SourceQualified { source_name, .. } => Some(*source_name),
                _ => None,
            };
            assert_eq!(
                classify_operation_id(input).source_name(),
                expected,
                "source name for {input:?}"
            );
        }
    }

    #[test]
    fn malformed_is_reported_only_for_malformed_values() {
        for (input, target) in TABLE {
            let expected = match target {
                OperationIdTarget::Malformed(reason) => Some(*reason),
                _ => None,
            };
            assert_eq!(
                classify_operation_id(input).malformed(),
                expected,
                "malformed verdict for {input:?}"
            );
        }
    }

    /// The negative case the specification's example most invites: a source
    /// name and an operation name that differ only in case are different
    /// references, and neither is silently folded into the other.
    #[test]
    fn qualified_segments_keep_their_case() {
        let classified = classify_operation_id("$sourceDescriptions.Alpha.GetPet");
        assert_eq!(
            classified,
            OperationIdTarget::SourceQualified {
                source_name: "Alpha",
                operation_id: "GetPet",
            }
        );
    }
}
