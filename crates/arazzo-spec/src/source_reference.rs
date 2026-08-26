//! The `source-reference` production, shared by every `$sourceDescriptions.…`
//! field.
//!
//! Arazzo 1.1 §5.9 Runtime Expressions, verbatim from `spec/arazzo/v1.1.0.html`:
//!
//! ```abnf
//! source-reference    = source-name "." source-reference-id
//! source-name         = identifier-strict
//! source-reference-id = 1*CHAR
//!     ; operationIds have no character restrictions in OpenAPI/AsyncAPI
//! identifier-strict   = 1*( ALPHA / DIGIT / "-" / "_" )
//!     ; For step IDs, workflow IDs, and sourceDescription names (no dots)
//! ```
//!
//! Two consequences are easy to get wrong, and both are load-bearing: the
//! source name may not contain a dot, and the trailing id may — so the split
//! is at the *first* dot, and `$sourceDescriptions.alpha.svc.v1.getPet` names
//! the operation `svc.v1.getPet` in source `alpha`. `dependsOn` and a step's
//! `operationId` are the same production, so they parse it here rather than
//! each carrying a reading of it.

/// A `source-reference` split into its two segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceReference<'a> {
    pub(crate) source_name: &'a str,
    pub(crate) reference_id: &'a str,
}

/// Which rule of the production a value broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceReferenceError {
    /// No `"." source-reference-id` follows the source name.
    MissingSeparator,
    /// `source-name` is empty or is not `identifier-strict`.
    SourceName,
    /// `source-reference-id` is empty or leaves the `CHAR` rule.
    ReferenceId,
}

/// Parses the text that follows `"$sourceDescriptions."`.
///
/// Callers strip the namespace themselves because they disagree about what a
/// non-namespace value means — a `dependsOn` value is a local workflow id, a
/// step's `operationId` is an unqualified operation name.
pub(crate) fn parse_source_reference(
    reference: &str,
) -> Result<SourceReference<'_>, SourceReferenceError> {
    // At the first dot, not the last: `source-name` is `identifier-strict`
    // and so cannot contain one, while `source-reference-id` is `1*CHAR` and
    // can.
    let Some((source_name, reference_id)) = reference.split_once('.') else {
        return Err(SourceReferenceError::MissingSeparator);
    };
    if !is_identifier_strict(source_name) {
        return Err(SourceReferenceError::SourceName);
    }
    if !is_source_reference_id(reference_id) {
        return Err(SourceReferenceError::ReferenceId);
    }
    Ok(SourceReference {
        source_name,
        reference_id,
    })
}

/// Validates the vendored `identifier-strict` rule: `1*( ALPHA / DIGIT / "-" / "_" )`.
pub(crate) fn is_identifier_strict(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Validates the vendored `CHAR` rule used by `source-reference-id`.
///
/// The rule permits Unicode characters except `{`, `}`, `"`, and `\`, with
/// JSON-style escapes for those characters and for controls. The
/// source-reference-id itself must contain at least one CHAR token.
pub(crate) fn is_source_reference_id(value: &str) -> bool {
    let mut chars = value.chars();
    let mut token_count = 0;
    while let Some(ch) = chars.next() {
        token_count += 1;
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                return false;
            };
            match escaped {
                '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => {}
                'u' => {
                    if (0..4).any(|_| chars.next().is_none_or(|hex| !hex.is_ascii_hexdigit())) {
                        return false;
                    }
                }
                _ => return false,
            }
        } else if !matches!(
            ch as u32,
            0x20..=0x21 | 0x23..=0x5b | 0x5d..=0x7a | 0x7c | 0x7e..=0x10ffff
        ) {
            return false;
        }
    }
    token_count > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per way the production can be satisfied or broken. Both
    /// `dependsOn` and a step's `operationId` inherit every row.
    const TABLE: &[(&str, Result<SourceReference<'static>, SourceReferenceError>)] = &[
        (
            "alpha.getPet",
            Ok(SourceReference {
                source_name: "alpha",
                reference_id: "getPet",
            }),
        ),
        // The trailing id is `1*CHAR`, so dots in it belong to the operation.
        (
            "alpha.svc.v1.getPet",
            Ok(SourceReference {
                source_name: "alpha",
                reference_id: "svc.v1.getPet",
            }),
        ),
        // `identifier-strict` allows `-` and `_`, and both segments are
        // case-sensitive.
        (
            "Alpha-2_beta.Get Pet",
            Ok(SourceReference {
                source_name: "Alpha-2_beta",
                reference_id: "Get Pet",
            }),
        ),
        ("alpha", Err(SourceReferenceError::MissingSeparator)),
        ("", Err(SourceReferenceError::MissingSeparator)),
        (".getPet", Err(SourceReferenceError::SourceName)),
        // A dot is exactly what `identifier-strict` excludes, so the first one
        // always ends the source name. A value meant to name a source called
        // `al.pha` silently names `al` instead — which is why an unknown-source
        // error, not a grammar error, is what the caller reports here.
        (
            "al.pha.getPet",
            Ok(SourceReference {
                source_name: "al",
                reference_id: "pha.getPet",
            }),
        ),
        ("al pha.getPet", Err(SourceReferenceError::SourceName)),
        ("al/pha.getPet", Err(SourceReferenceError::SourceName)),
        ("alpha.", Err(SourceReferenceError::ReferenceId)),
        // `CHAR` is adapted to exclude braces, quotes, and lone backslashes.
        ("alpha.get{Pet}", Err(SourceReferenceError::ReferenceId)),
        ("alpha.get\"Pet", Err(SourceReferenceError::ReferenceId)),
        ("alpha.getPet\\", Err(SourceReferenceError::ReferenceId)),
        ("alpha.getPet\\q", Err(SourceReferenceError::ReferenceId)),
        ("alpha.getPet\n", Err(SourceReferenceError::ReferenceId)),
    ];

    #[test]
    fn production_table() {
        for (input, expected) in TABLE {
            assert_eq!(
                parse_source_reference(input),
                *expected,
                "source-reference for {input:?}"
            );
        }
    }

    #[test]
    fn identifier_strict_rejects_dots_and_emptiness() {
        assert!(is_identifier_strict("a-Z_0"));
        assert!(!is_identifier_strict(""));
        assert!(!is_identifier_strict("a.b"));
        assert!(!is_identifier_strict("a b"));
    }
}
