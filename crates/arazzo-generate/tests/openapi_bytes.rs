use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use arazzo_generate::{ensure_supported_openapi_version, parse_openapi_bytes, parse_openapi_file};

const YAML: &[u8] = b"openapi: 3.0.3\ninfo:\n  title: Byte parser\n  version: 1.0.0\npaths:\n  /pets:\n    get:\n      operationId: listPets\n      responses:\n        '200':\n          description: Pet list\n";
const JSON: &[u8] = br#"{
    "openapi": "3.0.3",
    "info": {"title": "Byte parser", "version": "1.0.0"},
    "paths": {"/pets": {"get": {
        "operationId": "listPets",
        "responses": {"200": {"description": "Pet list"}}
    }}}
}"#;

struct TempSpec(PathBuf);

impl TempSpec {
    fn new(bytes: &[u8]) -> std::io::Result<Self> {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-openapi-bytes-{}-{}.yaml",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let spec = Self(path);
        file.write_all(bytes)?;
        Ok(spec)
    }

    fn path(&self) -> &str {
        match self.0.to_str() {
            Some(path) => path,
            None => panic!("temporary fixture path must be UTF-8: {:?}", self.0),
        }
    }
}

impl Drop for TempSpec {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn yaml_and_json_bytes_match_file_models() -> Result<(), Box<dyn std::error::Error>> {
    let mut models = Vec::new();
    for bytes in [YAML, JSON] {
        let file = TempSpec::new(bytes)?;
        let from_bytes = parse_openapi_bytes(bytes)?;
        let from_file = parse_openapi_file(file.path())?;
        assert_eq!(from_bytes.openapi, "3.0.3");
        assert_eq!(from_bytes.info.title, "Byte parser");
        assert_eq!(
            from_bytes
                .paths
                .paths
                .get("/pets")
                .and_then(|item| item.as_item())
                .and_then(|path| path.get.as_ref())
                .and_then(|operation| operation.operation_id.as_deref()),
            Some("listPets")
        );
        let model = serde_json::to_value(from_bytes)?;
        assert_eq!(model, serde_json::to_value(from_file)?);
        models.push(model);
    }
    assert_eq!(models[0], models[1]);
    Ok(())
}

#[test]
fn malformed_and_missing_version_errors_preserve_file_wording() -> std::io::Result<()> {
    for (bytes, detail) in [
        (
            b"not: [valid".as_slice(),
            "did not find expected ',' or ']' at line 2 column 1, while parsing a flow sequence at line 1 column 6",
        ),
        (
            b"info:\n  title: Byte parser\n  version: 1.0.0\npaths: {}\n".as_slice(),
            "missing field `openapi`",
        ),
    ] {
        let file = TempSpec::new(bytes)?;
        assert_eq!(
            parse_openapi_file(file.path()).map(|_| ()),
            Err(format!("parsing OpenAPI spec \"{}\": {detail}", file.path()))
        );
        assert_eq!(
            parse_openapi_bytes(bytes).map(|_| ()),
            Err(format!("parsing OpenAPI spec: {detail}"))
        );
    }
    Ok(())
}

#[test]
fn unsupported_versions_keep_the_actionable_message() -> std::io::Result<()> {
    for version in ["3.1.0", "3.2.0"] {
        let expected = format!(
            "generate supports OpenAPI 3.0.x, but this spec declares {version}. \
             OpenAPI 3.1/3.2 ingestion is not yet supported (these versions use \
             JSON Schema 2020-12 constructs such as `type` arrays)."
        );
        for document in [
            format!("openapi: {version}\n"),
            format!(r#"{{"openapi":"{version}"}}"#),
        ] {
            let bytes = document.as_bytes();
            let file = TempSpec::new(bytes)?;
            assert_eq!(
                parse_openapi_bytes(bytes).map(|_| ()),
                Err(expected.clone())
            );
            assert_eq!(
                parse_openapi_file(file.path()).map(|_| ()),
                Err(expected.clone())
            );
            assert_eq!(
                ensure_supported_openapi_version(bytes),
                Err(expected.clone())
            );
        }
    }
    Ok(())
}

#[test]
fn version_guard_leaves_malformed_missing_and_non_string_versions_to_typed_parser() {
    for bytes in [
        b"not: [valid".as_slice(),
        b"info: {}\npaths: {}\n".as_slice(),
        b"openapi: 3\ninfo: {}\npaths: {}\n".as_slice(),
    ] {
        assert!(ensure_supported_openapi_version(bytes).is_ok());
        assert!(parse_openapi_bytes(bytes).is_err());
    }
}

#[test]
fn file_read_error_keeps_the_supplied_path() -> std::io::Result<()> {
    let file = TempSpec::new(YAML)?;
    std::fs::remove_file(&file.0)?;
    let io_error = match std::fs::read(&file.0) {
        Err(error) => error,
        Ok(_) => panic!("removed fixture must fail to read: {}", file.path()),
    };
    assert_eq!(
        parse_openapi_file(file.path()).map(|_| ()),
        Err(format!(
            "reading OpenAPI spec \"{}\": {io_error}",
            file.path()
        ))
    );
    Ok(())
}
