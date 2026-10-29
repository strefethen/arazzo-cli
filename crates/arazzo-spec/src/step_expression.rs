//! Borrowed, syntax-neutral expression sites effective for one Step.

use serde_yaml_ng::Value;

use crate::{
    effective_step_parameters, selector_admission::borrowed_selector_context,
    EffectiveParameterOrigin, OutputValue, ValueSource, Workflow,
};

/// The field's expression language contract, without inspecting its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepExpressionSiteMode {
    /// A string value that may be literal text, an expression, or interpolation.
    LiteralOrExpression,
    /// A field whose value is a Runtime Expression.
    RequiredExpression,
    /// A simple Criterion condition.
    SimpleCondition,
    /// A typed Criterion condition with brace-delimited Runtime Expressions.
    EmbeddedCondition,
}

/// The original declaration supplying a Step expression site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepExpressionDeclaration<'a> {
    /// An effective parameter, preserving Workflow or Step provenance.
    EffectiveParameterValue { origin: EffectiveParameterOrigin },
    /// The Step's request body payload.
    RequestBodyPayload,
    /// An authored request body replacement value.
    RequestBodyReplacementValue { replacement_index: usize },
    /// A Step success Criterion's context.
    SuccessCriterionContext { criterion_index: usize },
    /// A Step success Criterion's condition.
    SuccessCriterionCondition { criterion_index: usize },
    /// A named Step output.
    Output { name: &'a str },
    /// The Step's declared `correlationId`, independent of source applicability.
    CorrelationId,
}

/// A position within a declaration's value, retaining typed mapping identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepValuePathSegment<'a> {
    SequenceIndex(usize),
    /// Stored entry index plus a borrowed string label when available.
    /// Keys themselves are never expression sites.
    MappingEntry {
        index: usize,
        key: Option<&'a str>,
    },
    /// The context of an admitted or directly typed Selector Object.
    SelectorContext,
}

/// Original text and its field contract and declaration-relative position.
#[derive(Clone, Debug)]
pub struct StepExpressionSite<'a> {
    pub text: &'a str,
    pub mode: StepExpressionSiteMode,
    pub declaration: StepExpressionDeclaration<'a>,
    pub nested_path: Vec<StepValuePathSegment<'a>>,
}

/// Inventories non-action expression-bearing fields effective for a Step.
///
/// Order is effective parameters, request payload, authored replacements,
/// success Criteria (context before condition), outputs in key order, then
/// `correlationId`. Nested literal values use depth-first stored order. Typed
/// Selector roots and raw children admitted by the model contribute only their
/// contexts; directly typed Literal roots retain their literal interpretation.
/// Tagged value nodes are opaque. All text and key labels borrow the input.
///
/// This function neither parses nor validates expressions, selector languages,
/// references, Criterion declaration forms/versions, or source applicability.
/// Known normalized Criterion names select modes even for invalid declarations.
/// Targets, actions, dependency declarations, metadata, extensions, reusable
/// reference strings, and Workflow outputs are excluded.
///
/// An invalid Step index returns `None`; a valid empty inventory is `Some([])`.
pub fn step_expression_sites(
    workflow: &Workflow,
    step_index: usize,
) -> Option<Vec<StepExpressionSite<'_>>> {
    let step = workflow.steps.get(step_index)?;
    let mut sites = Vec::new();
    for effective in effective_step_parameters(workflow, step_index)? {
        visit_value_source(
            &effective.parameter.value,
            StepExpressionDeclaration::EffectiveParameterValue {
                origin: effective.origin,
            },
            &mut sites,
        );
    }
    if let Some(body) = &step.request_body {
        if let Some(payload) = &body.payload {
            visit_value_source(
                payload,
                StepExpressionDeclaration::RequestBodyPayload,
                &mut sites,
            );
        }
        for (replacement_index, replacement) in body.replacements.iter().enumerate() {
            visit_value_source(
                &replacement.value,
                StepExpressionDeclaration::RequestBodyReplacementValue { replacement_index },
                &mut sites,
            );
        }
    }
    for (criterion_index, criterion) in step.success_criteria.iter().enumerate() {
        if !criterion.context.is_empty() {
            push_site(
                &criterion.context,
                StepExpressionSiteMode::RequiredExpression,
                StepExpressionDeclaration::SuccessCriterionContext { criterion_index },
                &[],
                &mut sites,
            );
        }
        let mode = match criterion.resolved_type_name().as_str() {
            "simple" => StepExpressionSiteMode::SimpleCondition,
            "regex" | "jsonpath" | "xpath" => StepExpressionSiteMode::EmbeddedCondition,
            _ => continue,
        };
        push_site(
            &criterion.condition,
            mode,
            StepExpressionDeclaration::SuccessCriterionCondition { criterion_index },
            &[],
            &mut sites,
        );
    }
    for (name, output) in &step.outputs {
        let declaration = StepExpressionDeclaration::Output { name };
        match output {
            OutputValue::RuntimeExpression(text) => push_site(
                text,
                StepExpressionSiteMode::RequiredExpression,
                declaration,
                &[],
                &mut sites,
            ),
            OutputValue::Selector(selector) => {
                push_selector_context(&selector.context, declaration, &mut Vec::new(), &mut sites)
            }
        }
    }
    if let Some(text) = &step.correlation_id {
        push_site(
            text,
            StepExpressionSiteMode::LiteralOrExpression,
            StepExpressionDeclaration::CorrelationId,
            &[],
            &mut sites,
        );
    }
    Some(sites)
}

fn visit_value_source<'a>(
    value: &'a ValueSource,
    declaration: StepExpressionDeclaration<'a>,
    sites: &mut Vec<StepExpressionSite<'a>>,
) {
    let mut path = Vec::new();
    match value {
        ValueSource::Selector(selector) => {
            push_selector_context(&selector.context, declaration, &mut path, sites);
        }
        ValueSource::Literal(value) => visit_literal(value, declaration, &mut path, sites),
    }
}

fn visit_literal<'a>(
    value: &'a Value,
    declaration: StepExpressionDeclaration<'a>,
    path: &mut Vec<StepValuePathSegment<'a>>,
    sites: &mut Vec<StepExpressionSite<'a>>,
) {
    match value {
        Value::String(text) => push_site(
            text,
            StepExpressionSiteMode::LiteralOrExpression,
            declaration,
            path,
            sites,
        ),
        Value::Sequence(values) => {
            for (index, child) in values.iter().enumerate() {
                path.push(StepValuePathSegment::SequenceIndex(index));
                visit_child(child, declaration, path, sites);
                path.pop();
            }
        }
        Value::Mapping(values) => {
            for (index, (key, child)) in values.iter().enumerate() {
                path.push(StepValuePathSegment::MappingEntry {
                    index,
                    key: key.as_str(),
                });
                visit_child(child, declaration, path, sites);
                path.pop();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Tagged(_) => {}
    }
}

fn visit_child<'a>(
    child: &'a Value,
    declaration: StepExpressionDeclaration<'a>,
    path: &mut Vec<StepValuePathSegment<'a>>,
    sites: &mut Vec<StepExpressionSite<'a>>,
) {
    if let Some(context) = borrowed_selector_context(child) {
        push_selector_context(context, declaration, path, sites);
    } else {
        visit_literal(child, declaration, path, sites);
    }
}

fn push_selector_context<'a>(
    text: &'a str,
    declaration: StepExpressionDeclaration<'a>,
    path: &mut Vec<StepValuePathSegment<'a>>,
    sites: &mut Vec<StepExpressionSite<'a>>,
) {
    path.push(StepValuePathSegment::SelectorContext);
    push_site(
        text,
        StepExpressionSiteMode::RequiredExpression,
        declaration,
        path,
        sites,
    );
    path.pop();
}

fn push_site<'a>(
    text: &'a str,
    mode: StepExpressionSiteMode,
    declaration: StepExpressionDeclaration<'a>,
    path: &[StepValuePathSegment<'a>],
    sites: &mut Vec<StepExpressionSite<'a>>,
) {
    sites.push(StepExpressionSite {
        text,
        mode,
        declaration,
        nested_path: path.to_vec(),
    });
}
