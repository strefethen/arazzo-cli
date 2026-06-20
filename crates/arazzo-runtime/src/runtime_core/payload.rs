use std::borrow::Cow;

use super::*;

pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

pub(super) fn resolve_payload(value: &serde_yaml_ng::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(v) => Value::Bool(*v),
        serde_yaml_ng::Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else {
                Value::Null
            }
        }
        serde_yaml_ng::Value::String(v) => eval.resolve_value(v),
        serde_yaml_ng::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(resolve_payload(item, eval));
            }
            Value::Array(out)
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                out.insert(key, resolve_payload(v, eval));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

/// Apply Arazzo §4.6.14 replacements to a resolved request body.
///
/// Replacements are applied in array order; when two entries target the same
/// JSON Pointer or resolve to the same XPath node, the later entry wins. JSON
/// Pointer targets mutate JSON values in place. XML/text string payloads dispatch
/// targets to XPath, including common absolute XPath forms that start with `/`.
/// Warnings are already prefixed with the replacement index.
pub(super) fn apply_replacements(
    mut body: Value,
    content_type: &str,
    replacements: &[arazzo_spec::Replacement],
    eval: &ExpressionEvaluator,
) -> (Value, Vec<String>) {
    let mut warnings = Vec::<String>::new();

    for (index, replacement) in replacements.iter().enumerate() {
        let target = replacement.target.trim();
        if target.is_empty() {
            warnings.push(replacement_warning(index, "target is empty"));
            continue;
        }

        let resolved = resolve_replacement_value(&replacement.value, eval);
        if !is_xml_payload(&body, content_type) {
            if target.starts_with('/') {
                if let Err(message) = apply_json_pointer_replacement(&mut body, target, resolved) {
                    warnings.push(replacement_warning(index, &message));
                }
                continue;
            }
            warnings.push(replacement_warning(
                index,
                &format!(
                    "XPath replacement requires XML/text string payload, got {} for contentType \"{}\"",
                    json_type_name(&body),
                    content_type
                ),
            ));
            continue;
        }

        let Value::String(xml) = &body else {
            warnings.push(replacement_warning(
                index,
                &format!(
                    "XPath replacement requires XML/text string payload, got {} for contentType \"{}\"",
                    json_type_name(&body),
                    content_type
                ),
            ));
            continue;
        };

        if matches!(resolved, Value::Array(_) | Value::Object(_)) {
            warnings.push(replacement_warning(
                index,
                "structured XML replacement value serialized as JSON string",
            ));
        }

        match apply_xpath_replacement(xml, target, resolved) {
            Ok(mutated) => body = Value::String(mutated),
            Err(message) => warnings.push(replacement_warning(index, &message)),
        }
    }

    (body, warnings)
}

fn is_xml_payload(body: &Value, content_type: &str) -> bool {
    matches!(body, Value::String(_)) && !content_type.to_ascii_lowercase().contains("json")
}

fn resolve_replacement_value(value: &serde_yaml_ng::Value, eval: &ExpressionEvaluator) -> Value {
    match value {
        serde_yaml_ng::Value::String(s) => eval.resolve_value(s),
        other => resolve_payload(other, eval),
    }
}

fn replacement_warning(index: usize, message: &str) -> String {
    format!("requestBody.replacements[{index}]: {message}")
}

fn apply_json_pointer_replacement(
    root: &mut Value,
    pointer: &str,
    replacement: Value,
) -> Result<(), String> {
    if pointer.is_empty() || !pointer.starts_with('/') {
        return Err("not a JSON Pointer".to_string());
    }

    let tokens = pointer
        .trim_start_matches('/')
        .split('/')
        .map(unescape_json_pointer_token)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err("empty JSON Pointer".to_string());
    }

    let mut current = root;
    for (index, token) in tokens.iter().enumerate() {
        let is_last = index == tokens.len() - 1;
        match current {
            Value::Object(map) => {
                if is_last {
                    map.insert(token.clone(), replacement);
                    return Ok(());
                }
                current = map
                    .get_mut(token)
                    .ok_or_else(|| format!("missing intermediate object key \"{token}\""))?;
            }
            Value::Array(array) => {
                let array_index = parse_replace_array_index(token)?;
                if array_index >= array.len() {
                    return Err(format!(
                        "array index {array_index} out of range for replacement"
                    ));
                }
                if is_last {
                    array[array_index] = replacement;
                    return Ok(());
                }
                current = &mut array[array_index];
            }
            other => {
                return Err(format!(
                    "cannot descend through {} at pointer segment \"{token}\"",
                    json_type_name(other)
                ));
            }
        }
    }

    Err("empty JSON Pointer".to_string())
}

fn parse_replace_array_index(token: &str) -> Result<usize, String> {
    if token == "-" {
        return Err("`-` is append, not replace".to_string());
    }
    token
        .parse::<usize>()
        .map_err(|_| format!("array index \"{token}\" is not numeric"))
}

fn unescape_json_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn apply_xpath_replacement(xml: &str, target: &str, replacement: Value) -> Result<String, String> {
    let mut doc = uppsala::parse_bytes(xml.as_bytes())
        .map_err(|err| format!("invalid XML payload: {err}"))?;
    doc.prepare_xpath();

    let nodes = {
        let eval = uppsala::XPathEvaluator::new();
        let root = doc.root();
        match eval.evaluate(&doc, root, target) {
            Ok(uppsala::XPathValue::NodeSet(nodes)) => nodes,
            Ok(_) => return Err("xpath did not resolve to a node set".to_string()),
            Err(err) => return Err(format!("invalid XPath target: {err}")),
        }
    };

    if nodes.is_empty() {
        return Err("xpath target matched no nodes".to_string());
    }

    let replacement_text = value_to_string(&replacement);
    for node in nodes {
        let Some(kind) = doc.node_kind(node).cloned() else {
            return Err("xpath target node no longer exists".to_string());
        };
        match kind {
            uppsala::NodeKind::Element(_) => {
                for child in doc.children(node) {
                    doc.remove_child(node, child);
                }
                let text = doc.create_text(replacement_text.clone());
                doc.append_child(node, text);
            }
            uppsala::NodeKind::Attribute(name, _) => {
                let parent = doc
                    .parent(node)
                    .ok_or_else(|| "xpath attribute target has no parent element".to_string())?;
                let element = doc
                    .element_mut(parent)
                    .ok_or_else(|| "xpath attribute parent is not an element".to_string())?;
                element.set_attribute(name, Cow::Owned(replacement_text.clone()));
            }
            uppsala::NodeKind::Text(_) => {
                let Some(uppsala::NodeKind::Text(text)) = doc.node_kind_mut(node) else {
                    return Err("xpath text target no longer exists".to_string());
                };
                *text = Cow::Owned(replacement_text.clone());
            }
            uppsala::NodeKind::CData(_) => {
                let Some(uppsala::NodeKind::CData(text)) = doc.node_kind_mut(node) else {
                    return Err("xpath cdata target no longer exists".to_string());
                };
                *text = Cow::Owned(replacement_text.clone());
            }
            _ => return Err("xpath target node kind is not replaceable".to_string()),
        }
    }

    Ok(doc.to_xml())
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub(super) fn to_json_path(expr: &str) -> String {
    if let Some(path) = expr.strip_prefix("$response.body.") {
        return path.to_string();
    }
    if let Some(path) = expr.strip_prefix("$response.body") {
        return path.trim_start_matches('.').to_string();
    }
    expr.to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arazzo_spec::Replacement;

    use super::*;

    fn evaluator() -> ExpressionEvaluator {
        ExpressionEvaluator::new(EvalContext::default())
    }

    fn evaluator_with_inputs(inputs: BTreeMap<String, Value>) -> ExpressionEvaluator {
        ExpressionEvaluator::new(EvalContext {
            inputs,
            ..EvalContext::default()
        })
    }

    fn evaluator_with_steps(
        steps: BTreeMap<String, BTreeMap<String, Value>>,
    ) -> ExpressionEvaluator {
        ExpressionEvaluator::new(EvalContext {
            steps: Arc::new(steps),
            ..EvalContext::default()
        })
    }

    fn yaml(value: Value) -> serde_yaml_ng::Value {
        match serde_yaml_ng::to_value(value) {
            Ok(value) => value,
            Err(err) => panic!("converting JSON to YAML: {err}"),
        }
    }

    fn replacement(target: &str, value: serde_yaml_ng::Value) -> Replacement {
        Replacement {
            target: target.to_string(),
            value,
        }
    }

    fn apply(
        body: Value,
        replacements: Vec<Replacement>,
        eval: &ExpressionEvaluator,
    ) -> (Value, Vec<String>) {
        apply_replacements(body, "application/json", &replacements, eval)
    }

    fn assert_warning_contains(warnings: &[String], needle: &str) {
        assert!(
            warnings.iter().any(|warning| warning.contains(needle)),
            "expected warning containing {needle:?}, got: {warnings:?}"
        );
    }

    #[test]
    fn json_pointer_replaces_top_level_key() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": 1, "b": [10, 20]}),
            vec![replacement("/a", yaml(json!(99)))],
            &eval,
        );

        assert_eq!(body, json!({"a": 99, "b": [10, 20]}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn json_pointer_replaces_nested_key() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": 1, "c": {"d": "old"}}),
            vec![replacement("/c/d", yaml(json!("new")))],
            &eval,
        );

        assert_eq!(body, json!({"a": 1, "c": {"d": "new"}}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn json_pointer_replaces_array_index() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": 1, "b": [10, 20]}),
            vec![replacement("/b/1", yaml(json!(21)))],
            &eval,
        );

        assert_eq!(body, json!({"a": 1, "b": [10, 21]}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn json_pointer_unescapes_tilde_one_and_tilde_zero() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({}),
            vec![
                replacement("/a~1b", yaml(json!("slash"))),
                replacement("/a~0b", yaml(json!("tilde"))),
            ],
            &eval,
        );

        assert_eq!(body, json!({"a/b": "slash", "a~b": "tilde"}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn json_pointer_unescape_order_handles_tilde_zero_one() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a~1b": "old"}),
            vec![replacement("/a~01b", yaml(json!("new")))],
            &eval,
        );

        assert_eq!(body, json!({"a~1b": "new"}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn json_pointer_missing_intermediate_warns_and_no_change() {
        let eval = evaluator();
        let original = json!({"a": 1, "b": [10, 20]});
        let (body, warnings) = apply(
            original.clone(),
            vec![replacement("/c/d", yaml(json!("x")))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "missing intermediate");
    }

    #[test]
    fn json_pointer_array_index_out_of_range_warns_and_no_change() {
        let eval = evaluator();
        let original = json!({"b": [10, 20]});
        let (body, warnings) = apply(
            original.clone(),
            vec![replacement("/b/9", yaml(json!(21)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "out of range");
    }

    #[test]
    fn json_pointer_dash_token_is_rejected_for_replace() {
        let eval = evaluator();
        let original = json!({"b": [10, 20]});
        let (body, warnings) = apply(
            original.clone(),
            vec![replacement("/b/-", yaml(json!(30)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "`-` is append, not replace");
    }

    #[test]
    fn replacement_value_expression_resolves_via_evaluator() {
        let eval = evaluator_with_inputs(BTreeMap::from([("userId".to_string(), json!("U-7"))]));
        let (body, warnings) = apply(
            json!({}),
            vec![replacement(
                "/x",
                serde_yaml_ng::Value::String("$inputs.userId".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, json!({"x": "U-7"}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn replacement_value_interpolation_resolves() {
        let eval = evaluator_with_inputs(BTreeMap::from([("userId".to_string(), json!("U-7"))]));
        let (body, warnings) = apply(
            json!({}),
            vec![replacement(
                "/x",
                serde_yaml_ng::Value::String("literal {$inputs.userId}".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, json!({"x": "literal U-7"}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn replacement_value_structured_recurses_through_resolve_payload() {
        let eval = evaluator_with_inputs(BTreeMap::from([("userId".to_string(), json!("U-7"))]));
        let (body, warnings) = apply(
            json!({}),
            vec![replacement(
                "/payload",
                yaml(json!({
                    "id": "$inputs.userId",
                    "label": "literal {$inputs.userId}"
                })),
            )],
            &eval,
        );

        assert_eq!(
            body,
            json!({"payload": {"id": "U-7", "label": "literal U-7"}})
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn replacements_array_order_later_wins() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": 0}),
            vec![
                replacement("/a", yaml(json!(1))),
                replacement("/a", yaml(json!(2))),
            ],
            &eval,
        );

        assert_eq!(body, json!({"a": 2}));
        assert!(warnings.is_empty());
    }

    #[test]
    fn empty_replacements_returns_body_unchanged() {
        let eval = evaluator();
        let original = json!({"a": 1, "b": [10, 20]});
        let (body, warnings) = apply(original.clone(), Vec::new(), &eval);

        assert_eq!(body, original);
        assert!(warnings.is_empty());
    }

    #[test]
    fn xpath_replaces_element_text_content() {
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String("<root><CustomerId>old</CustomerId></root>".to_string()),
            "text/xml",
            &[replacement(
                "//*[local-name()='CustomerId']",
                serde_yaml_ng::Value::String("C-99".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains("<CustomerId>C-99</CustomerId>"), "{xml}");
        assert!(warnings.is_empty());
    }

    #[test]
    fn xpath_replaces_attribute_value() {
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String(r#"<root><Customer id="old"/></root>"#.to_string()),
            "text/xml",
            &[replacement(
                "//*[local-name()='Customer']/@id",
                serde_yaml_ng::Value::String("X-1".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains(r#"<Customer id="X-1"/>"#), "{xml}");
        assert!(warnings.is_empty());
    }

    #[test]
    fn xpath_preserves_namespace_declarations() {
        let eval = evaluator();
        let original = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/" xmlns:tns="urn:test"><soap:Body><tns:CustomerId>old</tns:CustomerId></soap:Body></soap:Envelope>"#;
        let (body, warnings) = apply_replacements(
            Value::String(original.to_string()),
            "text/xml",
            &[replacement(
                "//tns:CustomerId",
                serde_yaml_ng::Value::String("C-99".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains(r#"xmlns:tns="urn:test""#), "{xml}");
        assert!(
            xml.contains("<tns:CustomerId>C-99</tns:CustomerId>"),
            "{xml}"
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn xpath_no_match_warns_and_returns_original_xml() {
        let eval = evaluator();
        let original = "<root><x>old</x></root>";
        let (body, warnings) = apply_replacements(
            Value::String(original.to_string()),
            "text/xml",
            &[replacement(
                "//bogus",
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, Value::String(original.to_string()));
        assert_warning_contains(&warnings, "matched no nodes");
    }

    #[test]
    fn xpath_non_nodeset_result_warns() {
        let eval = evaluator();
        let original = "<root><x>old</x></root>";
        let (body, warnings) = apply_replacements(
            Value::String(original.to_string()),
            "text/xml",
            &[replacement(
                "count(//x)",
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, Value::String(original.to_string()));
        assert_warning_contains(&warnings, "node set");
    }

    #[test]
    fn xpath_structured_value_warns_and_uses_json_string() {
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String("<root><x>old</x></root>".to_string()),
            "text/xml",
            &[replacement("//x", yaml(json!({"a": 1})))],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains("a"), "{xml}");
        assert!(xml.contains("1"), "{xml}");
        assert_warning_contains(&warnings, "structured XML replacement value");
    }

    #[test]
    fn replacement_value_from_dependent_step_output_resolves() {
        let eval = evaluator_with_steps(BTreeMap::from([(
            "create".to_string(),
            BTreeMap::from([("id".to_string(), json!("S-1"))]),
        )]));
        let (body, warnings) = apply(
            json!({}),
            vec![replacement(
                "/id",
                serde_yaml_ng::Value::String("$steps.create.outputs.id".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, json!({"id": "S-1"}));
        assert!(warnings.is_empty());
    }
}
