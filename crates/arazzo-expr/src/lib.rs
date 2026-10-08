#![forbid(unsafe_code)]

//! Expression parser and evaluator for Arazzo runtime expressions.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

mod body_pointer_hint;
mod expression_string;
mod interpolation;
mod resolution;
mod runtime_expression;
mod simple_condition;

pub use expression_string::{
    classify_value_string, EmbeddedExpression, ExpressionStringError, ExpressionStringErrorKind,
    ValueStringSyntax,
};

pub use body_pointer_hint::body_pointer_migration_hint;

pub use simple_condition::{
    parse_simple_condition, ConditionError, ConditionErrorKind, ConditionEvaluation,
    ParsedSimpleCondition,
};

pub use runtime_expression::{
    parse_runtime_expression, ComponentReference, ComponentReferenceSection,
    ParsedRuntimeExpression, RuntimeExpressionError, RuntimeExpressionErrorKind,
    RuntimeExpressionNamespace, RuntimeExpressionPointer, SourceDescriptionReference,
    StepOutputReference,
};

pub mod jsonpath;

pub use jsonpath::{
    resolve_json_path_pointers, select_json_path, JsonPathError, JsonPathMatch, JsonPathQuery,
    JsonPathSelection,
};

/// Error produced when evaluating an Arazzo dot-notation path against a JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The path string could not be tokenized (e.g. unclosed bracket, empty filter).
    InvalidSyntax { path: String, detail: String },
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSyntax { path, detail } => {
                write!(f, "invalid path syntax \"{path}\": {detail}")
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Warning produced when an expression resolves to `Null` due to a missing key,
/// unknown step, or unrecognised namespace. Collected by
/// [`ExpressionEvaluator::evaluate_with_diagnostics`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionWarning {
    pub expression: String,
    pub message: String,
}

impl std::fmt::Display for ExpressionWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.expression, self.message)
    }
}

static INTERPOLATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{(\$[^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)")
        .unwrap_or_else(|err| panic!("failed to compile interpolate regex: {err}"))
});

/// State snapshot for a completed workflow, used by `$workflows.<id>.*` expressions.
#[derive(Debug, Clone, Default)]
pub struct WorkflowEvalState {
    pub inputs: BTreeMap<String, Value>,
    pub outputs: BTreeMap<String, Value>,
}

/// Runtime-visible fields from a Source Description Object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceDescriptionContext {
    pub url: String,
    pub type_: String,
}

/// Evaluation context for expression resolution.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub inputs: BTreeMap<String, Value>,
    /// Step outputs, wrapped in `Arc` for cheap cloning during repeated evaluation.
    pub steps: Arc<BTreeMap<String, BTreeMap<String, Value>>>,
    pub outputs: BTreeMap<String, Value>,
    pub workflows: BTreeMap<String, WorkflowEvalState>,
    pub status_code: Option<i64>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub request_headers: BTreeMap<String, String>,
    pub request_query: BTreeMap<String, String>,
    pub request_path: BTreeMap<String, String>,
    pub request_body: Option<Value>,
    /// Headers from an asynchronous message when message execution is available.
    pub message_headers: BTreeMap<String, String>,
    /// Payload from an asynchronous message when message execution is available.
    pub message_payload: Option<Value>,
    pub self_uri: Option<String>,
    pub source_descriptions: BTreeMap<String, SourceDescriptionContext>,
    pub response_headers: BTreeMap<String, String>,
    pub response_body: Option<Value>,
}

/// Evaluates expressions and conditions using an [`EvalContext`].
#[derive(Debug, Clone, Default)]
pub struct ExpressionEvaluator {
    ctx: EvalContext,
}

impl ExpressionEvaluator {
    pub fn new(ctx: EvalContext) -> Self {
        Self { ctx }
    }

    pub fn context(&self) -> &EvalContext {
        &self.ctx
    }

    pub fn context_mut(&mut self) -> &mut EvalContext {
        &mut self.ctx
    }

    /// Resolve a complete expression with its JSON type, an embedded-expression
    /// template as a string, or literal text unchanged.
    pub fn resolve_value(&self, value: &str) -> Value {
        self.resolve_value_with_diagnostics(value).0
    }

    /// Resolve a value string while retaining ordered expression diagnostics.
    /// Malformed templates return their original text and one syntax warning.
    pub fn resolve_value_with_diagnostics(&self, value: &str) -> (Value, Vec<ExpressionWarning>) {
        interpolation::resolve_value(value, self.context())
    }

    /// Evaluate an expression and return a dynamic JSON value.
    ///
    /// Missing keys and unknown namespaces silently return `Value::Null`.
    /// Use [`evaluate_with_diagnostics`](Self::evaluate_with_diagnostics) to
    /// collect warnings about unresolved expressions.
    pub fn evaluate(&self, expr: &str) -> Value {
        self.evaluate_with_diagnostics(expr).0
    }

    /// Evaluate an expression, returning both the value and any diagnostic
    /// warnings produced when resolution falls back to `Null`.
    pub fn evaluate_with_diagnostics(&self, expr: &str) -> (Value, Vec<ExpressionWarning>) {
        match parse_runtime_expression(expr) {
            Ok(parsed) => resolution::resolve_parsed(&parsed, self.context()).into_public(),
            Err(error) => {
                let mut message = error.to_string();
                if let Some(hint) = body_pointer_migration_hint(expr, &error) {
                    message.push_str("; ");
                    message.push_str(&hint);
                }
                (
                    Value::Null,
                    vec![ExpressionWarning {
                        expression: expr.to_owned(),
                        message,
                    }],
                )
            }
        }
    }

    /// Evaluate an expression and convert to string with Go-compatible coercions.
    pub fn evaluate_string(&self, expr: &str) -> String {
        to_string_value(&self.evaluate(expr)).into_owned()
    }

    /// Interpolate `{$expr}` and `$inputs.foo` style segments in a string.
    pub fn interpolate_string(&self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut cursor = 0usize;

        for captures in INTERPOLATE_RE.captures_iter(input) {
            let Some(full) = captures.get(0) else {
                continue;
            };

            out.push_str(&input[cursor..full.start()]);

            let expr = if let Some(inner) = captures.get(1) {
                inner.as_str()
            } else {
                full.as_str()
            };
            out.push_str(&self.evaluate_string(expr));
            cursor = full.end();
        }

        out.push_str(&input[cursor..]);
        out
    }
}

fn get_header_case_insensitive<'a>(
    headers: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    if let Some(value) = headers.get(name) {
        return Some(value);
    }
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

pub(crate) fn to_string_value(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(v) => Cow::Borrowed(v.as_str()),
        Value::Number(n) => Cow::Owned(n.to_string()),
        Value::Bool(v) => Cow::Borrowed(if *v { "true" } else { "false" }),
        _ => Cow::Borrowed(""),
    }
}

pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{EvalContext, ExpressionEvaluator, SourceDescriptionContext};
    use proptest::prelude::*;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn selected(root: &Value, path: &str) -> super::JsonPathSelection {
        match super::select_json_path(root, path) {
            Ok(selection) => selection,
            Err(error) => panic!("selecting {path:?}: {error}"),
        }
    }

    #[test]
    fn evaluate_literal_and_unknown_expression() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("hello"), Value::Null);
        assert_eq!(eval.evaluate("$unknown.thing"), Value::Null);
    }

    #[test]
    fn resolve_value_dispatches_correctly() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        ctx.inputs.insert("token".to_string(), json!("xyz"));
        let eval = ExpressionEvaluator::new(ctx);

        // Full expression → evaluate
        assert_eq!(eval.resolve_value("$inputs.name"), json!("Alice"));

        // Interpolated → interpolate_string
        assert_eq!(
            eval.resolve_value("Bearer {$inputs.token}"),
            json!("Bearer xyz")
        );

        // Literal → as-is string
        assert_eq!(eval.resolve_value("literal"), json!("literal"));
    }

    #[test]
    fn evaluate_inputs_and_step_outputs() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("token".to_string(), json!("abc"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$inputs.name"), json!("Alice"));
        assert_eq!(eval.evaluate("$inputs.missing"), Value::Null);
        assert_eq!(eval.evaluate("$steps.s1.outputs.token"), json!("abc"));
        assert_eq!(eval.evaluate("$steps.nope.outputs.token"), Value::Null);
        assert_eq!(eval.evaluate("$steps.s1.token"), Value::Null);
    }

    #[test]
    fn evaluate_response_fields() {
        let mut ctx = EvalContext {
            status_code: Some(404),
            response_body: Some(json!({
                "user": {"name": "Bob"},
                "arr": [{"id": 7}],
                "users": [
                    {"id": 1, "name": "Alice", "group": "a"},
                    {"id": 2, "name": "Bob", "group": "b"},
                    {"id": 3, "name": "Cara", "group": "a"}
                ]
            })),
            ..EvalContext::default()
        };
        ctx.response_headers
            .insert("X-Request-Id".to_string(), "req-1".to_string());
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$statusCode"), json!(404));
        assert_eq!(
            eval.evaluate("$response.header.X-Request-Id"),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.header.x-request-id"),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.body"),
            json!({
                "user": {"name": "Bob"},
                "arr": [{"id": 7}],
                "users": [
                    {"id": 1, "name": "Alice", "group": "a"},
                    {"id": 2, "name": "Bob", "group": "b"},
                    {"id": 3, "name": "Cara", "group": "a"}
                ]
            })
        );
        assert_eq!(eval.evaluate("$response.body#/user/name"), json!("Bob"));
        assert_eq!(eval.evaluate("$response.body#/arr/0/id"), json!(7));
        for invalid in [
            "$response.body.user.name",
            "$response.body.arr[0].id",
            "$response.body.arr.#",
            "$response.body.users[*].id",
            "$response.body.users.#(id==2).name",
            "$response.body.users[?(@.group=='a')].id",
        ] {
            let (value, warnings) = eval.evaluate_with_diagnostics(invalid);
            assert_eq!(value, Value::Null);
            assert_eq!(warnings.len(), 1);
        }
    }

    #[test]
    fn evaluate_response_fields_without_response() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$statusCode"), Value::Null);
        assert_eq!(eval.evaluate("$response.header.X-Foo"), Value::Null);
        assert_eq!(eval.evaluate("$response.body.user.name"), Value::Null);
    }

    #[test]
    fn evaluate_env_namespace_rejected() {
        // $env is not part of the Arazzo 1.1.0 expression surface. Whether the
        // variable exists in the process environment or not, the evaluator must
        // return the unknown-namespace result and never leak the value.
        const SENTINEL_NAME: &str = "ARAZZO_EXPR_TEST_ENV_SENTINEL";
        const SENTINEL_VALUE: &str = "sentinel-secret-ac9c811";
        let expr = format!("$env.{SENTINEL_NAME}");

        let mut ctx = EvalContext::default();
        ctx.inputs
            .insert("secret".to_string(), json!("from-inputs"));
        let eval = ExpressionEvaluator::new(ctx);

        std::env::set_var(SENTINEL_NAME, SENTINEL_VALUE);
        let present = eval.evaluate_with_diagnostics(&expr);
        let present_interpolated = eval.interpolate_string(&format!("token={{{expr}}}"));
        std::env::remove_var(SENTINEL_NAME);
        let absent = eval.evaluate_with_diagnostics(&expr);

        for (value, warnings) in [&present, &absent] {
            assert_eq!(*value, Value::Null);
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].message.contains("unknown namespace"));
            assert!(!warnings[0].message.contains(SENTINEL_VALUE));
            assert!(!warnings[0].expression.contains(SENTINEL_VALUE));
        }
        assert_eq!(present_interpolated, "token=");

        // Controls: explicit inputs remain the supported route, and an unknown
        // non-env namespace takes the same rejection path.
        assert_eq!(eval.evaluate("$inputs.secret"), json!("from-inputs"));
        let (value, warnings) = eval.evaluate_with_diagnostics("$notaspace.secret");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown namespace"));
    }

    #[test]
    fn evaluate_string_coercions() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("s".to_string(), json!("hello"));
        ctx.inputs.insert("f".to_string(), json!(2.5));
        ctx.inputs.insert("i".to_string(), json!(42));
        ctx.inputs.insert("t".to_string(), json!(true));
        ctx.inputs.insert("f2".to_string(), json!(false));
        ctx.inputs.insert("arr".to_string(), json!([1, 2]));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate_string("$inputs.missing"), "");
        assert_eq!(eval.evaluate_string("$inputs.s"), "hello");
        assert_eq!(eval.evaluate_string("$inputs.f"), "2.5");
        assert_eq!(eval.evaluate_string("$inputs.i"), "42");
        assert_eq!(eval.evaluate_string("$inputs.t"), "true");
        assert_eq!(eval.evaluate_string("$inputs.f2"), "false");
        assert_eq!(eval.evaluate_string("$inputs.arr"), "");
    }

    #[test]
    fn method_expression() {
        let ctx = EvalContext {
            method: Some("GET".to_string()),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$method"), json!("GET"));

        let ctx_no_method = EvalContext::default();
        let eval_no_method = ExpressionEvaluator::new(ctx_no_method);
        assert_eq!(eval_no_method.evaluate("$method"), Value::Null);
    }

    #[test]
    fn interpolate_string_modes() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        ctx.inputs.insert("age".to_string(), json!(30));
        ctx.inputs.insert("a".to_string(), json!("X"));
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("b".to_string(), json!("Y"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.interpolate_string("Hello {$inputs.name}!"),
            "Hello Alice!"
        );
        assert_eq!(eval.interpolate_string("Age: $inputs.age"), "Age: 30");
        assert_eq!(
            eval.interpolate_string("{$inputs.a}-$steps.s1.outputs.b"),
            "X-Y"
        );
        assert_eq!(eval.interpolate_string("plain text"), "plain text");
        assert_eq!(
            eval.interpolate_string("Bearer {$inputs.name}"),
            "Bearer Alice"
        );
    }

    proptest! {
        #[test]
        fn interpolate_string_preserves_prefix_and_suffix(
            prefix in "[^$]{0,24}",
            value in "[a-zA-Z0-9 _\\-]{0,24}",
            suffix in "[^$]{0,24}",
        ) {
            let mut ctx = EvalContext::default();
            ctx.inputs.insert("token".to_string(), json!(value.clone()));
            let eval = ExpressionEvaluator::new(ctx);

            let expr = format!("{prefix}{{$inputs.token}}{suffix}");
            let rendered = eval.interpolate_string(&expr);
            prop_assert_eq!(rendered, format!("{prefix}{value}{suffix}"));
        }

        #[test]
        fn response_array_len_and_index_extraction_are_consistent(
            values in proptest::collection::vec(any::<i64>(), 0..20),
            idx in 0usize..25usize,
        ) {
            let eval = ExpressionEvaluator::new(EvalContext {
                response_body: Some(json!({"arr": values.clone()})),
                ..EvalContext::default()
            });

            let at_value = eval.evaluate(&format!("$response.body#/arr/{idx}"));
            if idx < values.len() {
                prop_assert_eq!(at_value, json!(values[idx]));
            } else {
                prop_assert_eq!(at_value, Value::Null);
            }
        }

    }

    /// Public RFC JSONPath retains its independent case-sensitive comparison rules.
    #[test]
    pub(super) fn json_path_filters_remain_case_sensitive_for_equality_and_ordering() {
        let root = json!({
            "items": [
                {"name": "Alpha"},
                {"name": "alpha"}
            ]
        });

        let equality = selected(&root, "$.items[?(@.name == 'alpha')].name");
        assert_eq!(equality.value, json!("alpha"));
        assert_eq!(equality.match_count, 1);

        let ordering = selected(&root, "$.items[?(@.name < 'alpha')].name");
        assert_eq!(ordering.value, json!("Alpha"));
        assert_eq!(ordering.match_count, 1);
    }

    #[test]
    fn evaluate_outputs_expression() {
        let mut ctx = EvalContext::default();
        ctx.outputs.insert("total".to_string(), json!(42));
        ctx.outputs
            .insert("nested".to_string(), json!({"a": {"b": "deep"}}));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$outputs.total"), json!(42));
        assert_eq!(eval.evaluate("$outputs.missing"), Value::Null);
        assert_eq!(
            eval.evaluate("$outputs.nested"),
            json!({"a": {"b": "deep"}})
        );
    }

    #[test]
    fn evaluate_outputs_json_pointer() {
        let mut ctx = EvalContext::default();
        ctx.outputs
            .insert("data".to_string(), json!({"items": [{"id": 1}, {"id": 2}]}));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$outputs.data#/items/0/id"), json!(1));
        assert_eq!(eval.evaluate("$outputs.data#/items/1/id"), json!(2));
        assert_eq!(eval.evaluate("$outputs.data#/missing"), Value::Null);
    }

    #[test]
    fn evaluate_named_value_json_pointers() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert(
            "user".to_string(),
            json!({"profile": {"email": "alice@example.com"}}),
        );
        Arc::make_mut(&mut ctx.steps).insert(
            "lookup".to_string(),
            BTreeMap::from([("payload".to_string(), json!({"items": [{"id": "item-1"}]}))]),
        );
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::from([("config".to_string(), json!({"env": "production"}))]),
                outputs: BTreeMap::from([(
                    "tokenPayload".to_string(),
                    json!({"token": "abc-123"}),
                )]),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$inputs.user#/profile/email"),
            json!("alice@example.com")
        );
        assert_eq!(
            eval.evaluate("$steps.lookup.outputs.payload#/items/0/id"),
            json!("item-1")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.tokenPayload#/token"),
            json!("abc-123")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.config#/env"),
            json!("production")
        );
    }

    #[test]
    fn evaluate_message_header_and_payload() {
        let ctx = EvalContext {
            message_headers: BTreeMap::from([(
                "x-request-id".to_string(),
                "request-123".to_string(),
            )]),
            message_payload: Some(json!({"order": {"id": "order-42"}})),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$message.header.X-Request-Id"),
            json!("request-123")
        );
        assert_eq!(
            eval.evaluate("$message.payload"),
            json!({"order": {"id": "order-42"}})
        );
        assert_eq!(
            eval.evaluate("$message.payload#/order/id"),
            json!("order-42")
        );
    }

    #[test]
    fn missing_json_pointer_returns_null_with_diagnostic() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("user".to_string(), json!({"profile": {}}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        let (value, warnings) = eval.evaluate_with_diagnostics("$inputs.user#/profile/email");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("JSON Pointer"));
        assert!(warnings[0].message.contains("/profile/email"));
    }

    #[test]
    fn missing_message_context_returns_null_with_diagnostics() {
        let eval = ExpressionEvaluator::new(EvalContext::default());

        let (header, header_warnings) =
            eval.evaluate_with_diagnostics("$message.header.X-Request-Id");
        assert_eq!(header, Value::Null);
        assert_eq!(header_warnings.len(), 1);
        assert!(header_warnings[0].message.contains("message header"));

        let (payload, payload_warnings) =
            eval.evaluate_with_diagnostics("$message.payload#/order/id");
        assert_eq!(payload, Value::Null);
        assert_eq!(payload_warnings.len(), 1);
        assert!(payload_warnings[0].message.contains("message payload"));
    }

    #[test]
    fn evaluate_response_body_json_pointer() {
        let ctx = EvalContext {
            response_body: Some(json!({
                "data": [{"name": "Alice"}, {"name": "Bob"}],
                "meta": {"total": 2}
            })),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$response.body#/data/0/name"), json!("Alice"));
        assert_eq!(eval.evaluate("$response.body#/data/1/name"), json!("Bob"));
        assert_eq!(eval.evaluate("$response.body#/meta/total"), json!(2));
        assert_eq!(eval.evaluate("$response.body#/nonexistent"), Value::Null);
    }

    #[test]
    fn evaluate_response_body_json_pointer_without_body() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$response.body#/data/0"), Value::Null);
    }

    #[test]
    fn diagnostics_missing_step_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$steps.missing.outputs.x");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("step \"missing\" not found"));
    }

    #[test]
    fn diagnostics_missing_output_key_warns() {
        let mut ctx = EvalContext::default();
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("a".to_string(), json!(1))]),
        );
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$steps.s1.outputs.nope");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("output \"nope\" not found"));
    }

    #[test]
    fn diagnostics_valid_expression_no_warnings() {
        let mut ctx = EvalContext::default();
        ctx.inputs.insert("name".to_string(), json!("Alice"));
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$inputs.name");
        assert_eq!(value, json!("Alice"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn diagnostics_unknown_namespace_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$foo.bar");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown namespace"));
    }

    #[test]
    fn diagnostics_missing_source_description_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$sourceDescriptions.missing.url");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .message
            .contains("source description \"missing\""));
    }

    #[test]
    fn self_expression_resolves_configured_uri() {
        let eval = ExpressionEvaluator::new(EvalContext {
            self_uri: Some("workflows/purchase.arazzo.yaml".to_string()),
            ..EvalContext::default()
        });
        let (value, warnings) = eval.evaluate_with_diagnostics("$self");
        assert_eq!(value, json!("workflows/purchase.arazzo.yaml"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn self_expression_without_uri_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$self");
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("self URI not found"));
    }

    #[test]
    fn source_description_url_and_type_resolve() {
        let mut ctx = EvalContext::default();
        ctx.source_descriptions.insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.url"),
            json!("https://api.example.com/openapi.yaml")
        );
        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.type"),
            json!("openapi")
        );
    }

    #[test]
    fn unsupported_source_description_reference_warns() {
        let mut ctx = EvalContext::default();
        ctx.source_descriptions.insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) =
            eval.evaluate_with_diagnostics("$sourceDescriptions.petstore.getPetById");

        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("getPetById"));
        assert!(warnings[0]
            .message
            .contains("without loaded source document metadata"));
    }

    #[test]
    fn evaluate_backward_compat_still_returns_null() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$steps.missing.outputs.x"), Value::Null);
        assert_eq!(eval.evaluate("$foo.bar"), Value::Null);
    }

    // ── Bug #14: $inputs nested traversal ─────────────────────────

    #[test]
    fn inputs_dotted_names_are_exact_keys() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": {"baz": 42}}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$inputs.foo.bar.baz"), Value::Null);
        assert_eq!(eval.evaluate("$inputs.foo#/bar/baz"), json!(42));
        // Top-level (flat key) still works
        assert_eq!(eval.evaluate("$inputs.foo"), json!({"bar": {"baz": 42}}));
    }

    #[test]
    fn inputs_flat_key_still_works() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("simple".to_string(), json!("hello"))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.simple"), json!("hello"));
    }

    #[test]
    fn inputs_nested_missing_sub_path_returns_null() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": 1}))]),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.foo.missing"), Value::Null);
    }

    // ── Phase 3: $workflows expression root ─────────────────────────

    #[test]
    fn workflows_expression_inputs() {
        let mut ctx = EvalContext::default();
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::from([("env".to_string(), json!("production"))]),
                outputs: BTreeMap::new(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.env"),
            json!("production")
        );
    }

    #[test]
    fn workflows_expression_outputs() {
        let mut ctx = EvalContext::default();
        ctx.workflows.insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::new(),
                outputs: BTreeMap::from([("token".to_string(), json!("abc-123"))]),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.token"),
            json!("abc-123")
        );
    }

    #[test]
    fn workflows_unknown_id_is_null() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.unknown.outputs.x");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("\"unknown\" not found"));
    }

    #[test]
    fn workflows_missing_sub_path_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.auth");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn workflows_invalid_sub_path_warns() {
        let mut ctx = EvalContext::default();
        ctx.workflows
            .insert("auth".to_string(), super::WorkflowEvalState::default());
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.auth.something.else");
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("invalid"));
    }
}
