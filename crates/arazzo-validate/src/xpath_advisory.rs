//! Advisory diagnostics for XPath declarations the runtime will not execute.
//!
//! Since ac-46638 the runtime routes every XPath criterion, selector, and
//! replacement through one version boundary: an explicit `xpath-10` reaches
//! the XPath 1.0 engine, and the omitted form and every other declared
//! version are rejected before evaluation. The declarations themselves stay
//! specification-valid: the Arazzo v1.1.0 §5.8.12.1 version table allows
//! `xpath-31, xpath-30, xpath-20, xpath-10` with `xpath-31` the default, and
//! §5.8.12 states "If this object is not defined, the default version for
//! the selector type MUST be used." So validation reports these declarations
//! at warning severity — the document is conformant, the executor is not —
//! and `--strict` promotes them to errors like every other warning.
//!
//! Wording follows the document's declared `arazzo` version. Arazzo 1.0.x
//! has no `xpath-31` token — §4.6.12.1 allows only "`xpath-30`, `xpath-20`,
//! or `xpath-10`" — while its omitted-version default is still XPath 3.1:
//! §4.6.11.3 says "If `xpath` the expression MUST conform to XML Path
//! Language 3.1" and §4.6.12 says "If this object is not defined, then the
//! following defaults apply: … XPath as described by XML Path Language 3.1".
//! A pre-1.1 document therefore gets an advisory naming "XML Path Language
//! 3.1" without the 1.1-only `xpath-31` token, and naming its "Criterion
//! Expression Type Object" rather than the 1.1 "Expression Type Object".

use crate::{declares_pre_1_1, Diagnostic, ValidationErrorKind};

/// One advisory for a schema-valid xpath-typed declaration site, or `None`
/// for the single form the runtime executes (an explicit `xpath-10`).
///
/// `declared_version` is `None` for the plain-name form (`type: xpath`),
/// which omits the version, and `Some(token)` for a schema-valid Expression
/// Type Object version. Name-form findings report at `base_path` (the
/// `type`/`targetSelectorType` field itself) and object-form findings at
/// `{base_path}.version`, matching the sibling error diagnostics from
/// `validate_expression_type`.
pub(crate) fn unexecutable_xpath_version(
    base_path: &str,
    declared_version: Option<&str>,
    arazzo_version: &str,
) -> Option<Diagnostic> {
    let (path, message) = match declared_version {
        Some("xpath-10") => return None,
        Some(declared) => (
            format!("{base_path}.version"),
            format!(
                "{base_path}.version {declared:?} is valid Arazzo metadata that this \
                 runtime does not implement, so execution rejects it before evaluation; \
                 declare version \"xpath-10\" to evaluate with the XPath 1.0 engine"
            ),
        ),
        None if declares_pre_1_1(arazzo_version) => (
            base_path.to_string(),
            format!(
                "{base_path} declares type \"xpath\" without a version; arazzo \
                 {arazzo_version} defaults it to XML Path Language 3.1, which this \
                 runtime does not implement, so execution rejects it before evaluation; \
                 declare a Criterion Expression Type Object with version \"xpath-10\" \
                 to evaluate with the XPath 1.0 engine"
            ),
        ),
        None => (
            base_path.to_string(),
            format!(
                "{base_path} declares type \"xpath\" without a version, which defaults \
                 to \"xpath-31\" (XML Path Language 3.1); this runtime implements only \
                 XPath 1.0 and rejects the omitted form before evaluation; declare an \
                 Expression Type Object with version \"xpath-10\" to evaluate with the \
                 XPath 1.0 engine"
            ),
        ),
    };
    Some(Diagnostic::warning(
        ValidationErrorKind::UnsupportedXpathVersion,
        path,
        message,
    ))
}
