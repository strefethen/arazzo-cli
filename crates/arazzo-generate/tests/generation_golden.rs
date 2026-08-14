#![forbid(unsafe_code)]

//! Golden snapshot of `generate_crud`'s full YAML output for the petstore
//! fixture.
//!
//! This is the first golden under `crates/arazzo-generate/tests/` — before
//! ac-91284 there was no snapshot corpus here, only inline `contains()`
//! assertions in `crud.rs`. Pinning the *whole* generated document, not a
//! handful of substrings, is what actually proves that
//! `sourceDescriptions[].url` — and nothing else — changed shape: the golden
//! diff for this ticket is exactly one line, `url: https://petstore.example
//! .com/v1` becoming `url: petstore.openapi.yaml`, with every operationPath,
//! step, and request body byte-identical.
//!
//! Regenerating is deliberate, never automatic:
//!
//! ```text
//! UPDATE_GENERATION_GOLDEN=1 cargo test -p arazzo-generate --test generation_golden
//! ```

use std::fs;
use std::path::PathBuf;

use arazzo_generate::crud::generate_crud;

/// Golden file, relative to this crate's manifest directory.
const GOLDEN_PATH: &str = "tests/golden/petstore-crud.arazzo.yaml";
/// Opt-in required to rewrite the golden. A plain `cargo test` never does.
const UPDATE_ENV: &str = "UPDATE_GENERATION_GOLDEN";

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(GOLDEN_PATH)
}

fn petstore_openapi() -> openapiv3::OpenAPI {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/petstore.openapi.yaml");
    let yaml =
        fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_yaml_ng::from_str(&yaml).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

#[test]
fn petstore_crud_generation_matches_golden() {
    let openapi = petstore_openapi();

    // Same-directory case: the document url defaults to the `--spec`
    // argument as typed, which is what a bare `arazzo generate --spec
    // petstore.openapi.yaml` (no `--output`) produces.
    let result = generate_crud(&openapi, "petstore.openapi.yaml", "petstore.openapi.yaml")
        .unwrap_or_else(|err| panic!("generate_crud: {err}"));
    let actual = serde_yaml_ng::to_string(&result.spec)
        .unwrap_or_else(|err| panic!("serializing generated spec: {err}"));

    if std::env::var(UPDATE_ENV).is_ok() {
        let path = golden_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .unwrap_or_else(|err| panic!("creating golden directory {}: {err}", dir.display()));
        }
        fs::write(&path, &actual)
            .unwrap_or_else(|err| panic!("writing golden {}: {err}", path.display()));
        eprintln!(
            "wrote {} — review the diff before committing \
             (regenerate deliberately: {UPDATE_ENV}=1 cargo test -p arazzo-generate --test generation_golden)",
            path.display()
        );
        return;
    }

    let path = golden_path();
    let expected = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "reading golden {}: {err}\n\
             regenerate with: {UPDATE_ENV}=1 cargo test -p arazzo-generate --test generation_golden",
            path.display()
        )
    });

    assert_eq!(
        actual, expected,
        "generated petstore CRUD document drifted from the golden at {}\n\
         review the diff — a changed url or operationPath is a behavior change, not a formality —\n\
         then regenerate deliberately with:\n  {UPDATE_ENV}=1 cargo test -p arazzo-generate --test generation_golden",
        path.display()
    );
}
