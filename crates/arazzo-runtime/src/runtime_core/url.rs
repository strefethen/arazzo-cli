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
    .add(b'\\')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// A substituted path segment would be normalized as `.` or `..` by a
/// WHATWG URL parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PathParameterExpansionError {
    pub parameter_names: Vec<String>,
}

/// Provenance needed to validate path substitutions after query assembly has
/// finalized the URL's component boundaries.
#[derive(Debug, Clone)]
pub(super) struct PathParameterValidation {
    parts: Vec<SubstitutedPart>,
}

#[derive(Debug, Clone)]
struct SubstitutedPart {
    start: usize,
    end: usize,
    parameter_name: String,
}

#[derive(Debug, Clone, Copy)]
struct Template<'a> {
    start: usize,
    end: usize,
    key: &'a str,
}

#[derive(Debug, Default)]
struct PathSegmentGuard {
    segment: String,
    segment_params: Vec<String>,
    unsafe_params: Vec<String>,
}

impl PathSegmentGuard {
    fn append_literal(&mut self, literal: &str) {
        for part in literal.split_inclusive(['/', '\\']) {
            if let Some(text) = part.strip_suffix(['/', '\\']) {
                self.segment.push_str(text);
                self.finish_segment();
            } else {
                self.segment.push_str(part);
            }
        }
    }

    fn append_substitution(&mut self, encoded: &str, parameter_name: &str) {
        self.segment.push_str(encoded);
        if !self
            .segment_params
            .iter()
            .any(|name| name == parameter_name)
        {
            self.segment_params.push(parameter_name.to_string());
        }
    }

    fn finish_segment(&mut self) {
        if is_whatwg_dot_segment(&self.segment) {
            for name in &self.segment_params {
                if !self.unsafe_params.contains(name) {
                    self.unsafe_params.push(name.clone());
                }
            }
        }
        self.segment.clear();
        self.segment_params.clear();
    }

    fn finish(mut self, at_url_end: bool) -> Vec<String> {
        if at_url_end {
            // Url::parse trims terminal C0 controls and spaces before parsing.
            // Only trim the classification buffer, never the emitted URL, and
            // never a segment followed by a slash, query, or fragment.
            let end = self.segment.trim_end_matches(|ch| ch <= '\u{20}').len();
            self.segment.truncate(end);
        }
        self.finish_segment();
        self.unsafe_params
    }
}

impl PathParameterValidation {
    pub(super) fn validate(&self, url: &str) -> Result<(), PathParameterExpansionError> {
        let (path_start, path_end) = path_component_bounds(url);
        let mut guard = PathSegmentGuard::default();
        let mut cursor = path_start;

        for part in self.parts.iter().filter(|part| {
            part.end <= url.len()
                && if part.start == part.end {
                    part.start >= path_start && part.start <= path_end
                } else {
                    part.start >= path_start && part.end <= path_end
                }
        }) {
            guard.append_literal(&url[cursor..part.start]);
            guard.append_substitution(&url[part.start..part.end], &part.parameter_name);
            cursor = part.end;
        }
        guard.append_literal(&url[cursor..path_end]);

        let unsafe_params = guard.finish(path_end == url.len());
        if unsafe_params.is_empty() {
            Ok(())
        } else {
            Err(PathParameterExpansionError {
                parameter_names: unsafe_params,
            })
        }
    }
}

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

pub(super) fn replace_path_params(
    path: &str,
    params: &BTreeMap<String, String>,
) -> (String, PathParameterValidation) {
    let templates = find_templates(path);
    let mut out = String::with_capacity(path.len());
    let mut parts = Vec::new();
    let mut cursor = 0;

    for template in templates {
        out.push_str(&path[cursor..template.start]);

        if let Some(value) = params.get(template.key) {
            let encoded = utf8_percent_encode(value, PATH_SEGMENT_ENCODE_SET).to_string();
            let start = out.len();
            out.push_str(&encoded);
            parts.push(SubstitutedPart {
                start,
                end: out.len(),
                parameter_name: template.key.to_string(),
            });
        } else {
            out.push_str(&path[template.start..template.end]);
        }
        cursor = template.end;
    }

    out.push_str(&path[cursor..]);
    (out, PathParameterValidation { parts })
}

/// Finds templates with the same permissive matching behavior as the original
/// substitution loop: the first `}` after a `{` closes the template, and an
/// unmatched `{` leaves the rest of the string literal.
fn find_templates(path: &str) -> Vec<Template<'_>> {
    let mut templates = Vec::new();
    let mut cursor = 0;

    while let Some(open_rel) = path[cursor..].find('{') {
        let open = cursor + open_rel;
        let Some(close_rel) = path[open + 1..].find('}') else {
            break;
        };
        let close = open + 1 + close_rel;
        templates.push(Template {
            start: open,
            end: close + 1,
            key: &path[open + 1..close],
        });
        cursor = close + 1;
    }

    templates
}

/// Returns the byte range of the finalized URL's path component.
fn path_component_bounds(path: &str) -> (usize, usize) {
    let authority_end = authority_prefix_end(path);
    let path_start = authority_end
        .and_then(|start| {
            find_byte(path, start, |byte| {
                matches!(byte, b'/' | b'\\' | b'?' | b'#')
            })
        })
        .map(|(position, _)| position)
        .unwrap_or_else(|| authority_end.map_or(0, |_| path.len()));
    let path_end = find_byte(path, path_start, |byte| matches!(byte, b'?' | b'#'))
        .map_or(path.len(), |(position, _)| position);

    (path_start, path_end)
}

fn authority_prefix_end(path: &str) -> Option<usize> {
    if path.starts_with("//") {
        return Some(2);
    }

    let colon = path.find(':')?;
    let scheme = &path[..colon];
    let valid_scheme = !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'));
    (valid_scheme && path[colon..].starts_with("://")).then_some(colon + 3)
}

fn find_byte(path: &str, start: usize, predicate: impl Fn(u8) -> bool) -> Option<(usize, u8)> {
    path.as_bytes()[start..]
        .iter()
        .copied()
        .enumerate()
        .find_map(|(offset, byte)| predicate(byte).then_some((start + offset, byte)))
}

fn is_whatwg_dot_segment(segment: &str) -> bool {
    // The WHATWG URL parser removes ASCII tab and newlines before parsing.
    // Ignore authored instances for classification while leaving the output
    // untouched; controls originating in parameter values are percent-encoded.
    let stripped: Vec<u8> = segment
        .bytes()
        .filter(|byte| !matches!(byte, b'\t' | b'\n' | b'\r'))
        .collect();
    let mut rest = stripped.as_slice();
    let mut dots = 0;

    while !rest.is_empty() && dots < 2 {
        if rest[0] == b'.' {
            rest = &rest[1..];
        } else if rest.len() >= 3 && rest[..3].eq_ignore_ascii_case(b"%2e") {
            rest = &rest[3..];
        } else {
            return false;
        }
        dots += 1;
    }

    rest.is_empty() && matches!(dots, 1 | 2)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn substitutions(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    fn validated(
        url: &str,
        entries: &[(&str, &str)],
    ) -> Result<String, PathParameterExpansionError> {
        let (result, validation) = replace_path_params(url, &substitutions(entries));
        validation.validate(&result)?;
        Ok(result)
    }

    fn replaced(url: &str, entries: &[(&str, &str)]) -> String {
        match validated(url, entries) {
            Ok(result) => result,
            Err(error) => panic!("safe URL substitution was refused: {error:?}"),
        }
    }

    #[test]
    fn rejects_parameter_influenced_whatwg_dot_segments() {
        let cases = [
            ("https://example.test/a/{id}/z", vec![("id", ".")]),
            ("https://example.test/a/{id}/z", vec![("id", "..")]),
            ("https://example.test/a/%2{tail}/z", vec![("tail", "e")]),
            ("https://example.test/a/%2{tail}/z", vec![("tail", "E")]),
            ("https://example.test/a/.%2{tail}/z", vec![("tail", "e")]),
            ("https://example.test/a/%2{tail}./z", vec![("tail", "E")]),
            (
                "https://example.test/a/%2{left}%2{right}/z",
                vec![("left", "e"), ("right", "E")],
            ),
            (
                "https://example.test/a/{left}{right}/z",
                vec![("left", "."), ("right", ".")],
            ),
            ("https://example.test/a/{empty}./z", vec![("empty", "")]),
            ("https://example.test/a/{id}{id}/z", vec![("id", ".")]),
            ("https://example.test/a/{id}\\details", vec![("id", "..")]),
            ("https://example.test/a/.\t{id}/z", vec![("id", ".")]),
            ("https://example.test/a/%2\r{id}/z", vec![("id", "E")]),
            ("https://example.test/a/{id}\n/z", vec![("id", "..")]),
            (
                "https://example.test/v1/foo://{id}/details",
                vec![("id", "..")],
            ),
            (
                "https://example.test/a/{part?name}/z",
                vec![("part?name", "..")],
            ),
            (
                "https://example.test/a/{part#name}/z",
                vec![("part#name", ".")],
            ),
            (
                "https://example.test/a/{part/name}/z",
                vec![("part/name", "..")],
            ),
        ];

        for (url, entries) in cases {
            let err = match validated(url, &entries) {
                Ok(result) => panic!("dot segment was not refused: {result}"),
                Err(error) => error,
            };
            let expected_names: Vec<String> = entries
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let actual_names: BTreeSet<String> = err.parameter_names.into_iter().collect();
            assert_eq!(
                actual_names,
                expected_names.into_iter().collect(),
                "case: {url}"
            );
        }
    }

    #[test]
    fn terminal_url_whitespace_cannot_hide_a_substituted_dot_segment() {
        for (url, value) in [
            ("https://example.test/a/{id} ", ".."),
            ("https://example.test/a/{id}\0\t ", "."),
            ("https://example.test/a/.. {id}", ""),
        ] {
            assert!(validated(url, &[("id", value)]).is_err());
        }
        for url in [
            "https://example.test/a/{id} /z",
            "https://example.test/a/{id} ?next=x",
            "https://example.test/a/{id} #fragment",
        ] {
            assert_eq!(replaced(url, &[("id", "..")]), url.replace("{id}", ".."));
        }
        assert_eq!(
            replaced("https://example.test/a/{id}", &[("id", ".. ")]),
            "https://example.test/a/..%20"
        );
    }

    #[test]
    fn validation_uses_final_query_boundaries_without_reparsing_query_braces() {
        let (mut exposed, exposed_validation) = replace_path_params(
            "https://example.test/a/{id} ?",
            &substitutions(&[("id", "..")]),
        );
        let query = match exposed.find('?') {
            Some(position) => position,
            None => panic!("fixture must carry a query delimiter"),
        };
        exposed.truncate(query);
        assert!(exposed_validation.validate(&exposed).is_err());

        let (mut protected, protected_validation) = replace_path_params(
            "https://example.test/a/{id} ",
            &substitutions(&[("id", "..")]),
        );
        protected.push_str("?raw={value?/with#delimiters}");
        assert_eq!(protected_validation.validate(&protected), Ok(()));
        assert_eq!(
            protected,
            "https://example.test/a/.. ?raw={value?/with#delimiters}"
        );
    }

    #[test]
    fn matched_template_delimiters_are_not_url_structure() {
        assert_eq!(
            replaced(
                "https://example.test/a/{part/name}/z?next={query#name}#at-{fragment?name}",
                &[
                    ("part/name", "safe"),
                    ("query#name", ".."),
                    ("fragment?name", ".."),
                ],
            ),
            "https://example.test/a/safe/z?next=..#at-.."
        );
        assert_eq!(
            replaced("https://{host/name}/items", &[("host/name", "..")]),
            "https://../items"
        );
    }

    #[test]
    fn unresolved_template_delimiters_remain_literal_url_structure() {
        assert_eq!(
            replaced(
                "https://example.test/a/{part\\name}/{missing/name}{id}/z",
                &[("part\\name", "safe"), ("id", ".")],
            ),
            "https://example.test/a/safe/{missing/name}./z"
        );
        for url in [
            "https://example.test/a/{missing?name}/{id}",
            "https://example.test/a/{missing#name}/{id}",
        ] {
            assert_eq!(replaced(url, &[("id", "..")]), url.replace("{id}", ".."));
        }
    }

    #[test]
    fn preserves_safe_dots_percent_literals_and_static_dot_segments() {
        let safe = ["file..txt", "...", "%2e", "%2e-name", "name%2e"];
        for value in safe {
            let result = replaced("https://example.test/a/{id}/z", &[("id", value)]);
            assert!(result.starts_with("https://example.test/a/"), "{result}");
            assert!(result.ends_with("/z"), "{result}");
        }

        assert_eq!(
            replaced(
                "https://example.test/a/../{id}/%2e?next={next}#value={fragment}",
                &[("id", "safe"), ("next", ".."), ("fragment", ".")],
            ),
            "https://example.test/a/../safe/%2e?next=..#value=."
        );
        assert_eq!(
            replaced("https://example.test/a/..\\{id}/z", &[("id", "safe")],),
            "https://example.test/a/..\\safe/z"
        );
    }

    #[test]
    fn preserves_substitution_while_encoding_path_separators_and_unicode() {
        assert_eq!(
            replaced(
                "https://example.test/a/{id}/z",
                &[("id", "a/b\\c snowman ☃")],
            ),
            "https://example.test/a/a%2Fb%5Cc%20snowman%20%E2%98%83/z"
        );
        assert_eq!(
            replaced(
                "https://example.test/{known}/{unknown}",
                &[("known", "value"), ("unused", "ignored")],
            ),
            "https://example.test/value/{unknown}"
        );
        assert_eq!(
            replaced("https://example.test/{unclosed", &[("unclosed", "value")]),
            "https://example.test/{unclosed"
        );
    }
}
