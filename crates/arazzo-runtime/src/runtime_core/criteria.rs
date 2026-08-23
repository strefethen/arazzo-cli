use super::*;

pub(crate) struct RegexCache {
    cache: Mutex<HashMap<String, Regex>>,
}

impl RegexCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Compile a regex (or return cached) and test whether it matches `text`.
    ///
    /// The lock is held for the duration of the match, but matching takes
    /// nanoseconds so contention is negligible.
    pub(crate) fn is_match(&self, pattern: &str, text: &str) -> Result<bool, regex::Error> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(re) = cache.get(pattern) {
            return Ok(re.is_match(text));
        }
        let re = Regex::new(pattern)?;
        let result = re.is_match(text);
        cache.insert(pattern.to_string(), re);
        Ok(result)
    }
}

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
    pub warnings: Vec<arazzo_expr::ExpressionWarning>,
}

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
    let type_name = criterion.resolved_type_name();
    let mut expr_warnings = Vec::new();
    let mut context_value = if criterion.context.trim().is_empty() {
        default_criterion_context(response)
    } else {
        let (val, warnings) = eval.evaluate_with_diagnostics(&criterion.context);
        expr_warnings = warnings;
        val
    };
    let mut error = None;

    let condition_result = match type_name.as_str() {
        "regex" => {
            let context_text = value_to_string(&context_value);
            match regex_cache.is_match(&criterion.condition, &context_text) {
                Ok(matched) => matched,
                Err(err) => {
                    error = Some(format!("invalid regex: {err}"));
                    false
                }
            }
        }
        "jsonpath" => {
            if context_value.is_null() {
                false
            } else {
                match evaluate_jsonpath_condition(eval, &context_value, &criterion.condition) {
                    JsonPathOutcome::Matched(result) => result,
                    JsonPathOutcome::Unsupported(reason) => {
                        error = Some(format!("unsupported JSONPath: {reason}"));
                        false
                    }
                }
            }
        }
        "xpath" => {
            // §5.8.11.4.4: a null/undefined context MUST fail the criterion,
            // same as the jsonpath arm. No fallback to the raw response body:
            // eval_context resolves $response.body to the raw text for
            // XML/non-JSON responses (strict UTF-8 — an undecodable body
            // yields null and fails closed here), so a null context means the
            // expression resolved to nothing to evaluate against.
            if context_value.is_null() {
                false
            } else {
                let xml_text = match &context_value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                context_value = Value::String(xml_text.clone());
                match select_xpath(xml_text.as_bytes(), &criterion.condition) {
                    // §5.8.11.4.4: the criterion passes on the effective boolean
                    // value of the raw XPath result, not on the truthiness of the
                    // normalized selection value — `false` and `0` stringify to
                    // non-empty strings and must still fail.
                    Ok(selection) => selection.truthy,
                    Err(message) => {
                        error = Some(message);
                        false
                    }
                }
            }
        }
        _ => {
            let (result, cond_warnings) =
                eval.evaluate_condition_with_diagnostics(&criterion.condition);
            expr_warnings.extend(cond_warnings);
            result
        }
    };

    CriterionEvaluation {
        type_name,
        type_version: criterion.declared_type_version().map(ToString::to_string),
        condition: criterion.condition.clone(),
        condition_result,
        matched: condition_result,
        context_expr: criterion.context.clone(),
        context_value,
        error,
        warnings: expr_warnings,
    }
}

pub(crate) fn evaluate_output_expression(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> Value {
    evaluate_output_expression_detailed(expr, eval, response).0
}

pub(crate) fn evaluate_output_expression_detailed(
    expr: &str,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    if expr.starts_with('/') {
        if let Some(resp) = response {
            return match select_xpath(&resp.body, expr) {
                Ok(selection) if selection.match_count > 0 => (selection.value, Vec::new()),
                Ok(selection) => (
                    selection.value,
                    vec![arazzo_expr::ExpressionWarning {
                        expression: expr.to_string(),
                        message: "XPath matched no values".to_string(),
                    }],
                ),
                Err(message) => (
                    Value::Null,
                    vec![arazzo_expr::ExpressionWarning {
                        expression: expr.to_string(),
                        message,
                    }],
                ),
            };
        }
        return (
            Value::Null,
            vec![arazzo_expr::ExpressionWarning {
                expression: expr.to_string(),
                message: "XPath output has no response context".to_string(),
            }],
        );
    }

    if expr.starts_with('$') {
        return eval.evaluate_with_diagnostics(expr);
    }

    let json_path = to_json_path(expr);
    eval.evaluate_with_diagnostics(&format!("$response.body.{json_path}"))
}

pub(crate) fn evaluate_output_value_detailed(
    output: &OutputValue,
    eval: &ExpressionEvaluator,
    response: Option<&Response>,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    match output {
        OutputValue::RuntimeExpression(expression) => {
            evaluate_output_expression_detailed(expression, eval, response)
        }
        OutputValue::Selector(selector) => resolve_selector(selector, eval),
    }
}

fn default_criterion_context(response: Option<&Response>) -> Value {
    match response {
        Some(resp) => {
            if let Some(json) = &resp.body_json {
                json.clone()
            } else if !resp.body.is_empty() {
                Value::String(String::from_utf8_lossy(&resp.body).to_string())
            } else {
                Value::Null
            }
        }
        None => Value::Null,
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
            )]),
            ..EvalContext::default()
        });

        let (value, warnings) =
            evaluate_output_expression_detailed("$inputs.user#/profile/email", &eval, None);

        assert_eq!(value, json!("alice@example.com"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn unsupported_jsonpath_criterion_surfaces_error_diagnostic() {
        let criterion = SuccessCriterion {
            condition: "$..foo".to_string(),
            context: "$response.body".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        };
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: Some(json!({"foo": 1})),
            ..EvalContext::default()
        });

        let evaluation = evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());

        assert!(!evaluation.matched);
        let error = match &evaluation.error {
            Some(error) => error,
            None => panic!("unsupported JSONPath must surface an error diagnostic"),
        };
        assert!(!error.is_empty());
        assert!(error.contains("unsupported JSONPath"), "got: {error}");
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
            response_body: Some(json!(xml)),
            ..EvalContext::default()
        });
        let criterion = SuccessCriterion {
            // A dot path into a string body has no match, so the explicitly
            // provided context resolves to null.
            context: "$response.body.missing".to_string(),
            condition: "count(//pet) > 0".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
            ..SuccessCriterion::default()
        };

        let evaluation =
            evaluate_criterion_detailed(&criterion, &eval, Some(&response), &RegexCache::new());

        assert!(!evaluation.matched);
        assert!(evaluation.context_value.is_null());
    }

    /// A body that is not valid UTF-8 never reaches `$response.body`
    /// (eval_context decodes strictly), so an explicit context over it
    /// resolves to null and the criterion fails closed per §5.8.11.4.4 —
    /// it must not fall back to lossy-decoding the raw bytes.
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
        // Mirror engine wiring (state.rs eval_context): strict UTF-8
        // decoding fails, so no $response.body value exists.
        let decoded = String::from_utf8(response.body.clone()).ok();
        assert!(decoded.is_none());
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: decoded.map(Value::String),
            ..EvalContext::default()
        });
        let criterion = SuccessCriterion {
            context: "$response.body".to_string(),
            condition: "count(//pet)".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
            ..SuccessCriterion::default()
        };

        let evaluation =
            evaluate_criterion_detailed(&criterion, &eval, Some(&response), &RegexCache::new());

        assert!(!evaluation.matched);
        assert!(evaluation.context_value.is_null());
    }

    /// §5.8.11.4.3 states the same null-context rule for JSONPath; the
    /// jsonpath arm is the model the xpath arm mirrors.
    #[test]
    fn jsonpath_null_context_fails() {
        let criterion = SuccessCriterion {
            context: "$response.body.missing".to_string(),
            condition: "$.pets[*]".to_string(),
            type_: Some(arazzo_spec::CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        };
        let eval = ExpressionEvaluator::new(EvalContext::default());

        let evaluation = evaluate_criterion_detailed(&criterion, &eval, None, &RegexCache::new());

        assert!(!evaluation.matched);
        assert!(evaluation.context_value.is_null());
    }

    /// §5.8.11.4.4 at the criterion decision point. The falsy boolean and
    /// zero-number cases are the regression proof: both previously passed
    /// unconditionally because the result was stringified ("false", "0")
    /// before the truthiness check.
    #[test]
    fn xpath_criteria_follow_the_spec_truth_table() {
        let xml = "<root><pets><pet>dog</pet></pets><empty/></root>";
        let eval = ExpressionEvaluator::new(EvalContext {
            response_body: Some(json!(xml)),
            ..EvalContext::default()
        });
        let cache = RegexCache::new();
        let matched = |condition: &str| {
            let criterion = SuccessCriterion {
                condition: condition.to_string(),
                context: "$response.body".to_string(),
                type_: Some(arazzo_spec::CriterionType::Name("xpath".to_string())),
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
}
