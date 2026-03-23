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
    serde_yaml_ng::from_slice(&bytes)
        .map_err(|err| format!("parsing OpenAPI spec \"{path}\": {err}"))
}
