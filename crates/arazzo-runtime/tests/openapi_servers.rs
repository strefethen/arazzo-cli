#![forbid(unsafe_code)]

//! Which OpenAPI Server Object a step's request is sent to.
//!
//! Arazzo 1.1 §5.7: for a step that references an operation, the endpoint URL
//! *"is determined by the OpenAPI description’s Server Object"*. OpenAPI 3.2
//! declares `servers` at three levels — the OpenAPI Object (§4.1.1), the Path
//! Item Object (§4.9.1) and the Operation Object (§4.10.1) — and a lower
//! level's array overrides every one above it.
//!
//! The fixtures stand one loopback server in for each level's host, so every
//! assertion is about which of them a request reaches — or, for a refusal,
//! that none of them is touched.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arazzo_runtime::{EngineBuilder, RuntimeError, RuntimeErrorKind};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};

use common::{
    logged_requests, new_request_log, record_request, start_server, MockHttpResponse, RequestLog,
    TestServer,
};

const WORKFLOW: &str = "wf";
const SOURCE: &str = "api";
const DOCUMENT: &str = "api.openapi.yaml";

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
            "arazzo-openapi-servers-{}-{nanos}-{}",
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

    /// Writes the source description's OpenAPI document.
    fn document(&self, contents: &str) {
        let target = self.path.join(DOCUMENT);
        fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
    }

    /// The label the runtime names the bound document by: its local path.
    fn document_label(&self) -> String {
        self.path.join(DOCUMENT).display().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A loopback server that answers everything with `200 {}` and records the
/// paths it was asked for.
struct Host {
    server: TestServer,
    log: RequestLog,
}

impl Host {
    fn start() -> Self {
        let log = new_request_log();
        let recorder = Arc::clone(&log);
        let server = start_server(move |method, url, headers, body| {
            record_request(&recorder, &method, &url, &headers, &body);
            MockHttpResponse::json(200, "{}")
        });
        Self { server, log }
    }

    fn url(&self) -> String {
        self.server.base_url.clone()
    }

    fn port(&self) -> String {
        self.server
            .base_url
            .rsplit(':')
            .next()
            .unwrap_or_else(|| panic!("no port in {}", self.server.base_url))
            .to_string()
    }

    fn hits(&self) -> Vec<String> {
        logged_requests(&self.log)
            .into_iter()
            .map(|request| request.url)
            .collect()
    }
}

/// A `servers` array holding one Server Object, as YAML flow text.
fn servers(url: &str) -> String {
    format!("[{{url: \"{url}\"}}]")
}

/// One path item with a single `get` operation. The two `servers` fields hold
/// YAML flow text written verbatim, so a test can declare any shape —
/// including the malformed ones.
struct PathItem {
    path: String,
    operation_id: String,
    path_servers: Option<String>,
    operation_servers: Option<String>,
}

impl PathItem {
    fn get(path: &str, operation_id: &str) -> Self {
        Self {
            path: path.to_string(),
            operation_id: operation_id.to_string(),
            path_servers: None,
            operation_servers: None,
        }
    }

    fn path_servers(mut self, yaml: &str) -> Self {
        self.path_servers = Some(yaml.to_string());
        self
    }

    fn operation_servers(mut self, yaml: &str) -> Self {
        self.operation_servers = Some(yaml.to_string());
        self
    }

    fn render(&self) -> String {
        let mut lines = vec![format!("  {}:", self.path)];
        if let Some(servers) = &self.path_servers {
            lines.push(format!("    servers: {servers}"));
        }
        lines.push("    get:".to_string());
        lines.push(format!("      operationId: {}", self.operation_id));
        if let Some(servers) = &self.operation_servers {
            lines.push(format!("      servers: {servers}"));
        }
        lines.join("\n")
    }
}

/// An OpenAPI 3.2 document. `root_servers` is the YAML flow text of the
/// OpenAPI Object's `servers`, or `None` to omit the field.
fn openapi(root_servers: Option<&str>, items: &[PathItem]) -> String {
    let mut lines = vec![
        "openapi: 3.2.0".to_string(),
        "info:".to_string(),
        "  title: servers fixture".to_string(),
        "  version: \"1.0.0\"".to_string(),
    ];
    if let Some(servers) = root_servers {
        lines.push(format!("servers: {servers}"));
    }
    lines.push("paths:".to_string());
    lines.extend(items.iter().map(PathItem::render));
    lines.push(String::new());
    lines.join("\n")
}

/// A one-source Arazzo document whose workflow runs one step per target.
fn spec_with(targets: Vec<StepTarget>) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "OpenAPI servers".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: SOURCE.to_string(),
            url: DOCUMENT.to_string(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: WORKFLOW.to_string(),
            steps: targets
                .into_iter()
                .enumerate()
                .map(|(index, target)| Step {
                    step_id: format!("step{index}"),
                    target: Some(target),
                    ..Step::default()
                })
                .collect(),
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

fn operation_ids(ids: &[&str]) -> Vec<StepTarget> {
    ids.iter()
        .map(|id| StepTarget::OperationId((*id).to_string()))
        .collect()
}

/// The same operations, named through the source-qualified form.
fn qualified(ids: &[&str]) -> Vec<StepTarget> {
    ids.iter()
        .map(|id| StepTarget::OperationId(format!("$sourceDescriptions.{SOURCE}.{id}")))
        .collect()
}

// ── Execution helpers ───────────────────────────────────────────────

/// Runs the workflow live and returns every URL it sent, in order.
async fn send(dir: &Path, targets: Vec<StepTarget>) -> Result<Vec<String>, RuntimeError> {
    let engine = EngineBuilder::new(spec_with(targets))
        .source_base_dir(dir)
        .trace(true)
        .build()?;
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    let sent = result
        .trace_steps()
        .iter()
        .filter_map(|step| step.request.as_ref())
        .map(|request| request.url.clone())
        .collect();
    result.outputs?;
    Ok(sent)
}

/// Resolves the workflow without sending, returning every planned URL.
async fn plan(dir: &Path, targets: Vec<StepTarget>) -> Result<Vec<String>, RuntimeError> {
    let engine = EngineBuilder::new(spec_with(targets))
        .source_base_dir(dir)
        .dry_run(true)
        .build()?;
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    let planned = result
        .dry_run_requests()
        .iter()
        .map(|request| request.url.clone())
        .collect();
    result.outputs?;
    Ok(planned)
}

/// The error one refused target produces on a **live** engine. Only a live
/// engine can witness that a refusal reaches no server; a dry run sends
/// nothing whatever the outcome.
async fn live_refusal(dir: &Path, target: &str) -> RuntimeError {
    match send(dir, operation_ids(&[target])).await {
        Ok(urls) => panic!("expected {target} to be refused, sent {urls:?}"),
        Err(err) => err,
    }
}

/// The error building an engine over the fixture document produces.
fn build_error(dir: &Path) -> RuntimeError {
    match EngineBuilder::new(spec_with(operation_ids(&["probe"])))
        .source_base_dir(dir)
        .dry_run(true)
        .build()
    {
        Ok(_) => panic!("expected the build to fail"),
        Err(err) => err,
    }
}

fn assert_contains(message: &str, fragment: &str) {
    assert!(
        message.contains(fragment),
        "expected {fragment:?} in: {message}"
    );
}

// ── Precedence: operation, then path item, then document ────────────

/// The four operations of the precedence matrix, in the order [`Matrix`]
/// expects them to be sent.
const MATRIX_IDS: [&str; 4] = ["docLevel", "pathLevel", "opOverPath", "opOverDoc"];

/// The four operations of the precedence matrix, each served by the level
/// that declares it.
struct Matrix {
    dir: TempDir,
    document: Host,
    path_item: Host,
    operation: Host,
}

impl Matrix {
    fn new() -> Self {
        let dir = TempDir::new();
        let (document, path_item, operation) = (Host::start(), Host::start(), Host::start());
        dir.document(&openapi(
            Some(&servers(&document.url())),
            &[
                PathItem::get("/doc-level", "docLevel"),
                PathItem::get("/path-level", "pathLevel").path_servers(&servers(&path_item.url())),
                PathItem::get("/op-over-path", "opOverPath")
                    .path_servers(&servers(&path_item.url()))
                    .operation_servers(&servers(&operation.url())),
                PathItem::get("/op-over-doc", "opOverDoc")
                    .operation_servers(&servers(&operation.url())),
            ],
        ));
        Self {
            dir,
            document,
            path_item,
            operation,
        }
    }

    fn expected_urls(&self) -> Vec<String> {
        vec![
            format!("{}/doc-level", self.document.url()),
            format!("{}/path-level", self.path_item.url()),
            format!("{}/op-over-path", self.operation.url()),
            format!("{}/op-over-doc", self.operation.url()),
        ]
    }
}

/// The ticket's defect: before the fix, every one of these went to the
/// document's server.
#[tokio::test]
async fn each_operation_is_sent_to_the_lowest_level_that_declares_servers() {
    let matrix = Matrix::new();
    for (form, targets) in [
        ("bare", operation_ids(&MATRIX_IDS)),
        ("qualified", qualified(&MATRIX_IDS)),
    ] {
        let sent = send(matrix.dir.path(), targets)
            .await
            .unwrap_or_else(|err| panic!("running the {form} matrix: {err}"));
        assert_eq!(sent, matrix.expected_urls(), "{form} targets");
    }

    // Each server saw exactly its own operations, once per form.
    assert_eq!(matrix.document.hits(), vec!["/doc-level", "/doc-level"]);
    assert_eq!(matrix.path_item.hits(), vec!["/path-level", "/path-level"]);
    assert_eq!(
        matrix.operation.hits(),
        vec![
            "/op-over-path",
            "/op-over-doc",
            "/op-over-path",
            "/op-over-doc"
        ]
    );
}

/// `--dry-run` plans exactly the hosts a live run reaches, so an operator can
/// trust it to show where each step will go.
#[tokio::test]
async fn a_dry_run_plans_the_hosts_a_live_run_reaches() {
    let matrix = Matrix::new();
    for (form, targets) in [
        ("bare", operation_ids(&MATRIX_IDS)),
        ("qualified", qualified(&MATRIX_IDS)),
    ] {
        let planned = plan(matrix.dir.path(), targets)
            .await
            .unwrap_or_else(|err| panic!("planning the {form} matrix: {err}"));
        assert_eq!(planned, matrix.expected_urls(), "{form} targets");
    }
    for host in [&matrix.document, &matrix.path_item, &matrix.operation] {
        assert!(host.hits().is_empty(), "a dry run sent {:?}", host.hits());
    }
}

/// Each Server Object substitutes its own variables' defaults — the rule the
/// document level already applies.
#[tokio::test]
async fn path_item_and_operation_servers_substitute_their_own_variable_defaults() {
    let dir = TempDir::new();
    let (document, path_item, operation) = (Host::start(), Host::start(), Host::start());
    let path_servers = format!(
        "[{{url: \"http://127.0.0.1:{{port}}/{{ver}}\", variables: {{port: {{default: \"{}\"}}, \
         ver: {{default: v9}}}}}}]",
        path_item.port()
    );
    let operation_servers = format!(
        "[{{url: \"http://{{host}}:{{port}}/{{ver}}\", variables: {{host: {{default: 127.0.0.1}}, \
         port: {{default: \"{}\"}}, ver: {{default: v7}}}}}}]",
        operation.port()
    );
    dir.document(&openapi(
        Some(&servers(&document.url())),
        &[
            PathItem::get("/path-vars", "pathVars").path_servers(&path_servers),
            PathItem::get("/op-vars", "opVars").operation_servers(&operation_servers),
        ],
    ));

    let sent = send(dir.path(), operation_ids(&["pathVars", "opVars"]))
        .await
        .unwrap_or_else(|err| panic!("running the variables fixture: {err}"));
    assert_eq!(
        sent,
        vec![
            format!("{}/v9/path-vars", path_item.url()),
            format!("{}/v7/op-vars", operation.url()),
        ]
    );
    assert_eq!(path_item.hits(), vec!["/v9/path-vars"]);
    assert_eq!(operation.hits(), vec!["/v7/op-vars"]);
    assert!(
        document.hits().is_empty(),
        "document: {:?}",
        document.hits()
    );
}

// ── Empty arrays declare nothing ────────────────────────────────────

/// `servers: []` below the root declares no server, so the next level up
/// applies. OpenAPI 3.2 §4.1.1 treats an absent and an empty array alike at
/// the root and is silent below it; swagger-client/ApiDOM and
/// openapi-generator read the lower levels the same way.
#[tokio::test]
async fn an_empty_servers_array_passes_to_the_level_above() {
    let dir = TempDir::new();
    let (document, path_item) = (Host::start(), Host::start());
    dir.document(&openapi(
        Some(&servers(&document.url())),
        &[
            PathItem::get("/op-empty", "opEmpty")
                .path_servers(&servers(&path_item.url()))
                .operation_servers("[]"),
            PathItem::get("/path-empty", "pathEmpty").path_servers("[]"),
            PathItem::get("/both-empty", "bothEmpty")
                .path_servers("[]")
                .operation_servers("[]"),
        ],
    ));

    let sent = send(
        dir.path(),
        operation_ids(&["opEmpty", "pathEmpty", "bothEmpty"]),
    )
    .await
    .unwrap_or_else(|err| panic!("running the empty-array fixture: {err}"));
    assert_eq!(
        sent,
        vec![
            format!("{}/op-empty", path_item.url()),
            format!("{}/path-empty", document.url()),
            format!("{}/both-empty", document.url()),
        ]
    );
    assert_eq!(path_item.hits(), vec!["/op-empty"]);
    assert_eq!(document.hits(), vec!["/path-empty", "/both-empty"]);
}

// ── Refusals: a declared level with no usable server ────────────────

/// One document mixing unusable declarations with operations that still
/// route, served by a usable document-level host.
struct Unusable {
    dir: TempDir,
    document: Host,
    operation: Host,
}

impl Unusable {
    fn new() -> Self {
        let dir = TempDir::new();
        let (document, operation) = (Host::start(), Host::start());
        dir.document(&openapi(
            Some(&servers(&document.url())),
            &[
                PathItem::get("/relative-root", "relativeRoot").operation_servers(&servers("/v2")),
                PathItem::get("/relative-dot", "relativeDot").operation_servers(&servers("./v2")),
                PathItem::get("/path-relative", "pathRelative").path_servers(&servers("./v3")),
                PathItem::get("/path-mapping", "pathMapping")
                    .path_servers(&format!("{{url: \"{}\"}}", operation.url())),
                PathItem::get("/op-null", "opNull").operation_servers("null"),
                PathItem::get("/op-no-url", "opNoUrl")
                    .operation_servers("[{description: \"no url\"}]"),
                PathItem::get("/op-over-bad-path", "opOverBadPath")
                    .path_servers(&servers("./v3"))
                    .operation_servers(&servers(&operation.url())),
                PathItem::get("/plain", "plain"),
            ],
        ));
        Self {
            dir,
            document,
            operation,
        }
    }

    fn assert_nothing_sent(&self, target: &str) {
        for host in [&self.document, &self.operation] {
            assert!(
                host.hits().is_empty(),
                "{target} reached a server: {:?}",
                host.hits()
            );
        }
    }
}

/// OpenAPI lets a server url be relative — to the location the document is
/// served from (§4.5.1, §4.5.2.1). A document this runtime read from disk has
/// no such HTTP location, so the refusal is this runtime's limit, and the
/// message must not blame the document.
#[tokio::test]
async fn a_relative_declared_server_is_refused_as_this_runtimes_limit() {
    let fixture = Unusable::new();
    let cases = [
        (
            "relativeRoot",
            "GET /relative-root",
            "Operation Object",
            "/v2",
        ),
        (
            "$sourceDescriptions.api.relativeDot",
            "GET /relative-dot",
            "Operation Object",
            "./v2",
        ),
        (
            "pathRelative",
            "GET /path-relative",
            "Path Item Object",
            "./v3",
        ),
    ];
    for (target, operation, level, url) in cases {
        let err = live_refusal(fixture.dir.path(), target).await;
        assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse, "{err}");
        assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_PARSE");
        let message = err.message.as_str();
        assert_contains(message, &format!("operationId \"{target}\""));
        assert_contains(message, &format!("sourceDescription \"{SOURCE}\""));
        assert_contains(message, &fixture.dir.document_label());
        assert_contains(message, operation);
        assert_contains(message, level);
        assert_contains(
            message,
            &format!("servers[0].url \"{url}\", which is relative"),
        );
        assert_contains(message, "this runtime reads the document from disk");
        assert!(
            !message.contains("the document must be corrected"),
            "a relative url is not a document fault: {message}"
        );
        fixture.assert_nothing_sent(target);
    }
}

/// A `servers` field that is not an array — `null` included — or a Server
/// Object without its REQUIRED `url` is the document's fault.
#[tokio::test]
async fn a_malformed_servers_declaration_is_refused_as_a_document_fault() {
    let fixture = Unusable::new();
    let cases = [
        (
            "pathMapping",
            "GET /path-mapping",
            "Path Item Object",
            "a servers field that is not an array",
        ),
        (
            "opNull",
            "GET /op-null",
            "Operation Object",
            "a servers field that is not an array",
        ),
        (
            "opNoUrl",
            "GET /op-no-url",
            "Operation Object",
            "a first Server Object with no url",
        ),
    ];
    for (target, operation, level, fault) in cases {
        let err = live_refusal(fixture.dir.path(), target).await;
        assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse, "{err}");
        assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_PARSE");
        let message = err.message.as_str();
        assert_contains(message, &format!("operationId \"{target}\""));
        assert_contains(message, &format!("sourceDescription \"{SOURCE}\""));
        assert_contains(message, operation);
        assert_contains(message, level);
        assert_contains(message, fault);
        assert_contains(message, "the document must be corrected");
        assert!(
            !message.contains("this runtime reads"),
            "a malformed document is not this runtime's limit: {message}"
        );
        fixture.assert_nothing_sent(target);
    }
}

/// A declared level is never skipped: an unusable path item does not fall back
/// to the document's perfectly usable server.
#[tokio::test]
async fn an_unusable_path_item_server_does_not_fall_back_to_the_document() {
    let fixture = Unusable::new();
    let err = live_refusal(fixture.dir.path(), "pathRelative").await;
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse, "{err}");
    fixture.assert_nothing_sent("pathRelative");
}

/// The lowest declaring level decides, so an operation's own usable server
/// wins whatever its path item declares.
#[tokio::test]
async fn a_usable_operation_server_wins_over_an_unusable_path_item_server() {
    let fixture = Unusable::new();
    let sent = send(fixture.dir.path(), operation_ids(&["opOverBadPath"]))
        .await
        .unwrap_or_else(|err| panic!("resolving opOverBadPath: {err}"));
    assert_eq!(
        sent,
        vec![format!("{}/op-over-bad-path", fixture.operation.url())]
    );
    assert!(fixture.document.hits().is_empty());
}

/// A refusal belongs to the operation that declares the unusable server: the
/// engine still builds, and the document's other operations still route.
#[tokio::test]
async fn an_unusable_declaration_leaves_the_documents_other_operations_routable() {
    let fixture = Unusable::new();
    let sent = send(fixture.dir.path(), operation_ids(&["plain"]))
        .await
        .unwrap_or_else(|err| panic!("resolving plain beside unusable declarations: {err}"));
    assert_eq!(sent, vec![format!("{}/plain", fixture.document.url())]);
    assert_eq!(fixture.document.hits(), vec!["/plain"]);
}

/// The public resolver reports the same refusal a step does.
#[tokio::test]
async fn resolve_operation_id_reports_an_unusable_declaration() {
    let fixture = Unusable::new();
    let engine = EngineBuilder::new(spec_with(operation_ids(&["relativeDot"])))
        .source_base_dir(fixture.dir.path())
        .build()
        .unwrap_or_else(|err| panic!("building over unusable declarations: {err}"));
    match engine.resolve_operation_id("relativeDot") {
        Ok(pair) => panic!("expected a refusal, resolved {pair:?}"),
        Err(err) => assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse, "{err}"),
    }
    match engine.resolve_operation_id("plain") {
        Ok((method, path)) => assert_eq!((method.as_str(), path.as_str()), ("GET", "/plain")),
        Err(err) => panic!("resolving plain: {err}"),
    }
}

// ── The document level ──────────────────────────────────────────────

/// Characterization of the build-time refusals that existed before
/// path-item and operation servers were honored; the refactor keeps them
/// byte-for-byte.
#[test]
fn a_document_without_a_usable_root_server_fails_the_build() {
    let cases = [
        ("no servers field", None),
        ("an empty array", Some("[]".to_string())),
        (
            "a mapping instead of an array",
            Some("{url: \"https://api.example.test\"}".to_string()),
        ),
        (
            "a Server Object without url",
            Some("[{description: \"no url\"}]".to_string()),
        ),
    ];
    for (case, root_servers) in cases {
        let dir = TempDir::new();
        dir.document(&openapi(
            root_servers.as_deref(),
            &[PathItem::get("/probe", "probe")],
        ));
        let err = build_error(dir.path());
        assert_eq!(
            err.kind,
            RuntimeErrorKind::SourceDescriptionParse,
            "{case}: {err}"
        );
        assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_PARSE", "{case}");
        assert_eq!(
            err.message,
            format!(
                "sourceDescription \"{SOURCE}\": OpenAPI document \"{}\" declares no \
                 servers[0].url; an absolute server URL is required to derive the request base",
                dir.document_label()
            ),
            "{case}"
        );
    }
}

/// Characterization: a root-relative document server has always been refused
/// at build, with this message.
#[test]
fn a_root_relative_document_server_fails_the_build() {
    let dir = TempDir::new();
    dir.document(&openapi(
        Some(&servers("/v1")),
        &[PathItem::get("/probe", "probe")],
    ));
    let err = build_error(dir.path());
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionParse, "{err}");
    assert_eq!(
        err.message,
        format!(
            "sourceDescription \"{SOURCE}\": server URL \"/v1\" in OpenAPI document \"{}\" is \
             relative; an absolute URL is required to derive the request base",
            dir.document_label()
        )
    );
}

/// Relative means an RFC 3986 relative reference — the test
/// `DocumentSet::bind` applies to a Source Description url — not merely a
/// leading `/`. OpenAPI's own §4.5.2.1 examples are `.` and `./test`; before
/// this rule those built, dry-ran to a URL with no scheme and failed only when
/// sent.
#[test]
fn every_relative_document_server_fails_the_build() {
    for url in [".", "./test", "api.example.test/v1"] {
        let dir = TempDir::new();
        dir.document(&openapi(
            Some(&servers(url)),
            &[PathItem::get("/probe", "probe")],
        ));
        let err = build_error(dir.path());
        assert_eq!(
            err.kind,
            RuntimeErrorKind::SourceDescriptionParse,
            "{url}: {err}"
        );
        assert_eq!(
            err.message,
            format!(
                "sourceDescription \"{SOURCE}\": server URL \"{url}\" in OpenAPI document \"{}\" \
                 is relative; an absolute URL is required to derive the request base",
                dir.document_label()
            )
        );
    }
}

// ── What stays as it was ────────────────────────────────────────────

/// An explicitly provided spec belongs to no source description (Arazzo 1.1:
/// an operationId names an operation "existing within one of the
/// sourceDescriptions"), so its operations keep the engine-wide base and its
/// `servers` are consulted at no level.
#[tokio::test]
async fn an_explicit_spec_keeps_the_engine_wide_base_whatever_it_declares() {
    let dir = TempDir::new();
    let (document, operation) = (Host::start(), Host::start());
    dir.document(&openapi(
        Some(&servers(&document.url())),
        &[PathItem::get("/doc-level", "docLevel")],
    ));
    let explicit = openapi(
        Some(&servers(&operation.url())),
        &[PathItem::get("/explicit", "explicitOp")
            .path_servers(&servers(&operation.url()))
            .operation_servers(&servers(&operation.url()))],
    );

    let engine = EngineBuilder::new(spec_with(operation_ids(&["explicitOp"])))
        .source_base_dir(dir.path())
        .openapi_spec(explicit.into_bytes(), None)
        .trace(true)
        .build()
        .unwrap_or_else(|err| panic!("building with an explicit spec: {err}"));
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("running the explicit-spec operation: {err}");
    }
    assert_eq!(document.hits(), vec!["/explicit"]);
    assert!(
        operation.hits().is_empty(),
        "operation: {:?}",
        operation.hits()
    );
}

/// Characterizes the README's documented extension, not a conformance claim:
/// `{name}./path` "selects a source description's base URL" and joins a path
/// string to it, so it does not look up the operation that path belongs to.
#[tokio::test]
async fn a_source_routed_operation_path_keeps_the_document_base() {
    let matrix = Matrix::new();
    let sent = send(
        matrix.dir.path(),
        vec![StepTarget::OperationPath(format!(
            "{{{SOURCE}}}./op-over-doc"
        ))],
    )
    .await
    .unwrap_or_else(|err| panic!("running the source-routed operationPath: {err}"));
    assert_eq!(sent, vec![format!("{}/op-over-doc", matrix.document.url())]);
    assert!(matrix.operation.hits().is_empty());
}
