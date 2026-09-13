//! JSONPath criterion decision over the shared RFC 9535 query owner.
//!
//! The runtime no longer parses or evaluates JSONPath itself: every
//! `type: jsonpath` criterion is admitted, version-checked and parsed by
//! [`JsonPathQuery::parse`] in `arazzo-expr`, and evaluated by
//! [`JsonPathQuery::query`]. This module only turns that query's nodelist into
//! the criterion decision.

use arazzo_expr::{JsonPathError, JsonPathQuery};
use serde_json::Value;

/// Decides a `type: jsonpath` criterion from the cardinality of the nodelist
/// `query` selects in `context`.
///
/// Arazzo v1.1.0 §5.8.11.4.3 is cardinality-only: a non-empty nodelist (one
/// or more nodes) passes and an empty nodelist (zero nodes) fails. No selected
/// value is inspected, so a single node holding `false`, `0`, `""` or `null`
/// passes. This deliberately differs from the XPath arm one section later
/// (§5.8.11.4.4), which carries a type truth table and delegates to Effective
/// Boolean Value; JSONPath has no such step.
///
/// The query arrives already parsed, so this helper does no string handling
/// of its own. An operational failure inside the query — a context nesting
/// limit or a regex-function resource failure, even under negation — is
/// returned as the shared [`JsonPathError`] so the caller fails the criterion
/// with an actionable diagnostic instead of a silent `false`.
pub(super) fn jsonpath_condition_holds(
    query: &JsonPathQuery,
    context: &Value,
) -> Result<bool, JsonPathError> {
    query.query(context).map(|nodes| !nodes.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn holds(condition: &str, context: &Value) -> Result<bool, JsonPathError> {
        let query = match JsonPathQuery::parse(condition, None) {
            Ok(query) => query,
            Err(error) => panic!("{condition} must parse: {error}"),
        };
        jsonpath_condition_holds(&query, context)
    }

    /// §5.8.11.4.3: the four values where nodelist cardinality and JSON
    /// truthiness diverge — each is one node, so each passes despite being
    /// falsy. The absent key is the negative half of the same rule.
    #[test]
    fn nodelist_cardinality_decides_not_truthiness() {
        let context = json!({"data": {
            "active": false,
            "count": 0,
            "name": "",
            "missing": null,
        }});
        for condition in [
            "$.data.active",
            "$.data.count",
            "$.data.name",
            "$.data.missing",
        ] {
            assert_eq!(
                holds(condition, &context),
                Ok(true),
                "{condition}: one node selected must pass"
            );
        }
        assert_eq!(
            holds("$.data.absent", &context),
            Ok(false),
            "an empty nodelist must fail"
        );
    }

    /// A regex-function resource failure invalidates the whole query rather
    /// than contributing a boolean, so negating the function cannot turn the
    /// failure into a pass. The helper returns the error; it never maps it
    /// to `false` itself.
    #[test]
    fn operational_failure_is_returned_even_under_negation() {
        let context = json!([{"value": "a"}]);
        assert_eq!(
            holds("$[?match(@.value, 'a')]", &context),
            Ok(true),
            "positive control: the regex function itself works"
        );
        for condition in [
            "$[?match(@.value, 'a{1000000000}')]",
            "$[?!match(@.value, 'a{1000000000}')]",
        ] {
            match holds(condition, &context) {
                Err(JsonPathError::Evaluation { detail }) => {
                    assert!(detail.contains("limit"), "{condition}: {detail}")
                }
                other => panic!("{condition}: expected an Evaluation error, got {other:?}"),
            }
        }
    }
}
