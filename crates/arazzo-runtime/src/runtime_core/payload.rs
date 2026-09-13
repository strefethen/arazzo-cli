use arazzo_expr::JsonPathQuery;

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

/// Resolve a value for ordinary readers: the projected value and warnings.
pub(super) fn resolve_value_source(
    value: &ValueSource,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    resolve_value_checked(value, eval).into_parts()
}

/// Value-resolution outcome with private hard-failure provenance.
///
/// Ordinary readers see only the projected `value` and `warnings`. Payload
/// replacement additionally needs to know whether a JSONPath Selector Object
/// anywhere inside the value failed hard — an unsupported version, invalid
/// syntax, an admission limit, an evaluation failure, or a context that did
/// not resolve — because the `Null` such a failure projects is not a selected
/// value and must not be written into a request body as one. Zero matches
/// and a legitimately selected `null` are ordinary selections, not failures.
#[derive(Debug)]
struct Resolution {
    value: Value,
    warnings: Vec<arazzo_expr::ExpressionWarning>,
    jsonpath_failed: bool,
}

impl Resolution {
    fn sound(value: Value, warnings: Vec<arazzo_expr::ExpressionWarning>) -> Self {
        Self {
            value,
            warnings,
            jsonpath_failed: false,
        }
    }

    fn jsonpath_failure(warnings: Vec<arazzo_expr::ExpressionWarning>) -> Self {
        Self {
            value: Value::Null,
            warnings,
            jsonpath_failed: true,
        }
    }

    fn into_parts(self) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
        (self.value, self.warnings)
    }
}

fn resolve_value_checked(value: &ValueSource, eval: &ExpressionEvaluator) -> Resolution {
    match value {
        ValueSource::Selector(selector) => resolve_selector_checked(selector, eval),
        ValueSource::Literal(value) => resolve_literal_checked(value, eval),
    }
}

fn resolve_literal_checked(value: &serde_yaml_ng::Value, eval: &ExpressionEvaluator) -> Resolution {
    match value {
        serde_yaml_ng::Value::Null => Resolution::sound(Value::Null, Vec::new()),
        serde_yaml_ng::Value::Bool(v) => Resolution::sound(Value::Bool(*v), Vec::new()),
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
            Resolution::sound(value, Vec::new())
        }
        serde_yaml_ng::Value::String(v) => {
            let (value, warnings) = eval.resolve_value_with_diagnostics(v);
            Resolution::sound(value, warnings)
        }
        // Nested containers are not parsed here: the existing `ValueSource`
        // model decides whether each item is a Selector Object, and this walk
        // only carries every item's outcome upward.
        serde_yaml_ng::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            let mut warnings = Vec::new();
            let mut jsonpath_failed = false;
            for item in seq {
                let item = resolve_value_checked(&item.clone().into(), eval);
                jsonpath_failed |= item.jsonpath_failed;
                out.push(item.value);
                warnings.extend(item.warnings);
            }
            Resolution {
                value: Value::Array(out),
                warnings,
                jsonpath_failed,
            }
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            let mut warnings = Vec::new();
            let mut jsonpath_failed = false;
            for (k, v) in map {
                let key = k.as_str().unwrap_or_default().to_string();
                let item = resolve_value_checked(&v.clone().into(), eval);
                jsonpath_failed |= item.jsonpath_failed;
                out.insert(key, item.value);
                warnings.extend(item.warnings);
            }
            Resolution {
                value: Value::Object(out),
                warnings,
                jsonpath_failed,
            }
        }
        _ => Resolution::sound(Value::Null, Vec::new()),
    }
}

/// Resolve a Selector Object for ordinary readers: the projected value and
/// warnings. A selector that fails reads as `Null` with one warning; zero
/// matches read as `Null` with the no-match warning.
pub(super) fn resolve_selector(
    selector: &SelectorObject,
    eval: &ExpressionEvaluator,
) -> (Value, Vec<arazzo_expr::ExpressionWarning>) {
    resolve_selector_checked(selector, eval).into_parts()
}

fn resolve_selector_checked(selector: &SelectorObject, eval: &ExpressionEvaluator) -> Resolution {
    let type_name = selector.type_.resolved_name();
    let version = selector.type_.declared_version();

    // A jsonpath selector is admitted through the shared owner — declared
    // version, resource budget and complete syntax — before its context is
    // resolved, so an unsupported version or a malformed expression is
    // reported even when the context turns out to be missing. Goessner is
    // not a supported version; the owner rejects it like any other.
    let jsonpath = match type_name.as_str() {
        "jsonpath" => match JsonPathQuery::parse(&selector.selector, version) {
            Ok(query) => Some(query),
            Err(error) => {
                return Resolution::jsonpath_failure(vec![selector_warning(
                    selector,
                    &error.to_string(),
                )]);
            }
        },
        _ => None,
    };

    let (context, mut warnings) = eval.evaluate_with_diagnostics(&selector.context);
    if !warnings.is_empty() {
        // The projection is unchanged: a context that did not resolve reads
        // as `Null`. For a JSONPath selector that is failed resolution, not a
        // selected value, so replacement use must not write it.
        return Resolution {
            value: Value::Null,
            warnings,
            jsonpath_failed: jsonpath.is_some(),
        };
    }

    let selected = if let Some(query) = jsonpath {
        match query.select(&context) {
            Ok(selection) => Ok((selection.value, selection.match_count)),
            Err(error) => {
                warnings.push(selector_warning(selector, &error.to_string()));
                return Resolution::jsonpath_failure(warnings);
            }
        }
    } else {
        resolve_non_jsonpath_selector(selector, &context, version)
    };

    match selected {
        Ok((value, 0)) => {
            warnings.push(selector_warning(selector, "selector matched no values"));
            Resolution::sound(value, warnings)
        }
        Ok((value, _)) => Resolution::sound(value, warnings),
        Err(message) => {
            warnings.push(selector_warning(selector, &message));
            Resolution::sound(Value::Null, warnings)
        }
    }
}

/// The JSON Pointer and XPath selector arms, unchanged by the JSONPath
/// cutover: their errors are ordinary selector warnings, not hard failures.
fn resolve_non_jsonpath_selector(
    selector: &SelectorObject,
    context: &Value,
    version: Option<&str>,
) -> Result<(Value, usize), String> {
    match selector.type_.resolved_name().as_str() {
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
        "xpath" => {
            // ac-46638 version routing: rejection precedes every other
            // check so a bad version is reported for any context shape.
            if let Some(rejection) = xpath_version_rejection(version) {
                Err(rejection)
            } else if let Value::String(xml) = context {
                select_xpath(xml.as_bytes(), &selector.selector, version).and_then(|selection| {
                    selection.value.map(|value| (value, selection.match_count))
                })
            } else {
                Err(format!(
                    "XPath selector context must resolve to an XML string, got {}",
                    json_type_name(context)
                ))
            }
        }
        other => Err(format!("unsupported selector type {other:?}")),
    }
}

fn selector_warning(selector: &SelectorObject, message: &str) -> arazzo_expr::ExpressionWarning {
    arazzo_expr::ExpressionWarning {
        expression: selector.selector.clone(),
        message: message.to_string(),
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

        // Value first: a hard JSONPath failure anywhere inside the value
        // projected `Null`, which is not a selected value. Nothing is written
        // and the target is not evaluated; later replacements continue.
        let resolution = resolve_value_checked(&replacement.value, eval);
        warnings.extend(
            resolution
                .warnings
                .iter()
                .map(|warning| replacement_warning(index, &warning.to_string())),
        );
        if resolution.jsonpath_failed {
            warnings.push(replacement_warning(
                index,
                "value did not resolve; replacement skipped and body unchanged",
            ));
            continue;
        }
        let resolved = resolution.value;

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
                // The shared owner admits the declared version, the resource
                // budget and the complete syntax before the current body is
                // queried; the pointers come from that one located query.
                let query = match JsonPathQuery::parse(target, version) {
                    Ok(query) => query,
                    Err(error) => {
                        warnings.push(replacement_warning(index, &error.to_string()));
                        continue;
                    }
                };
                let pointers = match query.query(&body) {
                    Ok(matches) => matches
                        .into_iter()
                        .map(|found| found.pointer)
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        warnings.push(replacement_warning(index, &error.to_string()));
                        continue;
                    }
                };
                // Decision 2 (ac-bd441, project rule): a replacement applies
                // iff its target resolves to exactly one location; zero and
                // many — repeated occurrences included — both report and
                // leave the body unchanged.
                match pointers.as_slice() {
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
                }
            }
            TargetKind::XPath => {
                // ac-46638 version routing: an unsupported version leaves
                // the body unchanged with exactly one warning.
                if let Some(rejection) = xpath_version_rejection(version) {
                    warnings.push(replacement_warning(index, &rejection));
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

                let replacement_text = value_to_string(&resolved);
                match replace_xpath(xml, target, version, &replacement_text) {
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

fn replacement_warning(index: usize, message: &str) -> String {
    format!("requestBody.replacements[{index}]: {message}")
}

fn apply_json_pointer_replacement(
    root: &mut Value,
    pointer: &str,
    replacement: Value,
) -> Result<(), String> {
    // RFC 6901 §3: a non-empty pointer is a sequence of reference tokens,
    // each introduced by exactly one "/", so only the first separator is
    // dropped. A token may be empty — "//child" names member "child" of the
    // member named "" — and stripping every leading "/" would rewrite it to
    // "/child" and overwrite a root-level sibling instead.
    let Some(reference) = pointer.strip_prefix('/') else {
        return Err("not a JSON Pointer".to_string());
    };
    let tokens = reference
        .split('/')
        .map(unescape_json_pointer_token)
        .collect::<Vec<_>>();

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
    fn explicit_null_and_empty_replacement_values_reach_json_pointer_resolution() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"nullValue": "old", "emptyValue": "old"}),
            vec![
                replacement("/nullValue", serde_yaml_ng::Value::Null),
                replacement("/emptyValue", serde_yaml_ng::Value::String(String::new())),
            ],
            &eval,
        );

        assert_eq!(body, json!({"nullValue": null, "emptyValue": ""}));
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
            &[typed_replacement(
                "//*[local-name()='CustomerId']",
                object_type("xpath", "xpath-10"),
                serde_yaml_ng::Value::String("C-99".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains("<CustomerId>C-99</CustomerId>"), "{xml}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn xpath_replaces_attribute_value() {
        let eval = evaluator();
        let (body, warnings) = apply_replacements(
            Value::String(r#"<root><Customer id="old"/></root>"#.to_string()),
            "text/xml",
            &[typed_replacement(
                "//*[local-name()='Customer']/@id",
                object_type("xpath", "xpath-10"),
                serde_yaml_ng::Value::String("X-1".to_string()),
            )],
            &eval,
        );

        let xml = body.as_str().unwrap_or_default();
        assert!(xml.contains(r#"<Customer id="X-1"/>"#), "{xml}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn xpath_preserves_namespace_declarations() {
        let eval = evaluator();
        let original = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/" xmlns:tns="urn:test"><soap:Body><tns:CustomerId>old</tns:CustomerId></soap:Body></soap:Envelope>"#;
        let (body, warnings) = apply_replacements(
            Value::String(original.to_string()),
            "text/xml",
            &[typed_replacement(
                "//tns:CustomerId",
                object_type("xpath", "xpath-10"),
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
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn xpath_no_match_warns_and_returns_original_xml() {
        let eval = evaluator();
        let original = "<root><x>old</x></root>";
        let (body, warnings) = apply_replacements(
            Value::String(original.to_string()),
            "text/xml",
            &[typed_replacement(
                "//bogus",
                object_type("xpath", "xpath-10"),
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
            &[typed_replacement(
                "count(//x)",
                object_type("xpath", "xpath-10"),
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
            &[typed_replacement(
                "//x",
                object_type("xpath", "xpath-10"),
                yaml(json!({"a": 1})),
            )],
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
    fn explicit_jsonpath_gjson_filter_is_invalid_syntax_and_body_unchanged() {
        // A GJSON dot-form filter is not RFC 9535 syntax: the shared owner
        // rejects the target at parse time — whether it would have matched
        // one item or two — with exactly one warning and an untouched body.
        let eval = evaluator();
        for original in [
            json!({"items": [{"sku": "A", "q": 1}, {"sku": "A", "q": 2}]}),
            json!({"items": [{"sku": "A", "q": 1}]}),
        ] {
            let (body, warnings) = apply(
                original.clone(),
                vec![typed_replacement(
                    "$.items.#(sku==\"A\").q",
                    name_type("jsonpath"),
                    yaml(json!(99)),
                )],
                &eval,
            );

            assert_eq!(body, original);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert_warning_contains(&warnings, "invalid JSONPath syntax");
        }
    }

    #[test]
    fn explicit_jsonpath_descent_and_slice_targets_resolve_one_location() {
        let eval = evaluator();
        let (body, warnings) = apply(
            json!({"a": {"deep": {"leaf": 1}}, "items": [10, 20, 30]}),
            vec![
                typed_replacement("$..leaf", name_type("jsonpath"), yaml(json!(2))),
                typed_replacement("$.items[1:2]", name_type("jsonpath"), yaml(json!(21))),
            ],
            &eval,
        );

        assert_eq!(
            body,
            json!({"a": {"deep": {"leaf": 2}}, "items": [10, 21, 30]})
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn explicit_jsonpath_duplicate_occurrences_count_as_many_locations() {
        let eval = evaluator();
        let original = json!({"items": [{"q": 1}]});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$.items[0,0].q",
                name_type("jsonpath"),
                yaml(json!(9)),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_warning_contains(&warnings, "matched 2 locations");
    }

    #[test]
    fn empty_name_pointer_segment_replaces_nested_child_not_root_sibling() {
        // $['']['child'] resolves to the RFC 6901 pointer "//child": member
        // "child" of the member named "". Dropping every leading "/" used to
        // rewrite it to "/child" and overwrite the root-level sibling.
        let eval = evaluator();
        let original = json!({"": {"child": "old"}, "child": "sibling"});
        let expected = json!({"": {"child": "new"}, "child": "sibling"});

        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$['']['child']",
                name_type("jsonpath"),
                yaml(json!("new")),
            )],
            &eval,
        );
        assert_eq!(body, expected);
        assert!(warnings.is_empty(), "{warnings:?}");

        // The same pointer written directly through the JSON Pointer arm.
        let (body, warnings) = apply(
            original,
            vec![replacement("//child", yaml(json!("new")))],
            &eval,
        );
        assert_eq!(body, expected);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    fn jsonpath_selector_yaml(context: &str, selector: &str) -> serde_yaml_ng::Value {
        yaml(json!({"context": context, "selector": selector, "type": "jsonpath"}))
    }

    #[test]
    fn hard_jsonpath_failure_in_a_nested_replacement_value_skips_only_that_replacement() {
        let eval = evaluator_with_inputs(BTreeMap::from([(
            "document".to_string(),
            json!({"value": null, "n": 7}),
        )]));
        let oversized = format!("${}", ".a".repeat(129));
        let (body, warnings) = apply(
            json!({
                "first": "old",
                "nested": "old",
                "context": "old",
                "null": "old",
                "zero": "old",
                "last": "old"
            }),
            vec![
                replacement("/first", yaml(json!("new"))),
                // A resource failure inside a map inside an array: the value
                // is a hard failure, so nothing is written at /nested.
                replacement(
                    "/nested",
                    yaml(
                        json!([{"inner": jsonpath_selector_yaml("$inputs.document", &oversized)}]),
                    ),
                ),
                // A valid query whose context is missing is failed resolution
                // for replacement use.
                replacement("/context", jsonpath_selector_yaml("$inputs.absent", "$.n")),
                // A legitimately selected null is one match and is written.
                replacement(
                    "/null",
                    jsonpath_selector_yaml("$inputs.document", "$.value"),
                ),
                // Zero matches keep their normalization: null plus the
                // existing no-match warning, still written.
                replacement(
                    "/zero",
                    jsonpath_selector_yaml("$inputs.document", "$.absent"),
                ),
                replacement("/last", yaml(json!("new"))),
            ],
            &eval,
        );

        assert_eq!(
            body,
            json!({
                "first": "new",
                "nested": "old",
                "context": "old",
                "null": null,
                "zero": null,
                "last": "new"
            })
        );
        let skipped: Vec<&String> = warnings
            .iter()
            .filter(|warning| warning.contains("replacement skipped"))
            .collect();
        assert_eq!(skipped.len(), 2, "{warnings:?}");
        assert!(
            skipped[0].starts_with("requestBody.replacements[1]:"),
            "{skipped:?}"
        );
        assert!(
            skipped[1].starts_with("requestBody.replacements[2]:"),
            "{skipped:?}"
        );
        assert_warning_contains(
            &warnings,
            "JSONPath query structural characters limit of 128 exceeded",
        );
        assert_warning_contains(
            &warnings,
            "requestBody.replacements[2]: $inputs.absent: input \"absent\" not found in context",
        );
        assert_warning_contains(
            &warnings,
            "requestBody.replacements[4]: $.absent: selector matched no values",
        );
        assert!(
            warnings
                .iter()
                .all(|warning| !warning.starts_with("requestBody.replacements[3]:")),
            "a selected null is not a failure: {warnings:?}"
        );
    }

    fn jsonpath_selector_object(
        context: &str,
        selector: &str,
        type_: SelectorType,
    ) -> SelectorObject {
        SelectorObject {
            context: context.to_string(),
            selector: selector.to_string(),
            type_,
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn jsonpath_selector_admission_precedes_context_resolution() {
        // No inputs: `$inputs.absent` resolves to null with a diagnostic.
        let eval = evaluator();
        for (selector, type_, needle) in [
            (
                "$.a",
                object_type("jsonpath", "draft-goessner-dispatch-jsonpath-00"),
                "unsupported JSONPath version \"draft-goessner-dispatch-jsonpath-00\"",
            ),
            ("$.a[", name_type("jsonpath"), "invalid JSONPath syntax"),
        ] {
            let object = jsonpath_selector_object("$inputs.absent", selector, type_);
            let (value, warnings) = resolve_selector(&object, &eval);
            assert_eq!(value, Value::Null);
            assert_eq!(warnings.len(), 1, "{selector}: {warnings:?}");
            assert!(
                warnings[0].message.contains(needle),
                "{selector}: {warnings:?}"
            );
        }

        // A valid query over a missing context keeps the context diagnostic.
        let valid = jsonpath_selector_object("$inputs.absent", "$.a", name_type("jsonpath"));
        let (value, warnings) = resolve_selector(&valid, &eval);
        assert_eq!(value, Value::Null);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0]
                .message
                .contains("input \"absent\" not found in context"),
            "{warnings:?}"
        );
    }

    #[test]
    fn jsonpath_selector_reads_keep_zero_one_many_and_selected_null_distinct() {
        let eval = evaluator_with_inputs(BTreeMap::from([(
            "document".to_string(),
            json!({"items": [{"id": 1}, {"id": 2}], "value": null}),
        )]));
        for (selector, expected, warning_count) in [
            ("$.items[0].id", json!(1), 0),
            ("$.items[*].id", json!([1, 2]), 0),
            ("$..id", json!([1, 2]), 0),
            ("$.items[1:].id", json!(2), 0),
            ("$.value", Value::Null, 0),
            ("$.absent", Value::Null, 1),
        ] {
            let object =
                jsonpath_selector_object("$inputs.document", selector, name_type("jsonpath"));
            let (value, warnings) = resolve_selector(&object, &eval);
            assert_eq!(value, expected, "{selector}");
            assert_eq!(warnings.len(), warning_count, "{selector}: {warnings:?}");
        }
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
    fn expression_type_object_xpath_30_is_rejected_before_evaluation() {
        // §5.8.15.2's own Expression Type Object shape, but a version this
        // runtime does not implement (ac-46638): unchanged body, one warning.
        let eval = evaluator();
        let original = Value::String("<root><x>old</x></root>".to_string());
        let (body, warnings) = apply_replacements(
            original.clone(),
            "application/xml",
            &[typed_replacement(
                "//x",
                object_type("xpath", "xpath-30"),
                serde_yaml_ng::Value::String("new".to_string()),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_eq!(capability_warnings(&warnings), 1, "{warnings:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
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
    fn xpath_versions_other_than_explicit_10_reject_with_one_warning_each() {
        // ac-46638, superseding Decision 3: plain `xpath`, xpath-20/-30/-31,
        // and the omitted field are each rejected before evaluation with
        // exactly one warning and an unchanged body; explicit xpath-10 is
        // covered by the silent tests above.
        let eval = evaluator();
        let rejected = [
            Some(name_type("xpath")),
            Some(object_type("xpath", "xpath-20")),
            Some(object_type("xpath", "xpath-30")),
            Some(object_type("xpath", "xpath-31")),
            None,
        ];
        for type_ in rejected {
            let entry = Replacement {
                target: "//x".to_string(),
                target_selector_type: type_.clone(),
                value: serde_yaml_ng::Value::String("new".to_string()).into(),
                ..Replacement::default()
            };
            let original = Value::String("<root><x>old</x></root>".to_string());
            let (body, warnings) =
                apply_replacements(original.clone(), "application/xml", &[entry], &eval);

            assert_eq!(body, original, "{type_:?}: {warnings:?}");
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
    fn goessner_jsonpath_target_is_rejected_before_the_body_is_queried() {
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply(
            original.clone(),
            vec![typed_replacement(
                "$.a",
                object_type("jsonpath", "draft-goessner-dispatch-jsonpath-00"),
                yaml(json!(2)),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_warning_contains(
            &warnings,
            "unsupported JSONPath version \"draft-goessner-dispatch-jsonpath-00\"",
        );
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
        // even when the resolved payload is structured JSON. The omitted
        // targetSelectorType carries no version, so the version rejection
        // fires first (ac-46638) — the XPath-flavored warning still proves
        // the dispatch.
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply_replacements(
            original.clone(),
            "application/xml",
            &[replacement("/a", yaml(json!(2)))],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(&warnings, "xpath-31");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn explicit_xpath_10_with_structured_body_warns_payload_type() {
        let eval = evaluator();
        let original = json!({"a": 1});
        let (body, warnings) = apply_replacements(
            original.clone(),
            "application/xml",
            &[typed_replacement(
                "/a",
                object_type("xpath", "xpath-10"),
                yaml(json!(2)),
            )],
            &eval,
        );

        assert_eq!(body, original);
        assert_warning_contains(
            &warnings,
            "XPath replacement requires XML/text string payload",
        );
    }
}
