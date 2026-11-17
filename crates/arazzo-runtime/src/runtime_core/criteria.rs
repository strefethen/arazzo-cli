use super::*;
use arazzo_expr::BodySlot;
use std::borrow::Cow;

#[derive(Debug, Clone)]
pub(crate) struct CriterionEvaluation {
    pub type_name: String,
    pub type_version: Option<String>,
    pub condition: String,
    pub condition_result: bool,
    pub matched: bool,
    pub context_expr: String,
    pub context_value: Value,
    pub error: Option<String>,
    /// The criterion read a value that exists but cannot be read. It fails,
    /// and its step fails with this text under `RUNTIME_VALUE_UNAVAILABLE`
    /// rather than the generic criteria failure.
    pub unavailable: Option<String>,
    pub warnings: Vec<arazzo_expr::ExpressionWarning>,
}

impl CriterionEvaluation {
    pub(crate) fn unavailable_error(&self, site: &str) -> Option<RuntimeError> {
        self.unavailable
            .as_ref()
            .map(|unavailable| RuntimeError::value_unavailable(site, unavailable))
    }
}

struct Unreadable(String);

impl From<arazzo_expr::ValueUnavailable> for Unreadable {
    fn from(unavailable: arazzo_expr::ValueUnavailable) -> Self {
        Self(unavailable.to_string())
    }
}

#[cfg(test)]
pub(crate) fn evaluate_criterion(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    regex_cache: &RegexCache,
) -> bool {
    evaluate_criterion_detailed(criterion, eval, response, regex_cache).matched
}

pub(crate) fn evaluate_criterion_detailed(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    regex_cache: &RegexCache,
) -> CriterionEvaluation {
    let mut evaluation = CriterionEvaluation {
        type_name: criterion.resolved_type_name(),
        type_version: criterion.declared_type_version().map(ToString::to_string),
        condition: criterion.condition.clone(),
        condition_result: false,
        matched: false,
        context_expr: criterion.context.clone(),
        context_value: Value::Null,
        error: None,
        unavailable: None,
        warnings: Vec::new(),
    };
    match criterion_holds(criterion, eval, response, regex_cache, &mut evaluation) {
        Ok(holds) => {
            evaluation.condition_result = holds;
            evaluation.matched = holds;
        }
        Err(Unreadable(unavailable)) => {
            evaluation.error = Some(unavailable.clone());
            evaluation.unavailable = Some(unavailable);
        }
    }
    evaluation
}

fn criterion_holds(
    criterion: &SuccessCriterion,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    regex_cache: &RegexCache,
    record: &mut CriterionEvaluation,
) -> Result<bool, Unreadable> {
    let typed = matches!(record.type_name.as_str(), "regex" | "jsonpath" | "xpath");
    let prepared_condition = if typed {
        prepare_typed_condition(&criterion.condition, eval, &mut record.warnings)?
    } else {
        Ok(Cow::Borrowed(criterion.condition.as_str()))
    };
    // §5.8.11.4.3 / §5.8.11.4.5: a jsonpath condition is version-admitted,
    // budgeted and parsed in full before its context is resolved, so an
    // unsupported version, an exceeded admission limit or a syntax error is
    // reported even when the context turns out to be null, and a predicate
    // that would short-circuit cannot hide a malformed remainder.
    let prepared_jsonpath = prepared_condition.as_ref().ok().and_then(|condition| {
        (record.type_name == "jsonpath").then(|| {
            arazzo_expr::JsonPathQuery::parse(condition, criterion.declared_type_version())
        })
    });
    record.context_value = if !criterion.context.trim().is_empty() {
        let (value, warnings) = eval.evaluate_with_diagnostics(&criterion.context)?;
        record.warnings.extend(warnings);
        value
    } else if typed {
        default_criterion_context(response)?
    } else {
        Value::Null
    };

    Ok(match (prepared_condition, prepared_jsonpath) {
        (Err(message), _) => {
            record.error = Some(message);
            false
        }
        (Ok(_), Some(prepared)) => {
            let decision = prepared.and_then(|query| {
                // §5.8.11.4.3: a null or undefined context MUST fail. This is the
                // spec's own rule, not an error, so it carries no diagnostic.
                if record.context_value.is_null() {
                    Ok(false)
                } else {
                    jsonpath_condition_holds(&query, &record.context_value)
                }
            });
            match decision {
                Ok(matched) => matched,
                Err(err) => {
                    // §5.8.11.4.5: the condition fails and the error is reported.
                    // The detailed error feeds the debugger locals; the single
                    // warning rides the existing nested/enclosing trace channels.
                    let message = err.to_string();
                    record.warnings.push(arazzo_expr::ExpressionWarning {
                        expression: criterion.condition.clone(),
                        message: message.clone(),
                    });
                    record.error = Some(message);
                    false
                }
            }
        }
        (Ok(condition), None) => {
            // `prepared_jsonpath` is `Some` for every jsonpath criterion, so no
            // "jsonpath" arm belongs here.
            match record.type_name.as_str() {
                "regex" => {
                    if record.context_value.is_null() {
                        false
                    } else {
                        let context_text = value_to_string(&record.context_value);
                        match regex_cache.is_match(&condition, &context_text) {
                            Ok(matched) => matched,
                            Err(err) => {
                                record.error = Some(format!("invalid regex: {err}"));
                                false
                            }
                        }
                    }
                }
                "xpath" => {
                    // §5.8.11.4.4: a null/undefined context MUST fail the criterion,
                    // same as the jsonpath arm. No fallback to the raw response body:
                    // a null context means the expression resolved to nothing to
                    // evaluate against, and a body that cannot be read as text has
                    // already failed the criterion as unavailable.
                    if record.context_value.is_null() {
                        false
                    } else {
                        let xml_text = match &record.context_value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        };
                        record.context_value = Value::String(xml_text.clone());
                        // The declared version routes the evaluation (ac-46638): only
                        // explicit `xpath-10` reaches the engine; the omitted form and
                        // every other version fail the criterion with an error before
                        // evaluation.
                        match select_xpath(
                            xml_text.as_bytes(),
                            &condition,
                            criterion.declared_type_version(),
                        ) {
                            // §5.8.11.4.4: the criterion passes on the effective boolean
                            // value of the raw XPath result, not on the truthiness of the
                            // normalized selection value — `false` and `0` stringify to
                            // non-empty strings and must still fail.
                            Ok(selection) => selection.truthy,
                            Err(message) => {
                                record.error = Some(message);
                                false
                            }
                        }
                    }
                }
                _ => {
                    let outcome = eval.evaluate_condition_detailed(&criterion.condition);
                    record.warnings.extend(outcome.warnings);
                    match outcome.error {
                        Some(error)
                            if error.kind == arazzo_expr::ConditionErrorKind::ValueUnavailable =>
                        {
                            return Err(Unreadable(error.message));
                        }
                        Some(error) => {
                            record.error = Some(format!(
                                "invalid simple condition at byte {}: {}",
                                error.byte_offset, error.message
                            ));
                        }
                        None => {}
                    }
                    outcome.result
                }
            }
        }
    })
}

/// Typed conditions use brace-only interpolation; native dollar syntax and
/// literal braces otherwise belong to the selected condition language.
fn prepare_typed_condition<'a>(
    condition: &'a str,
    eval: &ExpressionEvaluator,
    warnings: &mut Vec<arazzo_expr::ExpressionWarning>,
) -> Result<Result<Cow<'a, str>, String>, Unreadable> {
    if !condition.contains("{$") {
        return Ok(Ok(Cow::Borrowed(condition)));
    }
    let (value, substitution_warnings) = eval.resolve_value_with_diagnostics(condition)?;
    if !substitution_warnings.is_empty() {
        let details = substitution_warnings
            .iter()
            .map(|warning| format!("{}: {}", warning.expression, warning.message))
            .collect::<Vec<_>>()
            .join("; ");
        warnings.extend(substitution_warnings);
        return Ok(Err(format!(
            "typed condition interpolation failed: {details}"
        )));
    }
    Ok(match value {
        Value::String(text) => Ok(Cow::Owned(text)),
        _ => Err("typed condition interpolation did not produce a string".to_owned()),
    })
}

pub(crate) fn evaluate_output_expression_detailed(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> Result<(Value, Vec<arazzo_expr::ExpressionWarning>), arazzo_expr::ValueUnavailable> {
    let (value, warnings, _) = evaluate_provenanced_output(
        &OutputValue::RuntimeExpression(expr.to_owned()),
        eval,
        response,
        &VarStore::default(),
        None,
    )?;
    Ok((value, warnings))
}

fn evaluate_xpath_output(
    expr: &str,
    response: Option<&Response>,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>, bool) {
    if let Some(resp) = response {
        // Bare `//…` outputs are a repository extension routed
        // explicitly to XPath 1.0 (ac-46638).
        return match select_xpath(&resp.body, expr, Some("xpath-10")) {
            Ok(selection) => {
                let warnings = if selection.match_count > 0 {
                    Vec::new()
                } else {
                    vec![arazzo_expr::ExpressionWarning {
                        expression: expr.to_string(),
                        message: "XPath matched no values".to_string(),
                    }]
                };
                match selection.value {
                    Ok(value) => (value, warnings, false),
                    Err(message) => (
                        Value::Null,
                        vec![arazzo_expr::ExpressionWarning {
                            expression: expr.to_string(),
                            message,
                        }],
                        true,
                    ),
                }
            }
            Err(message) => (
                Value::Null,
                vec![arazzo_expr::ExpressionWarning {
                    expression: expr.to_string(),
                    message,
                }],
                true,
            ),
        };
    }
    (
        Value::Null,
        vec![arazzo_expr::ExpressionWarning {
            expression: expr.to_string(),
            message: "XPath output has no response context".to_string(),
        }],
        true,
    )
}

pub(crate) fn evaluate_output_value_detailed(
    output: &OutputValue,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> Result<(Value, Vec<arazzo_expr::ExpressionWarning>), arazzo_expr::ValueUnavailable> {
    match output {
        OutputValue::RuntimeExpression(expression) => {
            evaluate_output_expression_detailed(expression, eval, response)
        }
        OutputValue::Selector(selector) => resolve_selector(selector, eval),
    }
}

pub(super) fn evaluate_provenanced_output(
    output: &OutputValue,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
    vars: &VarStore,
    producer: Option<ValueProducer<'_>>,
) -> Result<
    (Value, Vec<arazzo_expr::ExpressionWarning>, ValueProvenance),
    arazzo_expr::ValueUnavailable,
> {
    let empty = ProvenancedValues::default();
    Ok(match output {
        OutputValue::RuntimeExpression(expression) if expression.starts_with('/') => {
            if let Some(BodySlot::Unavailable { reason }) =
                response.map(super::eval_context::response_body)
            {
                return Err(arazzo_expr::ValueUnavailable {
                    expression: expression.clone(),
                    reason,
                });
            }
            let (value, warnings, unresolved) = evaluate_xpath_output(expression, response);
            let mut metadata = ValueProvenance {
                unresolved,
                ..Default::default()
            };
            if let Some(producer) = producer {
                metadata
                    .skipped
                    .push(super::value_provenance::SkippedOrigin {
                        workflow_id: producer.workflow_id.to_owned(),
                        step_id: producer.step_id.to_owned(),
                        site: producer.site.to_owned(),
                        source: expression.clone(),
                    });
            }
            (value, warnings, metadata)
        }
        OutputValue::RuntimeExpression(expression) => {
            let source = if expression.starts_with('$') {
                expression.clone()
            } else {
                format!("$response.body.{}", to_json_path(expression))
            };
            let result = eval.evaluate_with_provenance(&source)?;
            let mut metadata = vars.expression_provenance(&result, &empty, producer);
            if !expression.starts_with('$') {
                for origin in &mut metadata.skipped {
                    origin.source = expression.clone();
                }
            }
            (result.value, result.warnings, metadata)
        }
        OutputValue::Selector(selector) => {
            let result = super::payload::resolve_selector_with_provenance(selector, eval)?;
            let metadata = vars.value_provenance(&result, &empty, producer);
            (result.value, result.warnings, metadata)
        }
    })
}

fn default_criterion_context(response: Option<&Response>) -> Result<Value, Unreadable> {
    match response.map(super::eval_context::response_body) {
        None | Some(BodySlot::Absent) => Ok(Value::Null),
        Some(BodySlot::Present(value)) => Ok(value),
        Some(BodySlot::Unavailable { reason }) => Err(arazzo_expr::ValueUnavailable {
            expression: "$response.body".to_owned(),
            reason,
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use arazzo_expr::EvalContext;
    use serde_json::json;

    use super::*;

    #[test]
    fn pointer_suffix_value_can_be_consumed_by_output_evaluation() {
        let eval = ExpressionEvaluator::new(EvalContext {
            inputs: BTreeMap::from([(
                "user".to_string(),
                json!({"profile": {"email": "alice@example.com"}}),
            )])
            .into(),
            ..EvalContext::default()
        });

        let (value, warnings) =
            evaluate_output_expression_detailed("$inputs.user#/profile/email", &eval, None)
                .unwrap_or_else(|unavailable| panic!("{unavailable}"));

        assert_eq!(value, json!("alice@example.com"));
        assert!(warnings.is_empty());
    }

    fn jsonpath_criterion(context: &str, condition: &str) -> SuccessCriterion {
        SuccessCriterion {
            condition: condition.to_string(),
            context: context.to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        }
    }

    fn goessner_criterion(context: &str, condition: &str) -> SuccessCriterion {
        SuccessCriterion {
            condition: condition.to_string(),
            context: context.to_string(),
            type_: Some(arazzo_spec::CriterionType::ExpressionType(
                arazzo_spec::CriterionExpressionType {
                    type_: "jsonpath".to_string(),
                    version: "draft-goessner-dispatch-jsonpath-00".to_string(),
                    ..arazzo_spec::CriterionExpressionType::default()
                },
            )),
            ..SuccessCriterion::default()
        }
    }

    /// Every JSONPath failure the shared owner reports lands the same way:
    /// the criterion fails, the detailed error (which the debugger projects
    /// as `criterionError`) names the cause, and exactly one warning bound to
    /// the condition carries the same text into the trace channels.
    fn assert_jsonpath_failure(evaluation: &CriterionEvaluation, needle: &str) {
        assert!(!evaluation.condition_result, "{}", evaluation.condition);
        assert!(!evaluation.matched, "{}", evaluation.condition);
        let error = match &evaluation.error {
            Some(error) => error,
            None => panic!("{}: must surface an error diagnostic", evaluation.condition),
        };
        assert!(
            error.contains(needle),
            "{}: got {error}",
            evaluation.condition
        );
        assert_eq!(
            evaluation.warnings.len(),
            1,
            "{}: {:?}",
            evaluation.condition,
            evaluation.warnings
        );
        assert_eq!(evaluation.warnings[0].expression, evaluation.condition);
        assert_eq!(&evaluation.warnings[0].message, error);
    }

    /// §5.8.11.4.5: a syntactically invalid condition fails and the error is
    /// reported. The delimiter runs double as the H1 no-panic control now
    /// that they are rejected by the parser instead of sliced by a splitter.
    #[test]
    fn jsonpath_syntax_error_fails_with_error_and_one_warning() {
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!({"a": true, "b": true})),
            ..EvalContext::default()
        });
        for condition in [
            "$[?(@.a &&& @.b)]",
            "$[?(@.a ||| @.b)]",
            "$[?(@.a &&&& @.b)]",
            "$[?(&&&)]",
            "$[?(",
            "",
        ] {
            let criterion = jsonpath_criterion("$response.body", condition);
            let evaluation =
                evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());
            assert_jsonpath_failure(&evaluation, "invalid JSONPath syntax");
        }
    }

    /// Version admission and parsing run before the context is resolved, so
    /// a context that evaluates to null cannot hide an unsupported version, a
    /// resource refusal, or a syntax error behind the spec's silent
    /// null-context failure.
    #[test]
    fn jsonpath_errors_take_precedence_over_null_context() {
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(Value::Null),
            ..EvalContext::default()
        });
        let over_nested = format!("$[?{}@.a{}]", "@[?".repeat(8), "]".repeat(8));
        let cases = [
            (
                goessner_criterion("$response.body", "$.pets[*]"),
                "unsupported JSONPath version \"draft-goessner-dispatch-jsonpath-00\"",
            ),
            (
                jsonpath_criterion("$response.body", &over_nested),
                "JSONPath query bracket nesting depth limit of 8 exceeded",
            ),
            (
                jsonpath_criterion("$response.body", "$[?("),
                "invalid JSONPath syntax",
            ),
        ];
        for (criterion, needle) in cases {
            let evaluation =
                evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());
            assert!(evaluation.context_value.is_null());
            assert_jsonpath_failure(&evaluation, needle);
        }
    }

    /// A regex-function resource failure is a runtime evaluation error of the
    /// whole query (§5.8.11.4.5), not a boolean the predicate can negate, so
    /// the negated form fails and reports exactly like the plain form.
    #[test]
    fn jsonpath_operational_failure_fails_under_negation_with_error() {
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!([{"value": "a"}])),
            ..EvalContext::default()
        });
        let control = jsonpath_criterion("$response.body", "$[?match(@.value, 'a')]");
        let evaluation = evaluate_criterion_detailed(&control, &eval, None, &RegexCache::new());
        assert!(evaluation.matched, "positive control: match() works");
        assert!(evaluation.error.is_none());
        assert!(evaluation.warnings.is_empty());

        for condition in [
            "$[?match(@.value, 'a{1000000000}')]",
            "$[?!match(@.value, 'a{1000000000}')]",
        ] {
            let criterion = jsonpath_criterion("$response.body", condition);
            let evaluation =
                evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());
            assert_jsonpath_failure(&evaluation, "JSONPath evaluation failed");
        }
    }

    /// §5.8.11.4.4: "If the `context` evaluates to `null` or `undefined`, or
    /// if the XPath expression is syntactically invalid, the condition MUST
    /// evaluate to *fail*." The response body here is valid XML that matches
    /// the condition, so this test proves the null context is not silently
    /// replaced by the raw body before evaluation.
    #[test]
    fn xpath_null_context_fails_even_when_raw_body_would_match() {
        let xml = "<root><pets><pet>dog</pet></pets></root>";
        let response = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: xml.as_bytes().to_vec(),
            body_json: None,
            content_type: ContentType::Xml,
            redirects: Vec::new(),
        };
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!(xml)),
            ..EvalContext::default()
        });
        let criterion = SuccessCriterion {
            // A pointer into a string body has no match, so the explicitly
            // provided context resolves to null.
            context: "$response.body#/missing".to_string(),
            condition: "count(//pet) > 0".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
            ..SuccessCriterion::default()
        };

        let evaluation =
            evaluate_criterion_detailed(&criterion, &eval, Some(&response), &RegexCache::new());

        assert!(!evaluation.matched);
        assert!(evaluation.context_value.is_null());
    }

    /// A body that is neither JSON nor valid UTF-8 exists but cannot be read,
    /// so `$response.body` is unavailable rather than null. An explicit
    /// context over it and the default context both fail the criterion with
    /// the unavailable diagnostic; nothing lossy-decodes the raw bytes.
    #[test]
    fn xpath_explicit_context_over_non_utf8_body_fails_closed() {
        // ISO-8859-1 XML: 0xE9 is `é` in Latin-1 but is not valid UTF-8.
        let mut body = b"<root><pet>caf".to_vec();
        body.push(0xE9);
        body.extend_from_slice(b"</pet></root>");
        let response = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body,
            body_json: None,
            content_type: ContentType::Xml,
            redirects: Vec::new(),
        };
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: crate::runtime_core::eval_context::response_body(&response),
            ..EvalContext::default()
        });
        for context in ["$response.body", "$response.body#/pet", ""] {
            let criterion = SuccessCriterion {
                context: context.to_string(),
                condition: "count(//pet)".to_string(),
                type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
                ..SuccessCriterion::default()
            };

            let evaluation =
                evaluate_criterion_detailed(&criterion, &eval, Some(&response), &RegexCache::new());

            let expression = if context.is_empty() {
                "$response.body"
            } else {
                context
            };
            let unavailable = format!("{expression}: response body is not valid UTF-8 text");
            assert!(!evaluation.matched, "{context:?}");
            assert!(!evaluation.condition_result, "{context:?}");
            assert!(evaluation.warnings.is_empty(), "{context:?}");
            assert_eq!(evaluation.unavailable.as_ref(), Some(&unavailable));
            assert_eq!(evaluation.error.as_ref(), Some(&unavailable));
            let Some(error) = evaluation.unavailable_error("successCriteria[0]") else {
                panic!("{context:?}: an unavailable context must be a runtime error");
            };
            assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE");
            assert_eq!(error.message, format!("successCriteria[0]: {unavailable}"));
        }
    }

    /// §5.8.11.4.3 states the same null-context rule for JSONPath; the
    /// jsonpath arm is the model the xpath arm mirrors. A valid query over a
    /// null context is the spec's own failure, not an error, so it carries no
    /// diagnostic.
    #[test]
    fn jsonpath_null_context_fails() {
        let present_null = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(Value::Null),
            ..EvalContext::default()
        });
        let absent = ExpressionEvaluator::new(EvalContext::default());
        for (eval, context, warned) in [
            (&present_null, "$response.body", Vec::new()),
            (
                &absent,
                "$response.body#/missing",
                vec!["$response.body#/missing"],
            ),
        ] {
            let criterion = jsonpath_criterion(context, "$.pets[*]");
            let evaluation =
                evaluate_criterion_detailed(&criterion, eval, None, &RegexCache::new());

            assert!(!evaluation.matched, "{context}");
            assert!(evaluation.context_value.is_null(), "{context}");
            assert!(evaluation.error.is_none(), "{context}");
            assert_eq!(
                evaluation
                    .warnings
                    .iter()
                    .map(|warning| warning.expression.as_str())
                    .collect::<Vec<_>>(),
                warned,
                "{context}"
            );
        }
    }

    #[test]
    fn typed_criterion_fails_when_an_interpolated_response_header_is_absent() {
        let criterion = SuccessCriterion {
            condition: "^{$response.header.X-Expected}$".to_owned(),
            context: "$response.body".to_owned(),
            type_: Some(arazzo_spec::CriterionType::Name("regex".to_owned())),
            ..SuccessCriterion::default()
        };
        let absent = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!("null")),
            ..EvalContext::default()
        });
        let evaluation = evaluate_criterion_detailed(&criterion, &absent, None, &RegexCache::new());
        assert!(!evaluation.matched);
        let error = match evaluation.error.as_deref() {
            Some(error) => error,
            None => panic!("an absent interpolated header must surface an error"),
        };
        assert!(
            error.starts_with("typed condition interpolation failed:"),
            "{error}"
        );
        assert!(error.contains("$response.header.X-Expected"), "{error}");
        assert_eq!(evaluation.warnings.len(), 1);
        assert_eq!(
            evaluation.warnings[0].expression,
            "$response.header.X-Expected"
        );

        let present = ExpressionEvaluator::new(EvalContext {
            response_headers: BTreeMap::from([("X-Expected".to_owned(), "ready".to_owned())]),
            response_body: arazzo_expr::BodySlot::Present(json!("ready")),
            ..EvalContext::default()
        });
        let evaluation =
            evaluate_criterion_detailed(&criterion, &present, None, &RegexCache::new());
        assert!(evaluation.matched);
        assert!(evaluation.error.is_none());
        assert!(evaluation.warnings.is_empty());
    }

    #[test]
    fn typed_interpolation_failures_preserve_detailed_errors_and_warnings() {
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!("ready")),
            ..EvalContext::default()
        });
        for kind in ["regex", "jsonpath", "xpath"] {
            for (condition, warning_count, needle) in [
                (
                    "ready|{$inputs.absent}|{$outputs.other}",
                    2,
                    "input \"absent\" not found",
                ),
                ("{$inputs.absent} then {$", 1, "missing closing '}'"),
            ] {
                let criterion = SuccessCriterion {
                    condition: condition.to_owned(),
                    context: "$response.body".to_owned(),
                    type_: Some(arazzo_spec::CriterionType::Name(kind.to_owned())),
                    ..SuccessCriterion::default()
                };
                let evaluation =
                    evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());
                assert!(!evaluation.condition_result);
                assert!(!evaluation.matched);
                let error = match evaluation.error.as_deref() {
                    Some(error) => error,
                    None => panic!("interpolation must retain a detailed error"),
                };
                assert!(error.starts_with("typed condition interpolation failed:"));
                assert!(error.contains(needle), "{error}");
                assert_eq!(evaluation.warnings.len(), warning_count);
                if warning_count == 2 {
                    assert_eq!(evaluation.warnings[0].expression, "$inputs.absent");
                    assert_eq!(evaluation.warnings[1].expression, "$outputs.other");
                } else {
                    assert_eq!(evaluation.warnings[0].expression, condition);
                    assert!(!error.contains("not found"), "no partial lookup: {error}");
                }
            }
        }
    }

    /// §5.8.11.4.4 at the criterion decision point. The falsy boolean and
    /// zero-number cases are the regression proof: both previously passed
    /// unconditionally because the result was stringified ("false", "0")
    /// before the truthiness check.
    #[test]
    fn xpath_criteria_follow_the_spec_truth_table() {
        let xml = "<root><pets><pet>dog</pet></pets><empty/></root>";
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!(xml)),
            ..EvalContext::default()
        });
        let cache = RegexCache::new();
        let matched = |condition: &str| {
            let criterion = SuccessCriterion {
                condition: condition.to_string(),
                context: "$response.body".to_string(),
                type_: Some(arazzo_spec::CriterionType::ExpressionType(
                    arazzo_spec::CriterionExpressionType {
                        type_: "xpath".to_string(),
                        version: "xpath-10".to_string(),
                        ..arazzo_spec::CriterionExpressionType::default()
                    },
                )),
                ..SuccessCriterion::default()
            };
            evaluate_criterion_detailed(&criterion, &eval, None, &cache).matched
        };

        // boolean results decide the criterion directly
        assert!(matched("count(//pet) = 1"));
        assert!(!matched("count(//pet) > 1"));
        assert!(!matched("not(//pet)"));

        // number results: non-zero passes, zero fails
        assert!(matched("count(//pet)"));
        assert!(!matched("count(//missing)"));

        // node-set with at least one node passes even when its text is empty
        assert!(matched("//empty"));
        assert!(!matched("//missing"));
    }

    /// ac-46638 version routing at the criterion surface: an xpath criterion
    /// without an explicit `xpath-10` version fails with an error before
    /// evaluation instead of silently executing under XPath 1.0.
    #[test]
    fn xpath_criterion_without_explicit_version_10_fails_with_error() {
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!("<root><pet>dog</pet></root>")),
            ..EvalContext::default()
        });
        let criterion = SuccessCriterion {
            condition: "//pet".to_string(),
            context: "$response.body".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
            ..SuccessCriterion::default()
        };

        let evaluation = evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());

        assert!(!evaluation.matched);
        let error = match &evaluation.error {
            Some(error) => error,
            None => panic!("an omitted xpath version must surface an error"),
        };
        assert!(error.contains("xpath-31"), "got: {error}");
        assert!(error.contains("xpath-10"), "got: {error}");
    }
}

#[cfg(test)]
mod simple_condition_adapter_tests {
    use super::{evaluate_criterion_detailed, RegexCache};
    use arazzo_expr::{EvalContext, ExpressionEvaluator};
    use arazzo_spec::SuccessCriterion;
    use serde_json::json;

    #[test]
    fn detailed_simple_errors_have_exact_text_and_no_synthetic_warning() {
        let evaluator = ExpressionEvaluator::new(EvalContext {
            response_body: arazzo_expr::BodySlot::Present(json!({"nil":null})),
            ..EvalContext::default()
        });
        let cache = RegexCache::new();
        for (condition, expected) in [
            (
                "1",
                "invalid simple condition at byte 0: boolean or null required",
            ),
            (
                "$response.body.nil.x == null",
                "invalid simple condition at byte 18: property access requires an object",
            ),
            (
                "null < null",
                "invalid simple condition at byte 5: ordering requires numbers or strings",
            ),
        ] {
            let criterion = SuccessCriterion {
                condition: condition.to_owned(),
                ..SuccessCriterion::default()
            };
            let evaluation = evaluate_criterion_detailed(&criterion, &evaluator, None, &cache);
            assert_eq!(evaluation.error.as_deref(), Some(expected));
            assert!(!evaluation.matched);
            assert!(!evaluation.condition_result);
            assert!(evaluation.warnings.is_empty());
        }
        let criterion = SuccessCriterion {
            condition: "$inputs.first == null && $inputs.second == null && 1".to_owned(),
            ..SuccessCriterion::default()
        };
        let evaluation = evaluate_criterion_detailed(&criterion, &evaluator, None, &cache);
        assert!(evaluation.error.is_some());
        assert_eq!(
            evaluation
                .warnings
                .iter()
                .map(|warning| warning.expression.as_str())
                .collect::<Vec<_>>(),
            ["$inputs.first", "$inputs.second"]
        );
        let criterion = SuccessCriterion {
            condition: "false".to_owned(),
            ..SuccessCriterion::default()
        };
        let evaluation = evaluate_criterion_detailed(&criterion, &evaluator, None, &cache);
        assert!(!evaluation.matched);
        assert!(evaluation.error.is_none());
    }
}
