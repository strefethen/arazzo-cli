use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use arazzo_spec::{PropertyDef, SchemaObject};
use regex::Regex;
use serde_json::Value;

use super::{DryRunRequest, TraceRequest};

pub const REDACTED: &str = "[REDACTED]";

/// Header/field names that are always sensitive (exact match, case-insensitive).
const SENSITIVE_EXACT: [&str; 9] = [
    "proxy-authorization",
    "set-cookie",
    "x-api-key",
    "api-key",
    "privatekey",
    "xauthkey",
    "authkey",
    "subscriptionkey",
    "ocpapimsubscriptionkey",
];

/// Stems matched via `contains` to catch compound names like `bearerToken`.
const SENSITIVE_STEMS: [&str; 10] = [
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "apikey",
    "cookie",
    "session",
    "credential",
    "pwd",
];

/// Lowercases `name` and drops the `-`, `_`, and space separators, so
/// `api_key`, `api-key`, `apiKey`, and `API KEY` all compare as `apikey`.
fn normalize_key(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether a header, query-parameter, JSON-field, or input name looks like a
/// credential. Matching is case- and separator-insensitive: the name and the
/// table entries are both compared with `-`, `_`, and space removed.
pub fn is_sensitive_key(name: &str) -> bool {
    let normalized = normalize_key(name);
    if SENSITIVE_EXACT
        .iter()
        .any(|key| normalized == normalize_key(key))
    {
        return true;
    }
    SENSITIVE_STEMS
        .iter()
        .any(|stem| normalized.contains(&normalize_key(stem)))
}

/// Whether a workflow input property's schema declares its value a secret:
/// the JSON Schema `writeOnly: true` annotation or `format: password`.
pub fn is_schema_declared_secret(property: &PropertyDef) -> bool {
    property.is_write_only() || property.format == "password"
}

/// The set of workflow input names whose values must never be written in
/// clear: every name in `names` that [`is_sensitive_key`] matches, plus every
/// top-level property of `schema` that [`is_schema_declared_secret`] marks.
/// Nested object input schemas are not consulted.
pub fn sensitive_input_names<'a>(
    schema: Option<&SchemaObject>,
    names: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<String> {
    let mut sensitive: BTreeSet<String> = names
        .into_iter()
        .filter(|name| is_sensitive_key(name))
        .map(str::to_string)
        .collect();
    if let Some(schema) = schema {
        sensitive.extend(
            schema
                .properties
                .iter()
                .filter(|(_, property)| is_schema_declared_secret(property))
                .map(|(name, _)| name.clone()),
        );
    }
    sensitive
}

/// Redacts a workflow-inputs map: a top-level entry named in `sensitive`
/// (see [`sensitive_input_names`]) is replaced whole; every other entry has
/// its nested JSON redacted by key name, as [`redact_json_value`] does.
pub fn redact_inputs(map: &mut BTreeMap<String, Value>, sensitive: &BTreeSet<String>) {
    for (key, value) in map {
        if sensitive.contains(key) || is_sensitive_key(key) {
            *value = Value::String(REDACTED.to_string());
        } else {
            redact_json_value(value);
        }
    }
}

pub fn redact_trace_request(request: &mut TraceRequest) {
    redact_headers(&mut request.headers);
    redact_url(&mut request.url);
    for hop in &mut request.redirects {
        redact_url(&mut hop.from);
        redact_url(&mut hop.to);
    }
    if let Some(body) = &mut request.body {
        redact_request_body(body);
    }
}

pub fn redact_dry_run_request(request: &mut DryRunRequest) {
    redact_headers(&mut request.headers);
    redact_url(&mut request.url);
    if let Some(body) = &mut request.body {
        redact_request_body(body);
    }
}

fn redact_request_body(body: &mut Value) {
    match body {
        Value::String(text) => *text = redact_text_body(text),
        structured => redact_json_value(structured),
    }
}

fn redact_text_body(text: &str) -> String {
    text.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_form_field_name(key) && is_sensitive_query_name(key) => {
                format!("{key}={REDACTED}")
            }
            _ => redact_text_patterns(pair),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_form_field_name(key: &str) -> bool {
    !key.is_empty() && !key.chars().any(char::is_whitespace)
}

pub fn redacted_dry_run_request(mut request: DryRunRequest) -> DryRunRequest {
    redact_dry_run_request(&mut request);
    request
}

pub fn redact_headers(headers: &mut BTreeMap<String, String>) {
    for (name, value) in headers {
        if is_sensitive_key(name) {
            *value = REDACTED.to_string();
        }
    }
}

fn is_sensitive_query_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            for _ in 0..2 {
                if !bytes.next().is_some_and(|digit| digit.is_ascii_hexdigit()) {
                    return is_sensitive_key(name);
                }
            }
        }
    }
    percent_encoding::percent_decode_str(name)
        .decode_utf8()
        .map_or_else(
            |_| is_sensitive_key(name),
            |decoded| is_sensitive_key(&decoded),
        )
}

pub fn redact_url(url: &mut String) {
    // Edit only the authority's userinfo, preserving the original URL bytes
    // everywhere else. An @ in the path, query, or fragment is not userinfo.
    if let Some(scheme_end) = url.find("://") {
        let authority_start = scheme_end + 3;
        let authority = &url[authority_start..];
        let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
        if let Some(userinfo_end) = authority[..authority_end].rfind('@') {
            url.replace_range(authority_start..authority_start + userinfo_end, REDACTED);
        }
    }
    // Parse query params without reconstructing the URL to avoid
    // normalization artifacts (percent-encoding changes, reordering)
    // that could cause replay drift.
    let query_end = url.find('#').unwrap_or(url.len());
    let Some(query_start) = url[..query_end].find('?') else {
        return;
    };
    let query = &url[query_start + 1..query_end];
    if query.is_empty() {
        return;
    }

    let mut redacted_query = String::with_capacity(query.len());
    for (i, pair) in query.split('&').enumerate() {
        if i > 0 {
            redacted_query.push('&');
        }
        if let Some((key, _value)) = pair.split_once('=') {
            if is_sensitive_query_name(key) {
                redacted_query.push_str(key);
                redacted_query.push('=');
                redacted_query.push_str(REDACTED);
            } else {
                redacted_query.push_str(pair);
            }
        } else {
            redacted_query.push_str(pair);
        }
    }

    url.replace_range(query_start + 1..query_end, &redacted_query);
}

pub(crate) fn redacted_url(url: &str) -> String {
    let mut redacted = url.to_string();
    redact_url(&mut redacted);
    redacted
}

pub fn redact_url_components(url: &mut String) {
    redact_url(url);
    let (before_fragment, fragment) = match url.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (url.as_str(), None),
    };
    let (before_query, query) = match before_fragment.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (before_fragment, None),
    };
    let mut redacted = redact_text_patterns(before_query);
    if let Some(query) = query {
        redacted.push('?');
        for (i, pair) in query.split('&').enumerate() {
            if i > 0 {
                redacted.push('&');
            }
            redacted.push_str(&redact_text_patterns(pair));
        }
    }
    if let Some(fragment) = fragment {
        redacted.push('#');
        redacted.push_str(&redact_text_patterns(fragment));
    }
    *url = redacted;
}

pub fn redact_json_object(map: &mut BTreeMap<String, Value>) {
    for (key, value) in map {
        if is_sensitive_key(key) {
            *value = Value::String(REDACTED.to_string());
        } else {
            redact_json_value(value);
        }
    }
}

pub fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if is_sensitive_key(key) {
                    *nested = Value::String(REDACTED.to_string());
                } else {
                    redact_json_value(nested);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

/// Redact common secret patterns in non-JSON text (XML, HTML, plain text, etc.).
/// Uses lazily-compiled regexes so the patterns are built once across all calls.
pub fn redact_text_patterns(text: &str) -> String {
    // Bearer / Basic / token auth headers embedded in text.
    static RE_BEARER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+")
            .unwrap_or_else(|err| panic!("failed to compile bearer regex: {err}"))
    });
    // key=value or key: value where the key looks sensitive.
    static RE_KV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(password|passwd|secret|token|authorization|apikey|api_key|credential|pwd|private[-_ ]*key|x[-_ ]*auth[-_ ]*key|auth[-_ ]*key|subscription[-_ ]*key|ocp[-_ ]*apim[-_ ]*subscription[-_ ]*key)(\s*[:=]\s*)\S+",
        )
        .unwrap_or_else(|err| panic!("failed to compile kv regex: {err}"))
    });

    let out = RE_BEARER.replace_all(text, |caps: &regex::Captures<'_>| {
        format!("{} {REDACTED}", &caps[1])
    });
    let out = RE_KV.replace_all(&out, |caps: &regex::Captures<'_>| {
        format!("{}{}{REDACTED}", &caps[1], &caps[2])
    });
    out.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_userinfo_with_multiple_at_signs_is_redacted_whole() {
        let mut url =
            "https://user:prefix@LEAKED-SECRET@EXAMPLE.com/%2f?Name=a+b&Name=%2F#fragment@visible"
                .to_string();
        redact_url(&mut url);
        assert_eq!(
            url,
            "https://[REDACTED]@EXAMPLE.com/%2f?Name=a+b&Name=%2F#fragment@visible"
        );
        assert!(!url.contains("LEAKED-SECRET"));
    }

    #[test]
    fn url_userinfo_is_redacted_without_normalizing_other_bytes() {
        for (input, expected) in [
            (
                "https://user:pass@EXAMPLE.com/%2f",
                "https://[REDACTED]@EXAMPLE.com/%2f",
            ),
            ("https://token@host/", "https://[REDACTED]@host/"),
            ("https://@host/", "https://[REDACTED]@host/"),
            (
                "https://EXAMPLE.com/%2f?Name=%2f&Name=%2F&name=a+b",
                "https://EXAMPLE.com/%2f?Name=%2f&Name=%2F&name=a+b",
            ),
            (
                "https://host/path?next=user@other",
                "https://host/path?next=user@other",
            ),
            ("https://host/path@other", "https://host/path@other"),
            ("https://host#user@other", "https://host#user@other"),
            (
                "https://user:pass@host/items?token=secret&page=%2f",
                "https://[REDACTED]@host/items?token=[REDACTED]&page=%2f",
            ),
        ] {
            let mut actual = input.to_string();
            redact_url(&mut actual);
            assert_eq!(actual, expected, "input: {input}");
        }
    }

    #[test]
    fn dry_run_request_redacts_sensitive_parts() {
        let mut req = DryRunRequest {
            step_id: "s1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/items?token=abc&page=1".to_string(),
            headers: BTreeMap::from([
                ("Authorization".to_string(), "Bearer abc".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
            ]),
            body: Some(json!({
                "clientSecret": "shh",
                "safeName": "alice",
                "nested": { "dbPassword": "hunter2" }
            })),
            warnings: Vec::new(),
        };

        redact_dry_run_request(&mut req);

        assert_eq!(req.url, "https://example.com/items?token=[REDACTED]&page=1");
        assert_eq!(
            req.headers.get("Authorization").map(String::as_str),
            Some(REDACTED)
        );
        assert_eq!(
            req.headers.get("Accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            req.body
                .as_ref()
                .and_then(|body| body.pointer("/clientSecret")),
            Some(&json!(REDACTED))
        );
        assert_eq!(
            req.body.as_ref().and_then(|body| body.pointer("/safeName")),
            Some(&json!("alice"))
        );
        assert_eq!(
            req.body
                .as_ref()
                .and_then(|body| body.pointer("/nested/dbPassword")),
            Some(&json!(REDACTED))
        );
    }

    fn redacted_string_body(body: &str) -> (Value, Value) {
        let mut trace = TraceRequest {
            method: "POST".to_string(),
            url: "https://example.com/token".to_string(),
            headers: BTreeMap::new(),
            body: Some(json!(body)),
            redirects: Vec::new(),
        };
        let mut dry_run = DryRunRequest {
            step_id: "s1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/token".to_string(),
            headers: BTreeMap::new(),
            body: Some(json!(body)),
            warnings: Vec::new(),
        };
        redact_trace_request(&mut trace);
        redact_dry_run_request(&mut dry_run);
        (
            trace.body.unwrap_or_else(|| panic!("trace body")),
            dry_run.body.unwrap_or_else(|| panic!("dry-run body")),
        )
    }

    #[test]
    fn form_text_body_redacts_sensitive_field_values() {
        for (body, expected) in [
            (
                "grant_type=password&username=u&password=hunter2",
                "grant_type=password&username=u&password=[REDACTED]",
            ),
            (
                "client_secret=s3cr3t&scope=read&refresh_token=r1&page=2",
                "client_secret=[REDACTED]&scope=read&refresh_token=[REDACTED]&page=2",
            ),
            (
                "private%5Fkey=pk&key=visible",
                "private%5Fkey=[REDACTED]&key=visible",
            ),
        ] {
            let (trace, dry_run) = redacted_string_body(body);
            assert_eq!(trace, json!(expected), "trace: {body}");
            assert_eq!(dry_run, json!(expected), "dry-run: {body}");
        }
    }

    #[test]
    fn text_body_redacts_embedded_credentials_like_response_bodies() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1In0.c2lnbmF0dXJl";
        for body in [
            format!("Authorization: Bearer {jwt}\nhello world"),
            format!(
                "<auth><header>Authorization: Bearer {jwt}</header><name>a&amp;b</name></auth>"
            ),
        ] {
            let expected = redact_text_patterns(&body);
            assert!(!expected.contains(jwt));
            assert!(expected.contains(REDACTED));
            let (trace, dry_run) = redacted_string_body(&body);
            assert_eq!(trace, json!(expected), "trace: {body}");
            assert_eq!(dry_run, json!(expected), "dry-run: {body}");
        }
    }

    #[test]
    fn sensitive_field_masks_its_whole_value_including_whitespace() {
        for (body, expected) in [
            ("password=hunter2\nusername=alice", "password=[REDACTED]"),
            ("token=abc and the rest of the message", "token=[REDACTED]"),
            ("password=correct horse battery staple", "password=[REDACTED]"),
            (
                "private_key=-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQ==\n-----END PRIVATE KEY-----",
                "private_key=[REDACTED]",
            ),
            (
                "grant_type=password&password=hunter2\nusername=alice",
                "grant_type=password&password=[REDACTED]",
            ),
            (
                "note: password=hunter2\nusername=alice",
                "note: password=[REDACTED]\nusername=alice",
            ),
        ] {
            let (trace, dry_run) = redacted_string_body(body);
            assert_eq!(trace, json!(expected), "trace: {body}");
            assert_eq!(dry_run, json!(expected), "dry-run: {body}");
        }
    }

    #[test]
    fn string_body_without_secrets_is_written_verbatim() {
        for body in [
            "grant_type=client_credentials&username=u&scope=read%20write",
            "plain text body with no secrets",
            "<order><item id=\"1\">book</item><note>a &amp; b</note></order>",
            "",
        ] {
            let (trace, dry_run) = redacted_string_body(body);
            assert_eq!(trace, json!(body));
            assert_eq!(dry_run, json!(body));
        }
    }

    #[test]
    fn redacted_string_body_is_stable_under_repeated_redaction() {
        let body = "grant_type=password&password=hunter2&note=Authorization: Bearer abc.def";
        let (once, _) = redacted_string_body(body);
        let (twice, _) =
            redacted_string_body(once.as_str().unwrap_or_else(|| panic!("string body")));
        assert_eq!(once, twice);
        assert!(!once.to_string().contains("hunter2"));
        assert!(!once.to_string().contains("abc.def"));
    }

    #[test]
    fn sensitive_key_matching_ignores_case_and_separators() {
        for name in [
            "api_key",
            "api-key",
            "apiKey",
            "API_KEY",
            "x_api_key",
            "X-Api-Key",
            "api key",
            "access_token",
            "authorization_key",
            // Existing-stem control: already matched `apikey` by `contains`.
            "apikeyless",
        ] {
            assert!(is_sensitive_key(name), "{name} should be sensitive");
        }
        for name in ["notes", "key", "api", "page", "x-request-id", "accept"] {
            assert!(!is_sensitive_key(name), "{name} should not be sensitive");
        }
    }

    #[test]
    fn exact_credential_names_ignore_case_and_repeated_separators() {
        for (camel, words) in [
            ("privateKey", vec!["private", "key"]),
            ("xAuthKey", vec!["x", "auth", "key"]),
            ("authKey", vec!["auth", "key"]),
            ("subscriptionKey", vec!["subscription", "key"]),
            (
                "ocpApimSubscriptionKey",
                vec!["ocp", "apim", "subscription", "key"],
            ),
        ] {
            assert!(is_sensitive_key(camel), "{camel}");
            for separator in ["", "-", "_", " ", "- _-- "] {
                let name = words.join(separator);
                assert!(is_sensitive_key(&name), "{name}");
                assert!(is_sensitive_key(&name.to_ascii_uppercase()), "{name}");
            }
        }
    }

    #[test]
    fn exact_credential_names_reach_all_structured_helpers() {
        for name in [
            "privateKey",
            "X-Auth-Key",
            "auth_key",
            "subscription key",
            "Ocp-Apim-Subscription-Key",
        ] {
            let mut fields = BTreeMap::from([
                (name.to_string(), json!({ "whole": "SYNTHETIC-subtree" })),
                (
                    "container".to_string(),
                    json!([{name: "SYNTHETIC-nested", "key": "visible"}]),
                ),
            ]);
            let expected = BTreeMap::from([
                (name.to_string(), json!(REDACTED)),
                (
                    "container".to_string(),
                    json!([{name: REDACTED, "key": "visible"}]),
                ),
            ]);
            let sensitive = sensitive_input_names(None, [name, "container"]);
            assert_eq!(sensitive, BTreeSet::from([name.to_string()]));
            let mut inputs = fields.clone();
            redact_inputs(&mut inputs, &sensitive);
            assert_eq!(inputs, expected, "{name}");
            redact_json_object(&mut fields);
            assert_eq!(fields, expected, "{name}");

            let mut request = DryRunRequest {
                step_id: "probe".to_string(),
                method: "POST".to_string(),
                url: format!("https://example.com/?{name}=SYNTHETIC-query&key=visible"),
                headers: BTreeMap::from([
                    (name.to_string(), "SYNTHETIC-header".to_string()),
                    ("key".to_string(), "visible".to_string()),
                ]),
                body: Some(
                    json!({name: {"whole": "SYNTHETIC-body"}, "items": [{name: "SYNTHETIC-array", "key": "visible"}]}),
                ),
                warnings: Vec::new(),
            };
            redact_dry_run_request(&mut request);
            assert_eq!(request.headers[name], REDACTED);
            assert_eq!(request.headers["key"], "visible");
            assert_eq!(
                request.url,
                format!("https://example.com/?{name}={REDACTED}&key=visible")
            );
            assert_eq!(
                request.body,
                Some(json!({name: REDACTED, "items": [{name: REDACTED, "key": "visible"}]}))
            );
        }
    }

    #[test]
    fn exact_credential_names_preserve_business_and_near_match_keys() {
        for name in [
            "key",
            "Key",
            "primaryKey",
            "partition_key",
            "sortKey",
            "foreignKey",
            "Idempotency-Key",
            "publicKey",
            "keyboardLayout",
            "x-private-key",
            "privateKeyId",
            "subscriptionKeyName",
            "x-subscription-key",
        ] {
            assert!(!is_sensitive_key(name), "{name}");
            let mut fields = BTreeMap::from([(name.to_string(), json!("visible"))]);
            let original = fields.clone();
            redact_json_object(&mut fields);
            assert_eq!(fields, original, "{name}");
            let sensitive = sensitive_input_names(None, [name]);
            assert!(sensitive.is_empty(), "{name}");
            redact_inputs(&mut fields, &sensitive);
            assert_eq!(fields, original, "{name}");
            let mut headers = BTreeMap::from([(name.to_string(), "visible".to_string())]);
            redact_headers(&mut headers);
            assert_eq!(headers[name], "visible");
            let mut url = format!("https://example.com/?{name}=visible");
            let original = url.clone();
            redact_url(&mut url);
            assert_eq!(url, original, "{name}");
        }
    }

    #[test]
    fn query_names_decode_once_and_leave_other_structured_names_undecoded() {
        for name in [
            "x%2Dauth%2Dkey",
            "x%2dauth%2dkey",
            "private%5Fkey",
            "Private%20Key",
            "api%5Fkey",
            "%74oken",
            "access%5Ftoken",
            "%70%72%69%76%61%74%65%4b%65%79",
        ] {
            let mut url = format!("https://example.com/?{name}=SYNTHETIC");
            redact_url(&mut url);
            assert_eq!(
                url,
                format!("https://example.com/?{name}={REDACTED}"),
                "{name}"
            );
        }
        for name in [
            "%6Bey",
            "private%255Fkey",
            "private+key",
            "private%2Bkey",
            "%2574oken",
        ] {
            let mut url = format!("https://example.com/?{name}=SYNTHETIC");
            let original = url.clone();
            redact_url(&mut url);
            assert_eq!(url, original, "{name}");
        }
        let mut headers = BTreeMap::from([("private%5Fkey".to_string(), "visible".to_string())]);
        redact_headers(&mut headers);
        assert_eq!(headers["private%5Fkey"], "visible");
        let mut value = json!({"private%5Fkey": "visible"});
        redact_json_value(&mut value);
        assert_eq!(value, json!({"private%5Fkey": "visible"}));
        assert!(sensitive_input_names(None, ["private%5Fkey"]).is_empty());
    }

    #[test]
    fn invalid_query_decoding_classifies_the_whole_original_name() {
        for (name, sensitive) in [
            ("%", false),
            ("%2", false),
            ("%GG", false),
            ("%2G", false),
            ("%FF", false),
            ("%C3%28", false),
            ("%C3", false),
            ("private%key", false),
            ("private%", false),
            ("private%FFkey", false),
            ("%74oken%", false),
            ("%74oken%2", false),
            ("%74oken%GG", false),
            ("%74oken%2G", false),
            ("%74oken%FF", false),
            ("%74oken%C3%28", false),
            ("%74oken%C3", false),
            ("token%GG", true),
            ("token%FF", true),
            ("t%C3%B6ken", false),
            ("pr%C4%B0vateKey", false),
            ("%F0%9F%94%91", false),
            ("%C3%A9token", true),
        ] {
            let mut url = format!("https://example.com/?{name}=SYNTHETIC");
            redact_url(&mut url);
            let expected = if sensitive { REDACTED } else { "SYNTHETIC" };
            assert_eq!(
                url,
                format!("https://example.com/?{name}={expected}"),
                "{name}"
            );
        }
    }

    #[test]
    fn query_redaction_preserves_all_other_url_bytes_and_fragment_boundaries() {
        for (original, expected) in [
            (
                "https://host/path?private%5Fkey=CANARY&key=visible#token=fragment-visible",
                "https://host/path?private%5Fkey=[REDACTED]&key=visible#token=fragment-visible",
            ),
            ("https://host/path#fragment?token=visible", "https://host/path#fragment?token=visible"),
            ("https://host/path?token=CANARY#fragment?token=visible", "https://host/path?token=[REDACTED]#fragment?token=visible"),
            ("https://host/path?#token=visible", "https://host/path?#token=visible"),
            (
                "https://u:p@EXAMPLE.com/PaTh/%2f?privateKey=CANARY&&key=a=b%26c%3Dd&private%5fkey=&privateKey=SECOND&flag&=empty&privateKey&tail=#token=visible&privateKey=fragment",
                "https://[REDACTED]@EXAMPLE.com/PaTh/%2f?privateKey=[REDACTED]&&key=a=b%26c%3Dd&private%5fkey=[REDACTED]&privateKey=[REDACTED]&flag&=empty&privateKey&tail=#token=visible&privateKey=fragment",
            ),
            ("https://host/path?&&key=&flag&&", "https://host/path?&&key=&flag&&"),
        ] {
            let mut actual = original.to_string();
            redact_url(&mut actual);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn url_components_keep_pairs_after_raw_names_and_mask_text_per_component() {
        for (original, expected) in [
            (
                "https://host/path?privateKey=CANARY&key=visible#fragment-visible",
                "https://host/path?privateKey=[REDACTED]&key=visible#fragment-visible",
            ),
            (
                "https://host/path?X-Auth-Key=A&key=visible&authKey=B&&subscription_key=C&page=&Ocp-Apim-Subscription-Key=D&tail=visible",
                "https://host/path?X-Auth-Key=[REDACTED]&key=visible&authKey=[REDACTED]&&subscription_key=[REDACTED]&page=&Ocp-Apim-Subscription-Key=[REDACTED]&tail=visible",
            ),
            (
                "https://host/path?private%5Fkey=CANARY&key=visible#token=fragment-visible",
                "https://host/path?private%5Fkey=[REDACTED]&key=visible#token=[REDACTED]",
            ),
            (
                "https://host/path?privateKey=ONE&privateKey=TWO&flag&=empty&privateKey&tail=",
                "https://host/path?privateKey=[REDACTED]&privateKey=[REDACTED]&flag&=empty&privateKey&tail=",
            ),
            (
                "https://host/path?q=token=CANARY&key=a=b%26c%3Dd&note=Bearer",
                "https://host/path?q=token=[REDACTED]&key=a=b%26c%3Dd&note=Bearer",
            ),
            (
                "https://host/token=CANARY",
                "https://host/token=[REDACTED]",
            ),
            (
                "https://host/token=CANARY?key=visible#fragment-visible",
                "https://host/token=[REDACTED]?key=visible#fragment-visible",
            ),
            (
                "https://host/path#token=CANARY",
                "https://host/path#token=[REDACTED]",
            ),
            (
                "https://host/path?key=visible#token=CANARY",
                "https://host/path?key=visible#token=[REDACTED]",
            ),
            (
                "https://host/path#fragment?privateKey=CANARY&key=visible",
                "https://host/path#fragment?privateKey=[REDACTED]",
            ),
            (
                "https://u:p@EXAMPLE.com/PaTh/%2f?x-private-key=CANARY&%6Bey=visible&private+key=visible&key=#",
                "https://[REDACTED]@EXAMPLE.com/PaTh/%2f?x-private-key=[REDACTED]&%6Bey=visible&private+key=visible&key=#",
            ),
            ("https://host/path?token:CANARY", "https://host/path?token:[REDACTED]"),
            (
                "https://host/path?a=1&token:CANARY&b=2#fragment-visible",
                "https://host/path?a=1&token:[REDACTED]&b=2#fragment-visible",
            ),
            ("https://host/path?flag", "https://host/path?flag"),
            (
                "https://host/path?my_token=CANARY&key=visible",
                "https://host/path?my_token=[REDACTED]&key=visible",
            ),
            (
                "https://host/path?privateKey=[REDACTED]&key=visible#fragment-visible",
                "https://host/path?privateKey=[REDACTED]&key=visible#fragment-visible",
            ),
            ("https://host/path?", "https://host/path?"),
            ("https://host/path", "https://host/path"),
        ] {
            let mut actual = original.to_string();
            redact_url_components(&mut actual);
            assert_eq!(actual, expected, "{original}");
        }
    }

    #[test]
    fn free_text_mirrors_exact_names_with_repeated_separators_and_original_delimiters() {
        for words in [
            vec!["private", "key"],
            vec!["x", "auth", "key"],
            vec!["auth", "key"],
            vec!["subscription", "key"],
            vec!["ocp", "apim", "subscription", "key"],
        ] {
            for separator in ["", "-", "_", " ", "- _-- "] {
                for delimiter in ["=", ": ", "  :  "] {
                    let name = words.join(separator).to_ascii_uppercase();
                    let text = format!("before {name}{delimiter}SYNTHETIC after");
                    assert_eq!(
                        redact_text_patterns(&text),
                        format!("before {name}{delimiter}{REDACTED} after")
                    );
                }
            }
        }
        for name in ["foo_authkey", "x-private-key", "x-subscription-key"] {
            assert!(!is_sensitive_key(name), "{name}");
            assert_eq!(
                redact_text_patterns(&format!("{name}=SYNTHETIC")),
                format!("{name}={REDACTED}")
            );
        }
        assert_eq!(
            redact_text_patterns("pRiVaTe KeY: expected PEM"),
            "pRiVaTe KeY: [REDACTED] PEM"
        );
        for text in [
            "key=visible",
            "keyboardLayout=visible",
            "primaryKey=visible",
            "partition_key=visible",
            "sortKey=visible",
            "foreignKey=visible",
            "Idempotency-Key=visible",
            "publicKey=visible",
            "no patterns here",
        ] {
            assert_eq!(redact_text_patterns(text), text);
        }
        assert_eq!(
            redact_text_patterns("Bearer abc123 Basic dXNlcjpwYXNz"),
            "Bearer [REDACTED] Basic [REDACTED]"
        );
    }

    #[test]
    fn query_param_api_key_is_redacted() {
        let mut url = "https://example.com/items?api_key=SECRET-QUERY-VALUE&page=2".to_string();
        redact_url(&mut url);
        assert_eq!(url, "https://example.com/items?api_key=[REDACTED]&page=2");
    }

    fn parse_schema(yaml: &str) -> SchemaObject {
        match serde_yaml_ng::from_str(yaml) {
            Ok(schema) => schema,
            Err(err) => panic!("deserializing schema YAML: {err}"),
        }
    }

    #[test]
    fn sensitive_input_names_unions_name_and_schema_declarations() {
        let schema = parse_schema(
            "type: object\nproperties:\n  handle:\n    type: string\n    writeOnly: true\n  pin:\n    type: string\n    format: password\n  plain:\n    type: string\n  visible:\n    type: string\n    writeOnly: false\n  nested:\n    type: object\n    properties:\n      inner:\n        type: string\n        writeOnly: true\n",
        );
        let names = sensitive_input_names(Some(&schema), ["api_key", "plain", "nested", "visible"]);
        let expected: BTreeSet<String> = ["api_key", "handle", "pin"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(names, expected);
        assert!(sensitive_input_names(None, ["plain"]).is_empty());
    }

    #[test]
    fn redact_inputs_redacts_named_set_and_nested_key_names_only() {
        let sensitive: BTreeSet<String> = ["handle".to_string()].into();
        let mut inputs = BTreeMap::from([
            ("handle".to_string(), json!("clear-handle")),
            ("api_key".to_string(), json!("clear-key")),
            ("plain".to_string(), json!("visible")),
            (
                "nested".to_string(),
                json!({ "inner": "kept", "clientSecret": "shh" }),
            ),
        ]);
        redact_inputs(&mut inputs, &sensitive);
        assert_eq!(inputs["handle"], json!(REDACTED));
        assert_eq!(inputs["api_key"], json!(REDACTED));
        assert_eq!(inputs["plain"], json!("visible"));
        assert_eq!(
            inputs["nested"],
            json!({ "inner": "kept", "clientSecret": REDACTED })
        );
    }

    #[test]
    fn text_redaction_preserves_non_secret_text() {
        assert_eq!(
            redact_text_patterns("Bearer abc123.xyz"),
            "Bearer [REDACTED]"
        );
        assert_eq!(
            redact_text_patterns("token=secret123 name=alice"),
            "token=[REDACTED] name=alice"
        );
        assert_eq!(
            redact_text_patterns("password: hunter2"),
            "password: [REDACTED]"
        );
        assert_eq!(redact_text_patterns("no secrets here"), "no secrets here");
    }
}
