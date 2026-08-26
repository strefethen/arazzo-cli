#![forbid(unsafe_code)]

//! Which document a `sourceDescriptions[].url` names, and how it is found.
//!
//! Arazzo 1.1 §5.6.1, "Establishing the Base URI": *"If the `$self` field is
//! present and is an absolute URI, the base URI is the `$self` URI"*, and
//! otherwise *"the base URI MUST be determined from the next possible base URI
//! source"* — most commonly *"the retrieval URI of the Arazzo Description
//! document"*.
//!
//! §5.5.2, "Identity-Based Referencing": *"references MUST use the target
//! document's `$self` URI if the `$self` field is present in that document."*
//!
//! §5.5 forbids treating a reference as unresolvable *"before completely
//! parsing all documents provided to the implementation"*, and §9.6 is why no
//! test here needs a network: identity-based referencing exists so an
//! implementation can *"locate documents from a provided collection without
//! making network requests"*.
//!
//! Every fixture server base below is a distinct `.invalid` host, so the
//! resolved request URL of a dry run names exactly which document was bound.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_runtime::{
    relative_openapi_source_paths, EngineBuilder, RuntimeError, RuntimeErrorKind,
};
use arazzo_spec::{
    ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, SuccessCriterion, Workflow,
};

const SOURCE: &str = "petstore";
const WORKFLOW: &str = "wf";

// ── Fixtures ────────────────────────────────────────────────────────

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-document-identity-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `contents` at `relative`, creating parent directories.
    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let target = self.path.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|err| panic!("creating {}: {err}", parent.display()));
        }
        fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
        target
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// An OpenAPI document served from `base`, defining `getPet`. `self_uri` adds
/// the `$self` that gives the document its identity.
fn openapi_document(base: &str, self_uri: Option<&str>) -> String {
    let mut lines = vec!["openapi: \"3.1.0\"".to_string()];
    if let Some(self_uri) = self_uri {
        lines.push(format!("$self: {self_uri}"));
    }
    lines.extend([
        "info:".to_string(),
        "  title: fixture".to_string(),
        "  version: \"1.0.0\"".to_string(),
        "servers:".to_string(),
        format!("  - url: {base}"),
        "paths:".to_string(),
        "  /pet:".to_string(),
        "    get:".to_string(),
        "      operationId: getPet".to_string(),
        String::new(),
    ]);
    lines.join("\n")
}

/// An Arazzo document with one `type: openapi` source and one step routed to
/// that source's `getPet`.
fn spec_with(self_uri: Option<&str>, source_url: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        self_uri: self_uri.map(str::to_string),
        info: Info {
            title: "document identity".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: SOURCE.to_string(),
            url: source_url.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: WORKFLOW.to_string(),
            steps: vec![Step {
                step_id: "probe".to_string(),
                target: Some(StepTarget::OperationId("getPet".to_string())),
                success_criteria: vec![SuccessCriterion {
                    condition: "$statusCode == 200".to_string(),
                    ..SuccessCriterion::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

/// The URL a dry run resolves for the single step, which is the bound
/// document's `servers[0].url` joined with the operation path.
async fn resolved_request_url(
    spec: ArazzoSpec,
    base_dir: Option<&Path>,
    provided: Vec<(Vec<u8>, Option<PathBuf>)>,
) -> String {
    let mut builder = EngineBuilder::new(spec).dry_run(true);
    if let Some(dir) = base_dir {
        builder = builder.source_base_dir(dir);
    }
    for (data, retrieval_path) in provided {
        builder = builder.openapi_spec(data, retrieval_path);
    }
    let engine = match builder.build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        panic!("expected a successful dry run, got: {err}");
    }
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1, "one step plans one request");
    requests[0].url.clone()
}

/// The build error, for the paths that must refuse.
fn build_error(
    spec: ArazzoSpec,
    base_dir: Option<&Path>,
    provided: Vec<(Vec<u8>, Option<PathBuf>)>,
) -> RuntimeError {
    let mut builder = EngineBuilder::new(spec).dry_run(true);
    if let Some(dir) = base_dir {
        builder = builder.source_base_dir(dir);
    }
    for (data, retrieval_path) in provided {
        builder = builder.openapi_spec(data, retrieval_path);
    }
    match builder.build() {
        Ok(_) => panic!("build should have refused the reference"),
        Err(err) => err,
    }
}

fn assert_names(message: &str, part: &str, what: &str) {
    assert!(
        message.contains(part),
        "message should name the {what} ({part:?}): {message}"
    );
}

// ── §5.6.1 Establishing the base URI ────────────────────────────────

#[tokio::test]
async fn absolute_self_resolves_source_urls_away_from_the_document_directory() {
    // Two documents of the same name in two directories. The document was read
    // from `here/`, but its `$self` says it lives in `elsewhere/`, and §5.6.1
    // makes that the base URI.
    let dir = TempDir::new();
    dir.write(
        "here/petstore.openapi.yaml",
        &openapi_document("http://directory.invalid", None),
    );
    dir.write(
        "elsewhere/petstore.openapi.yaml",
        &openapi_document("http://self-base.invalid", None),
    );
    let self_uri = format!(
        "file://{}/elsewhere/purchase.arazzo.yaml",
        dir.path().display()
    );

    let url = resolved_request_url(
        spec_with(Some(&self_uri), "./petstore.openapi.yaml"),
        Some(&dir.path().join("here")),
        vec![],
    )
    .await;

    assert_eq!(url, "http://self-base.invalid/pet");
}

#[tokio::test]
async fn absent_self_binds_the_same_file_the_document_directory_named() {
    // The compatibility property: with no `$self` the retrieval URI is the
    // base, so the reference lands on exactly the file the pre-resolution
    // `base_dir.join` would have read.
    let dir = TempDir::new();
    dir.write(
        "here/petstore.openapi.yaml",
        &openapi_document("http://directory.invalid", None),
    );
    dir.write(
        "elsewhere/petstore.openapi.yaml",
        &openapi_document("http://self-base.invalid", None),
    );

    let url = resolved_request_url(
        spec_with(None, "./petstore.openapi.yaml"),
        Some(&dir.path().join("here")),
        vec![],
    )
    .await;

    assert_eq!(url, "http://directory.invalid/pet");
}

#[tokio::test]
async fn a_relative_self_is_resolved_before_it_becomes_the_base_uri() {
    // §5.6.1: a relative `$self` "MUST first be resolved against the next
    // possible base URI source ... before being used as the base URI for
    // resolving other relative references." Appendix §9.5 is this shape: the
    // document is retrieved from the root, `$self` puts it under `workflows/`,
    // and `../specs/...` then lands beside `workflows/` rather than beside the
    // retrieval directory — where nothing exists to be read.
    let dir = TempDir::new();
    dir.write(
        "specs/petstore.openapi.yaml",
        &openapi_document("http://self-base.invalid", None),
    );

    let url = resolved_request_url(
        spec_with(
            Some("workflows/purchase.arazzo.yaml"),
            "../specs/petstore.openapi.yaml",
        ),
        Some(dir.path()),
        vec![],
    )
    .await;

    assert_eq!(url, "http://self-base.invalid/pet");
}

// ── §5.5.2 Identity-based referencing ───────────────────────────────

#[tokio::test]
async fn a_resolved_reference_binds_the_provided_document_that_owns_that_identity() {
    // The reference resolves to an `https` URI, so no filesystem path could
    // satisfy it: the only way this build succeeds is by matching the `$self`
    // of a document already provided to the engine.
    let dir = TempDir::new();
    let spec = spec_with(
        Some("https://workflows.example.com/canonical/purchase.arazzo.yaml"),
        "specs/petstore.yaml",
    );
    let provided = openapi_document(
        "http://provided.invalid",
        Some("https://workflows.example.com/canonical/specs/petstore.yaml"),
    );

    // Nothing is read from disk for this source, so nothing is offered to the
    // filesystem gate either.
    assert!(
        relative_openapi_source_paths(&spec, dir.path()).is_empty(),
        "an identity-resolved reference performs no disk read"
    );

    let url =
        resolved_request_url(spec, Some(dir.path()), vec![(provided.into_bytes(), None)]).await;

    assert_eq!(url, "http://provided.invalid/pet");
}

#[tokio::test]
async fn a_reference_resolves_to_a_document_provided_after_an_unrelated_one() {
    // §5.5: a reference is not unresolvable until the whole provided set has
    // been parsed. The document that answers this reference is supplied second,
    // behind one that answers nothing.
    let dir = TempDir::new();
    let unrelated = openapi_document(
        "http://unrelated.invalid",
        Some("https://other.example.com/unrelated.yaml"),
    );
    let target = openapi_document(
        "http://provided.invalid",
        Some("https://workflows.example.com/canonical/specs/petstore.yaml"),
    );

    let url = resolved_request_url(
        spec_with(
            Some("https://workflows.example.com/canonical/purchase.arazzo.yaml"),
            "specs/petstore.yaml",
        ),
        Some(dir.path()),
        vec![(unrelated.into_bytes(), None), (target.into_bytes(), None)],
    )
    .await;

    assert_eq!(url, "http://provided.invalid/pet");
}

#[tokio::test]
async fn a_retrieval_path_gives_a_provided_document_an_identity_to_be_bound_by() {
    // A document with no `$self` answers to the location it was read from, so a
    // relative reference resolving to that path binds to the bytes already in
    // hand — the file is never opened a second time.
    let dir = TempDir::new();
    let path = dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://on-disk.invalid", None),
    );
    let provided = openapi_document("http://provided.invalid", None);

    let url = resolved_request_url(
        spec_with(None, "./petstore.openapi.yaml"),
        Some(dir.path()),
        vec![(provided.into_bytes(), Some(path))],
    )
    .await;

    assert_eq!(
        url, "http://provided.invalid/pet",
        "the provided bytes win over re-reading the same path"
    );
}

// ── Refusals ────────────────────────────────────────────────────────

#[tokio::test]
async fn an_unprovided_reference_is_refused_rather_than_read_from_the_document_directory() {
    // The negative case. A file of exactly the referenced name sits in the
    // document's directory, so the pre-resolution behavior would have loaded it
    // and reported success. Under `$self` the reference resolves elsewhere, and
    // resolving elsewhere means refusing — never silently falling back.
    let dir = TempDir::new();
    let decoy = dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://directory.invalid", None),
    );

    let err = build_error(
        spec_with(
            Some("https://workflows.example.com/canonical/purchase.arazzo.yaml"),
            "./petstore.openapi.yaml",
        ),
        Some(dir.path()),
        vec![],
    );

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionNotFound);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND");
    assert_names(&err.message, SOURCE, "source description");
    assert_names(&err.message, "./petstore.openapi.yaml", "authored url");
    assert_names(
        &err.message,
        "https://workflows.example.com/canonical/purchase.arazzo.yaml",
        "base URI used",
    );
    assert_names(
        &err.message,
        "https://workflows.example.com/canonical/petstore.openapi.yaml",
        "resolved URI",
    );
    assert!(
        !err.message.contains(&decoy.display().to_string()),
        "the refusal must not point at the file the old path would have read: {}",
        err.message
    );
}

#[tokio::test]
async fn a_local_reference_that_cannot_be_read_names_the_resolution_it_attempted() {
    let dir = TempDir::new();

    let err = build_error(
        spec_with(None, "./missing.openapi.yaml"),
        Some(dir.path()),
        vec![],
    );

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_LOAD");
    let base = format!("file://{}/", dir.path().display());
    assert_names(&err.message, SOURCE, "source description");
    assert_names(&err.message, "./missing.openapi.yaml", "authored url");
    assert_names(&err.message, &base, "base URI used");
    assert_names(
        &err.message,
        &format!("{base}missing.openapi.yaml"),
        "resolved URI",
    );
}

#[tokio::test]
async fn a_relative_reference_without_any_base_uri_is_refused() {
    let err = build_error(spec_with(None, "./petstore.openapi.yaml"), None, vec![]);

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_names(&err.message, SOURCE, "source description");
    assert_names(&err.message, "./petstore.openapi.yaml", "authored url");
}

#[tokio::test]
async fn a_declared_self_that_cannot_be_resolved_is_named_as_the_cause() {
    // Not "you forgot the directory" — the directory is right here. When a
    // `$self` is present and unusable, saying so is the difference between a
    // remedy that works and one that sends the operator after the wrong thing.
    let dir = TempDir::new();

    let err = build_error(
        spec_with(Some("http://["), "./petstore.openapi.yaml"),
        Some(dir.path()),
        vec![],
    );

    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_names(&err.message, "http://[", "unusable $self");
    assert!(
        !err.message.contains("source_base_dir"),
        "the directory was supplied, so it is not the remedy: {}",
        err.message
    );
}

// ── Absolute `file` references ──────────────────────────────────────

#[tokio::test]
async fn an_absolute_file_url_is_read_like_any_other_local_reference() {
    // The resolved scheme decides, not the shape of the authored string: an
    // absolute `file://` url names a document to load, not a request base.
    let dir = TempDir::new();
    let path = dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://on-disk.invalid", None),
    );
    let authored = format!("file://{}", path.display());

    let url = resolved_request_url(spec_with(None, &authored), Some(dir.path()), vec![]).await;

    assert_eq!(url, "http://on-disk.invalid/pet");
}

#[test]
fn an_absolute_file_url_is_vetted_like_any_other_disk_read() {
    // Because the build opens it, the filesystem gate has to see it.
    let dir = TempDir::new();
    let path = dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://on-disk.invalid", None),
    );
    let authored = format!("file://{}", path.display());

    let paths = relative_openapi_source_paths(&spec_with(None, &authored), dir.path());

    assert_eq!(paths.len(), 1, "the read must be vetted: {paths:?}");
    assert_eq!(paths[0].1, path);
}

// ── The filesystem gate ─────────────────────────────────────────────

#[test]
fn the_vetting_list_names_every_reference_the_build_will_read() {
    let dir = TempDir::new();
    let expected = dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://directory.invalid", None),
    );

    let paths =
        relative_openapi_source_paths(&spec_with(None, "./petstore.openapi.yaml"), dir.path());

    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].0, SOURCE);
    assert_eq!(paths[0].1, expected);
}

#[test]
fn the_vetting_list_excludes_a_reference_that_resolves_off_the_filesystem() {
    // A reference that resolves to a non-`file` URI is refused at build, not
    // read, so it is absent here and the gate loses no coverage.
    let dir = TempDir::new();
    dir.write(
        "petstore.openapi.yaml",
        &openapi_document("http://directory.invalid", None),
    );

    let paths = relative_openapi_source_paths(
        &spec_with(
            Some("https://workflows.example.com/canonical/purchase.arazzo.yaml"),
            "./petstore.openapi.yaml",
        ),
        dir.path(),
    );

    assert!(
        paths.is_empty(),
        "a reference resolving off the filesystem is never read: {paths:?}"
    );
}
