use super::*;

static XMLNS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"xmlns(?::\w+)?="[^"]*""#)
        .unwrap_or_else(|err| panic!("failed to compile xmlns regex: {err}"))
});

static NS_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(/?)[\w-]+:")
        .unwrap_or_else(|err| panic!("failed to compile ns-prefix regex: {err}"))
});

pub(crate) fn extract_xpath(body: &[u8], expr: &str) -> Value {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Value::Null,
    };
    let text = XMLNS_RE.replace_all(text, "");
    let text = NS_PREFIX_RE.replace_all(&text, "<$1");
    let mut doc = match uppsala::parse(&text) {
        Ok(d) => d,
        Err(_) => return Value::Null,
    };
    doc.prepare_xpath();
    let eval = uppsala::XPathEvaluator::new();
    let root = doc.root();
    match eval.evaluate(&doc, root, expr) {
        Ok(uppsala::XPathValue::String(s)) if !s.is_empty() => Value::String(s),
        Ok(uppsala::XPathValue::NodeSet(nodes)) if !nodes.is_empty() => {
            let s = doc.text_content_deep(nodes[0]);
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Ok(uppsala::XPathValue::Number(n)) => {
            let s = n.to_string();
            if s.is_empty() {
                Value::Null
            } else {
                Value::String(s)
            }
        }
        Ok(uppsala::XPathValue::Boolean(b)) => Value::String(b.to_string()),
        _ => Value::Null,
    }
}
