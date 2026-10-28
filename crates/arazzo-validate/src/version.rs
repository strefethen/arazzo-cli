//! Arazzo version syntax and the supported major.minor feature sets.
//!
//! `spec/arazzo/v1.1.0.html#versions` assigns features to major.minor and
//! says tooling SHOULD NOT distinguish patches. Decimal components are
//! compared without integer conversion, so neither patch size nor leading
//! zeros change the feature set or the document's original version text.

use crate::{Diagnostic, Severity, ValidationErrorKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    Missing,
    Malformed,
    Unsupported,
    V1_0,
    V1_1,
}

fn classify(declared: &str) -> Version {
    if declared.is_empty() {
        return Version::Missing;
    }
    let mut parts = declared.split('.');
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Version::Malformed;
    };
    if [major, minor, patch]
        .iter()
        .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Version::Malformed;
    }
    // Removing leading zeros compares decimal spellings without imposing
    // a machine-integer bound. An all-zero component becomes the empty str.
    match (major.trim_start_matches('0'), minor.trim_start_matches('0')) {
        ("1", "") => Version::V1_0,
        ("1", "1") => Version::V1_1,
        _ => Version::Unsupported,
    }
}

/// One top-level finding, preserving the existing required-field contract.
pub(crate) fn diagnostic(declared: &str) -> Option<Diagnostic> {
    let (kind, message) = match classify(declared) {
        Version::Missing => (
            ValidationErrorKind::MissingRequiredField,
            "arazzo version is required".to_string(),
        ),
        Version::Malformed => (
            ValidationErrorKind::UnsupportedVersion,
            format!(
                "invalid arazzo version: {declared} (expected major.minor.patch with decimal components)"
            ),
        ),
        Version::Unsupported => (
            ValidationErrorKind::UnsupportedVersion,
            format!(
                "unsupported arazzo version: {declared} (supported feature sets: 1.0, 1.1)"
            ),
        ),
        Version::V1_0 | Version::V1_1 => return None,
    };
    Some(Diagnostic {
        severity: Severity::Error,
        kind,
        path: "arazzo".to_string(),
        message,
    })
}

/// Only a valid supported 1.0 declaration selects the pre-1.1 vocabulary.
pub(crate) fn declares_pre_1_1(declared: &str) -> bool {
    classify(declared) == Version::V1_0
}
