//! RFC 9535 JSONPath query owner.
//!
//! One synchronous, library-backed boundary validates a query before any data
//! is available and evaluates it against a context, returning the selected
//! nodes in query order, each paired with the RFC 6901 pointer that addresses
//! it. The upstream parser, query and node types stay private; consumers see
//! only [`JsonPathQuery`], [`JsonPathMatch`] and [`JsonPathError`].
//!
//! The admission limits below are explicit, conservative resource budgets
//! applied before the upstream parser or evaluator runs. They are not a
//! lexer, not a grammar check, and not a CPU, heap or result-size quota.

mod iregexp_adapter;

use std::fmt;

use serde_json::Value;
use serde_json_path::JsonPath;

/// The only explicit JSONPath version this owner evaluates.
const SUPPORTED_VERSION: &str = "rfc9535";

/// Maximum UTF-8 length of a query expression, in bytes.
const QUERY_BYTE_LIMIT: usize = 16_384;

/// Maximum combined count of the raw bytes `.` `[` `(` `!` `&` `|` in a query.
///
/// The count is deliberately conservative: bytes inside quoted or escaped
/// literals count too, so nesting cannot be smuggled past the budget inside
/// a string.
const QUERY_STRUCTURAL_LIMIT: usize = 128;

/// Maximum number of nested containers in a query context. The root container
/// counts as one; siblings are counted independently and add no depth.
const CONTEXT_DEPTH_LIMIT: usize = 128;

const RESOURCE_QUERY_BYTES: &str = "query bytes";
const RESOURCE_QUERY_STRUCTURAL: &str = "query structural characters";
const RESOURCE_CONTEXT_DEPTH: &str = "context nesting depth";

/// Failure produced by [`JsonPathQuery::parse`] or [`JsonPathQuery::query`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathError {
    /// The expression is not a valid RFC 9535 query.
    InvalidSyntax {
        /// Upstream parser diagnostic.
        detail: String,
    },
    /// An explicit JSONPath version other than `rfc9535` was requested.
    UnsupportedVersion {
        /// The version token that was requested.
        version: String,
    },
    /// An admission limit was exceeded before parsing or evaluation began.
    ResourceLimit {
        /// The budgeted resource.
        resource: &'static str,
        /// The budget that was exceeded.
        limit: usize,
    },
    /// Evaluation hit an operational failure, so the whole query is invalid.
    Evaluation {
        /// The first operational failure observed.
        detail: String,
    },
}

impl fmt::Display for JsonPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSyntax { detail } => write!(f, "invalid JSONPath syntax: {detail}"),
            Self::UnsupportedVersion { version } => write!(
                f,
                "unsupported JSONPath version \"{version}\": only {SUPPORTED_VERSION} is supported"
            ),
            Self::ResourceLimit { resource, limit } => {
                write!(f, "JSONPath {resource} limit of {limit} exceeded")
            }
            Self::Evaluation { detail } => write!(f, "JSONPath evaluation failed: {detail}"),
        }
    }
}

impl std::error::Error for JsonPathError {}

/// One selected node paired with the RFC 6901 pointer that addresses it.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPathMatch<'a> {
    /// The selected node, borrowed from the queried context.
    pub value: &'a Value,
    /// RFC 6901 pointer to `value` within the context; empty for the root.
    pub pointer: String,
}

/// A validated RFC 9535 query that owns its parsed representation.
#[derive(Debug, Clone)]
pub struct JsonPathQuery {
    compiled: JsonPath,
}

impl JsonPathQuery {
    /// Validate `expression` under `version` before any data is available.
    ///
    /// `version` admits only `None` or `Some("rfc9535")`; every other explicit
    /// version returns [`JsonPathError::UnsupportedVersion`] before the
    /// expression is examined. The byte and structural admission limits are
    /// checked next, and only an admitted expression reaches the upstream
    /// parser, which validates it completely.
    pub fn parse(expression: &str, version: Option<&str>) -> Result<Self, JsonPathError> {
        match version {
            None | Some(SUPPORTED_VERSION) => {}
            Some(other) => {
                return Err(JsonPathError::UnsupportedVersion {
                    version: other.to_string(),
                })
            }
        }
        admit_expression(expression)?;
        let compiled =
            JsonPath::parse(expression).map_err(|error| JsonPathError::InvalidSyntax {
                detail: error.to_string(),
            })?;
        Ok(Self { compiled })
    }

    /// Evaluate the query against `context`.
    ///
    /// Returns every selected node in query order, repeated occurrences
    /// included, paired with its pointer; the values borrow `context`. The
    /// context nesting limit is checked before evaluation. One located
    /// upstream query supplies both halves of each pair, and an operational
    /// failure inside a regex function invalidates the whole query, even when
    /// a predicate negates that function's result.
    pub fn query<'a>(&self, context: &'a Value) -> Result<Vec<JsonPathMatch<'a>>, JsonPathError> {
        admit_context(context)?;
        let located = iregexp_adapter::with_frame(|| self.compiled.query_located(context))
            .map_err(|detail| JsonPathError::Evaluation { detail })?;
        Ok(located
            .iter()
            .map(|node| JsonPathMatch {
                value: node.node(),
                pointer: node.location().to_json_pointer(),
            })
            .collect())
    }
}

/// Apply the byte and structural budgets to a raw expression.
fn admit_expression(expression: &str) -> Result<(), JsonPathError> {
    if expression.len() > QUERY_BYTE_LIMIT {
        return Err(JsonPathError::ResourceLimit {
            resource: RESOURCE_QUERY_BYTES,
            limit: QUERY_BYTE_LIMIT,
        });
    }
    let structural = expression
        .bytes()
        .filter(|byte| matches!(byte, b'.' | b'[' | b'(' | b'!' | b'&' | b'|'))
        .count();
    if structural > QUERY_STRUCTURAL_LIMIT {
        return Err(JsonPathError::ResourceLimit {
            resource: RESOURCE_QUERY_STRUCTURAL,
            limit: QUERY_STRUCTURAL_LIMIT,
        });
    }
    Ok(())
}

/// Reject a context nested deeper than the container budget.
///
/// The walk is iterative on purpose: the budget exists to keep recursive
/// evaluation off unbounded input, so the check itself must not recurse.
fn admit_context(context: &Value) -> Result<(), JsonPathError> {
    let mut pending: Vec<(&Value, usize)> = Vec::new();
    if is_container(context) {
        pending.push((context, 1));
    }
    while let Some((container, depth)) = pending.pop() {
        if depth > CONTEXT_DEPTH_LIMIT {
            return Err(JsonPathError::ResourceLimit {
                resource: RESOURCE_CONTEXT_DEPTH,
                limit: CONTEXT_DEPTH_LIMIT,
            });
        }
        match container {
            Value::Array(items) => pending.extend(
                items
                    .iter()
                    .filter(|item| is_container(item))
                    .map(|item| (item, depth + 1)),
            ),
            Value::Object(members) => pending.extend(
                members
                    .values()
                    .filter(|member| is_container(member))
                    .map(|member| (member, depth + 1)),
            ),
            _ => {}
        }
    }
    Ok(())
}

fn is_container(value: &Value) -> bool {
    matches!(value, Value::Array(_) | Value::Object(_))
}
