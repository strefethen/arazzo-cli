use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct XPathSelection {
    pub value: Value,
    pub match_count: usize,
    /// Effective boolean value of the raw XPath result, taken before the
    /// result is normalized into `value`. Arazzo §5.8.11.4.4 defines
    /// criterion pass/fail in exactly these terms: a boolean is itself, a
    /// number passes when non-zero (NaN fails), a string passes when
    /// non-empty, and a node-set passes when it holds at least one node —
    /// even if that node's text content is empty.
    pub truthy: bool,
}

pub(crate) fn select_xpath(body: &[u8], expr: &str) -> Result<XPathSelection, String> {
    // The document is parsed as the server sent it. Namespace handling is
    // uppsala's: prefixes resolve natively, and an unprefixed XPath name test
    // matches on local names, so `//Body` finds `<soap:Body>` without any
    // preprocessing. (A regex prepass used to strip xmlns declarations and
    // element prefixes from the raw text; it also rewrote text content, CDATA
    // sections, and comments that merely looked namespaced, and it missed
    // single-quoted xmlns declarations entirely.)
    let text = std::str::from_utf8(body).map_err(|err| format!("XML is not UTF-8: {err}"))?;
    let mut doc = uppsala::parse(text).map_err(|err| format!("invalid XML: {err}"))?;
    doc.prepare_xpath();
    let eval = uppsala::XPathEvaluator::new();
    let root = doc.root();
    let selected = eval
        .evaluate(&doc, root, expr)
        .map_err(|err| format!("invalid XPath selector {expr:?}: {err}"))?;

    // XPath 1.0 boolean coercion matches the §5.8.11.4.4 truth table arm
    // for arm; capture it from the typed result before normalization below
    // erases the boolean/number distinction (both become strings).
    let truthy = selected.to_boolean();
    let (value, match_count) = match selected {
        uppsala::XPathValue::String(s) => {
            let match_count = usize::from(!s.is_empty());
            (
                if s.is_empty() {
                    Value::Null
                } else {
                    Value::String(s)
                },
                match_count,
            )
        }
        uppsala::XPathValue::NodeSet(nodes) => {
            let match_count = nodes.len();
            let mut values = nodes
                .iter()
                .map(|node| {
                    let text = doc.text_content_deep(*node);
                    if text.is_empty() {
                        Value::Null
                    } else {
                        Value::String(text)
                    }
                })
                .collect::<Vec<_>>();
            let value = match values.len() {
                0 => Value::Null,
                1 => values.pop().unwrap_or(Value::Null),
                _ => Value::Array(values),
            };
            (value, match_count)
        }
        uppsala::XPathValue::Number(n) => (Value::String(n.to_string()), 1),
        uppsala::XPathValue::Boolean(b) => (Value::String(b.to_string()), 1),
    };
    Ok(XPathSelection {
        value,
        match_count,
        truthy,
    })
}

pub(crate) fn extract_xpath(body: &[u8], expr: &str) -> Value {
    select_xpath(body, expr).map_or(Value::Null, |selection| selection.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(xml: &[u8], xpath: &str) -> XPathSelection {
        match select_xpath(xml, xpath) {
            Ok(selection) => selection,
            Err(error) => panic!("selecting {xpath:?}: {error}"),
        }
    }

    fn selection_error(xml: &[u8], xpath: &str) -> String {
        match select_xpath(xml, xpath) {
            Ok(selection) => panic!("expected {xpath:?} to fail, got {selection:?}"),
            Err(error) => error,
        }
    }

    #[test]
    fn select_xpath_preserves_zero_one_and_many_cardinality() {
        let xml = b"<items><item>one</item><item>two</item></items>";

        let many = selected(xml, "//item");
        assert_eq!(many.value, json!(["one", "two"]));
        assert_eq!(many.match_count, 2);

        let one = selected(xml, "//item[1]");
        assert_eq!(one.value, json!("one"));
        assert_eq!(one.match_count, 1);

        let zero = selected(xml, "//missing");
        assert_eq!(zero.value, Value::Null);
        assert_eq!(zero.match_count, 0);
    }

    /// The body is evaluated as the server sent it. A regex prepass used to
    /// strip namespace syntax from the raw text, rewriting text content and
    /// CDATA sections that merely contained namespace-shaped characters —
    /// these are the regression cases.
    #[test]
    fn select_xpath_leaves_text_and_cdata_content_untouched() {
        let text = selected(br#"<note>declare xmlns="urn:x" here</note>"#, "//note");
        assert_eq!(text.value, json!("declare xmlns=\"urn:x\" here"));

        let cdata = selected(br#"<code><![CDATA[<f:x> and xmlns="u"]]></code>"#, "//code");
        assert_eq!(cdata.value, json!("<f:x> and xmlns=\"u\""));
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
        assert_eq!(selected(soap, "//Body//Id").value, json!("7"));

        let single_quoted: &[u8] = b"<f:root xmlns:f='urn:x'><f:item>one</f:item></f:root>";
        assert_eq!(selected(single_quoted, "//item").value, json!("one"));
        assert_eq!(selected(single_quoted, "//f:item").value, json!("one"));

        let prefixed_expr = selected(br#"<f:root xmlns:f="u"><f:x>1</f:x></f:root>"#, "//f:x");
        assert_eq!(prefixed_expr.value, json!("1"));
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
        assert!(invalid_xml.contains("invalid XML"));

        let invalid_selector = selection_error(b"<items/>", "//[");
        assert!(invalid_selector.contains("invalid XPath selector"));
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
        let empty_text_node = selected(xml, "//empty");
        assert_eq!(empty_text_node.value, Value::Null);
        assert_eq!(empty_text_node.match_count, 1);
        assert!(empty_text_node.truthy);
    }

    /// The normalized `value` deliberately keeps its stringly XPath 1.0
    /// string() form for scalars — outputs, selectors, and debugger watches
    /// consume it and their contract is unchanged by the truthiness fix.
    #[test]
    fn select_xpath_scalar_values_stay_stringly_after_the_truthiness_fix() {
        let xml = b"<items><item>one</item><item>two</item></items>";

        let boolean = selected(xml, "count(//item) = 3");
        assert_eq!(boolean.value, json!("false"));
        assert_eq!(boolean.match_count, 1);

        let number = selected(xml, "count(//item)");
        assert_eq!(number.value, json!("2"));
        assert_eq!(number.match_count, 1);
    }
}
