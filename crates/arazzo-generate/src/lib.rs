//! OpenAPI-to-Arazzo generation, introspection, and example value synthesis.

pub mod crud;
pub mod examples;
pub mod openapi_describe;
pub mod refs;
pub mod standalone_example;

/// Read and parse an OpenAPI spec file (YAML or JSON).
pub fn parse_openapi_file(path: &str) -> Result<openapiv3::OpenAPI, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("reading OpenAPI spec \"{path}\": {err}"))?;
    ensure_supported_openapi_version(&bytes)?;
    serde_yaml_ng::from_slice(&bytes)
        .map_err(|err| format!("parsing OpenAPI spec \"{path}\": {err}"))
}

/// Rejects OpenAPI 3.1 and newer with an actionable message.
///
/// The `openapiv3` model targets OpenAPI 3.0.x. A 3.1/3.2 document uses JSON
/// Schema 2020-12 constructs — for example `type` as an array such as
/// `["string", "null"]` — that otherwise fail to deserialize with an opaque
/// `invalid type: sequence, expected a string` error. Detecting the declared
/// version up front lets callers explain why ingestion failed.
pub fn ensure_supported_openapi_version(bytes: &[u8]) -> Result<(), String> {
    // A document that is not even valid YAML/JSON is left to the typed parser,
    // which reports the precise syntax error.
    let Ok(probe) = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(bytes) else {
        return Ok(());
    };
    let Some(version) = probe.get("openapi").and_then(|value| value.as_str()) else {
        return Ok(());
    };
    let minor = version
        .strip_prefix("3.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|minor| minor.parse::<u32>().ok());
    if matches!(minor, Some(minor) if minor >= 1) {
        return Err(format!(
            "generate supports OpenAPI 3.0.x, but this spec declares {version}. \
             OpenAPI 3.1/3.2 ingestion is not yet supported (these versions use \
             JSON Schema 2020-12 constructs such as `type` arrays)."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod version_guard_tests {
    use super::ensure_supported_openapi_version;

    #[test]
    fn accepts_openapi_30() {
        assert!(ensure_supported_openapi_version(br#"{"openapi":"3.0.3"}"#).is_ok());
    }

    #[test]
    fn rejects_openapi_31_and_32() {
        for version in ["3.1.0", "3.2.0"] {
            let doc = format!(r#"{{"openapi":"{version}"}}"#);
            let Err(err) = ensure_supported_openapi_version(doc.as_bytes()) else {
                panic!("expected {version} to be rejected");
            };
            assert!(err.contains(version), "message names the version: {err}");
            assert!(
                err.contains("3.0.x"),
                "message states the supported range: {err}"
            );
        }
    }

    #[test]
    fn ignores_documents_without_a_version() {
        // No `openapi` key and non-parseable input both defer to the typed parser.
        assert!(ensure_supported_openapi_version(br#"{"paths":{}}"#).is_ok());
        assert!(ensure_supported_openapi_version(b"not: [valid").is_ok());
    }
}
