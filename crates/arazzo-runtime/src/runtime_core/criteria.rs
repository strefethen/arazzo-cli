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
            let xml_text = match &context_value {
                Value::String(text) => text.clone(),
                Value::Null => match response {
                    Some(resp) => String::from_utf8_lossy(&resp.body).to_string(),
                    None => String::new(),
                },
                other => other.to_string(),
            };
            context_value = Value::String(xml_text.clone());
            is_truthy(&extract_xpath(xml_text.as_bytes(), &criterion.condition))
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
            return (extract_xpath(&resp.body, expr), Vec::new());
        }
        return (Value::Null, Vec::new());
    }

    if expr.starts_with('$') {
        return eval.evaluate_with_diagnostics(expr);
    }

    let json_path = to_json_path(expr);
    eval.evaluate_with_diagnostics(&format!("$response.body.{json_path}"))
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
}
