//! The standard source-document expression and JSON Pointer `operationPath`.
//!
//! Syntax is shared by validation and execution; selecting an actual operation
//! remains the runtime's responsibility.
use crate::source_reference::parse_source_reference;

/// The standard target syntax accepted by the shared parser.
pub const SUPPORTED_OPERATION_PATH_FORMS: &str =
    "\"{$sourceDescriptions.<name>.url}#<JSON Pointer>\"";

/// Why a value is not a standard operation target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedOperationPath {
    /// The document must be named by its source URL runtime expression.
    SourceExpression,
    /// The fragment must encode a valid UTF-8 RFC 6901 JSON Pointer.
    JsonPointer,
}

impl std::fmt::Display for MalformedOperationPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SourceExpression => "must name a source URL using {$sourceDescriptions.<name>.url} followed by # and a JSON Pointer",
            Self::JsonPointer => "must carry a valid URI-fragment JSON Pointer",
        })
    }
}

/// A source-bound pointer, or the reason the authored target is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPathTarget<'a> {
    /// `encoded_pointer` excludes the leading `#` and retains percent encoding.
    SourcePointer {
        source_name: &'a str,
        encoded_pointer: &'a str,
    },
    /// A custom request path, method prefix, or malformed standard expression.
    Malformed(MalformedOperationPath),
}

impl<'a> OperationPathTarget<'a> {
    /// The source this pointer names, when its syntax is valid.
    pub const fn source_name(self) -> Option<&'a str> {
        match self {
            Self::SourcePointer { source_name, .. } => Some(source_name),
            Self::Malformed(_) => None,
        }
    }

    /// The syntax error, if any.
    pub const fn malformed(self) -> Option<MalformedOperationPath> {
        match self {
            Self::Malformed(reason) => Some(reason),
            Self::SourcePointer { .. } => None,
        }
    }
}

/// Parses the standard source URL expression and URI-fragment JSON Pointer.
///
/// Bare paths, request URLs, source-routed shortcuts, and method prefixes are
/// not Arazzo operation targets and are rejected rather than interpreted.
pub fn classify_operation_path(value: &str) -> OperationPathTarget<'_> {
    let source = || OperationPathTarget::Malformed(MalformedOperationPath::SourceExpression);
    let Some(rest) = value.strip_prefix("{$sourceDescriptions.") else {
        return source();
    };
    let Some((reference, encoded_pointer)) = rest.split_once("}#") else {
        return source();
    };
    let Ok(reference) = parse_source_reference(reference) else {
        return source();
    };
    if reference.reference_id != "url" {
        return source();
    }
    if !valid_pointer_fragment(encoded_pointer) {
        return OperationPathTarget::Malformed(MalformedOperationPath::JsonPointer);
    }
    OperationPathTarget::SourcePointer {
        source_name: reference.source_name,
        encoded_pointer,
    }
}

fn valid_pointer_fragment(fragment: &str) -> bool {
    let mut decoded = Vec::with_capacity(fragment.len());
    let mut bytes = fragment.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let (Some(a), Some(b)) = (bytes.next(), bytes.next()) else {
                return false;
            };
            let (Some(a), Some(b)) = ((a as char).to_digit(16), (b as char).to_digit(16)) else {
                return false;
            };
            decoded.push((a * 16 + b) as u8);
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&byte) {
            decoded.push(byte);
        } else {
            return false;
        }
    }
    let Ok(pointer) = std::str::from_utf8(&decoded) else {
        return false;
    };
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

/// Static presentation cannot know the method without the bound document.
/// The authored target is retained verbatim, including malformed input.
pub fn presented_method_and_target(operation_path: &str) -> (Option<&str>, &str) {
    (None, operation_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_source_and_encoded_pointer_are_retained() {
        for pointer in [
            "/paths/~1pets/get",
            "/paths/~1a~0b%20c/query",
            "%2Fpaths%2F~1pets%2Fget",
            "",
        ] {
            let value = format!("{{$sourceDescriptions.api_1.url}}#{pointer}");
            assert_eq!(
                classify_operation_path(&value),
                OperationPathTarget::SourcePointer {
                    source_name: "api_1",
                    encoded_pointer: pointer,
                }
            );
        }
    }

    #[test]
    fn custom_and_incomplete_targets_are_rejected() {
        for value in [
            "/pets",
            "https://api.test/pets",
            "{api}./pets",
            "GET {$sourceDescriptions.api.url}#/paths/~1pets/get",
            "{$sourceDescriptions.api.url}",
            "#/paths/~1pets/get",
            "{$sourceDescriptions.a.b.url}#/paths/x/get",
            "{$sourceDescriptions.api.other}#/paths/x/get",
        ] {
            assert!(
                classify_operation_path(value).malformed().is_some(),
                "{value}"
            );
        }
        for pointer in [
            "paths/x/get",
            "/paths/~2/get",
            "/paths/%/get",
            "/paths/%GG/get",
            "/paths/%FF/get",
            "/paths/x#get",
            "/paths/a b/get",
            "/paths/{x}/get",
        ] {
            let value = format!("{{$sourceDescriptions.api.url}}#{pointer}");
            assert_eq!(
                classify_operation_path(&value).malformed(),
                Some(MalformedOperationPath::JsonPointer),
                "{value}"
            );
        }
    }

    #[test]
    fn static_presentation_never_guesses_a_method() {
        for target in [
            "{$sourceDescriptions.api.url}#/paths/~1pets/get",
            "POST /pets",
        ] {
            assert_eq!(presented_method_and_target(target), (None, target));
        }
    }
}
