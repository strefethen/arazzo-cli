//! Classification of a step's `operationPath` value.
//!
//! `operationPath` is read by more than one crate: `arazzo-runtime` resolves it
//! into a request URL, and `arazzo-validate` checks the Source Description it
//! names. Both route through [`classify_operation_path`] so the two surfaces
//! cannot disagree about what a value means.
//!
//! They did disagree. Each carried its own `starts_with('{')` prefix parser,
//! and only the runtime's stripped a leading `"<METHOD> "` token first — so the
//! validator's unknown-`sourceDescription` check was silently skipped for every
//! `"<METHOD> {source}./path"` step, 27 of them across seven `examples/` files.

/// The `operationPath` forms this runtime resolves, phrased for error and
/// warning text so every surface points at the same set.
pub const SUPPORTED_OPERATION_PATH_FORMS: &str =
    "\"{sourceName}./path\", an absolute URL, or a path resolved against the base URL, \
     each optionally prefixed with an HTTP method";

/// Why an `operationPath` cannot be resolved by this runtime.
///
/// The Arazzo specification defines `operationPath` as a Source Description
/// reference combined with a JSON Pointer, written in runtime expression syntax
/// — `{$sourceDescriptions.petstore.url}#/paths/~1pets/get`. Nothing here
/// resolves JSON Pointers against a source document, so such a value is
/// reported rather than pasted into a request URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnsupportedOperationPath {
    /// Carries an unresolved runtime expression, e.g. `{$sourceDescriptions.api.url}`.
    RuntimeExpression,
    /// Carries a JSON Pointer fragment, e.g. `#/paths/~1pets/get`.
    JsonPointer,
    /// Carries both — the fully conformant specification form.
    RuntimeExpressionWithJsonPointer,
}

impl UnsupportedOperationPath {
    /// Noun phrase naming what the value carries, for message interpolation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeExpression => "a runtime expression",
            Self::JsonPointer => "a JSON Pointer fragment",
            Self::RuntimeExpressionWithJsonPointer => {
                "a runtime expression and a JSON Pointer fragment"
            }
        }
    }
}

impl std::fmt::Display for UnsupportedOperationPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an `operationPath` resolves to, after any leading `"<METHOD> "` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPathForm<'a> {
    /// `{sourceName}.<path>` — routed through the named Source Description.
    SourceRouted { source_name: &'a str, path: &'a str },
    /// An absolute `http://` or `https://` request URL.
    AbsoluteUrl(&'a str),
    /// A path resolved against the base URL of the first Source Description.
    BasePath(&'a str),
    /// The specification form this runtime does not implement.
    Unsupported(UnsupportedOperationPath),
}

/// An `operationPath` split into its optional method token and its form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassifiedOperationPath<'a> {
    /// The leading HTTP method token, or `""` when the value carries none.
    pub method: &'a str,
    pub form: OperationPathForm<'a>,
}

impl<'a> ClassifiedOperationPath<'a> {
    /// The Source Description this value routes through, if any.
    pub const fn source_name(&self) -> Option<&'a str> {
        match self.form {
            OperationPathForm::SourceRouted { source_name, .. } => Some(source_name),
            _ => None,
        }
    }

    /// Why this value cannot be resolved, or `None` when it can be.
    pub const fn unsupported(&self) -> Option<UnsupportedOperationPath> {
        match self.form {
            OperationPathForm::Unsupported(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Splits an optional leading `"<METHOD> "` token off an `operationPath`.
///
/// Returns `("", operation_path)` when no known HTTP method prefixes the value,
/// so callers can use the result unconditionally.
pub fn split_operation_method(operation_path: &str) -> (&str, &str) {
    let Some(idx) = operation_path.find(' ') else {
        return ("", operation_path);
    };
    if idx == 0 || idx > 7 {
        return ("", operation_path);
    }
    let candidate = &operation_path[..idx];
    let valid = matches!(
        candidate,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
    );
    if valid {
        return (candidate, &operation_path[idx + 1..]);
    }
    ("", operation_path)
}

/// Classifies an `operationPath` into the method it names and the form it takes.
pub fn classify_operation_path(operation_path: &str) -> ClassifiedOperationPath<'_> {
    let (method, remainder) = split_operation_method(operation_path);
    ClassifiedOperationPath {
        method,
        form: classify_remainder(remainder),
    }
}

fn classify_remainder(value: &str) -> OperationPathForm<'_> {
    // Unsupported is decided first, and by scanning the whole value rather than
    // only its prefix: `{$sourceDescriptions.api.url}#/paths/~1status/get` has
    // no `}.` in it, so every prefix test below falls through to `BasePath` and
    // the expression text gets concatenated onto a base URL as literal text.
    let expression = value.contains("{$");
    let pointer = value.contains("#/");
    match (expression, pointer) {
        (true, true) => {
            return OperationPathForm::Unsupported(
                UnsupportedOperationPath::RuntimeExpressionWithJsonPointer,
            )
        }
        (true, false) => {
            return OperationPathForm::Unsupported(UnsupportedOperationPath::RuntimeExpression)
        }
        (false, true) => {
            return OperationPathForm::Unsupported(UnsupportedOperationPath::JsonPointer)
        }
        (false, false) => {}
    }

    if let Some((source_name, path)) = split_source_prefix(value) {
        return OperationPathForm::SourceRouted { source_name, path };
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return OperationPathForm::AbsoluteUrl(value);
    }
    OperationPathForm::BasePath(value)
}

/// Splits a `{sourceName}.` prefix off a value.
///
/// The dot after `}` is required: without it a leading path-parameter
/// placeholder such as `{petId}/pets` would be read as a source reference.
fn split_source_prefix(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix('{')?;
    let close = rest.find('}')?;
    let source_name = &rest[..close];
    if source_name.is_empty() {
        return None;
    }
    let path = rest[close + 1..].strip_prefix('.')?;
    Some((source_name, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per shape `operationPath` can take. A new shape means a new row
    /// here before it means new behavior anywhere else.
    const TABLE: &[(&str, &str, OperationPathForm<'static>)] = &[
        // Supported: a path resolved against the base URL.
        ("/pets", "", OperationPathForm::BasePath("/pets")),
        ("GET /pets", "GET", OperationPathForm::BasePath("/pets")),
        (
            "DELETE /pets/7",
            "DELETE",
            OperationPathForm::BasePath("/pets/7"),
        ),
        // Supported: a path-parameter placeholder is not a source reference.
        (
            "/pets/{petId}",
            "",
            OperationPathForm::BasePath("/pets/{petId}"),
        ),
        (
            "{petId}/pets",
            "",
            OperationPathForm::BasePath("{petId}/pets"),
        ),
        // Supported: routed through a named Source Description.
        (
            "{petstore}./pets",
            "",
            OperationPathForm::SourceRouted {
                source_name: "petstore",
                path: "/pets",
            },
        ),
        (
            "POST {petstore}./pet",
            "POST",
            OperationPathForm::SourceRouted {
                source_name: "petstore",
                path: "/pet",
            },
        ),
        (
            "GET {petstore}./pet/{petId}",
            "GET",
            OperationPathForm::SourceRouted {
                source_name: "petstore",
                path: "/pet/{petId}",
            },
        ),
        // Supported: an absolute URL.
        (
            "https://api.example.com/pets",
            "",
            OperationPathForm::AbsoluteUrl("https://api.example.com/pets"),
        ),
        (
            "PUT http://api.example.com/pets/7",
            "PUT",
            OperationPathForm::AbsoluteUrl("http://api.example.com/pets/7"),
        ),
        // Unsupported: the specification form, and each half of it alone.
        (
            "{$sourceDescriptions.petstore.url}#/paths/~1pets/get",
            "",
            OperationPathForm::Unsupported(
                UnsupportedOperationPath::RuntimeExpressionWithJsonPointer,
            ),
        ),
        (
            "GET {$sourceDescriptions.petstore.url}#/paths/~1pets/get",
            "GET",
            OperationPathForm::Unsupported(
                UnsupportedOperationPath::RuntimeExpressionWithJsonPointer,
            ),
        ),
        (
            "{$sourceDescriptions.petstore.url}",
            "",
            OperationPathForm::Unsupported(UnsupportedOperationPath::RuntimeExpression),
        ),
        (
            "#/paths/~1pets/get",
            "",
            OperationPathForm::Unsupported(UnsupportedOperationPath::JsonPointer),
        ),
    ];

    #[test]
    fn classification_table() {
        for (input, expected_method, expected_form) in TABLE {
            let classified = classify_operation_path(input);
            assert_eq!(classified.method, *expected_method, "method for {input:?}");
            assert_eq!(classified.form, *expected_form, "form for {input:?}");
        }
    }

    #[test]
    fn source_name_is_reported_only_for_source_routed_values() {
        for (input, _, form) in TABLE {
            let expected = match form {
                OperationPathForm::SourceRouted { source_name, .. } => Some(*source_name),
                _ => None,
            };
            assert_eq!(
                classify_operation_path(input).source_name(),
                expected,
                "source name for {input:?}"
            );
        }
    }

    #[test]
    fn unsupported_is_reported_only_for_unsupported_values() {
        for (input, _, form) in TABLE {
            let expected = match form {
                OperationPathForm::Unsupported(reason) => Some(*reason),
                _ => None,
            };
            assert_eq!(
                classify_operation_path(input).unsupported(),
                expected,
                "unsupported verdict for {input:?}"
            );
        }
    }

    #[test]
    fn method_token_split_rejects_unknown_and_misplaced_verbs() {
        assert_eq!(
            split_operation_method("UNKNOWN /pets"),
            ("", "UNKNOWN /pets")
        );
        assert_eq!(split_operation_method(" /pets"), ("", " /pets"));
        assert_eq!(split_operation_method("/pets"), ("", "/pets"));
        assert_eq!(split_operation_method(""), ("", ""));
        assert_eq!(split_operation_method("get /pets"), ("", "get /pets"));
    }

    #[test]
    fn empty_source_name_is_not_a_source_reference() {
        assert_eq!(
            classify_operation_path("{}./pets").form,
            OperationPathForm::BasePath("{}./pets")
        );
    }

    /// The `}` must be followed by `.` — otherwise `{petId}` at the head of a
    /// path would be swallowed as a source name.
    #[test]
    fn source_prefix_requires_the_dot() {
        assert_eq!(
            classify_operation_path("{petstore}/pets").form,
            OperationPathForm::BasePath("{petstore}/pets")
        );
    }
}
