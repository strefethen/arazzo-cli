#![forbid(unsafe_code)]

//! Expression parser and evaluator for Arazzo runtime expressions.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
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
mod step_output_reference;

pub use resolution::{
    BlobSlot, ExpressionResolution, StoredValueReference, ValueResolution, ValueUnavailable,
};

pub use step_output_reference::{
    local_step_output_references, LocalStepOutputReference, StepReferenceError, StepReferenceSyntax,
};

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

/// What a `$request.body` or `$response.body` reference finds. The unavailable
/// mark is the slot's own value, so every copy of a context carries it.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum BodySlot {
    /// No body exists; a reference resolves missing.
    #[default]
    Absent,
    Present(Arc<Value>),
    /// A body exists but cannot be read as a JSON value; every reference to it,
    /// whole or through a pointer, is a [`ValueUnavailable`] error.
    Unavailable {
        reason: String,
    },
}

impl BodySlot {
    pub fn present(value: Value) -> Self {
        Self::Present(Arc::new(value))
    }
}

impl From<Value> for BodySlot {
    fn from(value: Value) -> Self {
        Self::present(value)
    }
}

impl From<Option<Value>> for BodySlot {
    fn from(value: Option<Value>) -> Self {
        value.map_or(Self::Absent, Self::present)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobSlots {
    pub inputs: Arc<BTreeSet<String>>,
    pub workflow_inputs: Arc<BTreeMap<String, BTreeSet<String>>>,
}

/// Evaluation context for expression resolution.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub inputs: Arc<BTreeMap<String, Value>>,
    pub blob_slots: BlobSlots,
    /// Step outputs, wrapped in `Arc` for cheap cloning during repeated evaluation.
    pub steps: Arc<BTreeMap<String, BTreeMap<String, Value>>>,
    pub outputs: BTreeMap<String, Value>,
    pub workflows: Arc<BTreeMap<String, WorkflowEvalState>>,
    pub status_code: Option<i64>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub request_headers: BTreeMap<String, String>,
    pub request_query: BTreeMap<String, String>,
    pub request_path: BTreeMap<String, String>,
    pub request_body: BodySlot,
    /// Headers from an asynchronous message when message execution is available.
    pub message_headers: BTreeMap<String, String>,
    /// Payload from an asynchronous message when message execution is available.
    pub message_payload: Option<Value>,
    pub self_uri: Option<String>,
    pub source_descriptions: Arc<BTreeMap<String, SourceDescriptionContext>>,
    pub response_headers: BTreeMap<String, String>,
    pub response_body: BodySlot,
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

    /// Resolve a complete expression with its JSON type, an embedded-expression
    /// template as a string, or literal text unchanged.
    ///
    /// Like every resolution entry point here, a reference to a value that
    /// exists but cannot be read is the [`ValueUnavailable`] error, never
    /// `null`; missing values still resolve to `null` with a warning.
    pub fn resolve_value(&self, value: &str) -> Result<Value, ValueUnavailable> {
        Ok(self.resolve_value_with_diagnostics(value)?.0)
    }

    /// Resolve a value string while retaining ordered expression diagnostics.
    /// Malformed templates return their original text and one syntax warning.
    pub fn resolve_value_with_diagnostics(
        &self,
        value: &str,
    ) -> Result<(Value, Vec<ExpressionWarning>), ValueUnavailable> {
        Ok(self.resolve_value_with_provenance(value)?.into_public())
    }

    pub fn resolve_value_with_provenance(
        &self,
        value: &str,
    ) -> Result<ValueResolution, ValueUnavailable> {
        interpolation::resolve_value(value, self.context())
    }

    pub fn blob_slot(&self, value: &str) -> Option<BlobSlot> {
        if self.ctx.blob_slots.inputs.is_empty() {
            return None;
        }
        let Ok(ValueStringSyntax::Expression(expression)) = classify_value_string(value) else {
            return None;
        };
        match resolution::resolve_parsed(&expression, self.context()).value {
            resolution::ResolvedValue::Blob { slot, .. } => Some(slot),
            _ => None,
        }
    }

    /// Evaluate an expression and return a dynamic JSON value.
    ///
    /// Missing keys and unknown namespaces silently return `Value::Null`.
    /// Use [`evaluate_with_diagnostics`](Self::evaluate_with_diagnostics) to
    /// collect warnings about unresolved expressions.
    pub fn evaluate(&self, expr: &str) -> Result<Value, ValueUnavailable> {
        Ok(self.evaluate_with_diagnostics(expr)?.0)
    }

    /// Resolve a runtime expression without collapsing absence into JSON null.
    /// Invalid or unresolved expressions return `None`; an explicit JSON null
    /// returns `Some(Value::Null)`. Intended for diagnostic consumers.
    pub fn evaluate_resolved(&self, expr: &str) -> Result<Option<Value>, ValueUnavailable> {
        let Ok(parsed) = parse_runtime_expression(expr) else {
            return Ok(None);
        };
        match resolution::resolve_parsed(&parsed, self.context()).value {
            resolution::ResolvedValue::Present(value) => Ok(Some(value)),
            resolution::ResolvedValue::Missing => Ok(None),
            resolution::ResolvedValue::Unavailable(unavailable)
            | resolution::ResolvedValue::Blob { unavailable, .. } => Err(unavailable),
        }
    }

    /// Evaluate an expression, returning both the value and any diagnostic
    /// warnings produced when resolution falls back to `Null`.
    pub fn evaluate_with_diagnostics(
        &self,
        expr: &str,
    ) -> Result<(Value, Vec<ExpressionWarning>), ValueUnavailable> {
        Ok(self.evaluate_with_provenance(expr)?.into_public())
    }

    pub fn evaluate_with_provenance(
        &self,
        expression: &str,
    ) -> Result<ValueResolution, ValueUnavailable> {
        resolution::evaluate_with_provenance(expression, self.context())
    }

    /// Evaluate an expression and convert to string with Go-compatible coercions.
    pub fn evaluate_string(&self, expr: &str) -> Result<String, ValueUnavailable> {
        Ok(to_string_value(&self.evaluate(expr)?).into_owned())
    }

    /// Interpolate `{$expr}` and `$inputs.foo` style segments in a string.
    pub fn interpolate_string(&self, input: &str) -> Result<String, ValueUnavailable> {
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
            out.push_str(&self.evaluate_string(expr)?);
            cursor = full.end();
        }

        out.push_str(&input[cursor..]);
        Ok(out)
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
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn resolved_evaluation_preserves_absence_and_explicit_null() {
        let evaluator = super::ExpressionEvaluator::new(super::EvalContext {
            response_body: super::BodySlot::present(
                serde_json::json!({"features": [], "explicitNull": null, "value": 7}),
            ),
            ..Default::default()
        });
        assert_eq!(
            evaluator
                .evaluate_resolved("$response.body#/features/0/properties/event")
                .unwrap(),
            None
        );
        assert_eq!(
            evaluator
                .evaluate_resolved("$response.body#/explicitNull")
                .unwrap(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            evaluator
                .evaluate_resolved("$response.body#/value")
                .unwrap(),
            Some(serde_json::json!(7))
        );
        assert_eq!(
            evaluator.evaluate_resolved("not an expression").unwrap(),
            None
        );
    }

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
        assert_eq!(eval.evaluate("hello").unwrap(), Value::Null);
        assert_eq!(eval.evaluate("$unknown.thing").unwrap(), Value::Null);
    }

    #[test]
    fn resolve_value_dispatches_correctly() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("name".to_string(), json!("Alice"));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("token".to_string(), json!("xyz"));
        let eval = ExpressionEvaluator::new(ctx);

        // Full expression → evaluate
        assert_eq!(eval.resolve_value("$inputs.name").unwrap(), json!("Alice"));

        // Interpolated → interpolate_string
        assert_eq!(
            eval.resolve_value("Bearer {$inputs.token}").unwrap(),
            json!("Bearer xyz")
        );

        // Literal → as-is string
        assert_eq!(eval.resolve_value("literal").unwrap(), json!("literal"));
    }

    #[test]
    fn evaluate_inputs_and_step_outputs() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("name".to_string(), json!("Alice"));
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("token".to_string(), json!("abc"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$inputs.name").unwrap(), json!("Alice"));
        assert_eq!(eval.evaluate("$inputs.missing").unwrap(), Value::Null);
        assert_eq!(
            eval.evaluate("$steps.s1.outputs.token").unwrap(),
            json!("abc")
        );
        assert_eq!(
            eval.evaluate("$steps.nope.outputs.token").unwrap(),
            Value::Null
        );
        assert_eq!(eval.evaluate("$steps.s1.token").unwrap(), Value::Null);
    }

    #[test]
    fn evaluate_response_fields() {
        let mut ctx = EvalContext {
            status_code: Some(404),
            response_body: super::BodySlot::present(json!({
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

        assert_eq!(eval.evaluate("$statusCode").unwrap(), json!(404));
        assert_eq!(
            eval.evaluate("$response.header.X-Request-Id").unwrap(),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.header.x-request-id").unwrap(),
            json!("req-1")
        );
        assert_eq!(
            eval.evaluate("$response.body").unwrap(),
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
        assert_eq!(
            eval.evaluate("$response.body#/user/name").unwrap(),
            json!("Bob")
        );
        assert_eq!(eval.evaluate("$response.body#/arr/0/id").unwrap(), json!(7));
        for invalid in [
            "$response.body.user.name",
            "$response.body.arr[0].id",
            "$response.body.arr.#",
            "$response.body.users[*].id",
            "$response.body.users.#(id==2).name",
            "$response.body.users[?(@.group=='a')].id",
        ] {
            let (value, warnings) = eval.evaluate_with_diagnostics(invalid).unwrap();
            assert_eq!(value, Value::Null);
            assert_eq!(warnings.len(), 1);
        }
    }

    #[test]
    fn evaluate_response_fields_without_response() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(eval.evaluate("$statusCode").unwrap(), Value::Null);
        assert_eq!(
            eval.evaluate("$response.header.X-Foo").unwrap(),
            Value::Null
        );
        assert_eq!(
            eval.evaluate("$response.body.user.name").unwrap(),
            Value::Null
        );
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
        std::sync::Arc::make_mut(&mut ctx.inputs)
            .insert("secret".to_string(), json!("from-inputs"));
        let eval = ExpressionEvaluator::new(ctx);

        std::env::set_var(SENTINEL_NAME, SENTINEL_VALUE);
        let present = eval.evaluate_with_diagnostics(&expr).unwrap();
        let present_interpolated = eval
            .interpolate_string(&format!("token={{{expr}}}"))
            .unwrap();
        std::env::remove_var(SENTINEL_NAME);
        let absent = eval.evaluate_with_diagnostics(&expr).unwrap();

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
        assert_eq!(
            eval.evaluate("$inputs.secret").unwrap(),
            json!("from-inputs")
        );
        let (value, warnings) = eval.evaluate_with_diagnostics("$notaspace.secret").unwrap();
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown namespace"));
    }

    #[test]
    fn evaluate_string_coercions() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("s".to_string(), json!("hello"));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("f".to_string(), json!(2.5));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("i".to_string(), json!(42));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("t".to_string(), json!(true));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("f2".to_string(), json!(false));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("arr".to_string(), json!([1, 2]));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate_string("$inputs.missing").unwrap(), "");
        assert_eq!(eval.evaluate_string("$inputs.s").unwrap(), "hello");
        assert_eq!(eval.evaluate_string("$inputs.f").unwrap(), "2.5");
        assert_eq!(eval.evaluate_string("$inputs.i").unwrap(), "42");
        assert_eq!(eval.evaluate_string("$inputs.t").unwrap(), "true");
        assert_eq!(eval.evaluate_string("$inputs.f2").unwrap(), "false");
        assert_eq!(eval.evaluate_string("$inputs.arr").unwrap(), "");
    }

    #[test]
    fn method_expression() {
        let ctx = EvalContext {
            method: Some("GET".to_string()),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$method").unwrap(), json!("GET"));

        let ctx_no_method = EvalContext::default();
        let eval_no_method = ExpressionEvaluator::new(ctx_no_method);
        assert_eq!(eval_no_method.evaluate("$method").unwrap(), Value::Null);
    }

    #[test]
    fn interpolate_string_modes() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("name".to_string(), json!("Alice"));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("age".to_string(), json!(30));
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("a".to_string(), json!("X"));
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("b".to_string(), json!("Y"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.interpolate_string("Hello {$inputs.name}!").unwrap(),
            "Hello Alice!"
        );
        assert_eq!(
            eval.interpolate_string("Age: $inputs.age").unwrap(),
            "Age: 30"
        );
        assert_eq!(
            eval.interpolate_string("{$inputs.a}-$steps.s1.outputs.b")
                .unwrap(),
            "X-Y"
        );
        assert_eq!(eval.interpolate_string("plain text").unwrap(), "plain text");
        assert_eq!(
            eval.interpolate_string("Bearer {$inputs.name}").unwrap(),
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
            std::sync::Arc::make_mut(&mut ctx.inputs).insert("token".to_string(), json!(value.clone()));
            let eval = ExpressionEvaluator::new(ctx);

            let expr = format!("{prefix}{{$inputs.token}}{suffix}");
            let rendered = eval.interpolate_string(&expr).unwrap();
            prop_assert_eq!(rendered, format!("{prefix}{value}{suffix}"));
        }

        #[test]
        fn response_array_len_and_index_extraction_are_consistent(
            values in proptest::collection::vec(any::<i64>(), 0..20),
            idx in 0usize..25usize,
        ) {
            let eval = ExpressionEvaluator::new(EvalContext {
                response_body: super::BodySlot::present(json!({"arr": values.clone()})),
                ..EvalContext::default()
            });

            let at_value = eval.evaluate(&format!("$response.body#/arr/{idx}")).unwrap();
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

        assert_eq!(eval.evaluate("$outputs.total").unwrap(), json!(42));
        assert_eq!(eval.evaluate("$outputs.missing").unwrap(), Value::Null);
        assert_eq!(
            eval.evaluate("$outputs.nested").unwrap(),
            json!({"a": {"b": "deep"}})
        );
    }

    #[test]
    fn evaluate_outputs_json_pointer() {
        let mut ctx = EvalContext::default();
        ctx.outputs
            .insert("data".to_string(), json!({"items": [{"id": 1}, {"id": 2}]}));
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$outputs.data#/items/0/id").unwrap(),
            json!(1)
        );
        assert_eq!(
            eval.evaluate("$outputs.data#/items/1/id").unwrap(),
            json!(2)
        );
        assert_eq!(
            eval.evaluate("$outputs.data#/missing").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn evaluate_named_value_json_pointers() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert(
            "user".to_string(),
            json!({"profile": {"email": "alice@example.com"}}),
        );
        Arc::make_mut(&mut ctx.steps).insert(
            "lookup".to_string(),
            BTreeMap::from([("payload".to_string(), json!({"items": [{"id": "item-1"}]}))]),
        );
        std::sync::Arc::make_mut(&mut ctx.workflows).insert(
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
            eval.evaluate("$inputs.user#/profile/email").unwrap(),
            json!("alice@example.com")
        );
        assert_eq!(
            eval.evaluate("$steps.lookup.outputs.payload#/items/0/id")
                .unwrap(),
            json!("item-1")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.tokenPayload#/token")
                .unwrap(),
            json!("abc-123")
        );
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.config#/env").unwrap(),
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
            eval.evaluate("$message.header.X-Request-Id").unwrap(),
            json!("request-123")
        );
        assert_eq!(
            eval.evaluate("$message.payload").unwrap(),
            json!({"order": {"id": "order-42"}})
        );
        assert_eq!(
            eval.evaluate("$message.payload#/order/id").unwrap(),
            json!("order-42")
        );
    }

    #[test]
    fn missing_json_pointer_returns_null_with_diagnostic() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("user".to_string(), json!({"profile": {}}))]).into(),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        let (value, warnings) = eval
            .evaluate_with_diagnostics("$inputs.user#/profile/email")
            .unwrap();
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("JSON Pointer"));
        assert!(warnings[0].message.contains("/profile/email"));
    }

    #[test]
    fn missing_message_context_returns_null_with_diagnostics() {
        let eval = ExpressionEvaluator::new(EvalContext::default());

        let (header, header_warnings) = eval
            .evaluate_with_diagnostics("$message.header.X-Request-Id")
            .unwrap();
        assert_eq!(header, Value::Null);
        assert_eq!(header_warnings.len(), 1);
        assert!(header_warnings[0].message.contains("message header"));

        let (payload, payload_warnings) = eval
            .evaluate_with_diagnostics("$message.payload#/order/id")
            .unwrap();
        assert_eq!(payload, Value::Null);
        assert_eq!(payload_warnings.len(), 1);
        assert!(payload_warnings[0].message.contains("message payload"));
    }

    #[test]
    fn evaluate_response_body_json_pointer() {
        let ctx = EvalContext {
            response_body: super::BodySlot::present(json!({
                "data": [{"name": "Alice"}, {"name": "Bob"}],
                "meta": {"total": 2}
            })),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$response.body#/data/0/name").unwrap(),
            json!("Alice")
        );
        assert_eq!(
            eval.evaluate("$response.body#/data/1/name").unwrap(),
            json!("Bob")
        );
        assert_eq!(
            eval.evaluate("$response.body#/meta/total").unwrap(),
            json!(2)
        );
        assert_eq!(
            eval.evaluate("$response.body#/nonexistent").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn evaluate_response_body_json_pointer_without_body() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        assert_eq!(
            eval.evaluate("$response.body#/data/0").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn diagnostics_missing_step_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$steps.missing.outputs.x")
            .unwrap();
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
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$steps.s1.outputs.nope")
            .unwrap();
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("output \"nope\" not found"));
    }

    #[test]
    fn diagnostics_valid_expression_no_warnings() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.inputs).insert("name".to_string(), json!("Alice"));
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval.evaluate_with_diagnostics("$inputs.name").unwrap();
        assert_eq!(value, json!("Alice"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn diagnostics_unknown_namespace_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$foo.bar").unwrap();
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("unknown namespace"));
    }

    #[test]
    fn diagnostics_missing_source_description_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$sourceDescriptions.missing.url")
            .unwrap();
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
        let (value, warnings) = eval.evaluate_with_diagnostics("$self").unwrap();
        assert_eq!(value, json!("workflows/purchase.arazzo.yaml"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn self_expression_without_uri_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$self").unwrap();
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("self URI not found"));
    }

    #[test]
    fn source_description_url_and_type_resolve() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.source_descriptions).insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.url").unwrap(),
            json!("https://api.example.com/openapi.yaml")
        );
        assert_eq!(
            eval.evaluate("$sourceDescriptions.petstore.type").unwrap(),
            json!("openapi")
        );
    }

    #[test]
    fn unsupported_source_description_reference_warns() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.source_descriptions).insert(
            "petstore".to_string(),
            SourceDescriptionContext {
                url: "https://api.example.com/openapi.yaml".to_string(),
                type_: "openapi".to_string(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$sourceDescriptions.petstore.getPetById")
            .unwrap();

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
        assert_eq!(
            eval.evaluate("$steps.missing.outputs.x").unwrap(),
            Value::Null
        );
        assert_eq!(eval.evaluate("$foo.bar").unwrap(), Value::Null);
    }

    // ── Bug #14: $inputs nested traversal ─────────────────────────

    #[test]
    fn inputs_dotted_names_are_exact_keys() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": {"baz": 42}}))]).into(),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        assert_eq!(eval.evaluate("$inputs.foo.bar.baz").unwrap(), Value::Null);
        assert_eq!(eval.evaluate("$inputs.foo#/bar/baz").unwrap(), json!(42));
        // Top-level (flat key) still works
        assert_eq!(
            eval.evaluate("$inputs.foo").unwrap(),
            json!({"bar": {"baz": 42}})
        );
    }

    #[test]
    fn inputs_flat_key_still_works() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("simple".to_string(), json!("hello"))]).into(),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.simple").unwrap(), json!("hello"));
    }

    #[test]
    fn inputs_nested_missing_sub_path_returns_null() {
        let ctx = EvalContext {
            inputs: BTreeMap::from([("foo".to_string(), json!({"bar": 1}))]).into(),
            ..EvalContext::default()
        };
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(eval.evaluate("$inputs.foo.missing").unwrap(), Value::Null);
    }

    // ── Phase 3: $workflows expression root ─────────────────────────

    #[test]
    fn workflows_expression_inputs() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.workflows).insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::from([("env".to_string(), json!("production"))]),
                outputs: BTreeMap::new(),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.inputs.env").unwrap(),
            json!("production")
        );
    }

    #[test]
    fn workflows_expression_outputs() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.workflows).insert(
            "auth".to_string(),
            super::WorkflowEvalState {
                inputs: BTreeMap::new(),
                outputs: BTreeMap::from([("token".to_string(), json!("abc-123"))]),
            },
        );
        let eval = ExpressionEvaluator::new(ctx);
        assert_eq!(
            eval.evaluate("$workflows.auth.outputs.token").unwrap(),
            json!("abc-123")
        );
    }

    #[test]
    fn workflows_unknown_id_is_null() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$workflows.unknown.outputs.x")
            .unwrap();
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("\"unknown\" not found"));
    }

    #[test]
    fn workflows_missing_sub_path_warns() {
        let eval = ExpressionEvaluator::new(EvalContext::default());
        let (value, warnings) = eval.evaluate_with_diagnostics("$workflows.auth").unwrap();
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn workflows_invalid_sub_path_warns() {
        let mut ctx = EvalContext::default();
        std::sync::Arc::make_mut(&mut ctx.workflows)
            .insert("auth".to_string(), super::WorkflowEvalState::default());
        let eval = ExpressionEvaluator::new(ctx);
        let (value, warnings) = eval
            .evaluate_with_diagnostics("$workflows.auth.something.else")
            .unwrap();
        assert_eq!(value, Value::Null);
        assert!(!warnings.is_empty());
        assert!(warnings[0].message.contains("invalid"));
    }
}
