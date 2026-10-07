//! Typed expression-bearing validation: Selector Objects, expression type
//! declarations (Selector Object `type`, Criterion `type`, replacement
//! `targetSelectorType`), Criterion Objects, payload replacements, value
//! sources, and output values.
//!
//! The crate root keeps the call sites in `collect_diagnostics`,
//! `validate_parameter`, and `validate_actions` and delegates here.

use std::collections::HashSet;

use arazzo_spec::{OutputValue, SelectorObject, SelectorType, SuccessCriterion, ValueSource};

use crate::{check_unknown_fields, xpath_advisory, Diagnostic, Severity, ValidationErrorKind};

pub(crate) fn validate_output_value(
    path: &str,
    output: &OutputValue,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let OutputValue::Selector(selector) = output {
        validate_selector(path, selector, arazzo_version, diagnostics);
    }
}

pub(crate) fn validate_output_step_reference(
    path: &str,
    output: &OutputValue,
    step_ids: &HashSet<&str>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let expression = match output {
        OutputValue::RuntimeExpression(expression) => expression,
        OutputValue::Selector(selector) => &selector.context,
    };
    if let Some(after) = expression.strip_prefix("$steps.") {
        let step_name = after.split('.').next().unwrap_or_default();
        if !step_ids.contains(step_name) {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::InvalidReference,
                path: path.to_string(),
                message: format!("{path} references unknown step '{step_name}'"),
            });
        }
    }
}

pub(crate) fn validate_value_source(
    path: &str,
    value: &ValueSource,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match value {
        ValueSource::Selector(selector) => {
            validate_selector(path, selector, arazzo_version, diagnostics)
        }
        ValueSource::Literal(serde_yaml_ng::Value::Sequence(values)) => {
            for (index, value) in values.iter().enumerate() {
                validate_value_source(
                    &format!("{path}[{index}]"),
                    &value.clone().into(),
                    arazzo_version,
                    diagnostics,
                );
            }
        }
        ValueSource::Literal(serde_yaml_ng::Value::Mapping(values)) => {
            for (key, value) in values {
                let key = key.as_str().unwrap_or("<non-string-key>");
                validate_value_source(
                    &format!("{path}.{key}"),
                    &value.clone().into(),
                    arazzo_version,
                    diagnostics,
                );
            }
        }
        ValueSource::Literal(_) => {}
    }
}

pub(crate) fn validate_selector(
    path: &str,
    selector: &SelectorObject,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_unknown_fields(path, &selector.extensions, diagnostics);
    if selector.context.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.context"),
            message: format!("{path}.context is required"),
        });
    }

    let type_name = selector.type_.resolved_name();
    if selector.selector.trim().is_empty() && type_name != "jsonpointer" {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.selector"),
            message: format!("{path}.selector is required"),
        });
    }

    validate_expression_type(
        &format!("{path}.type"),
        &selector.type_,
        &SELECTOR_TYPE_RULES,
        arazzo_version,
        diagnostics,
    );
}

/// Per-site policy for [`validate_expression_type`]: which type names each
/// declaration form accepts and which diagnostic kind the site reports.
/// The version table itself is shared and never varies by site.
pub(crate) struct ExpressionTypeRules {
    /// Type names accepted in the plain-name form, plus the label used in
    /// the diagnostic message.
    allowed_names: &'static [&'static str],
    names_label: &'static str,
    /// Type names accepted in the Expression Type Object form, plus label.
    allowed_object_types: &'static [&'static str],
    object_types_label: &'static str,
    kind: ValidationErrorKind,
    /// Site identity for the xpath advisory's pre-1.1 wording.
    advisory_site: xpath_advisory::XpathSite,
}

const SELECTOR_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["jsonpath", "xpath", "jsonpointer"],
    names_label: "jsonpath, xpath, or jsonpointer",
    allowed_object_types: &["jsonpath", "xpath", "jsonpointer"],
    object_types_label: "jsonpath, xpath, or jsonpointer",
    kind: ValidationErrorKind::InvalidSelectorType,
    advisory_site: xpath_advisory::XpathSite::Selector,
};

const CRITERION_TYPE_RULES: ExpressionTypeRules = ExpressionTypeRules {
    allowed_names: &["simple", "regex", "jsonpath", "xpath"],
    names_label: "simple, regex, jsonpath, xpath",
    allowed_object_types: &["jsonpath", "xpath"],
    object_types_label: "jsonpath or xpath",
    kind: ValidationErrorKind::InvalidCriterionType,
    advisory_site: xpath_advisory::XpathSite::Criterion,
};

/// Arazzo v1.1.0 §5.8.12.1 version table, one accepted set for every site
/// (Selector Object `type`, Criterion `type`, `targetSelectorType`):
/// `jsonpath` → `rfc9535` | `draft-goessner-dispatch-jsonpath-00`; `xpath` →
/// `xpath-31` | `xpath-30` | `xpath-20` | `xpath-10`; `jsonpointer` →
/// `rfc6901`. The field description under §5.8.12.1 omits `xpath-31`, but the
/// table allows it and makes it the default, and the object's own Effective
/// Boolean Value list names "XPath 3.1 (default)"; the table is implemented.
pub(crate) fn expression_type_version_supported(type_name: &str, version: &str) -> bool {
    match type_name {
        "jsonpath" => matches!(version, "rfc9535" | "draft-goessner-dispatch-jsonpath-00"),
        "xpath" => matches!(version, "xpath-31" | "xpath-30" | "xpath-20" | "xpath-10"),
        "jsonpointer" => version == "rfc6901",
        _ => false,
    }
}

/// Validates a selector/criterion expression type declaration rooted at
/// `base_path` (the path of the `type`/`targetSelectorType` field itself).
/// Name-form errors report at `base_path`, object-form type errors at
/// `{base_path}.type`, and version errors at `{base_path}.version`.
pub(crate) fn validate_expression_type(
    base_path: &str,
    selector_type: &SelectorType,
    rules: &ExpressionTypeRules,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match selector_type {
        SelectorType::Name(name) => {
            let normalized = name.trim().to_lowercase();
            if !rules.allowed_names.contains(&normalized.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: base_path.to_string(),
                    message: format!("{base_path} must be one of {}", rules.names_label),
                });
            } else if normalized == "xpath" {
                diagnostics.extend(xpath_advisory::unexecutable_xpath_version(
                    base_path,
                    None,
                    arazzo_version,
                    rules.advisory_site,
                ));
            }
        }
        SelectorType::ExpressionType(expression_type) => {
            check_unknown_fields(
                &format!("{base_path}.type"),
                &expression_type.extensions,
                diagnostics,
            );
            let normalized = expression_type.type_.trim().to_lowercase();
            if !rules.allowed_object_types.contains(&normalized.as_str()) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: format!("{base_path}.type"),
                    message: format!(
                        "{base_path}.type must be one of {}",
                        rules.object_types_label
                    ),
                });
                return;
            }

            let version = expression_type.version.trim();
            if version.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: ValidationErrorKind::MissingRequiredField,
                    path: format!("{base_path}.version"),
                    message: format!("{base_path}.version is required"),
                });
                return;
            }

            if !expression_type_version_supported(&normalized, version) {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    kind: rules.kind.clone(),
                    path: format!("{base_path}.version"),
                    message: format!(
                        "{base_path}.version {version:?} is not supported for {normalized}"
                    ),
                });
            } else if normalized == "xpath" {
                diagnostics.extend(xpath_advisory::unexecutable_xpath_version(
                    base_path,
                    Some(version),
                    arazzo_version,
                    rules.advisory_site,
                ));
            }
        }
    }
}

pub(crate) fn validate_replacements(
    path_prefix: &str,
    replacements: &[arazzo_spec::Replacement],
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (replacement_idx, replacement) in replacements.iter().enumerate() {
        check_unknown_fields(
            &format!("{path_prefix}[{replacement_idx}]"),
            &replacement.extensions,
            diagnostics,
        );
        if replacement.target.trim().is_empty() {
            let path = format!("{path_prefix}[{replacement_idx}].target");
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                kind: ValidationErrorKind::MissingRequiredField,
                path: path.clone(),
                message: format!("{path} is required"),
            });
        }
        if let Some(selector_type) = &replacement.target_selector_type {
            validate_expression_type(
                &format!("{path_prefix}[{replacement_idx}].targetSelectorType"),
                selector_type,
                &SELECTOR_TYPE_RULES,
                arazzo_version,
                diagnostics,
            );
        }
        validate_value_source(
            &format!("{path_prefix}[{replacement_idx}].value"),
            &replacement.value,
            arazzo_version,
            diagnostics,
        );
    }
}

pub(crate) fn validate_criterion(
    path: &str,
    criterion: &SuccessCriterion,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    check_unknown_fields(path, &criterion.extensions, diagnostics);
    if criterion.condition.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.condition"),
            message: format!("{path}.condition is required"),
        });
    }

    if criterion.has_declared_type() && criterion.context.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            kind: ValidationErrorKind::MissingRequiredField,
            path: format!("{path}.context"),
            message: format!("{path}.context is required when type is specified"),
        });
    }

    let Some(type_) = &criterion.type_ else {
        return;
    };

    validate_expression_type(
        &format!("{path}.type"),
        type_,
        &CRITERION_TYPE_RULES,
        arazzo_version,
        diagnostics,
    );
}
