//! Canonical borrowed syntax for complete Arazzo 1.1 Runtime Expressions.

use std::{fmt, ops::Range};

use pest::{error::ErrorVariant, iterators::Pair, Parser};

include!(concat!(env!("OUT_DIR"), "/runtime_expression_parser.rs"));

/// The namespace selected by a complete Runtime Expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeExpressionNamespace {
    Url,
    Method,
    StatusCode,
    Request,
    Response,
    Message,
    Inputs,
    Outputs,
    Steps,
    Workflows,
    SourceDescriptions,
    Components,
    SelfUri,
}

/// The component collection selected by a component reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentReferenceSection {
    Parameters,
    SuccessActions,
    FailureActions,
}

/// A validated JSON Pointer suffix, including its leading `#`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeExpressionPointer<'a> {
    raw: &'a str,
}

impl<'a> RuntimeExpressionPointer<'a> {
    pub fn raw(&self) -> &'a str {
        self.raw
    }
}

/// Borrowed local Step output syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepOutputReference<'a> {
    step_id: &'a str,
    output_name: &'a str,
    pointer: Option<RuntimeExpressionPointer<'a>>,
}

impl<'a> StepOutputReference<'a> {
    pub fn step_id(&self) -> &'a str {
        self.step_id
    }
    pub fn output_name(&self) -> &'a str {
        self.output_name
    }
    pub fn pointer(&self) -> Option<RuntimeExpressionPointer<'a>> {
        self.pointer
    }
}

/// Borrowed source name and complete reference ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceDescriptionReference<'a> {
    source_name: &'a str,
    reference_id: &'a str,
}

impl<'a> SourceDescriptionReference<'a> {
    pub fn source_name(&self) -> &'a str {
        self.source_name
    }
    pub fn reference_id(&self) -> &'a str {
        self.reference_id
    }
}

/// Borrowed component collection and key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentReference<'a> {
    section: ComponentReferenceSection,
    name: &'a str,
}

impl<'a> ComponentReference<'a> {
    pub fn section(&self) -> ComponentReferenceSection {
        self.section
    }
    pub fn name(&self) -> &'a str {
        self.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RuntimeExpressionForm<'a> {
    Scalar,
    Header(&'a str),
    Query(&'a str),
    Path(&'a str),
    Body(Option<RuntimeExpressionPointer<'a>>),
    Payload(Option<RuntimeExpressionPointer<'a>>),
    Named(&'a str, Option<RuntimeExpressionPointer<'a>>),
    Step(StepOutputReference<'a>),
    Workflow {
        id: &'a str,
        field: &'a str,
        name: &'a str,
        pointer: Option<RuntimeExpressionPointer<'a>>,
    },
    Source(SourceDescriptionReference<'a>),
    Component(ComponentReference<'a>),
}

/// Complete syntax whose names and pointer suffixes borrow from the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRuntimeExpression<'a> {
    raw: &'a str,
    namespace: RuntimeExpressionNamespace,
    form: RuntimeExpressionForm<'a>,
}

impl<'a> ParsedRuntimeExpression<'a> {
    pub fn raw(&self) -> &'a str {
        self.raw
    }
    pub fn namespace(&self) -> RuntimeExpressionNamespace {
        self.namespace
    }
    pub fn step_output_reference(&self) -> Option<StepOutputReference<'a>> {
        match self.form {
            RuntimeExpressionForm::Step(reference) => Some(reference),
            _ => None,
        }
    }
    pub fn source_description_reference(&self) -> Option<SourceDescriptionReference<'a>> {
        match self.form {
            RuntimeExpressionForm::Source(reference) => Some(reference),
            _ => None,
        }
    }
    pub fn component_reference(&self) -> Option<ComponentReference<'a>> {
        match self.form {
            RuntimeExpressionForm::Component(reference) => Some(reference),
            _ => None,
        }
    }
}

/// Stable categories for Runtime Expression syntax errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeExpressionErrorKind {
    Empty,
    MissingDollarPrefix,
    UnknownNamespace,
    InvalidNamespaceForm,
    InvalidIdentifier,
    MissingRequiredName,
    InvalidJsonPointerEscape,
    TrailingInput,
}

/// An owned syntax error with a half-open UTF-8 byte range in the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeExpressionError {
    pub kind: RuntimeExpressionErrorKind,
    pub byte_range: Range<usize>,
}

impl fmt::Display for RuntimeExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use RuntimeExpressionErrorKind::*;
        let reason = match self.kind {
            Empty => "input is empty",
            MissingDollarPrefix => "expected '$'",
            UnknownNamespace => "unknown namespace",
            InvalidNamespaceForm => "invalid namespace form",
            InvalidIdentifier => "invalid identifier",
            MissingRequiredName => "missing required name",
            InvalidJsonPointerEscape => "invalid JSON Pointer escape",
            TrailingInput => "trailing input",
        };
        write!(
            f,
            "invalid Runtime Expression at bytes {}..{}: {reason}",
            self.byte_range.start, self.byte_range.end
        )
    }
}

impl std::error::Error for RuntimeExpressionError {}

/// Parse exactly one complete Arazzo 1.1 Runtime Expression.
#[allow(clippy::expect_used)] // Successful grammar entry necessarily emits its pair.
pub fn parse_runtime_expression(
    input: &str,
) -> Result<ParsedRuntimeExpression<'_>, RuntimeExpressionError> {
    if input.is_empty() {
        return Err(syntax_error(RuntimeExpressionErrorKind::Empty, 0..0));
    }
    if !input.starts_with('$') {
        return Err(syntax_error(
            RuntimeExpressionErrorKind::MissingDollarPrefix,
            0..input.chars().next().map_or(0, char::len_utf8),
        ));
    }
    match RuntimeExpressionParser::parse(Rule::runtime_expression, input) {
        Ok(mut pairs) => {
            // The entry production always emits one expression pair.
            let entry = pairs.next().expect("runtime_expression entry pair");
            Ok(map_runtime_expression(entry))
        }
        Err(error) => Err(map_error(input, error)),
    }
}

fn syntax_error(
    kind: RuntimeExpressionErrorKind,
    byte_range: Range<usize>,
) -> RuntimeExpressionError {
    RuntimeExpressionError { kind, byte_range }
}

#[allow(clippy::expect_used)] // Successful unanchored entry necessarily emits its pair.
fn map_error(input: &str, error: pest::error::Error<Rule>) -> RuntimeExpressionError {
    let position = match error.location {
        pest::error::InputLocation::Pos(position) => position,
        pest::error::InputLocation::Span((start, _)) => start,
    };
    let expected = match error.variant {
        ErrorVariant::ParsingError { positives, .. } => positives,
        ErrorVariant::CustomError { .. } => Vec::new(),
    };
    // The generated escaped production fails at its opening '~'. That failure
    // is farther than the optional-pointer prefix and must win over it.
    if expected.contains(&Rule::escaped) && input[position..].starts_with('~') {
        let end = position
            + input[position..]
                .chars()
                .take(2)
                .map(char::len_utf8)
                .sum::<usize>();
        return syntax_error(
            RuntimeExpressionErrorKind::InvalidJsonPointerEscape,
            position..end,
        );
    }
    if let Ok(mut prefix) = RuntimeExpressionParser::parse(Rule::rt_expression, input) {
        let end = prefix
            .next()
            .expect("rt_expression prefix pair")
            .as_span()
            .end();
        if end < input.len() {
            return syntax_error(RuntimeExpressionErrorKind::TrailingInput, end..input.len());
        }
    }
    if position == 0 {
        return syntax_error(RuntimeExpressionErrorKind::UnknownNamespace, 1..input.len());
    }
    let requires_name = expected.iter().any(|rule| {
        matches!(
            rule,
            Rule::identifier
                | Rule::identifier_strict
                | Rule::token
                | Rule::source_reference_id
                | Rule::tchar
                | Rule::CHAR
                | Rule::ALPHA
                | Rule::DIGIT
        )
    });
    let kind = if requires_name {
        if position == input.len() {
            // ALPHA/DIGIT here are repetition continuations after a strict
            // identifier already consumed input. Its following separator or
            // keyword is missing, rather than the identifier itself.
            if expected
                .iter()
                .all(|rule| matches!(rule, Rule::ALPHA | Rule::DIGIT))
            {
                RuntimeExpressionErrorKind::InvalidNamespaceForm
            } else {
                RuntimeExpressionErrorKind::MissingRequiredName
            }
        } else {
            RuntimeExpressionErrorKind::InvalidIdentifier
        }
    } else {
        RuntimeExpressionErrorKind::InvalidNamespaceForm
    };
    syntax_error(kind, position..input.len())
}

#[allow(clippy::expect_used)] // Every caller names a required field of a successful production.
fn field<'a>(pair: &Pair<'a, Rule>, rule: Rule) -> Pair<'a, Rule> {
    pair.clone()
        .into_inner()
        .flatten()
        .find(|child| child.as_rule() == rule)
        .expect("required field in a successfully parsed reference")
}

fn pointer<'a>(pair: &Pair<'a, Rule>) -> Option<RuntimeExpressionPointer<'a>> {
    pair.clone()
        .into_inner()
        .flatten()
        .find(|child| child.as_rule() == Rule::json_pointer)
        .map(|child| {
            let span = child.as_span();
            // The ABNF places a literal '#' immediately before json_pointer.
            RuntimeExpressionPointer {
                raw: &span.get_input()[span.start() - 1..span.end()],
            }
        })
}

/// Shared pair mapping seam for the later condition parser. Terminals remain
/// grammar-owned; this consumes their spans rather than scanning expression text.
#[allow(clippy::expect_used)] // Wrapper productions necessarily contain a terminal.
pub(crate) fn map_runtime_expression(mut pair: Pair<'_, Rule>) -> ParsedRuntimeExpression<'_> {
    use RuntimeExpressionNamespace as Namespace;
    while matches!(
        pair.as_rule(),
        Rule::runtime_expression | Rule::rt_expression
    ) {
        pair = pair.into_inner().next().expect("expression terminal");
    }
    let raw = pair.as_str();
    let namespace = match pair.as_rule() {
        Rule::rt_url => Namespace::Url,
        Rule::rt_method => Namespace::Method,
        Rule::rt_status_code => Namespace::StatusCode,
        Rule::rt_self => Namespace::SelfUri,
        Rule::rt_request => Namespace::Request,
        Rule::rt_response => Namespace::Response,
        Rule::rt_message => Namespace::Message,
        Rule::rt_inputs => Namespace::Inputs,
        Rule::rt_outputs => Namespace::Outputs,
        Rule::rt_steps => Namespace::Steps,
        Rule::rt_workflows => Namespace::Workflows,
        Rule::rt_source_descriptions => Namespace::SourceDescriptions,
        Rule::rt_components => Namespace::Components,
        _ => unreachable!("namespace terminal from the shared expression production"),
    };
    let form = match pair.into_inner().next() {
        None => RuntimeExpressionForm::Scalar,
        Some(reference) => match reference.as_rule() {
            Rule::header_reference => {
                RuntimeExpressionForm::Header(field(&reference, Rule::token).as_str())
            }
            Rule::query_reference => {
                RuntimeExpressionForm::Query(field(&reference, Rule::name).as_str())
            }
            Rule::path_reference => {
                RuntimeExpressionForm::Path(field(&reference, Rule::name).as_str())
            }
            Rule::body_reference => RuntimeExpressionForm::Body(pointer(&reference)),
            Rule::payload_reference => RuntimeExpressionForm::Payload(pointer(&reference)),
            Rule::inputs_reference | Rule::outputs_reference => RuntimeExpressionForm::Named(
                field(&reference, Rule::identifier).as_str(),
                pointer(&reference),
            ),
            Rule::steps_reference => RuntimeExpressionForm::Step(StepOutputReference {
                step_id: field(&reference, Rule::step_id).as_str(),
                output_name: field(&reference, Rule::output_name).as_str(),
                pointer: pointer(&reference),
            }),
            Rule::workflows_reference => RuntimeExpressionForm::Workflow {
                id: field(&reference, Rule::workflow_id).as_str(),
                field: field(&reference, Rule::workflow_field).as_str(),
                name: field(&reference, Rule::workflow_field_name).as_str(),
                pointer: pointer(&reference),
            },
            Rule::source_reference => RuntimeExpressionForm::Source(SourceDescriptionReference {
                source_name: field(&reference, Rule::source_name).as_str(),
                reference_id: field(&reference, Rule::source_reference_id).as_str(),
            }),
            Rule::components_reference => {
                let component_type = field(&reference, Rule::component_type);
                let section = if component_type.as_str().eq_ignore_ascii_case("parameters") {
                    ComponentReferenceSection::Parameters
                } else if component_type
                    .as_str()
                    .eq_ignore_ascii_case("successActions")
                {
                    ComponentReferenceSection::SuccessActions
                } else {
                    ComponentReferenceSection::FailureActions
                };
                RuntimeExpressionForm::Component(ComponentReference {
                    section,
                    name: field(&reference, Rule::component_name).as_str(),
                })
            }
            _ => unreachable!("reference production from the shared expression grammar"),
        },
    };
    ParsedRuntimeExpression {
        raw,
        namespace,
        form,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn all_namespace_forms_and_case_policy() {
        use RuntimeExpressionNamespace::*;
        let cases = [
            ("$url", Url),
            ("$method", Method),
            ("$statusCode", StatusCode),
            ("$self", SelfUri),
            ("$request.header.X-Test", Request),
            ("$request.query.a b", Request),
            ("$request.path.id", Request),
            ("$request.body", Request),
            ("$request.body#", Request),
            ("$request.body#/a~0~1/é", Request),
            ("$response.header.Server", Response),
            ("$response.query.a", Response),
            ("$response.path.b", Response),
            ("$response.body#/items/0", Response),
            ("$message.header.X", Message),
            ("$message.payload", Message),
            ("$message.payload#/a", Message),
            ("$inputs.foo.bar", Inputs),
            ("$inputs.foo#/bar", Inputs),
            ("$outputs.foo.bar#", Outputs),
            ("$steps.s-1.outputs.foo.bar#/0/id", Steps),
            ("$workflows.w_1.inputs.foo.bar#/x", Workflows),
            ("$workflows.w-1.outputs.x", Workflows),
            (
                "$sourceDescriptions.s.ref.with.dots/$[] !~2é",
                SourceDescriptions,
            ),
            ("$components.parameters.foo.bar", Components),
            ("$components.successActions.foo", Components),
            ("$components.failureActions.foo", Components),
            ("$request.query.", Request),
            ("$request.path.", Request),
            ("$response.query.", Response),
            ("$response.path.", Response),
            ("$STATUSCODE", StatusCode),
            ("$Inputs.X", Inputs),
            ("$REQUEST.HEADER.Accept", Request),
            ("$STEPS.Step.OUTPUTS.Name", Steps),
            ("$WORKFLOWS.Flow.OUTPUTS.Name", Workflows),
            ("$COMPONENTS.SUCCESSACTIONS.Name", Components),
            ("$sourceDescriptions.s.ref~2", SourceDescriptions),
            ("$sourceDescriptions.a.b.ref", SourceDescriptions),
            (r"$request.query.x\u007D", Request),
            (r"$sourceDescriptions.s.ref\u007B", SourceDescriptions),
        ];
        for (input, namespace) in cases {
            let parsed =
                parse_runtime_expression(input).unwrap_or_else(|error| panic!("{input}: {error}"));
            assert_eq!(parsed.raw(), input);
            assert_eq!(parsed.namespace(), namespace, "{input}");
            assert_eq!(parsed.clone(), parsed);
        }
        let dotted = parse_runtime_expression("$inputs.foo.bar").unwrap();
        assert_eq!(dotted.form, RuntimeExpressionForm::Named("foo.bar", None));
        let upper = parse_runtime_expression("$Inputs.X").unwrap();
        let lower = parse_runtime_expression("$inputs.x").unwrap();
        assert_ne!(upper.form, lower.form, "name values retain their case");
    }

    #[test]
    fn exact_errors_and_utf8_ranges() {
        use RuntimeExpressionErrorKind::*;
        let cases = [
            ("", Empty, 0..0),
            ("é", MissingDollarPrefix, 0..2),
            ("{$inputs.x}", MissingDollarPrefix, 0..1),
            ("$USD", UnknownNamespace, 1..4),
            ("$env.X", UnknownNamespace, 1..6),
            ("$", UnknownNamespace, 1..1),
            ("$message.query.x", InvalidNamespaceForm, 9..16),
            ("$message.path.x", InvalidNamespaceForm, 9..15),
            ("$message.body", InvalidNamespaceForm, 9..13),
            ("$request.payload", InvalidNamespaceForm, 9..16),
            ("$response.payload", InvalidNamespaceForm, 10..17),
            ("$request.", InvalidNamespaceForm, 9..9),
            ("$inputs.", MissingRequiredName, 8..8),
            ("$outputs.", MissingRequiredName, 9..9),
            ("$request.header.", MissingRequiredName, 16..16),
            ("$sourceDescriptions.s.", MissingRequiredName, 22..22),
            ("$components.parameters.", MissingRequiredName, 23..23),
            ("$steps.", MissingRequiredName, 7..7),
            ("$steps.a", InvalidNamespaceForm, 8..8),
            ("$sourceDescriptions.s", InvalidNamespaceForm, 21..21),
            ("$inputs.é", InvalidIdentifier, 8..10),
            ("$steps.a.b.outputs.x", InvalidIdentifier, 8..20),
            ("$workflows.a.b.inputs.x", InvalidNamespaceForm, 13..23),
            ("$request.header.é", InvalidIdentifier, 16..18),
            ("$response.body.status", TrailingInput, 14..21),
            ("$urlé", TrailingInput, 4..6),
            ("$inputs.x{y", TrailingInput, 9..11),
            ("$sourceDescriptions.s.ref}tail", TrailingInput, 25..30),
            ("$request.query.x{y", TrailingInput, 16..18),
            ("$request.path.x}y", TrailingInput, 15..17),
            ("$response.body#/a~2", InvalidJsonPointerEscape, 17..19),
            ("$inputs.x#/a~2", InvalidJsonPointerEscape, 12..14),
            ("$inputs.x#/a~", InvalidJsonPointerEscape, 12..13),
            ("$inputs.x#/a~é", InvalidJsonPointerEscape, 12..15),
            ("$response.body#/é~2", InvalidJsonPointerEscape, 18..20),
        ];
        for (input, kind, byte_range) in cases {
            let error = parse_runtime_expression(input).expect_err(input);
            assert_eq!(
                error,
                RuntimeExpressionError { kind, byte_range },
                "{input}"
            );
            assert!(input.is_char_boundary(error.byte_range.start));
            assert!(input.is_char_boundary(error.byte_range.end));
        }
    }

    #[test]
    fn public_accessors_borrow_exact_syntax() {
        let input = String::from("$STEPS.Step.OUTPUTS.Foo.bar#/a~1b");
        let parsed = parse_runtime_expression(&input).unwrap();
        let reference = parsed.step_output_reference().unwrap();
        assert_eq!(reference.step_id(), "Step");
        assert_eq!(reference.output_name(), "Foo.bar");
        assert_eq!(reference.pointer().unwrap().raw(), "#/a~1b");
        assert_eq!(reference.step_id().as_ptr(), input[7..].as_ptr());
        assert!(parsed.source_description_reference().is_none());
        assert!(parsed.component_reference().is_none());
        let no_pointer = parse_runtime_expression("$steps.s.outputs.x").unwrap();
        assert!(no_pointer
            .step_output_reference()
            .unwrap()
            .pointer()
            .is_none());
        let empty_pointer = parse_runtime_expression("$steps.s.outputs.x#").unwrap();
        assert_eq!(
            empty_pointer
                .step_output_reference()
                .unwrap()
                .pointer()
                .unwrap()
                .raw(),
            "#"
        );
        let source = parse_runtime_expression("$sourceDescriptions.S.ref.with.dots/~2é").unwrap();
        let source_ref = source.source_description_reference().unwrap();
        assert_eq!(source_ref.source_name(), "S");
        assert_eq!(source_ref.reference_id(), "ref.with.dots/~2é");
        assert!(source.step_output_reference().is_none());
        assert!(source.component_reference().is_none());
        for (keyword, section) in [
            ("PARAMETERS", ComponentReferenceSection::Parameters),
            ("SUCCESSACTIONS", ComponentReferenceSection::SuccessActions),
            ("FAILUREACTIONS", ComponentReferenceSection::FailureActions),
        ] {
            let text = format!("$components.{keyword}.Foo.bar");
            let component = parse_runtime_expression(&text).unwrap();
            let reference = component.component_reference().unwrap();
            assert_eq!(reference.section(), section);
            assert_eq!(reference.name(), "Foo.bar");
            assert!(component.step_output_reference().is_none());
            assert!(component.source_description_reference().is_none());
        }
        for input in [
            "$url",
            "$inputs.x",
            "$response.body",
            "$workflows.w.outputs.x",
        ] {
            let parsed = parse_runtime_expression(input).unwrap();
            assert!(parsed.step_output_reference().is_none());
            assert!(parsed.source_description_reference().is_none());
            assert!(parsed.component_reference().is_none());
        }
    }

    #[test]
    fn stable_error_display_and_error_trait() {
        use RuntimeExpressionErrorKind::*;
        for (kind, reason) in [
            (Empty, "input is empty"),
            (MissingDollarPrefix, "expected '$'"),
            (UnknownNamespace, "unknown namespace"),
            (InvalidNamespaceForm, "invalid namespace form"),
            (InvalidIdentifier, "invalid identifier"),
            (MissingRequiredName, "missing required name"),
            (InvalidJsonPointerEscape, "invalid JSON Pointer escape"),
            (TrailingInput, "trailing input"),
        ] {
            let error = RuntimeExpressionError {
                kind,
                byte_range: 2..5,
            };
            assert_eq!(
                error.to_string(),
                format!("invalid Runtime Expression at bytes 2..5: {reason}")
            );
            let trait_error: &dyn std::error::Error = &error;
            assert!(trait_error.source().is_none());
            assert_eq!(error.clone(), error);
        }
    }

    fn strip_html(text: &str) -> String {
        let mut plain = String::new();
        let mut in_tag = false;
        for character in text.chars() {
            match character {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                _ if !in_tag => plain.push(character),
                _ => {}
            }
        }
        // The extracted ABNF contains only this HTML entity; make drift explicit.
        plain.replace("&amp;", "&")
    }

    #[test]
    fn committed_abnf_matches_vendored_spec_with_only_recorded_repair() {
        let html = include_str!("../../../spec/arazzo/v1.1.0.html");
        let section = html
            .split_once("<section id=\"runtime-expressions\">")
            .unwrap()
            .1;
        let block = section
            .split_once("<pre class=\"nohighlight\"><code>")
            .unwrap()
            .1
            .split_once("</code></pre>")
            .unwrap()
            .0;
        let expected = strip_html(block).replacen("\n  )\n", "\n    )\n", 1);
        let source = include_str!("../grammar/arazzo-1.1-runtime-expression.abnf");
        let header = "; Vendored spec/arazzo/v1.1.0.html#runtime-expressions, Section 5.9.\n; HTML tags stripped and entities decoded; only repair: indent expression closing ).\n";
        assert_eq!(source.strip_prefix(header).unwrap(), expected);
        assert!(!expected.contains("&amp;"), "unexpected undecoded entity");
    }

    #[test]
    fn every_vendored_section_5_9_1_example_expression_parses() {
        let html = include_str!("../../../spec/arazzo/v1.1.0.html");
        let section = html
            .split_once("<section id=\"examples\">")
            .unwrap()
            .1
            .split_once("<section id=\"source-description-expression-resolution\">")
            .unwrap()
            .0;
        let mut count = 0;
        for code in section.split("<code>").skip(1) {
            let code = code.split_once("</code>").unwrap().0;
            if code.starts_with('$') {
                parse_runtime_expression(code).unwrap_or_else(|error| panic!("{code}: {error}"));
                count += 1;
            } else if code.contains("{$") {
                for embedded in code.split("{$").skip(1) {
                    let expression = format!("${}", embedded.split_once('}').unwrap().0);
                    parse_runtime_expression(&expression).unwrap();
                    count += 1;
                }
            }
        }
        assert_eq!(count, 27, "all standalone and embedded example expressions");
    }
}
