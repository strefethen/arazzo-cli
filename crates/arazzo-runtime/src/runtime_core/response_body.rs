use std::collections::BTreeMap;

use arazzo_expr::BodySlot;
use arazzo_spec::media_type::MediaType;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use serde_json::Value;

use super::{ContentType, Response, TraceRedirectHop};

const NOT_YET_A_VALUE: &str = "which is not yet a readable value";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

impl Response {
    pub(crate) fn decoded(
        status_code: i64,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
        redirects: Vec<TraceRedirectHop>,
    ) -> Self {
        let declared = content_type_header(&headers);
        let media_type = declared.and_then(MediaType::parse);
        let content_type = projection(declared, media_type.as_ref());
        let body_value = if body.is_empty() {
            BodySlot::Absent
        } else {
            match decode(declared, media_type.as_ref(), &body) {
                Ok(value) => BodySlot::present(value),
                Err(reason) => BodySlot::Unavailable { reason },
            }
        };
        Self {
            status_code,
            headers,
            body,
            body_value,
            content_type,
            redirects,
        }
    }
}

fn content_type_header(headers: &BTreeMap<String, String>) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
}

fn projection(declared: Option<&str>, media_type: Option<&MediaType>) -> ContentType {
    match media_type {
        Some(media_type) if media_type.is_json() => ContentType::Json,
        Some(media_type) if media_type.is_xml() => ContentType::Xml,
        _ => ContentType::Other(declared.unwrap_or_default().to_lowercase()),
    }
}

fn decode(
    declared: Option<&str>,
    media_type: Option<&MediaType>,
    body: &[u8],
) -> Result<Value, String> {
    let Some(declared) = declared else {
        return Err(format!(
            "response has no Content-Type header, so its body is bytes, {NOT_YET_A_VALUE}"
        ));
    };
    let Some(media_type) = media_type else {
        return Err(format!(
            "response Content-Type {declared:?} is not a valid media type, so its body is bytes, {NOT_YET_A_VALUE}"
        ));
    };
    if media_type.is_json() {
        let json = body.strip_prefix(UTF8_BOM).unwrap_or(body);
        return serde_json::from_slice(json).map_err(|err| {
            format!(
                "response body is not valid JSON ({}): {err}",
                media_type.essence()
            )
        });
    }
    if media_type.is_xml() {
        return decode_xml(media_type.charset(), body).map(Value::String);
    }
    if media_type.is_text() || media_type.is_form_urlencoded() || media_type.charset().is_some() {
        return decode_labeled(media_type.charset().unwrap_or("utf-8"), body).map(Value::String);
    }
    Err(format!(
        "response body is bytes ({}), {NOT_YET_A_VALUE}",
        media_type.essence()
    ))
}

fn decode_xml(charset: Option<&str>, body: &[u8]) -> Result<String, String> {
    if let Some(label) = charset {
        return decode_labeled(label, body);
    }
    if let Some((encoding, bom_length)) = Encoding::for_bom(body) {
        return decode_exact(encoding, &body[bom_length..]);
    }
    match xml_declared_encoding(body) {
        Some(label) => decode_labeled(label, body),
        None => decode_exact(UTF_8, body),
    }
}

fn decode_labeled(label: &str, body: &[u8]) -> Result<String, String> {
    let encoding = Encoding::for_label_no_replacement(label.as_bytes())
        .ok_or_else(|| format!("response charset {label:?} is not a supported encoding label"))?;
    let byte_order_open = is_utf16(encoding) && !label.trim().eq_ignore_ascii_case(encoding.name());
    match Encoding::for_bom(body) {
        Some((bom_encoding, bom_length))
            if bom_encoding == encoding || (byte_order_open && is_utf16(bom_encoding)) =>
        {
            decode_exact(bom_encoding, &body[bom_length..])
        }
        Some((bom_encoding, _)) => Err(format!(
            "response body starts with a {} byte order mark, contradicting charset {label:?}",
            bom_encoding.name()
        )),
        None if byte_order_open => Err(format!(
            "response charset {label:?} does not fix a byte order and the body has no byte order mark"
        )),
        None => decode_exact(encoding, body),
    }
}

fn is_utf16(encoding: &'static Encoding) -> bool {
    encoding == UTF_16LE || encoding == UTF_16BE
}

fn decode_exact(encoding: &'static Encoding, bytes: &[u8]) -> Result<String, String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| format!("response body is not valid {} text", encoding.name()))
}

fn xml_declared_encoding(body: &[u8]) -> Option<&str> {
    let rest = body.strip_prefix(b"<?xml")?;
    if !rest.first().copied().is_some_and(is_xml_space) {
        return None;
    }
    let end = rest.windows(2).position(|pair| pair == b"?>")?;
    let mut cursor = std::str::from_utf8(&rest[..end]).ok()?;
    loop {
        cursor = cursor.trim_start_matches(|ch: char| ch.is_ascii() && is_xml_space(ch as u8));
        if cursor.is_empty() {
            return None;
        }
        let name_end =
            cursor.find(|ch: char| ch == '=' || (ch.is_ascii() && is_xml_space(ch as u8)))?;
        let name = &cursor[..name_end];
        cursor = cursor[name_end..]
            .trim_start_matches(|ch: char| ch.is_ascii() && is_xml_space(ch as u8))
            .strip_prefix('=')?
            .trim_start_matches(|ch: char| ch.is_ascii() && is_xml_space(ch as u8));
        let quote = cursor
            .chars()
            .next()
            .filter(|ch| *ch == '"' || *ch == '\'')?;
        let value_end = cursor[1..].find(quote)? + 1;
        if name == "encoding" {
            return Some(&cursor[1..value_end]);
        }
        cursor = &cursor[value_end + 1..];
    }
}

fn is_xml_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn decoded(content_type: Option<&str>, body: &[u8]) -> Response {
        let headers = content_type
            .map(|value| BTreeMap::from([("content-type".to_owned(), value.to_owned())]))
            .unwrap_or_default();
        Response::decoded(200, headers, body.to_vec(), Vec::new())
    }

    fn value(content_type: Option<&str>, body: &[u8]) -> Value {
        match decoded(content_type, body).body_value {
            BodySlot::Present(value) => value.as_ref().clone(),
            other => panic!("{content_type:?} should decode, got {other:?}"),
        }
    }

    fn reason(content_type: Option<&str>, body: &[u8]) -> String {
        match decoded(content_type, body).body_value {
            BodySlot::Unavailable { reason } => reason,
            other => panic!("{content_type:?} should be unavailable, got {other:?}"),
        }
    }

    #[test]
    fn empty_bodies_are_absent_whatever_the_type() {
        for content_type in [None, Some("application/json"), Some("image/png")] {
            assert_eq!(decoded(content_type, b"").body_value, BodySlot::Absent);
        }
    }

    #[test]
    fn json_ignores_charset_and_a_leading_bom() {
        assert_eq!(
            value(
                Some("application/json; charset=iso-8859-1"),
                b"\xEF\xBB\xBF{\"a\":1}"
            ),
            json!({"a": 1})
        );
        assert_eq!(value(Some("application/problem+json"), b"[1]"), json!([1]));
        assert!(reason(Some("application/json"), b"{")
            .starts_with("response body is not valid JSON (application/json): "));
    }

    #[test]
    fn text_keeps_its_string_and_decodes_by_charset() {
        assert_eq!(value(Some("text/plain"), b"123"), json!("123"));
        assert_eq!(
            value(Some("text/plain; charset=iso-8859-1"), b"caf\xE9"),
            json!("caf\u{e9}")
        );
        assert_eq!(
            value(Some("application/x-www-form-urlencoded"), b"a=1"),
            json!("a=1")
        );
        assert_eq!(
            value(Some("application/javascript; charset=utf-8"), b"x()"),
            json!("x()")
        );
        assert_eq!(value(Some("text/plain"), b"\xEF\xBB\xBFhi"), json!("hi"));
        assert_eq!(
            reason(Some("text/plain"), b"caf\xE9"),
            "response body is not valid UTF-8 text"
        );
        assert_eq!(
            reason(Some("text/plain; charset=klingon"), b"x"),
            "response charset \"klingon\" is not a supported encoding label"
        );
        assert_eq!(
            reason(Some("text/plain; charset=iso-2022-kr"), b"x"),
            "response charset \"iso-2022-kr\" is not a supported encoding label"
        );
        assert_eq!(
            reason(Some("text/plain; charset=iso-8859-1"), b"\xEF\xBB\xBFx"),
            "response body starts with a UTF-8 byte order mark, contradicting charset \"iso-8859-1\""
        );
    }

    #[test]
    fn utf16_needs_an_explicit_byte_order() {
        assert_eq!(
            value(Some("text/plain; charset=utf-16le"), b"h\0i\0"),
            json!("hi")
        );
        assert_eq!(
            value(Some("text/plain; charset=UTF-16BE"), b"\0h\0i"),
            json!("hi")
        );
        assert_eq!(
            value(Some("text/plain; charset=utf-16"), b"\xFE\xFF\0h\0i"),
            json!("hi")
        );
        assert_eq!(
            value(Some("text/plain; charset=utf-16"), b"\xFF\xFEh\0i\0"),
            json!("hi")
        );
        assert_eq!(
            reason(Some("text/plain; charset=utf-16"), b"h\0i\0"),
            "response charset \"utf-16\" does not fix a byte order and the body has no byte order mark"
        );
        assert_eq!(
            reason(Some("text/plain; charset=utf-16le"), b"\xFE\xFF\0h"),
            "response body starts with a UTF-16BE byte order mark, contradicting charset \"utf-16le\""
        );
        assert_eq!(
            reason(Some("text/plain; charset=utf-16le"), b"\0\xD8"),
            "response body is not valid UTF-16LE text"
        );
    }

    #[test]
    fn xml_decodes_by_charset_then_bom_then_declaration_then_utf8() {
        let latin = b"<?xml version=\"1.0\" encoding='ISO-8859-1'?><p>caf\xE9</p>";
        assert_eq!(
            value(Some("application/xml"), latin),
            json!("<?xml version=\"1.0\" encoding='ISO-8859-1'?><p>caf\u{e9}</p>")
        );
        assert_eq!(
            value(Some("text/xml; charset=windows-1252"), b"<p>\x80</p>"),
            json!("<p>\u{20ac}</p>")
        );
        assert_eq!(
            value(Some("application/atom+xml"), b"\xFF\xFE<\0a\0/\0>\0"),
            json!("<a/>")
        );
        assert_eq!(
            value(Some("application/rss+xml"), "<r>\u{e9}</r>".as_bytes()),
            json!("<r>\u{e9}</r>")
        );
        assert_eq!(
            reason(Some("application/xml"), b"<p>caf\xE9</p>"),
            "response body is not valid UTF-8 text"
        );
        assert_eq!(
            reason(
                Some("application/xml"),
                b"<?xml version=\"1.0\" encoding=\"bogus\"?><p/>"
            ),
            "response charset \"bogus\" is not a supported encoding label"
        );
    }

    #[test]
    fn bytes_rows_name_why_the_body_is_not_a_value() {
        assert_eq!(
            reason(Some("image/png"), b"\x89PNG"),
            "response body is bytes (image/png), which is not yet a readable value"
        );
        assert_eq!(
            reason(None, b"{}"),
            "response has no Content-Type header, so its body is bytes, which is not yet a readable value"
        );
        assert_eq!(
            reason(Some("json"), b"{}"),
            "response Content-Type \"json\" is not a valid media type, so its body is bytes, which is not yet a readable value"
        );
    }

    #[test]
    fn the_trace_projection_follows_the_parsed_type() {
        assert_eq!(
            decoded(Some("Application/Problem+JSON"), b"{}").content_type,
            ContentType::Json
        );
        assert_eq!(
            decoded(Some("application/rss+xml"), b"<r/>").content_type,
            ContentType::Xml
        );
        assert_eq!(
            decoded(Some("Text/Plain"), b"x").content_type,
            ContentType::Other("text/plain".to_owned())
        );
        assert_eq!(
            decoded(None, b"x").content_type,
            ContentType::Other(String::new())
        );
        let header = Response::decoded(
            200,
            BTreeMap::from([("Content-Type".to_owned(), "text/plain".to_owned())]),
            b"x".to_vec(),
            Vec::new(),
        );
        assert_eq!(header.body_value, BodySlot::present(json!("x")));
    }
}
