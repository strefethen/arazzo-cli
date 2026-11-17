#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaType {
    type_: String,
    subtype: String,
    parameters: Vec<(String, String)>,
}

impl MediaType {
    pub fn parse(text: &str) -> Option<Self> {
        let essence_end = text.find(';').unwrap_or(text.len());
        let (type_, subtype) = text[..essence_end].trim_matches(is_ows).split_once('/')?;
        if !is_token(type_) || !is_token(subtype) {
            return None;
        }
        let parameters = parse_parameters(&text[essence_end..])?;
        Some(Self {
            type_: type_.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            parameters,
        })
    }

    pub fn type_(&self) -> &str {
        &self.type_
    }

    pub fn subtype(&self) -> &str {
        &self.subtype
    }

    pub fn essence(&self) -> String {
        format!("{}/{}", self.type_, self.subtype)
    }

    pub fn suffix(&self) -> Option<&str> {
        self.subtype
            .rsplit_once('+')
            .filter(|(base, suffix)| !base.is_empty() && !suffix.is_empty())
            .map(|(_, suffix)| suffix)
    }

    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn charset(&self) -> Option<&str> {
        self.parameter("charset")
    }

    pub fn boundary(&self) -> Option<&str> {
        self.parameter("boundary")
    }

    pub fn is_json(&self) -> bool {
        self.is("application", "json") || self.suffix() == Some("json")
    }

    pub fn is_xml(&self) -> bool {
        self.is("application", "xml") || self.is("text", "xml") || self.suffix() == Some("xml")
    }

    pub fn is_text(&self) -> bool {
        self.type_ == "text"
    }

    pub fn is_form_urlencoded(&self) -> bool {
        self.is("application", "x-www-form-urlencoded")
    }

    pub fn is_multipart_form_data(&self) -> bool {
        self.is("multipart", "form-data")
    }

    fn is(&self, type_: &str, subtype: &str) -> bool {
        self.type_ == type_ && self.subtype == subtype
    }
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.type_, self.subtype)?;
        for (name, value) in &self.parameters {
            if is_token(value) {
                write!(f, ";{name}={value}")?;
            } else {
                write!(f, ";{name}=\"")?;
                for ch in value.chars() {
                    if ch == '"' || ch == '\\' {
                        f.write_str("\\")?;
                    }
                    write!(f, "{ch}")?;
                }
                f.write_str("\"")?;
            }
        }
        Ok(())
    }
}

pub fn is_json(content_type: &str) -> bool {
    MediaType::parse(content_type).is_some_and(|media_type| media_type.is_json())
}

pub fn is_xml(content_type: &str) -> bool {
    MediaType::parse(content_type).is_some_and(|media_type| media_type.is_xml())
}

pub fn is_form_urlencoded(content_type: &str) -> bool {
    MediaType::parse(content_type).is_some_and(|media_type| media_type.is_form_urlencoded())
}

pub fn is_multipart_form_data(content_type: &str) -> bool {
    MediaType::parse(content_type).is_some_and(|media_type| media_type.is_multipart_form_data())
}

fn parse_parameters(mut rest: &str) -> Option<Vec<(String, String)>> {
    let mut parameters: Vec<(String, String)> = Vec::new();
    loop {
        rest = rest.trim_start_matches(is_ows);
        let Some(after_semicolon) = rest.strip_prefix(';') else {
            return rest.is_empty().then_some(parameters);
        };
        rest = after_semicolon.trim_start_matches(is_ows);
        if rest.is_empty() || rest.starts_with(';') {
            continue;
        }
        let name_end = rest.find(|ch: char| !is_tchar(ch)).unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = rest[name_end..].strip_prefix('=')?;
        if name.is_empty() {
            return None;
        }
        let value;
        (value, rest) = if let Some(quoted) = rest.strip_prefix('"') {
            parse_quoted_string(quoted)?
        } else {
            let value_end = rest.find(|ch: char| !is_tchar(ch)).unwrap_or(rest.len());
            if value_end == 0 {
                return None;
            }
            (rest[..value_end].to_owned(), &rest[value_end..])
        };
        if !parameters
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
        {
            parameters.push((name.to_ascii_lowercase(), value));
        }
    }
}

fn parse_quoted_string(text: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return Some((value, &text[index + 1..])),
            '\\' => value.push(chars.next().map(|(_, escaped)| escaped)?),
            ch if ch == '\t' || (' '..='~').contains(&ch) || !ch.is_ascii() => value.push(ch),
            _ => return None,
        }
    }
    None
}

fn is_ows(ch: char) -> bool {
    ch == ' ' || ch == '\t'
}

fn is_token(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_tchar)
}

fn is_tchar(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> MediaType {
        MediaType::parse(text).unwrap_or_else(|| panic!("{text:?} should parse"))
    }

    #[test]
    fn type_subtype_and_parameter_names_are_case_insensitive() {
        let media_type = parsed("Application/JSON; CharSet=UTF-8");
        assert_eq!(media_type.essence(), "application/json");
        assert_eq!(media_type.type_(), "application");
        assert_eq!(media_type.subtype(), "json");
        assert_eq!(media_type.charset(), Some("UTF-8"));
        assert_eq!(media_type.parameter("CHARSET"), Some("UTF-8"));
        assert!(media_type.is_json());
        assert!(parsed("TEXT/Plain").is_text());
    }

    #[test]
    fn structured_syntax_suffixes_classify_json_and_xml() {
        assert_eq!(parsed("application/problem+json").suffix(), Some("json"));
        assert!(parsed("application/problem+json").is_json());
        assert!(parsed("application/vnd.api+JSON").is_json());
        assert!(parsed("application/atom+xml").is_xml());
        assert!(parsed("application/rss+xml").is_xml());
        assert!(parsed("text/xml").is_xml());
        assert!(parsed("application/xml").is_xml());
        assert_eq!(parsed("application/+json").suffix(), None);
        assert_eq!(parsed("application/json+").suffix(), None);
        for text in [
            "application/x-ndjson",
            "application/jsonl",
            "text/json",
            "application/json-patch",
            "application/rss",
            "application/xhtml",
            "text/plain",
        ] {
            let media_type = parsed(text);
            assert!(!media_type.is_json(), "{text}");
            assert!(!media_type.is_xml(), "{text}");
        }
    }

    #[test]
    fn form_and_multipart_compare_the_essence_exactly() {
        assert!(is_form_urlencoded(
            "Application/X-WWW-Form-Urlencoded; charset=UTF-8"
        ));
        assert!(!is_form_urlencoded(
            "application/x-www-form-urlencoded-extra"
        ));
        let multipart = parsed("multipart/form-data; boundary=\"a b;c\"");
        assert!(multipart.is_multipart_form_data());
        assert_eq!(multipart.boundary(), Some("a b;c"));
        assert!(!is_multipart_form_data("multipart/mixed; boundary=x"));
    }

    #[test]
    fn quoted_parameter_values_unescape_and_may_hold_separators() {
        let media_type = parsed(r#"text/plain; charset="iso-8859-1"; note="a\"b\\c; d=e""#);
        assert_eq!(media_type.charset(), Some("iso-8859-1"));
        assert_eq!(media_type.parameter("note"), Some(r#"a"b\c; d=e"#));
        assert_eq!(media_type.parameter("d"), None);
    }

    #[test]
    fn whitespace_empty_segments_and_repeated_names_follow_the_grammar() {
        let media_type = parsed("  text/plain ;; charset=utf-8 ;\tCharset=latin1 ; ");
        assert_eq!(media_type.charset(), Some("utf-8"));
        assert_eq!(media_type.to_string(), "text/plain;charset=utf-8");
        assert_eq!(
            parsed(r#"multipart/form-data;boundary="x y""#).to_string(),
            r#"multipart/form-data;boundary="x y""#
        );
    }

    #[test]
    fn unparseable_input_is_rejected() {
        for text in [
            "",
            "   ",
            "json",
            "/json",
            "application/",
            "application /json",
            "application/json extra",
            "application/js(on",
            "text/plain; charset",
            "text/plain; charset=",
            "text/plain; =utf-8",
            "text/plain; charset = utf-8",
            "text/plain; charset=\"unterminated",
            "text/plain; charset=\"x\"y",
            "text/plain; charset=a b",
        ] {
            assert_eq!(MediaType::parse(text), None, "{text:?}");
            assert!(!is_json(text) && !is_xml(text), "{text:?}");
        }
    }
}
