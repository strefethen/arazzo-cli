//! Explicit source documents for existing CLI fixtures. No operation catalog is
//! inferred from an Arazzo target; every path and method is declared below.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Write a unique local OpenAPI description for these explicitly declared
/// test operations. The returned path is a document URL, and `base` appears
/// only in the document's Server Object.
pub fn fixture_url(base: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let file: PathBuf = std::env::temp_dir().join(format!(
        "arazzo-cli-source-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let document = fixture_document(base);
    fs::write(&file, document)
        .unwrap_or_else(|err| panic!("writing explicit OpenAPI fixture: {err}"));
    file.display().to_string()
}

pub fn write_in(dir: &Path, base: &str) {
    fs::write(dir.join("fixture.openapi.json"), fixture_document(base))
        .unwrap_or_else(|err| panic!("writing explicit OpenAPI fixture: {err}"));
    fs::write(
        dir.join("search.openapi.json"),
        fixture_document("https://search.example.com/v1"),
    )
    .unwrap_or_else(|err| panic!("writing search OpenAPI fixture: {err}"));
}

fn fixture_document(base: &str) -> String {
    let operations = [
        ("/a", "get", "fixtureGetA"),
        ("/b", "get", "fixtureGetB"),
        ("/s1", "get", "fixtureGetS1"),
        ("/s2", "get", "fixtureGetS2"),
        ("/s3", "get", "fixtureGetS3"),
        ("/items", "get", "fixtureGetItems"),
        ("/items", "post", "fixturePostItems"),
        ("/inspect", "get", "fixtureGetInspect"),
        ("/prepare", "get", "fixtureGetPrepare"),
        ("/test", "get", "fixtureGetTest"),
        ("/index", "get", "fixtureGetIndex"),
        ("/submit", "post", "fixturePostSubmit"),
        ("/get", "get", "fixtureGetGet"),
        ("/ok", "get", "fixtureGetOk"),
        ("/hop/0", "get", "fixtureGetHop0"),
        ("/full", "get", "fixtureGetFull"),
        ("/hidden", "get", "fixtureGetHidden"),
        ("/failed", "get", "fixtureGetFailed"),
        ("/one", "get", "fixtureGetOne"),
        ("/two", "get", "fixtureGetTwo"),
        ("/retry", "get", "fixtureGetRetry"),
        ("/catalog", "get", "fixtureGetCatalog"),
        ("/orders", "post", "fixturePostOrders"),
        ("/read", "get", "fixtureGetRead"),
        ("/write", "get", "fixtureGetWrite"),
        ("/data", "get", "fixtureGetData"),
        ("/credential", "get", "fixtureGetCredential"),
        ("/stall", "get", "fixtureGetStall"),
        ("/fail", "get", "fixtureGetFail"),
        ("/dependent", "get", "fixtureGetDependent"),
        ("/prerequisite", "get", "fixtureGetPrerequisite"),
        ("/health", "get", "fixtureGetHealth"),
    ];
    let mut paths = serde_json::Map::<String, Value>::new();
    for (path, method, id) in operations {
        paths
            .entry(path.to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap_or_else(|| panic!("fixture path item"))
            .insert(
                method.to_string(),
                json!({"operationId": id, "responses": {"200": {"description": "OK"}}}),
            );
    }
    let document = json!({"openapi": "3.0.3", "info": {"title": "fixture", "version": "1.0.0"},
        "servers": [{"url": base}], "paths": paths});
    document.to_string()
}

/// A genuine warning-only document, independent of rejected target syntax.
pub fn warning_spec() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "arazzo-cli-warning-{}-{}.yaml",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/petstore.openapi.yaml");
    let spec = json!({"arazzo": "1.0.1", "info": {"title": "Warning fixture", "version": "1.0.0"},
        "unknownField": true,
        "sourceDescriptions": [{"name": "api", "url": source.display().to_string(), "type": "openapi"}],
        "workflows": [{"workflowId": "probe", "steps": [{"stepId": "list-pets", "operationId": "listPets"}]}]});
    fs::write(&path, spec.to_string())
        .unwrap_or_else(|err| panic!("writing warning fixture: {err}"));
    path
}
