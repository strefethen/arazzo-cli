#![forbid(unsafe_code)]

//! Where a step's `operationId` sends its request.
//!
//! Arazzo 1.1 Step Object, `operationId`: *"If multiple (non arazzo type)
//! sourceDescriptions are defined, then the operationId MUST be specified
//! using a Runtime Expression (e.g., `$sourceDescriptions.<name>.<operationId>`)
//! to avoid ambiguity or potential clashes."*
//!
//! The fixtures below are the shape that MUST exists for — two OpenAPI
//! documents that both define `getPet`, each with its own loopback `servers`
//! base. Every assertion is about which of the two servers the request
//! reaches — or, for a refusal, that neither of them is touched.

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
const STEP: &str = "probe";

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
            "arazzo-operation-id-routing-{}-{nanos}-{}",
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

    fn write(&self, name: &str, contents: &str) {
        let target = self.path.join(name);
        fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", target.display()));
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// An OpenAPI document based at `base`, defining `getPet` under `pet_path`
/// plus one operation no other fixture defines.
fn openapi_document(base: &str, pet_path: &str, unique_op: &str) -> String {
    [
        "openapi: \"3.0.0\"".to_string(),
        "info:".to_string(),
        "  title: fixture".to_string(),
        "  version: \"1.0.0\"".to_string(),
        "servers:".to_string(),
        format!("  - url: {base}"),
        "paths:".to_string(),
        format!("  {pet_path}:"),
        "    get:".to_string(),
        "      operationId: getPet".to_string(),
        format!("  /{unique_op}:"),
        "    get:".to_string(),
        format!("      operationId: {unique_op}"),
        // A dotted operationId. `source-reference-id` is `1*CHAR`, so this is
        // reachable as `$sourceDescriptions.<name>.svc.v1.getPet`.
        "  /dotted:".to_string(),
        "    get:".to_string(),
        "      operationId: svc.v1.getPet".to_string(),
        String::new(),
    ]
    .join("\n")
}

fn openapi_source(name: &str, file: &str) -> SourceDescription {
    SourceDescription {
        name: name.to_string(),
        url: file.to_string(),
        type_: SourceType::OpenApi,
        ..SourceDescription::default()
    }
}

fn spec_with(sources: Vec<SourceDescription>, operation_id: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "operationId routing".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: sources,
        workflows: vec![Workflow {
            workflow_id: WORKFLOW.to_string(),
            steps: vec![Step {
                step_id: STEP.to_string(),
                target: Some(StepTarget::OperationId(operation_id.to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

/// A test server that answers everything with `200 {}` and records what it
/// was asked for.
fn recording_server() -> (TestServer, RequestLog) {
    let log = new_request_log();
    let recorder = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&recorder, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    (server, log)
}

/// The two colliding sources from the issue #5 repro, written to a temp
/// directory as the relative documents an Arazzo file would reference.
struct TwoSources {
    dir: TempDir,
    alpha: TestServer,
    beta: TestServer,
    alpha_log: RequestLog,
    beta_log: RequestLog,
}

impl TwoSources {
    fn new() -> Self {
        let (alpha, alpha_log) = recording_server();
        let (beta, beta_log) = recording_server();
        let dir = TempDir::new();
        dir.write(
            "alpha.openapi.yaml",
            &openapi_document(&alpha.base_url, "/pets", "alphaOnly"),
        );
        dir.write(
            "beta.openapi.yaml",
            &openapi_document(&beta.base_url, "/animals", "betaOnly"),
        );
        Self {
            dir,
            alpha,
            beta,
            alpha_log,
            beta_log,
        }
    }

    fn sources(&self) -> Vec<SourceDescription> {
        vec![
            openapi_source("alpha", "alpha.openapi.yaml"),
            openapi_source("beta", "beta.openapi.yaml"),
        ]
    }

    /// The paths each server was actually asked for.
    fn hits(&self) -> (Vec<String>, Vec<String>) {
        let paths = |log: &RequestLog| {
            logged_requests(log)
                .into_iter()
                .map(|request| request.url)
                .collect::<Vec<_>>()
        };
        (paths(&self.alpha_log), paths(&self.beta_log))
    }
}

// ── Execution helpers ───────────────────────────────────────────────

/// Runs the one-step workflow live and returns the URL it sent to.
async fn send(spec: ArazzoSpec, base_dir: &Path) -> Result<String, RuntimeError> {
    let engine = EngineBuilder::new(spec)
        .source_base_dir(base_dir)
        .trace(true)
        .build()?;
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    let sent: Vec<String> = result
        .trace_steps()
        .iter()
        .filter_map(|step| step.request.as_ref())
        .map(|request| request.url.clone())
        .collect();
    result.outputs?;
    assert_eq!(sent.len(), 1, "expected exactly one sent request");
    Ok(sent[0].clone())
}

/// Resolves the one-step workflow without sending, returning the planned URL.
async fn plan(spec: ArazzoSpec, base_dir: &Path) -> Result<String, RuntimeError> {
    let engine = EngineBuilder::new(spec)
        .source_base_dir(base_dir)
        .dry_run(true)
        .build()?;
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    let planned: Vec<String> = result
        .dry_run_requests()
        .iter()
        .map(|request| request.url.clone())
        .collect();
    result.outputs?;
    assert_eq!(planned.len(), 1, "expected exactly one planned request");
    Ok(planned[0].clone())
}

/// The error a refused `operationId` produces on a **live** engine, panicking
/// if it resolves.
///
/// Distinct from [`refusal`] on purpose: a dry run sends nothing whatever the
/// outcome, so only this helper can witness that a refusal reaches no server.
async fn live_refusal(spec: ArazzoSpec, base_dir: &Path) -> RuntimeError {
    match send(spec, base_dir).await {
        Ok(url) => panic!("expected a refusal, sent a request to {url}"),
        Err(err) => err,
    }
}

/// The error kind a refused `operationId` produces, panicking if it resolves.
async fn refusal(spec: ArazzoSpec, base_dir: &Path) -> RuntimeError {
    match plan(spec, base_dir).await {
        Ok(url) => panic!("expected a refusal, planned a request to {url}"),
        Err(err) => err,
    }
}

// ── Source-qualified routing ────────────────────────────────────────

/// The defect this ticket exists for: with two documents defining `getPet`,
/// the qualified target picks one of them, and the request goes to *that*
/// document's server rather than to the first source description's.
#[tokio::test]
async fn qualified_operation_id_routes_to_the_named_source_base() {
    let fixture = TwoSources::new();

    let alpha_url = match send(
        spec_with(fixture.sources(), "$sourceDescriptions.alpha.getPet"),
        fixture.dir.path(),
    )
    .await
    {
        Ok(url) => url,
        Err(err) => panic!("resolving the alpha-qualified target: {err}"),
    };
    let beta_url = match send(
        spec_with(fixture.sources(), "$sourceDescriptions.beta.getPet"),
        fixture.dir.path(),
    )
    .await
    {
        Ok(url) => url,
        Err(err) => panic!("resolving the beta-qualified target: {err}"),
    };

    assert_eq!(alpha_url, format!("{}/pets", fixture.alpha.base_url));
    assert_eq!(beta_url, format!("{}/animals", fixture.beta.base_url));
    assert_ne!(
        alpha_url, beta_url,
        "the two qualified targets must reach different hosts"
    );

    let (alpha_hits, beta_hits) = fixture.hits();
    assert_eq!(alpha_hits, vec!["/pets".to_string()]);
    assert_eq!(beta_hits, vec!["/animals".to_string()]);
}

/// `source-reference-id` is `1*CHAR`, and §5.9 spells out why: *"operationIds
/// have no character restrictions in OpenAPI/AsyncAPI"*. So the split is at
/// the first dot and a dotted operationId — what protobuf and gRPC-gateway
/// generators emit — stays reachable through the very form the MUST demands.
#[tokio::test]
async fn a_dotted_operation_id_routes_through_its_named_source() {
    let fixture = TwoSources::new();
    let url = match send(
        spec_with(fixture.sources(), "$sourceDescriptions.beta.svc.v1.getPet"),
        fixture.dir.path(),
    )
    .await
    {
        Ok(url) => url,
        Err(err) => panic!("resolving a dotted operationId: {err}"),
    };
    assert_eq!(url, format!("{}/dotted", fixture.beta.base_url));

    let (alpha_hits, beta_hits) = fixture.hits();
    assert!(alpha_hits.is_empty(), "alpha was reached: {alpha_hits:?}");
    assert_eq!(beta_hits, vec!["/dotted".to_string()]);
}

/// A dry run plans exactly what a live run sends, so `--dry-run` can be
/// trusted to show which host a step will target.
#[tokio::test]
async fn dry_run_plans_the_same_host_the_live_run_reaches() {
    let fixture = TwoSources::new();
    for (target, expected) in [
        (
            "$sourceDescriptions.alpha.getPet",
            format!("{}/pets", fixture.alpha.base_url),
        ),
        (
            "$sourceDescriptions.beta.getPet",
            format!("{}/animals", fixture.beta.base_url),
        ),
    ] {
        let planned = match plan(spec_with(fixture.sources(), target), fixture.dir.path()).await {
            Ok(url) => url,
            Err(err) => panic!("planning {target}: {err}"),
        };
        assert_eq!(planned, expected, "planned URL for {target}");
    }

    let (alpha_hits, beta_hits) = fixture.hits();
    assert!(
        alpha_hits.is_empty() && beta_hits.is_empty(),
        "a dry run must send nothing; alpha={alpha_hits:?} beta={beta_hits:?}"
    );
}

// ── Refusals, all of them before any request ────────────────────────

/// The specification's MUST is about how many sources are *defined*, not
/// about whether one particular name happens to be unique today —
/// `alphaOnly` is defined by exactly one document and is still refused.
#[tokio::test]
async fn unqualified_operation_id_is_refused_when_two_sources_are_defined() {
    let fixture = TwoSources::new();
    for target in ["getPet", "alphaOnly"] {
        let err = refusal(spec_with(fixture.sources(), target), fixture.dir.path()).await;
        assert_eq!(
            err.kind,
            RuntimeErrorKind::OperationIdAmbiguous,
            "kind for {target}: {err}"
        );
        assert_eq!(err.code(), "RUNTIME_OPERATION_ID_AMBIGUOUS");
        // The message has to say what to write instead, in both supported
        // forms, or the operator is left guessing.
        assert!(
            err.message
                .contains(&format!("$sourceDescriptions.<name>.{target}")),
            "message was: {err}"
        );
        assert!(
            err.message.contains("{<name>}./<path>"),
            "message was: {err}"
        );
        assert!(err.message.contains("alpha"), "message was: {err}");
        assert!(err.message.contains("beta"), "message was: {err}");
    }

    let (alpha_hits, beta_hits) = fixture.hits();
    assert!(
        alpha_hits.is_empty() && beta_hits.is_empty(),
        "a refused operationId must send nothing; alpha={alpha_hits:?} beta={beta_hits:?}"
    );
}

/// The refusal reaches no server — proved on a **live** engine, because a dry
/// run would satisfy this assertion no matter where the refusal happened.
#[tokio::test]
async fn a_refused_operation_id_reaches_no_server_on_a_live_engine() {
    let fixture = TwoSources::new();
    for target in [
        "getPet",
        "$sourceDescriptions.gamma.getPet",
        "$sourceDescriptions.alpha.betaOnly",
        "$sourceDescriptions.alpha.get{Pet}",
    ] {
        let err = live_refusal(spec_with(fixture.sources(), target), fixture.dir.path()).await;
        assert!(err.message.contains(target), "message was: {err}");
        let (alpha_hits, beta_hits) = fixture.hits();
        assert!(
            alpha_hits.is_empty() && beta_hits.is_empty(),
            "{target} reached a server; alpha={alpha_hits:?} beta={beta_hits:?}"
        );
    }
}

#[tokio::test]
async fn qualified_target_naming_an_unknown_source_is_refused() {
    let fixture = TwoSources::new();
    let err = refusal(
        spec_with(fixture.sources(), "$sourceDescriptions.gamma.getPet"),
        fixture.dir.path(),
    )
    .await;
    assert_eq!(err.kind, RuntimeErrorKind::SourceDescriptionNotFound);
    assert_eq!(err.code(), "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND");
    assert!(err.message.contains("\"gamma\""), "message was: {err}");
    assert!(
        err.message.contains("$sourceDescriptions.gamma.getPet"),
        "message was: {err}"
    );
}

/// Only an `openapi` source describes operations. An `arazzo` source holds
/// workflows, so naming one from `operationId` is a document error, not an
/// operation this runtime should go looking for — and the specification is
/// what says so, which the message must not soften into a runtime limit the
/// way the asyncapi refusal legitimately does.
#[tokio::test]
async fn qualified_target_naming_an_arazzo_source_is_refused_as_non_conformant() {
    let fixture = TwoSources::new();
    let mut sources = fixture.sources();
    sources.push(SourceDescription {
        name: "flows".to_string(),
        url: "other.arazzo.yaml".to_string(),
        type_: SourceType::Arazzo,
        ..SourceDescription::default()
    });
    let err = refusal(
        spec_with(sources, "$sourceDescriptions.flows.getPet"),
        fixture.dir.path(),
    )
    .await;
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedSourceDescriptionType);
    assert_eq!(err.code(), "RUNTIME_UNSUPPORTED_SOURCE_DESCRIPTION_TYPE");
    assert!(err.message.contains("\"flows\""), "message was: {err}");
    assert!(err.message.contains("\"arazzo\""), "message was: {err}");
    assert!(
        err.message
            .contains("the specification does not let an operationId reference one"),
        "an arazzo source is forbidden by the specification, not by this runtime; message was: \
         {err}"
    );
    assert!(
        err.message
            .contains("$sourceDescriptions.flows.<workflowId>"),
        "the remedy is the workflowId field; message was: {err}"
    );
    assert!(
        !err.message.contains("this runtime resolves"),
        "message was: {err}"
    );
}

/// The other half of the split: an asyncapi source naming an operation is
/// conformant Arazzo — v1.1.0's own async step example does it — so this
/// refusal must keep claiming only this runtime's missing transport, and must
/// not borrow the specification's authority the arazzo case has.
#[tokio::test]
async fn qualified_target_naming_an_asyncapi_source_is_refused_as_a_runtime_limit() {
    let fixture = TwoSources::new();
    let mut sources = fixture.sources();
    sources.push(SourceDescription {
        name: "events".to_string(),
        url: "other.asyncapi.yaml".to_string(),
        type_: SourceType::AsyncApi,
        ..SourceDescription::default()
    });
    let err = refusal(
        spec_with(sources, "$sourceDescriptions.events.getPet"),
        fixture.dir.path(),
    )
    .await;
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedSourceDescriptionType);
    assert_eq!(err.code(), "RUNTIME_UNSUPPORTED_SOURCE_DESCRIPTION_TYPE");
    assert!(err.message.contains("\"events\""), "message was: {err}");
    assert!(err.message.contains("\"asyncapi\""), "message was: {err}");
    assert!(
        err.message
            .contains("this runtime resolves an operationId only"),
        "message was: {err}"
    );
    assert!(
        !err.message.contains("the specification does not let"),
        "an asyncapi operationId is conformant; message was: {err}"
    );
}

/// `betaOnly` exists — in the other document. The refusal names the source
/// that was searched, because "not found" alone reads as a typo in the
/// operation name.
#[tokio::test]
async fn qualified_target_missing_from_the_named_source_is_refused() {
    let fixture = TwoSources::new();
    let err = refusal(
        spec_with(fixture.sources(), "$sourceDescriptions.alpha.betaOnly"),
        fixture.dir.path(),
    )
    .await;
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdNotFound);
    assert_eq!(err.code(), "RUNTIME_OPERATION_ID_NOT_FOUND");
    assert!(err.message.contains("\"betaOnly\""), "message was: {err}");
    assert!(err.message.contains("\"alpha\""), "message was: {err}");
}

/// Every value that reaches for the qualified form without being it. None of
/// them falls back to bare lookup, which would report a missing operation
/// named `$sourceDescriptions.alpha` instead of the typo actually present.
#[tokio::test]
async fn malformed_qualified_forms_are_refused_with_the_supported_shape() {
    let fixture = TwoSources::new();
    const MALFORMED: &[&str] = &[
        "$sourceDescriptions.getPet",
        "$sourceDescriptions",
        "$sourceDescriptions.",
        "$sourceDescriptions.alpha.",
        "$sourceDescriptions..getPet",
        // `source-name` is `identifier-strict`.
        "$sourceDescriptions.al pha.getPet",
        // `source-reference-id` is `CHAR`, which excludes braces.
        "$sourceDescriptions.alpha.get{Pet}",
    ];
    for target in MALFORMED {
        let err = refusal(spec_with(fixture.sources(), target), fixture.dir.path()).await;
        assert_eq!(
            err.kind,
            RuntimeErrorKind::UnsupportedOperationIdForm,
            "kind for {target}: {err}"
        );
        assert_eq!(err.code(), "RUNTIME_UNSUPPORTED_OPERATION_ID_FORM");
        assert!(err.message.contains(target), "message was: {err}");
        assert!(
            err.message
                .contains("$sourceDescriptions.<name>.<operationId>"),
            "message was: {err}"
        );
    }

    let (alpha_hits, beta_hits) = fixture.hits();
    assert!(
        alpha_hits.is_empty() && beta_hits.is_empty(),
        "a malformed operationId must send nothing; alpha={alpha_hits:?} beta={beta_hits:?}"
    );
}

// ── Single-source and explicit-spec compatibility ───────────────────

/// One source description is the case the MUST does not cover, so a bare id
/// keeps resolving — and now resolves against that source's own base rather
/// than an engine-wide one that merely happened to equal it.
#[tokio::test]
async fn a_single_source_still_resolves_a_bare_operation_id() {
    let fixture = TwoSources::new();
    let url = match send(
        spec_with(vec![openapi_source("beta", "beta.openapi.yaml")], "getPet"),
        fixture.dir.path(),
    )
    .await
    {
        Ok(url) => url,
        Err(err) => panic!("resolving a bare id against one source: {err}"),
    };
    assert_eq!(url, format!("{}/animals", fixture.beta.base_url));

    let (alpha_hits, beta_hits) = fixture.hits();
    assert!(alpha_hits.is_empty(), "alpha was reached: {alpha_hits:?}");
    assert_eq!(beta_hits, vec!["/animals".to_string()]);
}

/// `arazzo` sources do not count toward the MUST — it is scoped to
/// "(non arazzo type) sourceDescriptions" — so one OpenAPI source alongside
/// one Arazzo source keeps a bare id resolvable.
#[tokio::test]
async fn arazzo_sources_do_not_make_a_bare_operation_id_ambiguous() {
    let fixture = TwoSources::new();
    let sources = vec![
        openapi_source("alpha", "alpha.openapi.yaml"),
        SourceDescription {
            name: "flows".to_string(),
            url: "other.arazzo.yaml".to_string(),
            type_: SourceType::Arazzo,
            ..SourceDescription::default()
        },
    ];
    let url = match send(spec_with(sources, "getPet"), fixture.dir.path()).await {
        Ok(url) => url,
        Err(err) => panic!("resolving a bare id beside an arazzo source: {err}"),
    };
    assert_eq!(url, format!("{}/pets", fixture.alpha.base_url));
}

/// An explicitly provided spec belongs to no source description. It keeps
/// winning a bare id, and stays invisible to a qualified target — which is
/// what makes `$sourceDescriptions.alpha.<id>` mean "from alpha" and nothing
/// else.
#[tokio::test]
async fn an_explicit_spec_wins_a_bare_id_and_never_answers_a_qualified_one() {
    let fixture = TwoSources::new();
    let explicit = openapi_document(&fixture.beta.base_url, "/override", "explicitOnly");

    let bare = match send(
        spec_with(
            vec![openapi_source("alpha", "alpha.openapi.yaml")],
            "getPet",
        ),
        fixture.dir.path(),
    )
    .await
    {
        Ok(url) => url,
        Err(err) => panic!("resolving a bare id with no explicit spec: {err}"),
    };
    assert_eq!(
        bare,
        format!("{}/pets", fixture.alpha.base_url),
        "without an explicit spec the source document answers"
    );

    let engine = match EngineBuilder::new(spec_with(
        vec![openapi_source("alpha", "alpha.openapi.yaml")],
        "getPet",
    ))
    .source_base_dir(fixture.dir.path())
    .openapi_spec(explicit.clone().into_bytes())
    .dry_run(true)
    .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building an engine with an explicit spec: {err}"),
    };
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    if let Err(err) = result.outputs {
        panic!("resolving a bare id against an explicit spec: {err}");
    }
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url,
        format!("{}/override", fixture.alpha.base_url),
        "the explicit spec's path wins, against the engine base it always used"
    );

    // The same explicit spec cannot satisfy a source-qualified target.
    let engine = match EngineBuilder::new(spec_with(
        vec![openapi_source("alpha", "alpha.openapi.yaml")],
        "$sourceDescriptions.alpha.explicitOnly",
    ))
    .source_base_dir(fixture.dir.path())
    .openapi_spec(explicit.into_bytes())
    .dry_run(true)
    .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building an engine with an explicit spec: {err}"),
    };
    let err = match engine
        .execute_collect(WORKFLOW, BTreeMap::new())
        .await
        .outputs
    {
        Ok(outputs) => panic!("expected a refusal, got outputs {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdNotFound);
    assert!(err.message.contains("\"alpha\""), "message was: {err}");
}

/// An explicitly provided spec does not exempt a document from the MUST: the
/// refusal counts declared sourceDescriptions, so `--openapi` plus two sources
/// is still refused. The message must not then recommend the qualified form on
/// its own, because `in_source` deliberately cannot see an explicit spec — the
/// operator would follow the advice and land on OperationIdNotFound.
#[tokio::test]
async fn an_explicit_spec_does_not_exempt_a_multi_source_document() {
    let fixture = TwoSources::new();
    let explicit = openapi_document("https://explicit.example.com", "/override", "explicitOnly");

    let engine = match EngineBuilder::new(spec_with(fixture.sources(), "explicitOnly"))
        .source_base_dir(fixture.dir.path())
        .openapi_spec(explicit.into_bytes())
        .dry_run(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building an engine with an explicit spec: {err}"),
    };
    let err = match engine
        .execute_collect(WORKFLOW, BTreeMap::new())
        .await
        .outputs
    {
        Ok(outputs) => panic!("expected a refusal, got outputs {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdAmbiguous);
    assert!(
        err.message.contains("absolute-URL operationPath"),
        "the remedy must name a form that can actually reach it; message was: {err}"
    );
    assert!(
        err.message.contains("neither of those forms can reach it"),
        "message was: {err}"
    );
    let conformant = err
        .message
        .find("declare that document as a sourceDescription")
        .unwrap_or_else(|| panic!("the conformant remedy must be offered; message was: {err}"));
    let debt = err
        .message
        .find("absolute-URL operationPath")
        .unwrap_or_else(|| panic!("message was: {err}"));
    assert!(
        conformant < debt,
        "the sourceDescription remedy is the conformant one and must lead the F1 absolute-URL \
         form; message was: {err}"
    );
    assert!(
        err.message.contains("<METHOD> <url>"),
        "without the method token a PUT or DELETE step silently takes the GET/POST default; \
         message was: {err}"
    );
}

/// The converse branch: with no separately provided spec there is nothing to
/// warn about, and the note must not appear.
#[tokio::test]
async fn the_explicit_spec_note_appears_only_when_a_spec_was_provided() {
    let fixture = TwoSources::new();
    let err = refusal(spec_with(fixture.sources(), "getPet"), fixture.dir.path()).await;
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdAmbiguous);
    assert!(
        !err.message.contains("separately provided OpenAPI spec"),
        "message was: {err}"
    );
}

/// One document defining the same operationId under two paths is a real
/// document error, and both definitions carry the same origin. The count stays
/// truthful while the origin is named once — naming it twice reads as a bug in
/// the message rather than a clash inside the document.
#[tokio::test]
async fn an_ambiguity_within_one_document_names_that_document_once() {
    let fixture = TwoSources::new();
    fixture.dir.write(
        "twice.openapi.yaml",
        &[
            "openapi: \"3.0.0\"",
            "info:",
            "  title: fixture",
            "  version: \"1.0.0\"",
            "servers:",
            "  - url: https://twice.example.com",
            "paths:",
            "  /first:",
            "    get:",
            "      operationId: getPet",
            "  /second:",
            "    get:",
            "      operationId: getPet",
            "",
        ]
        .join("\n"),
    );

    let err = refusal(
        spec_with(
            vec![openapi_source("twice", "twice.openapi.yaml")],
            "getPet",
        ),
        fixture.dir.path(),
    )
    .await;
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdAmbiguous);
    assert!(
        err.message.contains("defined 2 times"),
        "the count must stay truthful; message was: {err}"
    );
    assert_eq!(
        err.message.matches("sourceDescription \"twice\"").count(),
        1,
        "the one origin must be named once; message was: {err}"
    );
}

/// Two explicit specs defining the same id used to resolve to whichever was
/// passed last. Insertion order is not an answer, so the clash is reported.
#[tokio::test]
async fn duplicate_ids_across_explicit_specs_are_refused_rather_than_ordered() {
    let fixture = TwoSources::new();
    let first = openapi_document("https://first.example.com", "/first", "firstOnly");
    let second = openapi_document("https://second.example.com", "/second", "secondOnly");

    let engine = match EngineBuilder::new(spec_with(
        vec![openapi_source("alpha", "alpha.openapi.yaml")],
        "getPet",
    ))
    .source_base_dir(fixture.dir.path())
    .openapi_spec(first.into_bytes())
    .openapi_spec(second.into_bytes())
    .dry_run(true)
    .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building an engine with two explicit specs: {err}"),
    };
    let err = match engine
        .execute_collect(WORKFLOW, BTreeMap::new())
        .await
        .outputs
    {
        Ok(outputs) => panic!("expected a refusal, got outputs {outputs:?}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::OperationIdAmbiguous);
    assert_eq!(err.code(), "RUNTIME_OPERATION_ID_AMBIGUOUS");
    assert!(err.message.contains("spec #1"), "message was: {err}");
    assert!(err.message.contains("spec #2"), "message was: {err}");
}
