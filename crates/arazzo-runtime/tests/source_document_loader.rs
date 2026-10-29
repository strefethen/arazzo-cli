#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_runtime::{Engine, EngineBuilder, RuntimeError, RuntimeErrorKind, SourceDocumentLoader};
use arazzo_spec::{ArazzoSpec, SourceDescription, SourceType, Step, StepTarget, Workflow};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-source-loader-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path.canonicalize()?))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn document(base: &str) -> Vec<u8> {
    format!(
        "openapi: 3.1.0\nservers:\n  - url: {base}\npaths:\n  /loaded:\n    get:\n      operationId: loadedOperation\n"
    )
    .into_bytes()
}

fn spec(url: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        source_descriptions: vec![SourceDescription {
            name: "catalog".to_string(),
            url: url.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "probe".to_string(),
            steps: vec![Step {
                step_id: "load".to_string(),
                target: Some(StepTarget::OperationId("loadedOperation".to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

fn build_error(builder: EngineBuilder) -> RuntimeError {
    match builder.build() {
        Ok(_) => panic!("expected source binding to fail"),
        Err(err) => err,
    }
}

async fn assert_request(engine: Engine, expected_url: &str) -> TestResult {
    let result = engine.execute_collect("probe", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        return Err(err.to_string().into());
    }
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].url, expected_url);
    Ok(())
}

#[tokio::test]
async fn native_relative_document_indexes_operations_and_servers() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(
        fixture.path().join("api.yaml"),
        document("https://native.invalid/v1"),
    )?;
    let engine = EngineBuilder::new(spec("./api.yaml"))
        .dry_run(true)
        .source_base_dir(fixture.path())
        .build()?;
    assert_request(engine, "https://native.invalid/v1/loaded").await
}

#[test]
fn native_missing_document_preserves_exact_error_without_source() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path().join("missing.yaml");
    let io_error = match fs::read(&path) {
        Ok(_) => panic!("missing fixture unexpectedly exists"),
        Err(err) => err,
    };
    let base = url::Url::from_directory_path(fixture.path())
        .map_err(|()| "fixture must be an absolute directory")?;
    let resolved = base.join("./missing.yaml")?;
    let err =
        build_error(EngineBuilder::new(spec("./missing.yaml")).source_base_dir(fixture.path()));
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_LOAD");
    assert_eq!(err.message, format!(
        "sourceDescription \"catalog\": url \"./missing.yaml\" resolved against base URI \"{base}\" to \"{resolved}\"; reading OpenAPI document \"{}\": {io_error}",
        path.display()
    ));
    assert!(err.source().is_none());
    println!("native missing-file baseline: {}", err.message);
    Ok(())
}

#[test]
fn native_malformed_document_preserves_exact_parse_error() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path().join("malformed.yaml");
    let bytes = b"[unterminated";
    fs::write(&path, bytes)?;
    let parse_error = match serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(bytes) {
        Ok(_) => panic!("malformed fixture parsed"),
        Err(err) => err,
    };
    let err =
        build_error(EngineBuilder::new(spec("./malformed.yaml")).source_base_dir(fixture.path()));
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_PARSE");
    assert_eq!(
        err.message,
        format!(
            "sourceDescription \"catalog\": parsing OpenAPI document \"{}\": {parse_error}",
            path.display()
        )
    );
    assert!(err.source().is_none());
    println!("native malformed baseline: {}", err.message);
    Ok(())
}

#[test]
fn native_relative_document_without_base_preserves_exact_error() {
    let err = build_error(EngineBuilder::new(spec("./api.yaml")));
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionLoad);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_LOAD");
    assert_eq!(err.message, "sourceDescription \"catalog\": relative url \"./api.yaml\" has no base URI to resolve against: declare an absolute $self, or provide the Arazzo document's directory via EngineBuilder::source_base_dir");
    assert!(err.source().is_none());
    println!("native no-base baseline: {}", err.message);
}

#[derive(Debug)]
struct DenialSource;

impl std::fmt::Display for DenialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("source-loader-denial-cause")
    }
}

impl Error for DenialSource {}

struct RecordingLoader {
    calls: Mutex<Vec<(String, PathBuf)>>,
    bytes: Option<Vec<u8>>,
}

impl RecordingLoader {
    fn new(bytes: Option<Vec<u8>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            bytes,
        }
    }

    fn calls(&self) -> Result<Vec<(String, PathBuf)>, Box<dyn Error>> {
        Ok(self.calls.lock().map_err(|err| err.to_string())?.clone())
    }
}

impl SourceDocumentLoader for RecordingLoader {
    fn load(&self, source_name: &str, resolved_path: &Path) -> Result<Vec<u8>, RuntimeError> {
        self.calls
            .lock()
            .map_err(|err| RuntimeError::new(RuntimeErrorKind::InternalError, err.to_string()))?
            .push((source_name.to_string(), resolved_path.to_path_buf()));
        match &self.bytes {
            Some(bytes) => Ok(bytes.clone()),
            None => Err(RuntimeError::with_source(
                RuntimeErrorKind::InputValidation,
                "sentinel loader denial",
                DenialSource,
            )),
        }
    }
}

#[tokio::test]
async fn injected_bytes_drive_index_and_server_with_exact_single_resolved_call() -> TestResult {
    let fixture = Fixture::new()?;
    // The normalized path is absent: any native read instead of the supplied
    // bytes would fail, and the operation and server must both come from bytes.
    let loader = Arc::new(RecordingLoader::new(Some(document(
        "https://injected.invalid/v2",
    ))));
    let engine = EngineBuilder::new(spec("./nested/../api.yaml"))
        .dry_run(true)
        .source_base_dir(fixture.path())
        .source_document_loader(loader.clone())
        .build()?;
    assert_request(engine, "https://injected.invalid/v2/loaded").await?;
    assert_eq!(
        loader.calls()?,
        vec![("catalog".to_string(), fixture.path().join("api.yaml"))]
    );
    Ok(())
}

#[tokio::test]
async fn distinct_source_declarations_each_load_once_even_for_the_same_path() -> TestResult {
    let fixture = Fixture::new()?;
    let mut input = spec("./api.yaml");
    let mut second = input.source_descriptions[0].clone();
    second.name = "second".to_string();
    input.source_descriptions.push(second);
    input.workflows[0].steps[0].target = Some(StepTarget::OperationId(
        "$sourceDescriptions.catalog.loadedOperation".to_string(),
    ));
    let loader = Arc::new(RecordingLoader::new(Some(document(
        "https://shared.invalid",
    ))));
    let engine = EngineBuilder::new(input)
        .source_base_dir(fixture.path())
        .source_document_loader(loader.clone())
        .dry_run(true)
        .build()?;
    assert_request(engine, "https://shared.invalid/loaded").await?;
    let path = fixture.path().join("api.yaml");
    assert_eq!(
        loader.calls()?,
        vec![
            ("catalog".to_string(), path.clone()),
            ("second".to_string(), path)
        ]
    );
    Ok(())
}

#[test]
fn injected_denial_preserves_kind_message_and_boxed_cause_without_native_fallback() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path().join("api.yaml");
    fs::write(&path, document("https://native-would-succeed.invalid"))?;
    let loader = Arc::new(RecordingLoader::new(None));
    let err = build_error(
        EngineBuilder::new(spec("./api.yaml"))
            .source_base_dir(fixture.path())
            .source_document_loader(loader.clone()),
    );
    assert_eq!(err.kind, RuntimeErrorKind::InputValidation);
    assert_eq!(err.message, "sentinel loader denial");
    let cause = err.source().ok_or("loader cause was lost")?;
    assert!(cause.downcast_ref::<DenialSource>().is_some());
    assert_eq!(cause.to_string(), "source-loader-denial-cause");
    assert_eq!(loader.calls()?, vec![("catalog".to_string(), path)]);
    Ok(())
}

#[tokio::test]
async fn provided_retrieval_identity_bypasses_loader_and_conflicting_disk_bytes() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path().join("api.yaml");
    fs::write(&path, document("https://conflicting.invalid"))?;
    let loader = Arc::new(RecordingLoader::new(None));
    let engine = EngineBuilder::new(spec("./api.yaml"))
        .source_base_dir(fixture.path())
        .source_document_loader(loader.clone())
        .openapi_spec(document("https://provided.invalid"), Some(path))
        .dry_run(true)
        .build()?;
    assert_request(engine, "https://provided.invalid/loaded").await?;
    assert!(loader.calls()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn provided_absolute_self_identity_bypasses_loader_without_a_path() -> TestResult {
    let loader = Arc::new(RecordingLoader::new(None));
    let mut bytes = b"$self: https://identity.invalid/openapi.yaml\n".to_vec();
    bytes.extend(document("https://self-provided.invalid"));
    let engine = EngineBuilder::new(spec("https://identity.invalid/openapi.yaml"))
        .source_document_loader(loader.clone())
        .openapi_spec(bytes, None)
        .dry_run(true)
        .build()?;
    assert_request(engine, "https://self-provided.invalid/loaded").await?;
    assert!(loader.calls()?.is_empty());
    Ok(())
}

#[test]
fn refused_remote_reference_and_missing_base_never_invoke_loader() -> TestResult {
    for (url, kind) in [
        (
            "https://remote.invalid/openapi.yaml",
            RuntimeErrorKind::SourceDescriptionNotFound,
        ),
        ("./api.yaml", RuntimeErrorKind::SourceDescriptionLoad),
    ] {
        let loader = Arc::new(RecordingLoader::new(None));
        let err = build_error(EngineBuilder::new(spec(url)).source_document_loader(loader.clone()));
        assert_eq!(err.kind, kind);
        assert!(loader.calls()?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn absolute_file_uri_uses_the_same_native_and_injected_boundary() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path().join("api.yaml");
    fs::write(&path, document("https://absolute-native.invalid"))?;
    let uri = url::Url::from_file_path(&path).map_err(|()| "absolute fixture path required")?;
    let native = EngineBuilder::new(spec(uri.as_str()))
        .dry_run(true)
        .build()?;
    assert_request(native, "https://absolute-native.invalid/loaded").await?;
    let loader = Arc::new(RecordingLoader::new(Some(document(
        "https://absolute-injected.invalid",
    ))));
    let injected = EngineBuilder::new(spec(uri.as_str()))
        .source_document_loader(loader.clone())
        .dry_run(true)
        .build()?;
    assert_request(injected, "https://absolute-injected.invalid/loaded").await?;
    assert_eq!(loader.calls()?, vec![("catalog".to_string(), path)]);
    Ok(())
}

#[tokio::test]
async fn external_ref_is_not_loaded_during_build_or_execution() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(
        fixture.path().join("external.yaml"),
        document("https://external.invalid"),
    )?;
    let bytes = b"openapi: 3.1.0\nservers:\n  - url: https://ref.invalid\npaths:\n  /loaded:\n    $ref: external.yaml#/paths/~1loaded\n".to_vec();
    let loader = Arc::new(RecordingLoader::new(Some(bytes)));
    let engine = EngineBuilder::new(spec("./api.yaml"))
        .source_base_dir(fixture.path())
        .source_document_loader(loader.clone())
        .dry_run(true)
        .build()?;
    let result = engine.execute_collect("probe", BTreeMap::new()).await;
    let err = match &result.outputs {
        Ok(_) => panic!("external ref must not provide an indexed operation"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdNotFound);
    assert!(result.dry_run_requests().is_empty());
    assert_eq!(
        loader.calls()?,
        vec![("catalog".to_string(), fixture.path().join("api.yaml"))]
    );
    Ok(())
}
