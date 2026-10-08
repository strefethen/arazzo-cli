//! Diagnostic guidance for rejected standalone body and payload member paths.

use crate::{parse_runtime_expression, RuntimeExpressionError, RuntimeExpressionErrorKind};

/// Suggest explicit JSON Pointer syntax after a rejected body or payload path.
///
/// Pass the original input and its error from [`parse_runtime_expression`].
/// This helper only supplies diagnostic text: it does not accept, rewrite, or
/// evaluate the input, classify literal values, or modify the canonical error.
/// Ambiguous paths receive guidance without an inferred pointer.
pub fn body_pointer_migration_hint(input: &str, error: &RuntimeExpressionError) -> Option<String> {
    if error.kind != RuntimeExpressionErrorKind::TrailingInput
        || error.byte_range.end != input.len()
    {
        return None;
    }
    let prefix = input.get(..error.byte_range.start)?;
    let suffix = input.get(error.byte_range.clone())?;
    if !["$request.body", "$response.body", "$message.payload"]
        .iter()
        .any(|candidate| prefix.eq_ignore_ascii_case(candidate))
        || parse_runtime_expression(prefix).is_err()
        || !suffix.starts_with(['.', '['])
    {
        return None;
    }

    if let Some(members) = suffix.strip_prefix('.') {
        let ordinary = members.split('.').all(|member| {
            !member.is_empty()
                && member
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
        if ordinary {
            let nested = members.replace('.', "/");
            return Some(if members.contains('.') {
                format!(
                    "for nested member access, use '{prefix}#/{nested}'; for a literal property named '{members}', use '{prefix}#/{members}'"
                )
            } else {
                format!("use '{prefix}#/{members}'")
            });
        }
    }

    Some("use an explicit JSON Pointer after '#'; choose the property tokens explicitly and escape '~' as '~0' and '/' as '~1'; this path cannot be converted automatically".to_owned())
}
