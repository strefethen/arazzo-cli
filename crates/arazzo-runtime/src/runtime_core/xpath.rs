use super::*;

static XMLNS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"xmlns(?::\w+)?="[^"]*""#)
        .unwrap_or_else(|err| panic!("failed to compile xmlns regex: {err}"))
});

static NS_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(/?)[\w-]+:")
        .unwrap_or_else(|err| panic!("failed to compile ns-prefix regex: {err}"))
});

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct XPathSelection {
    pub value: Value,
    pub match_count: usize,
}

pub(crate) fn select_xpath(body: &[u8], expr: &str) -> Result<XPathSelection, String> {
    let text = std::str::from_utf8(body).map_err(|err| format!("XML is not UTF-8: {err}"))?;
    let text = XMLNS_RE.replace_all(text, "");
    let text = NS_PREFIX_RE.replace_all(&text, "<$1");
    let mut doc = uppsala::parse(&text).map_err(|err| format!("invalid XML: {err}"))?;
    doc.prepare_xpath();
    let eval = uppsala::XPathEvaluator::new();
    let root = doc.root();
    let selected = eval
        .evaluate(&doc, root, expr)
        .map_err(|err| format!("invalid XPath selector {expr:?}: {err}"))?;

    let selection = match selected {
        uppsala::XPathValue::String(s) => XPathSelection {
            match_count: usize::from(!s.is_empty()),
            value: if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            },
        },
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
            XPathSelection { value, match_count }
        }
        uppsala::XPathValue::Number(n) => {
            let s = n.to_string();
            XPathSelection {
                match_count: usize::from(!s.is_empty()),
                value: if s.is_empty() {
                    Value::Null
                } else {
                    Value::String(s)
                },
            }
        }
        uppsala::XPathValue::Boolean(b) => XPathSelection {
            value: Value::String(b.to_string()),
            match_count: 1,
        },
    };
    Ok(selection)
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

    #[test]
    fn select_xpath_reports_invalid_xml_and_selector() {
        let invalid_xml = selection_error(b"<items>", "//item");
        assert!(invalid_xml.contains("invalid XML"));

        let invalid_selector = selection_error(b"<items/>", "//[");
        assert!(invalid_selector.contains("invalid XPath selector"));
    }
}
