use std::borrow::Cow;
use std::collections::BTreeMap;

use arazzo_expr::BodySlot;
use arazzo_spec::media_type::MediaType;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};
use serde_json::Value;

use super::{BytesDescriptor, Response, TraceRedirectHop, TraceResponse};

const NOT_YET_A_VALUE: &str = "which is not yet a readable value";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

pub(crate) enum BodyView<'a> {
    Absent,
    Text(Cow<'a, str>),
    Bytes {
        descriptor: Option<BytesDescriptor>,
        decode_error: Option<&'a str>,
    },
}

impl Response {
    pub(crate) fn decoded(
        status_code: i64,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
        redirects: Vec<TraceRedirectHop>,
    ) -> Self {
        let declared = content_type_header(&headers);
        let media_type = declared.and_then(MediaType::parse);
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
            media_type,
            redirects,
        }
    }

    pub(crate) fn replayed(recorded: TraceResponse, redirects: Vec<TraceRedirectHop>) -> Self {
        let media_type = recorded.media_type.as_deref().and_then(MediaType::parse);
        let (body, body_value) = match (recorded.body, recorded.body_blob) {
            (Some(text), _) => {
                let value = replayed_text(
                    content_type_header(&recorded.headers),
                    media_type.as_ref(),
                    &text,
                );
                (text.into_bytes(), value)
            }
            (None, Some(_)) => {
                let reason = recorded.body_error.unwrap_or_else(|| {
                    bytes_reason(content_type_header(&recorded.headers), media_type.as_ref())
                });
                (Vec::new(), BodySlot::Unavailable { reason })
            }
            (None, None) => (Vec::new(), BodySlot::Absent),
        };
        Self {
            status_code: recorded.status_code,
            headers: recorded.headers,
            body,
            body_value,
            media_type,
            redirects,
        }
    }

    pub(crate) fn media_type_text(&self) -> Option<String> {
        self.media_type.as_ref().map(ToString::to_string)
    }

    pub(crate) fn body_view(&self) -> BodyView<'_> {
        match &self.body_value {
            BodySlot::Absent => BodyView::Absent,
            BodySlot::Present(value) => BodyView::Text(match value.as_ref() {
                _ if self.media_type.as_ref().is_some_and(MediaType::is_json) => {
                    std::str::from_utf8(self.body.strip_prefix(UTF8_BOM).unwrap_or(&self.body))
                        .map_or_else(|_| Cow::Owned(value.to_string()), Cow::Borrowed)
                }
                Value::String(text) => Cow::Borrowed(text.as_str()),
                other => Cow::Owned(other.to_string()),
            }),
            BodySlot::Unavailable { reason } => BodyView::Bytes {
                descriptor: (!self.body.is_empty()).then(|| {
                    BytesDescriptor::of(
                        &self.body,
                        self.media_type.as_ref().map(MediaType::essence),
                        None,
                    )
                }),
                decode_error: self
                    .media_type
                    .as_ref()
                    .filter(|media_type| is_textual(media_type))
                    .map(|_| reason.as_str()),
            },
        }
    }
}

pub(crate) fn truncate_with_ellipsis(text: &mut String, max_bytes: usize) {
    if text.len() > max_bytes {
        let mut end = max_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("...");
    }
}

fn content_type_header(headers: &BTreeMap<String, String>) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
}

fn is_textual(media_type: &MediaType) -> bool {
    media_type.is_json()
        || media_type.is_xml()
        || media_type.is_text()
        || media_type.is_form_urlencoded()
        || media_type.charset().is_some()
}

fn bytes_reason(declared: Option<&str>, media_type: Option<&MediaType>) -> String {
    match (declared, media_type) {
        (None, _) => format!(
            "response has no Content-Type header, so its body is bytes, {NOT_YET_A_VALUE}"
        ),
        (Some(declared), None) => format!(
            "response Content-Type {declared:?} is not a valid media type, so its body is bytes, {NOT_YET_A_VALUE}"
        ),
        (Some(_), Some(media_type)) => format!(
            "response body is bytes ({}), {NOT_YET_A_VALUE}",
            media_type.essence()
        ),
    }
}

fn replayed_text(declared: Option<&str>, media_type: Option<&MediaType>, text: &str) -> BodySlot {
    match media_type {
        Some(media_type) if media_type.is_json() => match serde_json::from_str(text) {
            Ok(value) => BodySlot::present(value),
            Err(err) => BodySlot::Unavailable {
                reason: json_reason(media_type, &err),
            },
        },
        Some(media_type) if is_textual(media_type) => {
            BodySlot::present(Value::String(text.to_owned()))
        }
        _ => BodySlot::Unavailable {
            reason: bytes_reason(declared, media_type),
        },
    }
}

fn json_reason(media_type: &MediaType, err: &serde_json::Error) -> String {
    format!(
        "response body is not valid JSON ({}): {err}",
        media_type.essence()
    )
}

fn decode(
    declared: Option<&str>,
    media_type: Option<&MediaType>,
    body: &[u8],
) -> Result<Value, String> {
    let Some(media_type) = media_type.filter(|media_type| is_textual(media_type)) else {
        return Err(bytes_reason(declared, media_type));
    };
    if media_type.is_json() {
        let json = body.strip_prefix(UTF8_BOM).unwrap_or(body);
        return serde_json::from_slice(json).map_err(|err| json_reason(media_type, &err));
    }
    if media_type.is_xml() {
        return decode_xml(media_type.charset(), body).map(Value::String);
    }
    decode_labeled(media_type.charset().unwrap_or("utf-8"), body).map(Value::String)
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
    fn the_media_type_is_the_parsed_type_and_absent_when_unparseable() {
        assert_eq!(
            decoded(Some("Application/Problem+JSON; Charset=UTF-8"), b"{}")
                .media_type_text()
                .as_deref(),
            Some("application/problem+json;charset=UTF-8")
        );
        assert_eq!(decoded(Some("json"), b"x").media_type_text(), None);
        assert_eq!(decoded(None, b"x").media_type_text(), None);
        let header = Response::decoded(
            200,
            BTreeMap::from([("Content-Type".to_owned(), "text/plain".to_owned())]),
            b"x".to_vec(),
            Vec::new(),
        );
        assert_eq!(header.body_value, BodySlot::present(json!("x")));
    }

    fn view_text(response: &Response) -> String {
        match response.body_view() {
            BodyView::Text(text) => text.into_owned(),
            _ => panic!("expected a text view of {:?}", response.body_value),
        }
    }

    #[test]
    fn body_view_is_decoded_text_never_lossy_bytes() {
        assert_eq!(
            view_text(&decoded(
                Some("application/json"),
                b"\xEF\xBB\xBF{ \"a\" : 1 }"
            )),
            "{ \"a\" : 1 }"
        );
        assert_eq!(
            view_text(&decoded(Some("application/json"), b"\"caf\xC3\xA9\"")),
            "\"caf\u{e9}\""
        );
        assert_eq!(
            view_text(&decoded(Some("text/plain; charset=iso-8859-1"), b"caf\xE9")),
            "caf\u{e9}"
        );
        assert!(matches!(
            decoded(Some("text/plain"), b"").body_view(),
            BodyView::Absent
        ));
    }

    #[test]
    fn a_bytes_descriptor_names_only_the_media_type_essence() {
        let response = decoded(Some("Image/PNG; token=FAKE-PARAM"), b"\x89PNG");
        let BodyView::Bytes {
            descriptor: Some(descriptor),
            ..
        } = response.body_view()
        else {
            panic!("a PNG body is bytes");
        };
        assert_eq!(
            descriptor,
            BytesDescriptor::of(b"\x89PNG", Some("image/png".to_owned()), None)
        );
    }

    #[test]
    fn body_view_describes_bytes_and_keeps_only_real_decode_errors() {
        let png = decoded(Some("image/png"), b"\x89PNG");
        let BodyView::Bytes {
            descriptor: Some(descriptor),
            decode_error: None,
        } = png.body_view()
        else {
            panic!("a PNG body is bytes with no decode error");
        };
        assert_eq!(
            descriptor,
            BytesDescriptor::of(b"\x89PNG", Some("image/png".to_owned()), None)
        );
        let broken = decoded(Some("application/json"), b"{\xFF");
        let BodyView::Bytes {
            descriptor: Some(_),
            decode_error: Some(error),
        } = broken.body_view()
        else {
            panic!("undecodable JSON keeps its decode error");
        };
        assert!(error.starts_with("response body is not valid JSON (application/json): "));
        let untyped = decoded(None, b"{}");
        assert!(matches!(
            untyped.body_view(),
            BodyView::Bytes {
                descriptor: Some(_),
                decode_error: None
            }
        ));
    }

    fn recorded(media_type: Option<&str>, body: Option<&str>) -> TraceResponse {
        TraceResponse {
            status_code: 200,
            media_type: media_type.map(str::to_owned),
            body: body.map(str::to_owned),
            ..TraceResponse::default()
        }
    }

    #[test]
    fn replay_rebuilds_text_by_the_recorded_media_type_without_its_charset() {
        let latin = Response::replayed(
            recorded(Some("text/plain;charset=iso-8859-1"), Some("caf\u{e9}")),
            Vec::new(),
        );
        assert_eq!(latin.body_value, BodySlot::present(json!("caf\u{e9}")));
        let json = Response::replayed(
            recorded(Some("application/json"), Some("{\"a\":1}")),
            Vec::new(),
        );
        assert_eq!(json.body_value, BodySlot::present(json!({"a": 1})));
        let truncated = Response::replayed(
            recorded(Some("application/json"), Some("{\"a\"...")),
            Vec::new(),
        );
        assert!(matches!(truncated.body_value, BodySlot::Unavailable { .. }));
        let untyped = Response::replayed(recorded(None, Some("{}")), Vec::new());
        assert_eq!(
            untyped.body_value,
            BodySlot::Unavailable {
                reason: "response has no Content-Type header, so its body is bytes, which is not yet a readable value".to_owned()
            }
        );
    }

    #[test]
    fn replay_rebuilds_a_recorded_descriptor_as_an_unavailable_body() {
        let mut png = recorded(Some("image/png"), None);
        png.headers = BTreeMap::from([("content-type".to_owned(), "image/png".to_owned())]);
        png.body_blob = Some(BytesDescriptor::of(
            b"\x89PNG",
            Some("image/png".into()),
            None,
        ));
        let replayed = Response::replayed(png.clone(), Vec::new());
        assert_eq!(
            replayed.body_value,
            BodySlot::Unavailable {
                reason: "response body is bytes (image/png), which is not yet a readable value"
                    .to_owned()
            }
        );
        assert!(replayed.body.is_empty());
        png.body_error = Some("response body is not valid UTF-8 text".to_owned());
        assert_eq!(
            Response::replayed(png, Vec::new()).body_value,
            BodySlot::Unavailable {
                reason: "response body is not valid UTF-8 text".to_owned()
            }
        );
        assert_eq!(
            Response::replayed(recorded(Some("text/plain"), None), Vec::new()).body_value,
            BodySlot::Absent
        );
    }
}
