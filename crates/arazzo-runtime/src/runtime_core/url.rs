use super::*;

/// Characters to percent-encode in path segment values per RFC 3986 §3.3.
/// Allows unreserved chars (§2.3), sub-delimiters (§2.2), ':', and '@'
/// (all part of the `pchar` production). Non-ASCII bytes are always encoded.
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Result of building a URL from an operationPath, including resolved parameters.
#[derive(Debug, Clone)]
pub(crate) struct UrlBuildResult {
    pub url: String,
    pub path_params: BTreeMap<String, String>,
    pub query_params: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

/// Parse `{sourceName}./path` prefix from an operationPath.
/// Returns None if no `{name}.` prefix is found — the dot after `}` is required
/// to distinguish source references from path parameter placeholders like `/{id}/resource`.
pub(super) fn parse_source_prefix(op_path: &str) -> Option<(&str, &str)> {
    if !op_path.starts_with('{') {
        return None;
    }
    let close = op_path.find('}')?;
    let name = &op_path[1..close];
    if name.is_empty() {
        return None;
    }
    let remaining = &op_path[close + 1..];
    let path = remaining.strip_prefix('.')?;
    Some((name, path))
}

pub(crate) fn parse_method(operation_path: &str) -> (&str, &str) {
    let Some(idx) = operation_path.find(' ') else {
        return ("", operation_path);
    };
    if idx == 0 || idx > 7 {
        return ("", operation_path);
    }
    let candidate = &operation_path[..idx];
    let valid = matches!(
        candidate,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
    );
    if valid {
        return (candidate, &operation_path[idx + 1..]);
    }
    ("", operation_path)
}

pub(super) fn replace_path_params(path: &str, params: &BTreeMap<String, String>) -> String {
    let mut remaining = path;
    let mut out = String::with_capacity(path.len());

    loop {
        let Some(open) = remaining.find('{') else {
            out.push_str(remaining);
            break;
        };
        let Some(close_rel) = remaining[open + 1..].find('}') else {
            out.push_str(remaining);
            break;
        };
        let close = open + 1 + close_rel;
        out.push_str(&remaining[..open]);
        let key = &remaining[open + 1..close];
        if let Some(value) = params.get(key) {
            let encoded = utf8_percent_encode(value, PATH_SEGMENT_ENCODE_SET).to_string();
            out.push_str(&encoded);
        } else {
            out.push_str(&remaining[open..=close]);
        }
        remaining = &remaining[close + 1..];
    }

    out
}

/// Percent-encode cookie-unsafe characters in a cookie value.
///
/// RFC 6265 §4.1.1 forbids semicolons, commas, spaces, equals signs,
/// double quotes, and backslashes inside unquoted cookie values.
/// Percent-encoding these characters prevents the server from
/// mis-parsing a single cookie value as multiple cookies.
pub(super) fn encode_cookie_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            ';' => out.push_str("%3B"),
            ',' => out.push_str("%2C"),
            ' ' => out.push_str("%20"),
            '=' => out.push_str("%3D"),
            '"' => out.push_str("%22"),
            '\\' => out.push_str("%5C"),
            _ => out.push(ch),
        }
    }
    out
}
