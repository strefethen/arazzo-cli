//! Parsed Runtime Expression lookup with absence retained until public projection.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::runtime_expression::RuntimeExpressionForm;
use crate::{
    body_pointer_migration_hint, get_header_case_insensitive, parse_runtime_expression, BodySlot,
    EvalContext, ExpressionWarning, ParsedRuntimeExpression, RuntimeExpressionNamespace,
    RuntimeExpressionPointer,
};

/// A referenced value exists but cannot be read as a JSON value. No conversion
/// to a JSON value exists: every resolution entry point returns it as an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueUnavailable {
    pub expression: String,
    pub reason: String,
}

impl std::fmt::Display for ValueUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.expression, self.reason)
    }
}

impl std::error::Error for ValueUnavailable {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueResolution {
    pub value: Value,
    pub warnings: Vec<ExpressionWarning>,
    pub expressions: Vec<ExpressionResolution>,
    pub syntax_error: bool,
}

impl ValueResolution {
    pub(crate) fn into_public(self) -> (Value, Vec<ExpressionWarning>) {
        (self.value, self.warnings)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionResolution {
    pub expression: String,
    pub namespace: RuntimeExpressionNamespace,
    pub reference: Option<StoredValueReference>,
    pub missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredValueReference {
    Input {
        name: String,
        pointer: Option<String>,
    },
    Output {
        name: String,
        pointer: Option<String>,
    },
    StepOutput {
        step_id: String,
        name: String,
        pointer: Option<String>,
    },
    WorkflowInput {
        workflow_id: String,
        name: String,
        pointer: Option<String>,
    },
    WorkflowOutput {
        workflow_id: String,
        name: String,
        pointer: Option<String>,
    },
}

impl StoredValueReference {
    fn from_parsed(expression: &ParsedRuntimeExpression<'_>) -> Option<Self> {
        use RuntimeExpressionForm as Form;
        use RuntimeExpressionNamespace as Namespace;

        let owned_pointer = |pointer: Option<RuntimeExpressionPointer<'_>>| {
            pointer.map(|pointer| pointer.raw().to_owned())
        };
        match (expression.namespace(), expression.form()) {
            (Namespace::Inputs, Form::Named(name, pointer)) => Some(Self::Input {
                name: (*name).to_owned(),
                pointer: owned_pointer(*pointer),
            }),
            (Namespace::Outputs, Form::Named(name, pointer)) => Some(Self::Output {
                name: (*name).to_owned(),
                pointer: owned_pointer(*pointer),
            }),
            (Namespace::Steps, Form::Step(reference)) => Some(Self::StepOutput {
                step_id: reference.step_id().to_owned(),
                name: reference.output_name().to_owned(),
                pointer: owned_pointer(reference.pointer()),
            }),
            (
                Namespace::Workflows,
                Form::Workflow {
                    id,
                    field,
                    name,
                    pointer,
                },
            ) => {
                let workflow_id = (*id).to_owned();
                let name = (*name).to_owned();
                let pointer = owned_pointer(*pointer);
                Some(if field.eq_ignore_ascii_case("inputs") {
                    Self::WorkflowInput {
                        workflow_id,
                        name,
                        pointer,
                    }
                } else {
                    Self::WorkflowOutput {
                        workflow_id,
                        name,
                        pointer,
                    }
                })
            }
            _ => None,
        }
    }
}

pub(crate) fn evaluate_with_provenance(
    expression: &str,
    context: &EvalContext,
) -> Result<ValueResolution, ValueUnavailable> {
    match parse_runtime_expression(expression) {
        Ok(parsed) => resolve_parsed_with_provenance(&parsed, context),
        Err(error) => {
            let mut message = error.to_string();
            if let Some(hint) = body_pointer_migration_hint(expression, &error) {
                message.push_str("; ");
                message.push_str(&hint);
            }
            Ok(ValueResolution {
                value: Value::Null,
                warnings: vec![ExpressionWarning {
                    expression: expression.to_owned(),
                    message,
                }],
                expressions: Vec::new(),
                syntax_error: true,
            })
        }
    }
}

pub(crate) fn resolve_parsed_with_provenance(
    expression: &ParsedRuntimeExpression<'_>,
    context: &EvalContext,
) -> Result<ValueResolution, ValueUnavailable> {
    let resolution = resolve_parsed(expression, context);
    let occurrence = ExpressionResolution {
        expression: expression.raw().to_owned(),
        namespace: expression.namespace(),
        reference: StoredValueReference::from_parsed(expression),
        missing: matches!(resolution.value, ResolvedValue::Missing),
    };
    let (value, warnings) = resolution.into_public()?;
    Ok(ValueResolution {
        value,
        warnings,
        expressions: vec![occurrence],
        syntax_error: false,
    })
}

#[derive(Debug, PartialEq)]
pub(crate) enum ResolvedValue {
    Present(Value),
    Missing,
    Unavailable(ValueUnavailable),
}

#[derive(Debug, PartialEq)]
pub(crate) struct Resolution {
    pub(crate) value: ResolvedValue,
    pub(crate) warnings: Vec<ExpressionWarning>,
}

impl Resolution {
    pub(crate) fn into_public(self) -> Result<(Value, Vec<ExpressionWarning>), ValueUnavailable> {
        match self.value {
            ResolvedValue::Present(value) => Ok((value, self.warnings)),
            ResolvedValue::Missing => Ok((Value::Null, self.warnings)),
            ResolvedValue::Unavailable(unavailable) => Err(unavailable),
        }
    }

    fn from_option(value: Option<Value>) -> Self {
        Self {
            value: value.map_or(ResolvedValue::Missing, ResolvedValue::Present),
            warnings: Vec::new(),
        }
    }

    fn from_lookup(
        expression: &ParsedRuntimeExpression<'_>,
        lookup: Result<Value, String>,
    ) -> Self {
        match lookup {
            Ok(value) => Self::from_option(Some(value)),
            Err(message) => Self::missing(expression, message),
        }
    }

    fn missing(expression: &ParsedRuntimeExpression<'_>, message: String) -> Self {
        Self {
            value: ResolvedValue::Missing,
            warnings: vec![ExpressionWarning {
                expression: expression.raw().to_owned(),
                message,
            }],
        }
    }

    fn unavailable(expression: &ParsedRuntimeExpression<'_>, reason: &str) -> Self {
        Self {
            value: ResolvedValue::Unavailable(ValueUnavailable {
                expression: expression.raw().to_owned(),
                reason: reason.to_owned(),
            }),
            warnings: Vec::new(),
        }
    }
}

pub(crate) fn resolve_parsed(
    expression: &ParsedRuntimeExpression<'_>,
    context: &EvalContext,
) -> Resolution {
    use RuntimeExpressionForm as Form;
    use RuntimeExpressionNamespace as Namespace;

    match expression.namespace() {
        Namespace::Url => Resolution::from_lookup(
            expression,
            context
                .url
                .clone()
                .map(Value::String)
                .ok_or_else(|| "URL not found in context".to_owned()),
        ),
        Namespace::Method => Resolution::from_lookup(
            expression,
            context
                .method
                .clone()
                .map(Value::String)
                .ok_or_else(|| "method not found in context".to_owned()),
        ),
        Namespace::StatusCode => Resolution::from_lookup(
            expression,
            context
                .status_code
                .map(Value::from)
                .ok_or_else(|| "status code not found in context".to_owned()),
        ),
        Namespace::SelfUri => match &context.self_uri {
            Some(uri) => Resolution::from_option(Some(Value::String(uri.clone()))),
            None => Resolution::missing(expression, "self URI not found in context".to_owned()),
        },
        Namespace::Inputs | Namespace::Outputs => {
            let Form::Named(name, pointer) = expression.form() else {
                unreachable!("canonical input/output syntax has a named form")
            };
            let (values, label) = if expression.namespace() == Namespace::Inputs {
                (context.inputs.as_ref(), "input")
            } else {
                (&context.outputs, "output")
            };
            resolve_named(values, name, *pointer, expression, || {
                format!("{label} \"{name}\" not found in context")
            })
        }
        Namespace::Steps => {
            let Form::Step(reference) = expression.form() else {
                unreachable!("canonical step syntax has a step form")
            };
            let id = reference.step_id();
            match context.steps.get(id) {
                Some(outputs) => resolve_named(
                    outputs,
                    reference.output_name(),
                    reference.pointer(),
                    expression,
                    || {
                        format!(
                            "output \"{}\" not found in step \"{id}\"",
                            reference.output_name()
                        )
                    },
                ),
                None => {
                    Resolution::missing(expression, format!("step \"{id}\" not found in context"))
                }
            }
        }
        Namespace::Workflows => {
            let Form::Workflow {
                id,
                field,
                name,
                pointer,
            } = expression.form()
            else {
                unreachable!("canonical workflow syntax has a workflow form")
            };
            let Some(state) = context.workflows.get(*id) else {
                return Resolution::missing(
                    expression,
                    format!("workflow \"{id}\" not found in workflows context"),
                );
            };
            let (values, label) = if field.eq_ignore_ascii_case("inputs") {
                (&state.inputs, "input")
            } else {
                (&state.outputs, "output")
            };
            resolve_named(values, name, *pointer, expression, || {
                format!("{label} \"{name}\" not found in workflow \"{id}\"")
            })
        }
        Namespace::SourceDescriptions => {
            let Form::Source(reference) = expression.form() else {
                unreachable!("canonical source syntax has a source form")
            };
            let name = reference.source_name();
            let reference = reference.reference_id();
            match context.source_descriptions.get(name) {
                Some(source) => match reference {
                    "url" => Resolution::from_option(Some(Value::String(source.url.clone()))),
                    "type" => Resolution::from_option(Some(Value::String(source.type_.clone()))),
                    _ => Resolution::missing(expression, format!(
                        "source description reference \"{reference}\" for \"{name}\" cannot be resolved without loaded source document metadata"
                    )),
                },
                None => Resolution::missing(
                    expression,
                    format!("source description \"{name}\" not found in context"),
                ),
            }
        }
        Namespace::Components => Resolution::missing(
            expression,
            format!("unknown expression namespace \"{}\"", expression.raw()),
        ),
        Namespace::Request | Namespace::Response | Namespace::Message => {
            resolve_message_source(expression, context)
        }
    }
}

fn resolve_message_source(
    expression: &ParsedRuntimeExpression<'_>,
    context: &EvalContext,
) -> Resolution {
    use RuntimeExpressionForm as Form;
    use RuntimeExpressionNamespace as Namespace;

    let lookup = match (expression.namespace(), expression.form()) {
        (namespace, Form::Header(name)) => {
            let (headers, label) = match namespace {
                Namespace::Request => (&context.request_headers, "request"),
                Namespace::Response => (&context.response_headers, "response"),
                _ => (&context.message_headers, "message"),
            };
            get_header_case_insensitive(headers, name)
                .cloned()
                .map(Value::String)
                .ok_or_else(|| format!("{label} header \"{name}\" not found in context"))
        }
        (Namespace::Request, Form::Query(name)) => context
            .request_query
            .get(*name)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| format!("request query parameter \"{name}\" not found in context")),
        (Namespace::Request, Form::Path(name)) => context
            .request_path
            .get(*name)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| format!("request path parameter \"{name}\" not found in context")),
        (Namespace::Request, Form::Body(pointer)) => {
            return resolve_body(expression, &context.request_body, *pointer, "request body")
        }
        (Namespace::Response, Form::Body(pointer)) => {
            return resolve_body(
                expression,
                &context.response_body,
                *pointer,
                "response body",
            )
        }
        (Namespace::Message, Form::Payload(pointer)) => select_document(
            context.message_payload.as_ref(),
            *pointer,
            "message payload",
        ),
        (Namespace::Response, Form::Query(name)) => Err(format!(
            "response query parameter \"{name}\" is not available in context"
        )),
        (Namespace::Response, Form::Path(name)) => Err(format!(
            "response path parameter \"{name}\" is not available in context"
        )),
        _ => unreachable!("canonical message sources have namespace-specific forms"),
    };
    Resolution::from_lookup(expression, lookup)
}

fn resolve_body(
    expression: &ParsedRuntimeExpression<'_>,
    slot: &BodySlot,
    pointer: Option<RuntimeExpressionPointer<'_>>,
    label: &str,
) -> Resolution {
    let document = match slot {
        BodySlot::Absent => None,
        BodySlot::Present(document) => Some(document),
        BodySlot::Unavailable { reason } => return Resolution::unavailable(expression, reason),
    };
    Resolution::from_lookup(expression, select_document(document, pointer, label))
}

fn select_document(
    document: Option<&Value>,
    pointer: Option<RuntimeExpressionPointer<'_>>,
    label: &str,
) -> Result<Value, String> {
    let document = document.ok_or_else(|| format!("{label} not found in context"))?;
    select_pointer(document, pointer).ok_or_else(|| {
        format!(
            "{label} suffix \"{}\" did not resolve",
            pointer.map_or("", |pointer| pointer.raw()),
        )
    })
}

fn select_pointer(value: &Value, pointer: Option<RuntimeExpressionPointer<'_>>) -> Option<Value> {
    match pointer {
        // The parser validated this suffix and its leading '#'.
        Some(pointer) => value.pointer(&pointer.raw()[1..]).cloned(),
        None => Some(value.clone()),
    }
}

fn resolve_named(
    values: &BTreeMap<String, Value>,
    name: &str,
    pointer: Option<RuntimeExpressionPointer<'_>>,
    expression: &ParsedRuntimeExpression<'_>,
    missing_message: impl FnOnce() -> String,
) -> Resolution {
    let Some(value) = values.get(name) else {
        return Resolution::missing(expression, missing_message());
    };
    match select_pointer(value, pointer) {
        Some(value) => Resolution::from_option(Some(value)),
        None => Resolution::missing(
            expression,
            format!(
                "JSON Pointer \"{}\" did not resolve in value \"{name}\"",
                pointer.map_or("", |pointer| &pointer.raw()[1..]),
            ),
        ),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // Valid syntax and initialized fixtures precede lookup assertions.
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::{
        parse_runtime_expression, ExpressionEvaluator, SourceDescriptionContext, WorkflowEvalState,
    };

    fn context() -> EvalContext {
        let body = json!({
            "items": [{"id": 7}], "nil": null,
            "a.b": "literal", "a": {"b": "nested"},
            "a/b": {"~key": "escaped"}, "é": "unicode", "": "empty"
        });
        EvalContext {
            inputs: BTreeMap::from([("user".to_owned(), body.clone())]).into(),
            outputs: BTreeMap::from([("data".to_owned(), body.clone())]),
            steps: Arc::new(BTreeMap::from([(
                "lookup".to_owned(),
                BTreeMap::from([("payload".to_owned(), body.clone())]),
            )])),
            workflows: BTreeMap::from([(
                "auth".to_owned(),
                WorkflowEvalState {
                    inputs: BTreeMap::from([("config".to_owned(), body.clone())]),
                    outputs: BTreeMap::from([("result".to_owned(), body.clone())]),
                },
            )])
            .into(),
            status_code: Some(201),
            method: Some("POST".to_owned()),
            url: Some("https://example.test/items".to_owned()),
            request_headers: BTreeMap::from([("x-request-id".to_owned(), "request".to_owned())]),
            request_query: BTreeMap::from([("q".to_owned(), "search".to_owned())]),
            request_path: BTreeMap::from([("id".to_owned(), "path-id".to_owned())]),
            request_body: crate::BodySlot::Present(body.clone()),
            response_headers: BTreeMap::from([("x-response-id".to_owned(), "response".to_owned())]),
            response_body: crate::BodySlot::Present(body.clone()),
            message_headers: BTreeMap::from([("x-message-id".to_owned(), "message".to_owned())]),
            message_payload: Some(body),
            self_uri: Some("workflows/example.yaml".to_owned()),
            source_descriptions: BTreeMap::from([(
                "api".to_owned(),
                SourceDescriptionContext {
                    url: "https://example.test/openapi.yaml".to_owned(),
                    type_: "openapi".to_owned(),
                },
            )])
            .into(),
        }
    }

    fn resolve(input: &str, context: &EvalContext) -> Resolution {
        resolve_parsed(
            &parse_runtime_expression(input).expect("valid expression fixture"),
            context,
        )
    }

    fn warning(input: &str, message: &str) -> ExpressionWarning {
        ExpressionWarning {
            expression: input.to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn supported_forms_preserve_values_and_public_behavior() {
        let context = context();
        let BodySlot::Present(body) = context.request_body.clone() else {
            panic!("body fixture")
        };
        let cases = [
            ("$url", json!("https://example.test/items")),
            ("$method", json!("POST")),
            ("$statusCode", json!(201)),
            ("$self", json!("workflows/example.yaml")),
            ("$request.header.X-Request-Id", json!("request")),
            ("$response.header.X-Response-Id", json!("response")),
            ("$message.header.X-Message-Id", json!("message")),
            ("$request.query.q", json!("search")),
            ("$request.path.id", json!("path-id")),
            ("$request.body", body.clone()),
            ("$response.body", body.clone()),
            ("$message.payload", body.clone()),
            ("$inputs.user", body.clone()),
            ("$outputs.data", body.clone()),
            ("$steps.lookup.outputs.payload", body.clone()),
            ("$workflows.auth.inputs.config", body.clone()),
            ("$workflows.auth.outputs.result", body.clone()),
            ("$request.body#", body.clone()),
            ("$response.body#", body.clone()),
            ("$message.payload#", body.clone()),
            ("$inputs.user#", body),
            ("$request.body#/items/0/id", json!(7)),
            ("$response.body#/items/0/id", json!(7)),
            ("$message.payload#/items/0/id", json!(7)),
            ("$inputs.user#/items/0/id", json!(7)),
            ("$outputs.data#/items/0/id", json!(7)),
            ("$steps.lookup.outputs.payload#/items/0/id", json!(7)),
            ("$workflows.auth.inputs.config#/items/0/id", json!(7)),
            ("$workflows.auth.outputs.result#/items/0/id", json!(7)),
            (
                "$sourceDescriptions.api.url",
                json!("https://example.test/openapi.yaml"),
            ),
            ("$sourceDescriptions.api.type", json!("openapi")),
        ];
        let evaluator = ExpressionEvaluator::new(context.clone());
        for (input, expected) in cases {
            let resolution = resolve(input, &context);
            assert_eq!(
                resolution.value,
                ResolvedValue::Present(expected.clone()),
                "{input}"
            );
            assert!(resolution.warnings.is_empty(), "{input}");
            assert_eq!(
                resolution.into_public().unwrap(),
                evaluator.evaluate_with_diagnostics(input).unwrap(),
                "{input}"
            );
        }
    }

    #[test]
    fn keywords_ignore_case_but_names_and_source_fields_keep_their_bytes() {
        let context = context();
        for (input, expected) in [
            ("$URL", json!("https://example.test/items")),
            ("$MeThOd", json!("POST")),
            ("$STATUSCODE", json!(201)),
            ("$SeLf", json!("workflows/example.yaml")),
            ("$ReQuEsT.HeAdEr.X-REQUEST-ID", json!("request")),
            ("$ReSpOnSe.BoDy#/items/0/id", json!(7)),
            ("$MESSAGE.PAYLOAD#/items/0/id", json!(7)),
            ("$INPUTS.user#/items/0/id", json!(7)),
            ("$OUTPUTS.data#/items/0/id", json!(7)),
            ("$STEPS.lookup.OUTPUTS.payload#/items/0/id", json!(7)),
            ("$WORKFLOWS.auth.INPUTS.config#/items/0/id", json!(7)),
            ("$WoRkFlOwS.auth.OuTpUtS.result#/items/0/id", json!(7)),
            (
                "$SOURCEDESCRIPTIONS.api.url",
                json!("https://example.test/openapi.yaml"),
            ),
        ] {
            assert_eq!(
                resolve(input, &context).value,
                ResolvedValue::Present(expected),
                "{input}"
            );
        }
        for input in [
            "$inputs.User",
            "$steps.Lookup.outputs.payload",
            "$workflows.Auth.inputs.config",
            "$request.query.Q",
            "$sourceDescriptions.API.url",
            "$sourceDescriptions.api.URL",
        ] {
            assert_eq!(
                resolve(input, &context).value,
                ResolvedValue::Missing,
                "{input}"
            );
        }
    }

    #[test]
    fn dotted_names_are_exact_keys_and_only_pointers_traverse() {
        let mut context = context();
        std::sync::Arc::make_mut(&mut context.inputs)
            .insert("user.items".to_owned(), json!("exact-input"));
        context
            .outputs
            .insert("data.items".to_owned(), json!("exact-output"));
        Arc::make_mut(&mut context.steps)
            .get_mut("lookup")
            .expect("step fixture")
            .insert("payload.items".to_owned(), json!("exact-step"));
        let workflow = std::sync::Arc::make_mut(&mut context.workflows)
            .get_mut("auth")
            .expect("workflow fixture");
        workflow
            .inputs
            .insert("config.items".to_owned(), json!("exact-workflow-input"));
        workflow
            .outputs
            .insert("result.items".to_owned(), json!("exact-workflow-output"));
        for (input, expected) in [
            ("$inputs.user.items", json!("exact-input")),
            ("$outputs.data.items", json!("exact-output")),
            ("$steps.lookup.outputs.payload.items", json!("exact-step")),
            (
                "$workflows.auth.inputs.config.items",
                json!("exact-workflow-input"),
            ),
            (
                "$workflows.auth.outputs.result.items",
                json!("exact-workflow-output"),
            ),
            ("$inputs.user#/items", json!([{"id": 7}])),
            ("$inputs.user#/a.b", json!("literal")),
            ("$inputs.user#/a/b", json!("nested")),
            ("$response.body#/a.b", json!("literal")),
            ("$response.body#/a/b", json!("nested")),
        ] {
            assert_eq!(
                resolve(input, &context).value,
                ResolvedValue::Present(expected),
                "{input}"
            );
        }
        std::sync::Arc::make_mut(&mut context.inputs).remove("user.items");
        assert_eq!(
            resolve("$inputs.user.items", &context).value,
            ResolvedValue::Missing
        );
    }

    #[test]
    fn punctuation_escapes_and_empty_tokens_retain_identity() {
        let mut context = context();
        context
            .request_query
            .insert(r"q.name/é\u0061".to_owned(), "exact-query".to_owned());
        context
            .request_path
            .insert("name.with.dots/é".to_owned(), "exact-path".to_owned());
        context
            .message_headers
            .insert("!#$%&'*+-.^_`|~".to_owned(), "token".to_owned());
        for (input, expected) in [
            (r"$request.query.q.name/é\u0061", json!("exact-query")),
            ("$request.path.name.with.dots/é", json!("exact-path")),
            ("$message.header.!#$%&'*+-.^_`|~", json!("token")),
            ("$request.body#/a~1b/~0key", json!("escaped")),
            ("$response.body#/é", json!("unicode")),
            ("$message.payload#/", json!("empty")),
        ] {
            let resolution = resolve(input, &context);
            assert_eq!(
                resolution.value,
                ResolvedValue::Present(expected),
                "{input}"
            );
            assert_eq!(
                resolution.into_public().unwrap(),
                ExpressionEvaluator::new(context.clone())
                    .evaluate_with_diagnostics(input)
                    .unwrap(),
                "{input}"
            );
        }
    }

    #[test]
    fn headers_preserve_exact_match_precedence_and_case_insensitive_lookup() {
        let mut context = context();
        for headers in [
            &mut context.request_headers,
            &mut context.response_headers,
            &mut context.message_headers,
        ] {
            headers.insert("X-Duplicate".to_owned(), "upper".to_owned());
            headers.insert("x-duplicate".to_owned(), "lower".to_owned());
        }
        for namespace in ["request", "response", "message"] {
            for (name, expected) in [
                ("X-Duplicate", "upper"),
                ("x-duplicate", "lower"),
                ("X-DUPLICATE", "upper"),
            ] {
                let input = format!("${namespace}.header.{name}");
                let resolution = resolve(&input, &context);
                assert_eq!(resolution.value, ResolvedValue::Present(json!(expected)));
                assert_eq!(
                    resolution.into_public().unwrap(),
                    ExpressionEvaluator::new(context.clone())
                        .evaluate_with_diagnostics(&input)
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn present_null_remains_distinct_from_missing_root_key_and_pointer() {
        let context = context();
        for root in [
            "$request.body",
            "$response.body",
            "$message.payload",
            "$inputs.user",
            "$outputs.data",
            "$steps.lookup.outputs.payload",
            "$workflows.auth.inputs.config",
            "$workflows.auth.outputs.result",
        ] {
            let present = resolve(&format!("{root}#/nil"), &context);
            assert_eq!(present.value, ResolvedValue::Present(Value::Null), "{root}");
            assert!(present.warnings.is_empty());
            let missing = resolve(&format!("{root}#/missing"), &context);
            assert_eq!(missing.value, ResolvedValue::Missing, "{root}");
            assert_eq!(missing.into_public().unwrap().0, Value::Null);
            assert_eq!(
                resolve(&format!("{root}#/items/1"), &context).value,
                ResolvedValue::Missing,
                "{root}"
            );
            assert_eq!(
                resolve(&format!("{root}#/nil/child"), &context).value,
                ResolvedValue::Missing,
                "{root}"
            );
            assert_eq!(
                resolve(root, &EvalContext::default()).value,
                ResolvedValue::Missing,
                "{root}"
            );
        }
        let mut context = context;
        context.request_body = BodySlot::Present(Value::Null);
        context.response_body = BodySlot::Present(Value::Null);
        context.message_payload = Some(Value::Null);
        std::sync::Arc::make_mut(&mut context.inputs).insert("user".to_owned(), Value::Null);
        context.outputs.insert("data".to_owned(), Value::Null);
        Arc::make_mut(&mut context.steps)
            .get_mut("lookup")
            .expect("step fixture")
            .insert("payload".to_owned(), Value::Null);
        let workflow = std::sync::Arc::make_mut(&mut context.workflows)
            .get_mut("auth")
            .expect("workflow fixture");
        workflow.inputs.insert("config".to_owned(), Value::Null);
        workflow.outputs.insert("result".to_owned(), Value::Null);
        for root in [
            "$request.body",
            "$response.body",
            "$message.payload",
            "$inputs.user",
            "$outputs.data",
            "$steps.lookup.outputs.payload",
            "$workflows.auth.inputs.config",
            "$workflows.auth.outputs.result",
        ] {
            for input in [root.to_owned(), format!("{root}#")] {
                let resolution = resolve(&input, &context);
                assert_eq!(
                    resolution.value,
                    ResolvedValue::Present(Value::Null),
                    "{input}"
                );
                assert!(resolution.warnings.is_empty());
                assert_eq!(resolution.into_public().unwrap(), (Value::Null, Vec::new()));
            }
        }
    }

    #[test]
    fn missing_values_preserve_exact_warnings_in_encounter_order() {
        let context = context();
        let cases = [
            ("$inputs.absent#/x", "input \"absent\" not found in context"),
            ("$outputs.absent", "output \"absent\" not found in context"),
            ("$steps.absent.outputs.payload#/x", "step \"absent\" not found in context"),
            ("$steps.lookup.outputs.absent", "output \"absent\" not found in step \"lookup\""),
            ("$workflows.absent.outputs.result", "workflow \"absent\" not found in workflows context"),
            ("$workflows.auth.inputs.absent", "input \"absent\" not found in workflow \"auth\""),
            ("$workflows.auth.outputs.absent", "output \"absent\" not found in workflow \"auth\""),
            ("$inputs.user#/absent", "JSON Pointer \"/absent\" did not resolve in value \"user\""),
            ("$outputs.data#/absent", "JSON Pointer \"/absent\" did not resolve in value \"data\""),
            ("$steps.lookup.outputs.payload#/absent", "JSON Pointer \"/absent\" did not resolve in value \"payload\""),
            ("$workflows.auth.inputs.config#/absent", "JSON Pointer \"/absent\" did not resolve in value \"config\""),
            ("$workflows.auth.outputs.result#/absent", "JSON Pointer \"/absent\" did not resolve in value \"result\""),
            ("$message.header.Absent", "message header \"Absent\" not found in context"),
            ("$message.payload#/absent", "message payload suffix \"#/absent\" did not resolve"),
            ("$sourceDescriptions.absent.url", "source description \"absent\" not found in context"),
            ("$sourceDescriptions.api.operation.id/é", "source description reference \"operation.id/é\" for \"api\" cannot be resolved without loaded source document metadata"),
        ];
        let evaluator = ExpressionEvaluator::new(context.clone());
        let mut actual_warnings = Vec::new();
        let mut expected_warnings = Vec::new();
        for (input, message) in cases {
            let resolution = resolve(input, &context);
            assert_eq!(resolution.value, ResolvedValue::Missing, "{input}");
            let (value, warnings) = resolution.into_public().unwrap();
            assert_eq!(
                (value.clone(), warnings.clone()),
                evaluator.evaluate_with_diagnostics(input).unwrap(),
                "{input}"
            );
            assert_eq!(value, Value::Null);
            actual_warnings.extend(warnings);
            expected_warnings.push(warning(input, message));
        }
        assert_eq!(actual_warnings, expected_warnings);
        for (input, message) in [
            ("$self", "self URI not found in context"),
            (
                "$message.payload#/x",
                "message payload not found in context",
            ),
        ] {
            assert_eq!(
                resolve(input, &EvalContext::default())
                    .into_public()
                    .unwrap(),
                (Value::Null, vec![warning(input, message)])
            );
        }
        for (input, message) in [
            ("$url", "URL not found in context"),
            ("$method", "method not found in context"),
            ("$statusCode", "status code not found in context"),
            ("$request.body", "request body not found in context"),
            ("$response.body#/x", "response body not found in context"),
        ] {
            let resolution = resolve(input, &EvalContext::default());
            assert_eq!(resolution.value, ResolvedValue::Missing, "{input}");
            assert_eq!(
                resolution.into_public().unwrap(),
                (Value::Null, vec![warning(input, message)])
            );
        }
    }

    #[test]
    fn absent_request_and_response_values_warn_like_inputs_and_message() {
        let context = context();
        let evaluator = ExpressionEvaluator::new(context.clone());
        for (input, message) in [
            (
                "$request.header.Absent",
                "request header \"Absent\" not found in context",
            ),
            (
                "$response.header.Absent",
                "response header \"Absent\" not found in context",
            ),
            (
                "$request.query.absent",
                "request query parameter \"absent\" not found in context",
            ),
            (
                "$request.path.absent",
                "request path parameter \"absent\" not found in context",
            ),
            (
                "$request.body#/absent",
                "request body suffix \"#/absent\" did not resolve",
            ),
            (
                "$response.body#/absent",
                "response body suffix \"#/absent\" did not resolve",
            ),
            (
                "$response.query.q",
                "response query parameter \"q\" is not available in context",
            ),
            (
                "$response.path.id",
                "response path parameter \"id\" is not available in context",
            ),
        ] {
            let resolution = resolve(input, &context);
            assert_eq!(resolution.value, ResolvedValue::Missing, "{input}");
            let public = resolution.into_public().unwrap();
            assert_eq!(
                public,
                (Value::Null, vec![warning(input, message)]),
                "{input}"
            );
            assert_eq!(
                public,
                evaluator.evaluate_with_diagnostics(input).unwrap(),
                "{input}"
            );
        }
    }

    #[test]
    fn absent_request_and_response_values_interpolate_as_null_with_warnings() {
        let evaluator = ExpressionEvaluator::new(context());
        let (value, warnings) = evaluator
            .resolve_value_with_diagnostics("^{$response.header.X-Expected}$ {$inputs.absent}")
            .unwrap();
        assert_eq!(value, json!("^null$ null"));
        assert_eq!(
            warnings,
            vec![
                warning(
                    "$response.header.X-Expected",
                    "response header \"X-Expected\" not found in context"
                ),
                warning("$inputs.absent", "input \"absent\" not found in context"),
            ]
        );
        let (value, warnings) = evaluator
            .resolve_value_with_diagnostics("^{$response.header.X-Response-Id}$")
            .unwrap();
        assert_eq!(value, json!("^response$"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn unsupported_canonical_forms_retain_legacy_outcomes() {
        let context = context();
        let evaluator = ExpressionEvaluator::new(context.clone());
        for input in [
            "$components.parameters.foo",
            "$components.successActions.foo",
            "$components.failureActions.foo",
        ] {
            let resolution = resolve(input, &context);
            assert_eq!(resolution.value, ResolvedValue::Missing, "{input}");
            assert_eq!(
                resolution.into_public().unwrap(),
                evaluator.evaluate_with_diagnostics(input).unwrap(),
                "{input}"
            );
        }
    }

    #[test]
    fn public_postfix_pointer_literal_and_interpolation_controls_after_cutover() {
        let evaluator = ExpressionEvaluator::new(context());
        for condition in [
            "$response.body.items[0].id == 7",
            "$response.body#/items/0/id == 7",
            "$inputs.user#/items/0/id == 7",
        ] {
            assert!(evaluator.evaluate_condition(condition), "{condition}");
        }
        assert_eq!(
            evaluator.evaluate("$response.body.items[0].id").unwrap(),
            Value::Null
        );
        assert_eq!(
            evaluator.resolve_value("ordinary text").unwrap(),
            json!("ordinary text")
        );
        assert_eq!(evaluator.resolve_value("$USD").unwrap(), json!("$USD"));
        assert_eq!(
            evaluator
                .resolve_value("id={$inputs.user#/items/0/id}")
                .unwrap(),
            json!("id=7")
        );
        assert_eq!(
            evaluator
                .interpolate_string("id={$response.body.items[0].id}")
                .unwrap(),
            "id="
        );
        assert_eq!(
            evaluator
                .interpolate_string("id={$response.body#/items/0/id}")
                .unwrap(),
            "id=7"
        );
        for invalid in [
            "$response.body.items[0].id",
            "$message.payload.items",
            "$inputs.user#/a~2",
            "$env.SECRET",
            "$steps.lookup.inputs.payload",
            "$request.payload#/items/0/id",
            "$response.payload#/items/0/id",
            "$message.body#/items/0/id",
            "$message.query.q",
            "$message.path.id",
        ] {
            assert!(parse_runtime_expression(invalid).is_err(), "{invalid}");
        }
    }

    fn unreadable() -> EvalContext {
        EvalContext {
            status_code: Some(200),
            request_body: BodySlot::Unavailable {
                reason: "request body is unreadable".to_owned(),
            },
            response_body: BodySlot::Unavailable {
                reason: "response body is unreadable".to_owned(),
            },
            ..EvalContext::default()
        }
    }

    #[test]
    fn unavailable_never_becomes_null_through_any_resolution_entry_point() {
        let context = unreadable();
        let evaluator = ExpressionEvaluator::new(context.clone());
        for (input, reason) in [
            ("$response.body", "response body is unreadable"),
            ("$response.body#", "response body is unreadable"),
            ("$response.body#/items/0/id", "response body is unreadable"),
            ("$RESPONSE.BODY#/items", "response body is unreadable"),
            ("$request.body", "request body is unreadable"),
            ("$request.body#/items/0/id", "request body is unreadable"),
        ] {
            let unavailable = ValueUnavailable {
                expression: input.to_owned(),
                reason: reason.to_owned(),
            };
            assert_eq!(unavailable.to_string(), format!("{input}: {reason}"));
            let resolution = resolve(input, &context);
            assert_eq!(
                resolution.value,
                ResolvedValue::Unavailable(unavailable.clone()),
                "{input}"
            );
            assert!(resolution.warnings.is_empty(), "{input}");
            assert_eq!(
                resolution.into_public(),
                Err(unavailable.clone()),
                "{input}"
            );
            let template = format!("before {{{input}}} after");
            assert_eq!(evaluator.evaluate(input), Err(unavailable.clone()));
            assert_eq!(evaluator.evaluate_resolved(input), Err(unavailable.clone()));
            assert_eq!(
                evaluator.evaluate_with_diagnostics(input),
                Err(unavailable.clone())
            );
            assert_eq!(
                evaluator.evaluate_with_provenance(input),
                Err(unavailable.clone())
            );
            assert_eq!(evaluator.evaluate_string(input), Err(unavailable.clone()));
            for value in [input, template.as_str()] {
                assert_eq!(evaluator.resolve_value(value), Err(unavailable.clone()));
                assert_eq!(
                    evaluator.resolve_value_with_diagnostics(value),
                    Err(unavailable.clone())
                );
                assert_eq!(
                    evaluator.resolve_value_with_provenance(value),
                    Err(unavailable.clone())
                );
                assert_eq!(
                    evaluator
                        .interpolate_string(value)
                        .map_err(|unavailable| unavailable.reason),
                    Err(reason.to_owned())
                );
            }
        }
        assert_eq!(evaluator.evaluate("$statusCode"), Ok(json!(200)));
        assert_eq!(
            evaluator.resolve_value("{$statusCode} then {$response.body}"),
            Err(ValueUnavailable {
                expression: "$response.body".to_owned(),
                reason: "response body is unreadable".to_owned(),
            })
        );
    }

    #[test]
    fn an_unavailable_mark_travels_with_every_copy_of_its_slot() {
        let context = unreadable();
        let copied = EvalContext {
            response_body: context.response_body.clone(),
            ..EvalContext::default()
        };
        for context in [context.clone(), copied] {
            assert_eq!(
                resolve("$response.body", &context).into_public(),
                Err(ValueUnavailable {
                    expression: "$response.body".to_owned(),
                    reason: "response body is unreadable".to_owned(),
                })
            );
        }
        assert_eq!(BodySlot::from(None), BodySlot::Absent);
        assert_eq!(
            BodySlot::from(Some(Value::Null)),
            BodySlot::Present(Value::Null)
        );
        assert_eq!(BodySlot::from(json!(1)), BodySlot::Present(json!(1)));
        assert_eq!(
            resolve("$response.body", &EvalContext::default()).value,
            ResolvedValue::Missing
        );
    }

    #[test]
    fn unavailable_condition_operands_fail_closed_and_untouched_operands_still_decide() {
        let evaluator = ExpressionEvaluator::new(unreadable());
        for (condition, byte_offset, message) in [
            (
                "$response.body == null",
                0,
                "$response.body: response body is unreadable",
            ),
            (
                "$response.body#/a == 1",
                0,
                "$response.body#/a: response body is unreadable",
            ),
            (
                "!($response.body == 1)",
                2,
                "$response.body: response body is unreadable",
            ),
            (
                "$response.body.items[0].id == 7",
                0,
                "$response.body: response body is unreadable",
            ),
            (
                "$statusCode == 500 || $request.body == 1",
                22,
                "$request.body: request body is unreadable",
            ),
        ] {
            let evaluation = evaluator.evaluate_condition_detailed(condition);
            assert!(!evaluation.result, "{condition}");
            assert_eq!(
                evaluation.error,
                Some(crate::ConditionError {
                    kind: crate::ConditionErrorKind::ValueUnavailable,
                    byte_offset,
                    message: message.to_owned(),
                }),
                "{condition}"
            );
            assert!(evaluation.warnings.is_empty(), "{condition}");
            assert!(!evaluator.evaluate_condition(condition), "{condition}");
        }
        for condition in [
            "$statusCode == 200",
            "$statusCode == 200 || $response.body == 1",
        ] {
            let evaluation = evaluator.evaluate_condition_detailed(condition);
            assert!(evaluation.result, "{condition}");
            assert_eq!(evaluation.error, None, "{condition}");
        }
    }
}
