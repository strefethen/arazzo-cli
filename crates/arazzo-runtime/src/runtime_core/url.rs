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

/// Splits the optional leading `"<METHOD> "` token off an operationPath.
///
/// Thin wrapper over the shared classifier in `arazzo-spec`: the runtime and
/// `arazzo-validate` must never drift on what counts as a method token, which
/// is exactly the drift that used to disable the validator's unknown-source
/// check for `"<METHOD> {source}./path"` steps.
pub(crate) fn parse_method(operation_path: &str) -> (&str, &str) {
    arazzo_spec::split_operation_method(operation_path)
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
