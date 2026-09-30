//! Pre-parse structural admission for untrusted XML text (ac-d1649, audit I1).
//!
//! The XPath backend's parser recurses once per element nesting level and
//! expands DTD entities with a cycle check but no size cap, so a small hostile
//! response can overflow a worker stack or exhaust memory in synchronous work
//! that `--execution-timeout` cannot interrupt. This module refuses such text
//! with one forward, non-recursive pass before any parser sees it.
//!
//! The check is deliberately backend-independent: any XML parser that replaces
//! the current one must still be called only on text this module admitted.
//!
//! It is not a well-formedness validator. It is conservative for the two
//! properties it enforces: text it cannot classify is refused, and it never
//! reports a nesting depth lower than a parser would reach.

/// Deepest element nesting admitted. Depth 64 is proven stack-safe for parse,
/// text collection, and descendant evaluation in a debug build on a default
/// 2 MiB thread; real API and SOAP responses rarely exceed about 30.
pub(crate) const MAX_XML_NESTING_DEPTH: usize = 64;

/// Refuses XML text whose element nesting exceeds [`MAX_XML_NESTING_DEPTH`] or
/// whose DOCTYPE declares an internal subset (where entity declarations live).
/// Errors use the same `invalid XML:` wording as parse failures, so every
/// caller reports a refusal exactly as it reports malformed XML.
pub(crate) fn admit_xml(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut pos = 0usize;
    while let Some(offset) = text[pos..].find('<') {
        let start = pos + offset;
        let rest = &text[start..];
        pos = if rest.starts_with("<!--") {
            end_after(text, start + 4, "-->", "comment")?
        } else if rest.starts_with("<![CDATA[") {
            end_after(text, start + 9, "]]>", "CDATA section")?
        } else if rest.starts_with("<?") {
            end_after(text, start + 2, "?>", "processing instruction")?
        } else if rest.starts_with("<!DOCTYPE") {
            doctype_end(bytes, start + 9)?
        } else if rest.starts_with("<!") {
            return Err("invalid XML: unexpected markup declaration".to_string());
        } else if rest.starts_with("</") {
            depth = depth.saturating_sub(1);
            end_after(text, start + 2, ">", "end tag")?
        } else {
            let (end, self_closing) = start_tag_end(bytes, start + 1)?;
            if !self_closing {
                depth += 1;
                if depth > MAX_XML_NESTING_DEPTH {
                    return Err(format!(
                        "invalid XML: element nesting exceeds the \
                         {MAX_XML_NESTING_DEPTH}-level limit"
                    ));
                }
            }
            end
        };
    }
    Ok(())
}

/// Position just past `terminator`, searching from `from`.
fn end_after(text: &str, from: usize, terminator: &str, what: &str) -> Result<usize, String> {
    text[from..]
        .find(terminator)
        .map(|offset| from + offset + terminator.len())
        .ok_or_else(|| format!("invalid XML: unterminated {what}"))
}

/// Position just past a start tag's `>`, and whether it closed with `/>`.
/// A `>` inside a quoted attribute value does not end the tag.
fn start_tag_end(bytes: &[u8], from: usize) -> Result<(usize, bool), String> {
    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(from) {
        match (quote, byte) {
            (Some(open), b) if b == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => return Ok((index + 1, index > from && bytes[index - 1] == b'/')),
            (None, _) => {}
        }
    }
    Err("invalid XML: unterminated start tag".to_string())
}

/// Position just past a DOCTYPE declaration without an internal subset. A `[`
/// outside the quoted system/public literals opens an internal subset, which
/// is refused.
fn doctype_end(bytes: &[u8], from: usize) -> Result<usize, String> {
    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(from) {
        match (quote, byte) {
            (Some(open), b) if b == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'[') => {
                return Err(
                    "invalid XML: DOCTYPE internal subsets (entity and markup declarations) \
                     are not accepted"
                        .to_string(),
                )
            }
            (None, b'>') => return Ok(index + 1),
            (None, _) => {}
        }
    }
    Err("invalid XML: unterminated DOCTYPE declaration".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> String {
        format!("{}x{}", "<a>".repeat(depth), "</a>".repeat(depth))
    }

    fn refusal(xml: &str) -> String {
        match admit_xml(xml) {
            Ok(()) => panic!("expected {xml:?} to be refused"),
            Err(error) => error,
        }
    }

    #[test]
    fn nesting_is_admitted_up_to_the_limit_and_refused_past_it() {
        assert!(admit_xml(&nested(MAX_XML_NESTING_DEPTH - 1)).is_ok());
        assert!(admit_xml(&nested(MAX_XML_NESTING_DEPTH)).is_ok());
        let error = refusal(&nested(MAX_XML_NESTING_DEPTH + 1));
        assert!(error.starts_with("invalid XML:"), "{error}");
        assert!(error.contains("64-level limit"), "{error}");
    }

    #[test]
    fn a_hostile_depth_is_refused_without_recursion() {
        assert!(admit_xml(&nested(100_000)).is_err());
    }

    #[test]
    fn sibling_elements_and_self_closing_tags_do_not_nest() {
        let limit = MAX_XML_NESTING_DEPTH;
        // Wide rather than deep: siblings close before the next opens.
        let wide = format!("<r>{}</r>", "<a>x</a>".repeat(10 * limit));
        assert!(admit_xml(&wide).is_ok());
        let empties = format!("<r>{}</r>", "<e/><e />".repeat(10 * limit));
        assert!(admit_xml(&empties).is_ok());
        // A self-closing leaf at the limit does not add a level.
        let at_limit = format!(
            "{}<leaf attr='1'/>{}",
            "<a>".repeat(limit),
            "</a>".repeat(limit)
        );
        assert!(admit_xml(&at_limit).is_ok());
    }

    #[test]
    fn markup_inside_comments_cdata_pis_and_attributes_is_not_counted() {
        let limit = MAX_XML_NESTING_DEPTH;
        let decoy = "<a><a><a><a>".repeat(limit);
        for wrapped in [
            format!("<r><!--{decoy}--></r>"),
            format!("<r><![CDATA[{decoy}]]></r>"),
            format!("<?pi {decoy}?><r/>"),
            format!("<r a=\"{}\" b='>[/'/>", ">".repeat(limit)),
            // `[` in content or CDATA is not a DOCTYPE subset.
            "<r>[x]<![CDATA[[<!DOCTYPE d [ ]>]]></r>".to_string(),
        ] {
            assert!(admit_xml(&wrapped).is_ok(), "{wrapped:?}");
        }
    }

    #[test]
    fn doctype_without_internal_subset_is_admitted() {
        assert!(admit_xml("<?xml version=\"1.0\"?><!DOCTYPE root><root/>").is_ok());
        assert!(admit_xml("<!DOCTYPE root SYSTEM \"urn:x[1].dtd\"><root/>").is_ok());
        assert!(admit_xml(
            "<!DOCTYPE root PUBLIC \"-//X//DTD Y//EN\" 'http://example.test/y.dtd'><root/>"
        )
        .is_ok());
    }

    #[test]
    fn doctype_internal_subset_is_refused() {
        let error = refusal("<!DOCTYPE r [<!ENTITY e \"x\">]><r>&e;</r>");
        assert!(error.starts_with("invalid XML:"), "{error}");
        assert!(error.contains("internal subset"), "{error}");
        assert!(admit_xml("<!DOCTYPE r []><r/>").is_err());
        assert!(admit_xml("<!DOCTYPE r SYSTEM 'x.dtd' [ ]><r/>").is_err());
    }

    #[test]
    fn the_audit_entity_cascade_is_refused() {
        let mut dtd = String::from("<!ENTITY e0 \"lol\">");
        for level in 1..=8 {
            let refs = format!("&e{};", level - 1).repeat(10);
            dtd.push_str(&format!("<!ENTITY e{level} \"{refs}\">"));
        }
        let bomb = format!("<!DOCTYPE r [{dtd}]><r>&e8;</r>");
        assert!(refusal(&bomb).contains("internal subset"));
    }

    #[test]
    fn unterminated_or_unexpected_markup_is_refused() {
        for xml in [
            "<r><!-- open",
            "<r><![CDATA[ open",
            "<?pi open",
            "<r attr='>",
            "<!DOCTYPE r",
            "<r></r",
            "<!ENTITY e 'x'><r/>",
            "<!doctype r><r/>",
        ] {
            assert!(refusal(xml).starts_with("invalid XML:"), "{xml:?}");
        }
    }

    #[test]
    fn text_without_markup_is_left_to_the_parser() {
        assert!(admit_xml("").is_ok());
        assert!(admit_xml("plain text").is_ok());
    }
}
