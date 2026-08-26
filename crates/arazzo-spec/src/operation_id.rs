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

/// The runtime-expression namespace the source-qualified form names.
///
/// Matched case-sensitively: Arazzo runtime expressions are not case-folded,
/// so `$sourcedescriptions.alpha.getPet` names no source.
const QUALIFIED_NAMESPACE: &str = "$sourceDescriptions";

/// The same namespace as it appears inside a `{$…}` interpolation.
const BRACED_NAMESPACE: &str = "{$sourceDescriptions";

/// The `operationId` forms this runtime resolves, phrased for error text so
/// every surface points at the same set.
pub const SUPPORTED_OPERATION_ID_FORMS: &str =
    "a bare \"operationId\", or \"$sourceDescriptions.<name>.<operationId>\" naming exactly one \
     source description and one operation";

/// Why an `operationId` that reaches for the source-qualified form is not one.
///
/// Every variant is a refusal, never a fallback to bare lookup: a value that
/// names `$sourceDescriptions` and then fails to name a source and an
/// operation is a typo, and looking it up as a literal operation name would
/// report the wrong problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MalformedOperationId {
    /// A required segment is absent or empty — `$sourceDescriptions.getPet`,
    /// `$sourceDescriptions.alpha.`, `$sourceDescriptions..getPet`.
    MissingSegment,
    /// More than one source segment and one operation segment follow the
    /// namespace — `$sourceDescriptions.alpha.v2.getPet`.
    ExtraSegment,
    /// Written as a `{$…}` interpolation — `{$sourceDescriptions.alpha.getPet}`.
    /// The field carries the expression itself, not a template embedding one.
    Braced,
}

impl MalformedOperationId {
    /// Phrase naming what is wrong with the value, for message interpolation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingSegment => "does not name both a source description and an operation",
            Self::ExtraSegment => "names more than one source description segment",
            Self::Braced => "is wrapped in string-interpolation braces",
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
    // Braces are decided first, and by scanning the whole value rather than
    // only its prefix, for the reason `operation_path.rs` learned: a prefix
    // test alone lets `{$sourceDescriptions.alpha.getPet}` fall through to
    // bare lookup, where it is reported as a missing operation name instead
    // of as the brace typo it is.
    if operation_id.contains(BRACED_NAMESPACE) {
        return OperationIdTarget::Malformed(MalformedOperationId::Braced);
    }
    let Some(rest) = operation_id.strip_prefix(QUALIFIED_NAMESPACE) else {
        return OperationIdTarget::Bare(operation_id);
    };
    let Some(segments) = rest.strip_prefix('.') else {
        // `$sourceDescriptions` alone reaches for the qualified form and names
        // nothing. `$sourceDescriptionsAlpha` is an operation whose name merely
        // starts with the namespace text — the separating dot is what makes a
        // value a reference.
        return if rest.is_empty() {
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment)
        } else {
            OperationIdTarget::Bare(operation_id)
        };
    };

    let mut parts = segments.split('.');
    let (Some(source_name), Some(operation)) = (parts.next(), parts.next()) else {
        return OperationIdTarget::Malformed(MalformedOperationId::MissingSegment);
    };
    if parts.next().is_some() {
        return OperationIdTarget::Malformed(MalformedOperationId::ExtraSegment);
    }
    if source_name.is_empty() || operation.is_empty() {
        return OperationIdTarget::Malformed(MalformedOperationId::MissingSegment);
    }
    OperationIdTarget::SourceQualified {
        source_name,
        operation_id: operation,
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
        // Malformed: one segment where two are required.
        (
            "$sourceDescriptions.getPet",
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment),
        ),
        (
            "$sourceDescriptions",
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment),
        ),
        (
            "$sourceDescriptions.",
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment),
        ),
        // Malformed: an empty segment is a missing one.
        (
            "$sourceDescriptions.alpha.",
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment),
        ),
        (
            "$sourceDescriptions..getPet",
            OperationIdTarget::Malformed(MalformedOperationId::MissingSegment),
        ),
        // Malformed: a dotted operation name is an extra segment, not a
        // source named `alpha.v2`.
        (
            "$sourceDescriptions.alpha.v2.getPet",
            OperationIdTarget::Malformed(MalformedOperationId::ExtraSegment),
        ),
        // Malformed: the field carries the expression, never a `{$…}` template.
        (
            "{$sourceDescriptions.alpha.getPet}",
            OperationIdTarget::Malformed(MalformedOperationId::Braced),
        ),
        (
            "{$sourceDescriptions.alpha.getPet}extra",
            OperationIdTarget::Malformed(MalformedOperationId::Braced),
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
