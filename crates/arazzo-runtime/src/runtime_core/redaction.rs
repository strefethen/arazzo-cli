use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::DryRunRequest;

pub const REDACTED: &str = "[REDACTED]";

/// Header/field names that are always sensitive (exact match, case-insensitive).
const SENSITIVE_EXACT: [&str; 4] = ["proxy-authorization", "set-cookie", "x-api-key", "api-key"];

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

pub fn is_sensitive_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if SENSITIVE_EXACT.iter().any(|key| lower == *key) {
        return true;
    }
    SENSITIVE_STEMS.iter().any(|stem| lower.contains(stem))
}

pub fn redact_dry_run_request(request: &mut DryRunRequest) {
    redact_headers(&mut request.headers);
    redact_url_query(&mut request.url);
    if let Some(body) = &mut request.body {
        redact_json_value(body);
    }
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

pub fn redact_url_query(url: &mut String) {
    // Parse query params without reconstructing the URL to avoid
    // normalization artifacts (percent-encoding changes, reordering)
    // that could cause replay drift.
    let Some(query_start) = url.find('?') else {
        return;
    };
    let query = &url[query_start + 1..];
    if query.is_empty() {
        return;
    }

    let mut redacted_query = String::with_capacity(query.len());
    for (i, pair) in query.split('&').enumerate() {
        if i > 0 {
            redacted_query.push('&');
        }
        if let Some((key, _value)) = pair.split_once('=') {
            if is_sensitive_key(key) {
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

    url.truncate(query_start + 1);
    url.push_str(&redacted_query);
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
            r"(?i)(password|passwd|secret|token|authorization|apikey|api_key|credential|pwd)(\s*[:=]\s*)\S+",
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
