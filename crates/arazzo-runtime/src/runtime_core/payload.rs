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

pub(super) fn resolve_payload_detailed(
    value: &ValueSource,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    resolve_value_source(value, eval)
}

pub(super) fn resolve_value_source(
    value: &ValueSource,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    match value {
        ValueSource::Selector(selector) => resolve_selector(selector, eval),
        ValueSource::Literal(value) => resolve_literal_value(value, eval),
    }
}

fn resolve_literal_value(
    value: &serde_yaml_ng::Value,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    match value {
        serde_yaml_ng::Value::Null => (Value::Null, Vec::new()),
        serde_yaml_ng::Value::Bool(v) => (Value::Bool(*v), Vec::new()),
        serde_yaml_ng::Value::Number(v) => {
            let value = if let Some(i) = v.as_i64() {
                json!(i)
            } else if let Some(u) = v.as_u64() {
                json!(u)
            } else if let Some(f) = v.as_f64() {
                json!(f)
            } else {
                Value::Null
            };
            (value, Vec::new())
        }
        serde_yaml_ng::Value::String(v) => eval.resolve_value_with_diagnostics(v),
        serde_yaml_ng::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            let mut warnings = Vec::new();
            for item in seq {
                let (value, item_warnings) = resolve_value_source(&item.clone().into(), eval);
                out.push(value);
                warnings.extend(item_warnings);
            }
            (Value::Array(out), warnings)
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            let mut warnings = Vec::new();
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                let (value, item_warnings) = resolve_value_source(&v.clone().into(), eval);
                out.insert(key, value);
                warnings.extend(item_warnings);
            }
            (Value::Object(out), warnings)
        }
        _ => (Value::Null, Vec::new()),
    }
}

pub(super) fn resolve_selector(
    selector: &SelectorObject,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    let (context, mut warnings) = eval.evaluate_with_diagnostics(&selector.context);
    if !warnings.is_empty() {
        return (Value::Null, warnings);
    }

    let type_name = selector.type_.resolved_name();
    let version = selector.type_.declared_version();
    let selected = match type_name.as_str() {
        "jsonpath" => {
            if !matches!(
                version,
                None | Some("rfc9535") | Some("draft-goessner-dispatch-jsonpath-00")
            ) {
                Err(format!(
                    "unsupported JSONPath version {:?}",
                    version.unwrap_or_default()
                ))
            } else {
                arazzo_expr::select_json_path(&context, &selector.selector)
                    .map(|selection| (selection.value, selection.match_count))
                    .map_err(|err| err.to_string())
            }
        }
        "jsonpointer" => {
            if !matches!(version, None | Some("rfc6901")) {
                Err(format!(
                    "unsupported JSON Pointer version {:?}",
                    version.unwrap_or_default()
                ))
            } else if !selector.selector.is_empty() && !selector.selector.starts_with('/') {
                Err("JSON Pointer selector must be empty or start with '/'".to_string())
            } else {
                Ok(context
                    .pointer(&selector.selector)
                    .cloned()
                    .map_or((Value::Null, 0), |value| (value, 1)))
            }
        }
        "xpath" => match xpath_capability_gap(version) {
            Err(message) => Err(message),
            Ok(capability_gap) => {
                if let Some(gap) = capability_gap {
                    warnings.push(selector_warning(selector, &gap));
                }
                if let Value::String(xml) = &context {
                    select_xpath(xml.as_bytes(), &selector.selector)
                        .map(|selection| (selection.value, selection.match_count))
                } else {
                    Err(format!(
                        "XPath selector context must resolve to an XML string, got {}",
                        json_type_name(&context)
                    ))
                }
            }
        },
        other => Err(format!("unsupported selector type {other:?}")),
    };

    match selected {
        Ok((value, 0)) => {
            warnings.push(selector_warning(selector, "selector matched no values"));
            (value, warnings)
        }
        Ok((value, _)) => (value, warnings),
        Err(message) => {
            warnings.push(selector_warning(selector, &message));
            (Value::Null, warnings)
        }
    }
}

fn selector_warning(selector: &SelectorObject, message: &str) -> arazzo_expr::ExpressionWarning {
    arazzo_expr::ExpressionWarning {
        expression: selector.selector.clone(),
        message: message.to_string(),
    }
}

/// Decision 3 (ac-bd441): every version token from the Arazzo v1.1.0 §5.8.12.1
/// table is valid document metadata, but this runtime's engine is XPath 1.0.
/// `xpath-10` executes silently; the omitted form (which §5.8.12 defaults to
/// `xpath-31`), `xpath-20`, `xpath-30`, and `xpath-31` execute under XPath 1.0
/// semantics with exactly one capability diagnostic naming the gap. Tokens
/// outside the table are invalid metadata and do not execute.
fn xpath_capability_gap(version: Option<&str>) -> Result<Option<String>, String> {
    match version {
        Some("xpath-10") => Ok(None),
        None => Ok(Some(
            "XPath target defaults to version \"xpath-31\" (XML Path Language 3.1), which is \
             valid Arazzo metadata but executes under this runtime's XPath 1.0 engine"
                .to_string(),
        )),
        Some(declared @ ("xpath-20" | "xpath-30" | "xpath-31")) => Ok(Some(format!(
            "XPath version {declared:?} is valid Arazzo metadata but executes under this \
             runtime's XPath 1.0 engine"
        ))),
        Some(other) => Err(format!("unsupported XPath version {other:?}")),
    }
}

/// How a replacement `target` is interpreted (Arazzo v1.1.0 §5.8.15.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    JsonPointer,
    JsonPath,
    XPath,
}

/// Apply Payload Replacement Objects (Arazzo v1.1.0 §5.8.15.1) to a resolved
/// request body.
///
/// An explicit `targetSelectorType` wins. When it is omitted, interpretation
/// is keyed on the declared media type: JSON media types take JSON Pointer,
/// XML-based media types take XPath. Replacements are applied in array order;
/// when two entries resolve to the same location, the later entry wins.
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

        let (resolved, selector_warnings) = resolve_replacement_value(&replacement.value, eval);
        warnings.extend(
            selector_warnings
                .into_iter()
                .map(|warning| replacement_warning(index, &warning.to_string())),
        );

        let declared = replacement.target_selector_type.as_ref();
        let kind = match declared {
            Some(selector_type) => match selector_type.resolved_name().as_str() {
                "jsonpointer" => TargetKind::JsonPointer,
                "jsonpath" => TargetKind::JsonPath,
                "xpath" => TargetKind::XPath,
                other => {
                    warnings.push(replacement_warning(
                        index,
                        &format!(
                            "unsupported targetSelectorType {other:?}; \
                             expected jsonpath, xpath, or jsonpointer"
                        ),
                    ));
                    continue;
                }
            },
            None => infer_target_kind(&body, content_type),
        };
        let version = declared.and_then(arazzo_spec::SelectorType::declared_version);

        match kind {
            TargetKind::JsonPointer => {
                if !matches!(version, None | Some("rfc6901")) {
                    warnings.push(replacement_warning(
                        index,
                        &format!(
                            "unsupported JSON Pointer version {:?}",
                            version.unwrap_or_default()
                        ),
                    ));
                    continue;
                }
                if let Err(message) = apply_json_pointer_replacement(&mut body, target, resolved) {
                    warnings.push(replacement_warning(index, &message));
                }
            }
            TargetKind::JsonPath => {
                if !matches!(
                    version,
                    None | Some("rfc9535") | Some("draft-goessner-dispatch-jsonpath-00")
                ) {
                    warnings.push(replacement_warning(
                        index,
                        &format!(
                            "unsupported JSONPath version {:?}",
                            version.unwrap_or_default()
                        ),
                    ));
                    continue;
                }
                match arazzo_expr::resolve_json_path_pointers(&body, target) {
                    Err(err) => warnings.push(replacement_warning(index, &err.to_string())),
                    // Decision 2 (ac-bd441, project rule): a replacement
                    // applies iff its target resolves to exactly one
                    // location; zero and many both report and leave the
                    // body unchanged.
                    Ok(pointers) => match pointers.as_slice() {
                        [] => warnings.push(replacement_warning(
                            index,
                            "JSONPath target matched no locations; body unchanged",
                        )),
                        // The root selector `$` resolves to the empty RFC 6901
                        // pointer — one location, the whole document — which
                        // the segment-walking applier cannot express.
                        [pointer] if pointer.is_empty() => body = resolved,
                        [pointer] => {
                            if let Err(message) =
                                apply_json_pointer_replacement(&mut body, pointer, resolved)
                            {
                                warnings.push(replacement_warning(index, &message));
                            }
                        }
                        many => warnings.push(replacement_warning(
                            index,
                            &format!(
                                "JSONPath target matched {} locations; a replacement applies \
                                 only when it resolves to exactly one location; body unchanged",
                                many.len()
                            ),
                        )),
                    },
                }
            }
            TargetKind::XPath => {
                match xpath_capability_gap(version) {
                    Err(message) => {
                        warnings.push(replacement_warning(index, &message));
                        continue;
                    }
                    Ok(Some(gap)) => warnings.push(replacement_warning(index, &gap)),
                    Ok(None) => {}
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
        }
    }

    (body, warnings)
}

/// §5.8.15.1 inference for an omitted `targetSelectorType`: a JSON media type
/// means JSON Pointer, an XML-based media type means XPath. Media types that
/// are neither JSON nor XML-based are not settled by the specification; as a
/// project rule (ac-bd441) they keep the pre-existing behavior — string
/// payloads dispatch to XPath, structured payloads to JSON Pointer.
fn infer_target_kind(body: &Value, content_type: &str) -> TargetKind {
    let normalized = content_type.to_ascii_lowercase();
    if normalized.contains("json") {
        TargetKind::JsonPointer
    } else if normalized.contains("xml") || matches!(body, Value::String(_)) {
        TargetKind::XPath
    } else {
        TargetKind::JsonPointer
    }
}

fn resolve_replacement_value(
    value: &ValueSource,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    resolve_value_source(value, eval)
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

pub(super) fn json_type_name(value: &Value) -> &'static str {
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

    use arazzo_spec::{ExpressionType, Replacement, SelectorType};

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
            value: value.into(),
            ..Replacement::default()
        }
    }

    fn typed_replacement(
        target: &str,
        type_: SelectorType,
        value: serde_yaml_ng::Value,
    ) -> Replacement {
        Replacement {
            target: target.to_string(),
            target_selector_type: Some(type_),
            value: value.into(),
            ..Replacement::default()
        }
    }

    fn name_type(name: &str) -> SelectorType {
        SelectorType::Name(name.to_string())
    }

    fn object_type(name: &str, version: &str) -> SelectorType {
        SelectorType::ExpressionType(ExpressionType {
            type_: name.to_string(),
            version: version.to_string(),
            ..ExpressionType::default()
        })
    }

    fn capability_warnings(warnings: &[String]) -> usize {
        warnings
            .iter()
            .filter(|warning| warning.contains("XPath 1.0 engine"))
            .count()
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
        // Decision 3: an omitted targetSelectorType on an XML payload means
        // the spec's xpath-31 default, one capability diagnostic per entry.
        assert_eq!(capability_warnings(&warnings), 1, "{warnings:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
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
        assert_eq!(capability_warnings(&warnings), 1, "{warnings:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
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
        assert_eq!(capability_warnings(&warnings), 1, "{warnings:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
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

    #[test]
    fn explicit_jsonpath_applies_at_resolved_location() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": {"b": 1}}),
            vec![typed_replacement(
                "$.a.b",
                name_type("jsonpath"),
                yaml(json!(2)),
            )],
            &eval,
        );

        assert_eq!(body, json!({"a": {"b": 2}}));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn explicit_jsonpath_spec_example_filter_applies() {
        // Arazzo v1.1.0 §5.8.15.2 example target.
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"items": [
                {"sku": "ABC123", "quantity": 1},
                {"sku": "XYZ999", "quantity": 5}
            ]}),
            vec![typed_replacement(
                "$.items[?(@.sku=='ABC123')].quantity",
                name_type("jsonpath"),
                yaml(json!(3)),
            )],
            &eval,
        );

        assert_eq!(
            body,
            json!({"items": [
                {"sku": "ABC123", "quantity": 3},
                {"sku": "XYZ999", "quantity": 5}
            ]})
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn explicit_jsonpath_root_replaces_whole_body() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": 1}),
            vec![typed_replacement(
                "$",
                name_type("jsonpath"),
                yaml(json!({"b": 2})),
            )],
            &eval,
        );

        assert_eq!(body, json!({"b": 2}));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn explicit_jsonpath_zero_matches_warns_and_body_unchanged() {
        let eval = evaluator();
        let original = json!({"items": [{"sku": "A"}]});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$.items[?(@.sku=='NOPE')].sku",
                name_type("jsonpath"),
                yaml(json!("B")),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "matched no locations");
    }

    #[test]
    fn explicit_jsonpath_many_matches_warns_and_body_unchanged() {
        let eval = evaluator();
        let original = json!({"items": [{"q": 1}, {"q": 2}]});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$.items[*].q",
                name_type("jsonpath"),
                yaml(json!(9)),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "matched 2 locations");
    }

    #[test]
    fn explicit_jsonpointer_behaves_like_omitted_field() {
        let eval = evaluator();
        let original = json!({"a": 1});
        let (explicit_body, explicit_warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "/a",
                name_type("jsonpointer"),
                yaml(json!(2)),
            )],
            &eval,
        );
        let (omitted_body, omitted_warnings) =
            apply(original, vec![replacement("/a", yaml(json!(2)))], &eval);

        assert_eq!(explicit_body, omitted_body);
        assert_eq!(explicit_warnings, omitted_warnings);
        assert_eq!(explicit_body, json!({"a": 2}));
    }

    #[test]
    fn expression_type_object_xpath_30_is_honored_with_capability_diagnostic() {
        // §5.8.15.2's own Expression Type Object shape.
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String("<root><x>old</x></root>".to_string()),
            "application/xml",
            &[typed_replacement(
                "//x",
                object_type("xpath", "xpath-30"),
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains("<x>new</x>"), "{xml}");
        assert_eq!(capability_warnings(&warnings), 1, "{warnings:?}");
    }

    #[test]
    fn xpath_10_executes_without_capability_diagnostic() {
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String("<root><x>old</x></root>".to_string()),
            "application/xml",
            &[typed_replacement(
                "//x",
                object_type("xpath", "xpath-10"),
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains("<x>new</x>"), "{xml}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn xpath_capability_diagnostic_fires_once_per_accepted_version_gap() {
        // Decision 3: plain `xpath`, xpath-20/-30/-31, and the omitted field
        // each execute under XPath 1.0 with exactly one capability
        // diagnostic; xpath-10 is covered by the silent test above.
        let eval = evaluator();
        let gapped = [
            Some(name_type("xpath")),
            Some(object_type("xpath", "xpath-20")),
            Some(object_type("xpath", "xpath-30")),
            Some(object_type("xpath", "xpath-31")),
            None,
        ];
        for type_ in gapped {
            let entry = Replacement {
                target: "//x".to_string(),
                target_selector_type: type_.clone(),
                value: serde_yaml_ng::Value::String("new".to_string()).into(),
                ..Replacement::default()
            };
            let (body, warnings) = apply_replacements(
                Value::String("<root><x>old</x></root>".to_string()),
                "application/xml",
                &[entry],
                &eval,
            );

            let xml = body.as_str().unwrap_or_default();
            assert!(xml.contains("<x>new</x>"), "{type_:?}: {xml}");
            assert_eq!(capability_warnings(&warnings), 1, "{type_:?}: {warnings:?}");
            assert_eq!(warnings.len(), 1, "{type_:?}: {warnings:?}");
        }
    }

    #[test]
    fn unsupported_xpath_version_token_warns_and_does_not_execute() {
        let eval = evaluator();
        let original = Value::String("<root><x>old</x></root>".to_string());
        let (body, warnings) = apply_replacements(
            original.clone(),
            "application/xml",
            &[typed_replacement(
                "//x",
                object_type("xpath", "10"),
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "unsupported XPath version");
        assert_eq!(capability_warnings(&warnings), 0, "{warnings:?}");
    }

    #[test]
    fn unsupported_jsonpath_version_token_warns_and_body_unchanged() {
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$.a",
                object_type("jsonpath", "rfc6901"),
                yaml(json!(2)),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "unsupported JSONPath version");
    }

    #[test]
    fn unsupported_target_selector_type_name_warns_and_body_unchanged() {
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement("/a", name_type("regex"), yaml(json!(2)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "unsupported targetSelectorType");
    }

    #[test]
    fn omitted_type_on_json_media_routes_non_slash_target_to_json_pointer() {
        // §5.8.15.1 routing: application/json means JSON Pointer, so a
        // JSONPath-looking target is a JSON Pointer error — not the old
        // "XPath replacement requires XML/text string payload" message.
        let eval = evaluator();
        let original = json!({"a": {"b": 1}});
        let (body, warnings) = apply(
            original.clone(),
            vec![replacement("$.a.b", yaml(json!(2)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "not a JSON Pointer");
        assert!(
            warnings.iter().all(|warning| !warning.contains("XPath")),
            "{warnings:?}"
        );
    }

    #[test]
    fn omitted_type_on_xml_media_with_structured_body_warns_xpath() {
        // Media-type-keyed routing: an XML content type dispatches to XPath
        // even when the resolved payload is structured JSON.
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply_replacements(
            original.clone(),
            "application/xml",
            &[replacement("/a", yaml(json!(2)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(
            &warnings,
            "XPath replacement requires XML/text string payload",
        );
    }
}
