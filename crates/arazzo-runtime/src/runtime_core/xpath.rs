use std::borrow::Cow;

use super::*;

/// Result of one read-only XPath evaluation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct XPathSelection {
    /// Effective boolean value of the raw XPath result, taken before the
    /// result is normalized into `value`. Arazzo §5.8.11.4.4 defines
    /// criterion pass/fail in exactly these terms: a boolean is itself, a
    /// number passes when non-zero (NaN fails), a string passes when
    /// non-empty, and a node-set passes when it holds at least one node —
    /// even if that node's text content is empty.
    pub truthy: bool,
    /// Match cardinality: the node count for node-set results, and 1 for
    /// every scalar result — including the empty string.
    pub match_count: usize,
    /// The normalized selection value (ac-46638): scalars keep their native
    /// JSON type (string, boolean, integer-collapsed finite number), a
    /// single matched node is its text — attribute nodes yield their value,
    /// an empty text is a real `""` — multiple nodes are an ordered string
    /// array, and only an empty node-set is `Null`. `Err` only for a
    /// non-finite number result, which JSON cannot represent; `truthy` and
    /// `match_count` remain usable alongside it.
    pub value: Result<Value, String>,
}

/// §5.8.12.1 version routing (ac-46638, superseding the Decision 3 /
/// ac-bd441 warn-and-run stance): explicit `xpath-10` alone reaches the
/// Uppsala XPath 1.0 engine. The omitted form — which §5.8.12 defaults to
/// `xpath-31` — and the other table tokens are rejected before evaluation,
/// because this runtime implements no other version and no compatibility
/// fallback is allowed. Tokens outside the table stay invalid metadata.
pub(crate) fn xpath_version_rejection(version: Option<&str>) -> Option<String> {
    match version {
        Some("xpath-10") => None,
        None => Some(
            "XPath target version is omitted and defaults to \"xpath-31\" (XML Path \
             Language 3.1), which this runtime does not implement; declare version \
             \"xpath-10\" to evaluate with the XPath 1.0 engine"
                .to_string(),
        ),
        Some(declared @ ("xpath-20" | "xpath-30" | "xpath-31")) => Some(format!(
            "XPath version {declared:?} is valid Arazzo metadata but is not implemented \
             by this runtime; declare version \"xpath-10\" to evaluate with the XPath \
             1.0 engine"
        )),
        Some(other) => Some(format!("unsupported XPath version {other:?}")),
    }
}

/// One shared parse/version/namespace setup behind both entry points.
///
/// The document is parsed as the server sent it — no rewriting; a document
/// using an undeclared prefix is reported as invalid XML. The document
/// element's non-empty in-scope prefix bindings are registered on the
/// evaluator, so prefixed name tests resolve by namespace URI at root scope:
/// equal local names under prefixes bound to different URIs are
/// distinguishable, and two prefixes bound to one URI unify. A prefix that is
/// not declared on the document element stays unregistered and falls back to
/// uppsala's textual prefix matching. XPath 1.0 has no default element
/// namespace, so the empty prefix is never registered.
///
/// The document borrows the caller's bytes — both entry points return only
/// owned values, so nothing forces a deep-cloned `'static` document.
fn xpath_backend<'a>(
    body: &'a [u8],
    version: Option<&str>,
) -> Result<(uppsala::Document<'a>, uppsala::XPathEvaluator), String> {
    if let Some(rejection) = xpath_version_rejection(version) {
        return Err(rejection);
    }
    let text = std::str::from_utf8(body).map_err(|err| format!("XML is not UTF-8: {err}"))?;
    let mut doc = uppsala::parse(text).map_err(|err| format!("invalid XML: {err}"))?;
    doc.prepare_xpath();
    let mut eval = uppsala::XPathEvaluator::new();
    if let Some(root) = doc.document_element() {
        if let Some(element) = doc.element(root) {
            for (prefix, uri) in &element.namespace_declarations {
                if !prefix.is_empty() {
                    eval.add_namespace(prefix.to_string(), uri.to_string());
                }
            }
        }
    }
    Ok((doc, eval))
}

/// Read-only evaluation: original XML bytes, expression, and declared
/// version in; owned EBV, normalized value, and cardinality out. No Uppsala
/// state escapes this module.
pub(crate) fn select_xpath(
    body: &[u8],
    expr: &str,
    version: Option<&str>,
) -> Result<XPathSelection, String> {
    let (doc, eval) = xpath_backend(body, version)?;
    let root = doc.root();
    let selected = eval
        .evaluate(&doc, root, expr)
        .map_err(|err| format!("invalid XPath selector {expr:?}: {err}"))?;

    // XPath 1.0 boolean coercion matches the §5.8.11.4.4 truth table arm
    // for arm; capture it from the typed result before normalization.
    let truthy = selected.to_boolean();
    let (value, match_count) = match selected {
        uppsala::XPathValue::String(s) => (Ok(Value::String(s)), 1),
        uppsala::XPathValue::Boolean(b) => (Ok(Value::Bool(b)), 1),
        uppsala::XPathValue::Number(n) => (finite_number_value(n), 1),
        uppsala::XPathValue::NodeSet(nodes) => {
            let match_count = nodes.len();
            let mut values = nodes
                .iter()
                .map(|node| Value::String(node_text(&doc, *node)))
                .collect::<Vec<_>>();
            let value = match match_count {
                0 => Value::Null,
                1 => values.pop().unwrap_or(Value::Null),
                _ => Value::Array(values),
            };
            (Ok(value), match_count)
        }
    };
    Ok(XPathSelection {
        truthy,
        match_count,
        value,
    })
}

/// Replacement evaluation: original XML text, target expression, declared
/// version, and the replacement text in; serialized XML out. Selector and
/// replacement dispatch, warning cardinality, and the unchanged-body policy
/// stay with the caller (`payload.rs`).
pub(crate) fn replace_xpath(
    xml: &str,
    target: &str,
    version: Option<&str>,
    replacement_text: &str,
) -> Result<String, String> {
    let (mut doc, eval) = xpath_backend(xml.as_bytes(), version)?;

    let nodes = {
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

    for node in nodes {
        let Some(kind) = doc.node_kind(node).cloned() else {
            return Err("xpath target node no longer exists".to_string());
        };
        match kind {
            uppsala::NodeKind::Element(_) => {
                for child in doc.children(node) {
                    doc.remove_child(node, child);
                }
                let text = doc.create_text(replacement_text.to_string());
                doc.append_child(node, text);
            }
            uppsala::NodeKind::Attribute(name, _) => {
                let parent = doc
                    .parent(node)
                    .ok_or_else(|| "xpath attribute target has no parent element".to_string())?;
                let element = doc
                    .element_mut(parent)
                    .ok_or_else(|| "xpath attribute parent is not an element".to_string())?;
                element.set_attribute(name, Cow::Owned(replacement_text.to_string()));
            }
            uppsala::NodeKind::Text(_) => {
                let Some(uppsala::NodeKind::Text(text)) = doc.node_kind_mut(node) else {
                    return Err("xpath text target no longer exists".to_string());
                };
                *text = Cow::Owned(replacement_text.to_string());
            }
            uppsala::NodeKind::CData(_) => {
                let Some(uppsala::NodeKind::CData(text)) = doc.node_kind_mut(node) else {
                    return Err("xpath cdata target no longer exists".to_string());
                };
                *text = Cow::Owned(replacement_text.to_string());
            }
            _ => return Err("xpath target node kind is not replaceable".to_string()),
        }
    }

    Ok(doc.to_xml())
}

/// Text of one matched node: attribute nodes yield their value, everything
/// else its deep text content. An empty result is a real empty string, per
/// the ac-46638 normalization contract.
fn node_text(doc: &uppsala::Document<'_>, node: uppsala::NodeId) -> String {
    match doc.node_kind(node) {
        Some(uppsala::NodeKind::Attribute(_, value)) => value.to_string(),
        _ => doc.text_content_deep(node),
    }
}

/// JSON value for a finite XPath number. Integer-valued results collapse to
/// JSON integers so `count()` interpolates as `"2"`, not `"2.0"`; other
/// finite numbers keep their f64 form. Non-finite numbers have no JSON
/// representation and fail visibly; the caller's EBV remains usable.
fn finite_number_value(n: f64) -> Result<Value, String> {
    const MAX_LOSSLESS_INTEGER: f64 = 9_007_199_254_740_992.0; // 2^53
    if n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_LOSSLESS_INTEGER {
        return Ok(Value::from(n as i64));
    }
    // `from_f64` returns `None` exactly for non-finite input, so this is the
    // one decision point for the fail-visibly contract.
    serde_json::Number::from_f64(n)
        .map(Value::Number)
        .ok_or_else(|| format!("XPath number result {n} has no JSON representation"))
}

/// Bare `//…` output and debugger watch extension: routed explicitly to the
/// XPath 1.0 operation (ac-46638). Errors degrade to null, the extension's
/// existing contract; non-representable numbers now degrade to null too
/// (they previously stringified as `"inf"`/`"NaN"`).
pub(crate) fn extract_xpath(body: &[u8], expr: &str) -> Value {
    select_xpath(body, expr, Some("xpath-10")).map_or(Value::Null, |selection| {
        selection.value.unwrap_or(Value::Null)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(xml: &[u8], xpath: &str) -> XPathSelection {
        match select_xpath(xml, xpath, Some("xpath-10")) {
            Ok(selection) => selection,
            Err(error) => panic!("selecting {xpath:?}: {error}"),
        }
    }

    fn selection_error(xml: &[u8], xpath: &str) -> String {
        match select_xpath(xml, xpath, Some("xpath-10")) {
            Ok(selection) => panic!("expected {xpath:?} to fail, got {selection:?}"),
            Err(error) => error,
        }
    }

    fn value_of(selection: &XPathSelection) -> Value {
        match &selection.value {
            Ok(value) => value.clone(),
            Err(error) => panic!("expected a representable value, got error: {error}"),
        }
    }

    #[test]
    fn select_xpath_preserves_zero_one_and_many_cardinality() {
        let xml = b"<items><item>one</item><item>two</item></items>";

        let many = selected(xml, "//item");
        assert_eq!(value_of(&many), json!(["one", "two"]));
        assert_eq!(many.match_count, 2);

        let one = selected(xml, "//item[1]");
        assert_eq!(value_of(&one), json!("one"));
        assert_eq!(one.match_count, 1);

        let zero = selected(xml, "//missing");
        assert_eq!(value_of(&zero), Value::Null);
        assert_eq!(zero.match_count, 0);
    }

    /// The ac-46638 normalization contract: scalars keep their native JSON
    /// type, empty scalar strings and empty-text nodes are count-one
    /// strings, attribute nodes yield their values, and only an empty
    /// node-set is null/count 0. This replaces the interim stringly pin
    /// (`select_xpath_scalar_values_stay_stringly_after_the_truthiness_fix`).
    #[test]
    fn select_xpath_preserves_native_scalar_types_and_cardinality() {
        let xml = br#"<items><item id="7">one</item><item>two</item><empty/></items>"#;

        let boolean = selected(xml, "count(//item) = 3");
        assert_eq!(value_of(&boolean), json!(false));
        assert_eq!(boolean.match_count, 1);

        let number = selected(xml, "count(//item)");
        assert_eq!(value_of(&number), json!(2));
        assert_eq!(number.match_count, 1);

        let fractional = selected(xml, "count(//item) div 2 + 0.25");
        assert_eq!(value_of(&fractional), json!(1.25));

        let string = selected(xml, "string(//item[1])");
        assert_eq!(value_of(&string), json!("one"));
        assert_eq!(string.match_count, 1);

        // An empty scalar string is a count-one string, not a miss.
        let empty_string = selected(xml, "string(//missing)");
        assert_eq!(value_of(&empty_string), json!(""));
        assert_eq!(empty_string.match_count, 1);
        assert!(!empty_string.truthy);

        // A matched node with empty text is a count-one empty string.
        let empty_node = selected(xml, "//empty");
        assert_eq!(value_of(&empty_node), json!(""));
        assert_eq!(empty_node.match_count, 1);
        assert!(empty_node.truthy);

        // Attribute nodes yield their values.
        let attribute = selected(xml, "//item[1]/@id");
        assert_eq!(value_of(&attribute), json!("7"));
        assert_eq!(attribute.match_count, 1);
    }

    /// Non-finite numbers have no JSON value; EBV and cardinality stay
    /// usable so criteria keep working while selectors fail visibly.
    #[test]
    fn select_xpath_non_finite_numbers_fail_visibly_with_usable_ebv() {
        let xml = b"<items><item>one</item></items>";

        let infinite = selected(xml, "1 div 0");
        assert!(infinite.value.is_err(), "{infinite:?}");
        assert!(infinite.truthy);
        assert_eq!(infinite.match_count, 1);

        let nan = selected(xml, "number('not-a-number')");
        assert!(nan.value.is_err(), "{nan:?}");
        assert!(!nan.truthy);
    }

    /// §5.8.11.4.4 truth table, arm by arm. The negative half of each pair is
    /// the regression case: booleans and numbers used to be stringified before
    /// the truthiness decision, so `false` and `0` (non-empty strings) passed.
    #[test]
    fn select_xpath_effective_boolean_value_follows_the_criterion_truth_table() {
        let xml = b"<items><item>one</item><item>two</item><empty/></items>";

        // boolean: itself
        assert!(selected(xml, "count(//item) = 2").truthy);
        assert!(!selected(xml, "count(//item) = 3").truthy);
        assert!(!selected(xml, "not(//item)").truthy);

        // number: non-zero passes, zero and NaN fail
        assert!(selected(xml, "count(//item)").truthy);
        assert!(!selected(xml, "count(//missing)").truthy);
        assert!(!selected(xml, "number('not-a-number')").truthy);

        // string: non-empty passes, empty fails
        assert!(selected(xml, "string(//item[1])").truthy);
        assert!(!selected(xml, "string(//missing)").truthy);

        // node-set: at least one node passes, even with empty text content
        assert!(selected(xml, "//item").truthy);
        assert!(!selected(xml, "//missing").truthy);
        assert!(selected(xml, "//empty").truthy);
    }

    /// The body is evaluated as the server sent it. A regex prepass used to
    /// strip namespace syntax from the raw text, rewriting text content and
    /// CDATA sections that merely contained namespace-shaped characters —
    /// these are the regression cases.
    #[test]
    fn select_xpath_leaves_text_and_cdata_content_untouched() {
        let text = selected(br#"<note>declare xmlns="urn:x" here</note>"#, "//note");
        assert_eq!(value_of(&text), json!("declare xmlns=\"urn:x\" here"));

        let cdata = selected(br#"<code><![CDATA[<f:x> and xmlns="u"]]></code>"#, "//code");
        assert_eq!(value_of(&cdata), json!("<f:x> and xmlns=\"u\""));
    }

    /// Unprefixed XPath name tests match on local names, so namespaced
    /// documents need no preprocessing, whatever the server's xmlns quoting
    /// style. The prefixed-expression assertions are the regression cases:
    /// the old prepass stripped prefixes out of the document while leaving
    /// them in the expression, so `//f:item` could never match.
    #[test]
    fn select_xpath_matches_local_names_in_namespaced_documents() {
        let soap = br#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body><Reply><Id>7</Id></Reply></soap:Body>
</soap:Envelope>"#;
        assert_eq!(value_of(&selected(soap, "//Body//Id")), json!("7"));

        let single_quoted: &[u8] = b"<f:root xmlns:f='urn:x'><f:item>one</f:item></f:root>";
        assert_eq!(value_of(&selected(single_quoted, "//item")), json!("one"));
        assert_eq!(value_of(&selected(single_quoted, "//f:item")), json!("one"));

        let prefixed_expr = selected(br#"<f:root xmlns:f="u"><f:x>1</f:x></f:root>"#, "//f:x");
        assert_eq!(value_of(&prefixed_expr), json!("1"));
    }

    /// Root-scope prefix bindings are registered on the evaluator, so
    /// prefixed name tests resolve by namespace URI: equal local names under
    /// prefixes bound to different URIs are distinguishable, and two
    /// prefixes bound to one URI unify.
    #[test]
    fn select_xpath_resolves_root_scope_prefixes_by_namespace_uri() {
        let collision: &[u8] = br#"<root xmlns:a="urn:one" xmlns:b="urn:two">
  <a:item>from-a</a:item>
  <b:item>from-b</b:item>
</root>"#;
        assert_eq!(value_of(&selected(collision, "//a:item")), json!("from-a"));
        assert_eq!(value_of(&selected(collision, "//b:item")), json!("from-b"));
        let both = selected(collision, "//item");
        assert_eq!(value_of(&both), json!(["from-a", "from-b"]));

        // Two prefixes bound to the same URI unify: the expression's prefix
        // matches elements written with the other prefix.
        let aliased: &[u8] = br#"<root xmlns:a="urn:same" xmlns:b="urn:same">
  <b:item>aliased</b:item>
</root>"#;
        assert_eq!(value_of(&selected(aliased, "//a:item")), json!("aliased"));

        // The discriminating case for URI resolution: a descendant re-binds
        // the same prefix text to a different URI. Textual matching would
        // return both elements; root-scope URI resolution returns only the
        // element in the root binding's namespace.
        let rebound: &[u8] = br#"<root xmlns:a="urn:one">
  <a:item>one</a:item>
  <inner xmlns:a="urn:two"><a:item>two</a:item></inner>
</root>"#;
        let scoped = selected(rebound, "//a:item");
        assert_eq!(value_of(&scoped), json!("one"));
        assert_eq!(scoped.match_count, 1);
    }

    /// A document using an undeclared namespace prefix is namespace-malformed
    /// XML: it is reported as invalid rather than silently rewritten into a
    /// parseable document, as the old regex prepass did.
    #[test]
    fn select_xpath_reports_undeclared_namespace_prefixes() {
        let error = selection_error(b"<f:root><f:x>1</f:x></f:root>", "//x");
        assert!(error.contains("invalid XML"), "got: {error}");
        assert!(error.contains("prefix"), "got: {error}");
    }

    #[test]
    fn select_xpath_reports_invalid_xml_and_selector() {
        let invalid_xml = selection_error(b"<items>", "//item");
        assert!(invalid_xml.contains("invalid XML"), "got: {invalid_xml}");

        let invalid_selector = selection_error(b"<items/>", "//[");
        assert!(
            invalid_selector.contains("invalid XPath selector"),
            "got: {invalid_selector}"
        );
    }

    /// ac-46638 version routing: explicit `xpath-10` alone evaluates. The
    /// omitted form and every other §5.8.12.1 token are rejected before
    /// evaluation; unknown tokens keep their invalid-metadata message.
    #[test]
    fn xpath_version_routing_rejects_everything_but_explicit_xpath_10() {
        let xml: &[u8] = b"<items><item>one</item></items>";

        assert!(select_xpath(xml, "//item", Some("xpath-10")).is_ok());

        let omitted = match select_xpath(xml, "//item", None) {
            Err(error) => error,
            Ok(selection) => panic!("omitted version must be rejected, got {selection:?}"),
        };
        assert!(omitted.contains("xpath-31"), "got: {omitted}");
        assert!(omitted.contains("xpath-10"), "got: {omitted}");

        for declared in ["xpath-20", "xpath-30", "xpath-31"] {
            let error = match select_xpath(xml, "//item", Some(declared)) {
                Err(error) => error,
                Ok(selection) => panic!("{declared} must be rejected, got {selection:?}"),
            };
            assert!(error.contains(declared), "got: {error}");
            assert!(error.contains("XPath 1.0 engine"), "got: {error}");
        }

        let unknown = match select_xpath(xml, "//item", Some("20")) {
            Err(error) => error,
            Ok(selection) => panic!("unknown token must be rejected, got {selection:?}"),
        };
        assert_eq!(unknown, "unsupported XPath version \"20\"");

        // Rejection happens before parsing: even invalid XML reports the
        // version problem, not a parse problem.
        let before_parse = match select_xpath(b"<broken", "//item", None) {
            Err(error) => error,
            Ok(selection) => panic!("omitted version must be rejected, got {selection:?}"),
        };
        assert!(before_parse.contains("xpath-31"), "got: {before_parse}");
    }

    /// The replacement entry point runs the same backend setup and preserves
    /// declarations, attributes, and text unrelated to the selected nodes.
    #[test]
    fn replace_xpath_preserves_unrelated_content_and_routes_versions() {
        let xml = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/" xmlns:tns="urn:test"><soap:Body note="keep"><tns:CustomerId>old</tns:CustomerId><tns:Other>stay</tns:Other></soap:Body></soap:Envelope>"#;

        let mutated = match replace_xpath(xml, "//CustomerId", Some("xpath-10"), "new") {
            Ok(mutated) => mutated,
            Err(error) => panic!("replacing: {error}"),
        };
        assert!(mutated.contains(">new<"), "{mutated}");
        assert!(
            mutated.contains(r#"xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/""#),
            "{mutated}"
        );
        assert!(mutated.contains(r#"xmlns:tns="urn:test""#), "{mutated}");
        assert!(mutated.contains(r#"note="keep""#), "{mutated}");
        assert!(mutated.contains(">stay<"), "{mutated}");

        let rejected = match replace_xpath(xml, "//CustomerId", None, "new") {
            Err(error) => error,
            Ok(mutated) => panic!("omitted version must be rejected, got {mutated}"),
        };
        assert!(rejected.contains("xpath-31"), "got: {rejected}");
    }

    /// The bare `//…` extension routes explicitly to XPath 1.0 and keeps its
    /// degrade-to-null contract for errors.
    #[test]
    fn extract_xpath_routes_the_bare_extension_to_xpath_10() {
        let xml = b"<items><item>one</item></items>";
        assert_eq!(extract_xpath(xml, "//item"), json!("one"));
        assert_eq!(extract_xpath(xml, "1 div 0"), Value::Null);
        assert_eq!(extract_xpath(b"<broken", "//item"), Value::Null);
    }
}
