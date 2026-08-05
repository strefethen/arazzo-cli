#![forbid(unsafe_code)]

//! Request parity for every `operationPath` form this runtime resolves.
//!
//! Three of the four supported forms are also pinned end to end by the golden
//! sweep in `arazzo-cli/tests/golden_spec_baseline.rs`: bare paths appear in 16
//! of the 18 `examples/` documents, `{source}./path` in
//! `examples/multi-api-orchestration.arazzo.yaml`, and `"METHOD {source}./path"`
//! in `examples/swagger-petstore-crud.arazzo.yaml`. The absolute-URL form
//! appears in no `examples/` or `testdata/` document at all, so it is pinned
//! here or nowhere.

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};

/// First source description, and therefore the base bare paths resolve against.
const PRIMARY_BASE: &str = "https://primary.example.com/v1";
const SECONDARY_BASE: &str = "https://secondary.example.com/v2";

fn spec_with(operation_path: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: "operationPath forms".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![
            SourceDescription {
                name: "primary".to_string(),
                url: PRIMARY_BASE.to_string(),
                type_: SourceType::OpenApi,
                ..SourceDescription::default()
            },
            SourceDescription {
                name: "secondary".to_string(),
                url: SECONDARY_BASE.to_string(),
                type_: SourceType::OpenApi,
                ..SourceDescription::default()
            },
        ],
        workflows: vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![Step {
                step_id: "probe".to_string(),
                target: Some(StepTarget::OperationPath(operation_path.to_string())),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

/// Resolves one `operationPath` to the `(method, url)` a dry run would send.
async fn dry_run(operation_path: &str) -> Result<(String, String), RuntimeErrorKind> {
    let engine = match EngineBuilder::new(spec_with(operation_path))
        .dry_run(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine for {operation_path:?}: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        return Err(err.kind);
    }
    let requests = result.dry_run_requests();
    assert_eq!(
        requests.len(),
        1,
        "expected exactly one planned request for {operation_path:?}"
    );
    Ok((requests[0].method.clone(), requests[0].url.clone()))
}

fn expect_url(operation_path: &str) -> (String, String) {
    match futures_block_on(dry_run(operation_path)) {
        Ok(pair) => pair,
        Err(kind) => panic!(
            "expected {operation_path:?} to resolve, got {}",
            kind.code()
        ),
    }
}

/// Minimal single-threaded block-on so each case reads as a plain assertion.
fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => panic!("building tokio runtime: {err}"),
    };
    runtime.block_on(future)
}

#[test]
fn bare_path_joins_the_first_source_base() {
    let (method, url) = expect_url("/pets");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://primary.example.com/v1/pets");
}

#[test]
fn source_routed_path_joins_that_source_base() {
    let (method, url) = expect_url("{secondary}./pets");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://secondary.example.com/v2/pets");
}

#[test]
fn method_prefixed_source_routed_path_keeps_method_and_base() {
    let (method, url) = expect_url("POST {secondary}./pets");
    assert_eq!(method, "POST");
    assert_eq!(url, "https://secondary.example.com/v2/pets");
}

/// The form no `examples/` or `testdata/` document exercises: an absolute URL
/// replaces the base entirely rather than being appended to it.
#[test]
fn absolute_url_replaces_every_base() {
    let (method, url) = expect_url("https://elsewhere.example.com/pets?page=2");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://elsewhere.example.com/pets?page=2");
}

#[test]
fn method_prefixed_absolute_url_keeps_method_and_replaces_the_base() {
    let (method, url) = expect_url("DELETE https://elsewhere.example.com/pets/7");
    assert_eq!(method, "DELETE");
    assert_eq!(url, "https://elsewhere.example.com/pets/7");
}

/// A path-parameter placeholder is not a Source Description reference: the
/// `{sourceName}.` form requires the dot after `}`.
#[test]
fn path_parameter_placeholder_is_left_for_parameter_substitution() {
    let (method, url) = expect_url("/pets/{petId}");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://primary.example.com/v1/pets/{petId}");
}

/// Every shape this runtime cannot resolve, and the exact text it used to
/// paste into a request URL instead of failing.
const UNSUPPORTED: &[&str] = &[
    "{$sourceDescriptions.primary.url}#/paths/~1pets/get",
    "GET {$sourceDescriptions.primary.url}#/paths/~1pets/get",
    "{$sourceDescriptions.primary.url}",
    "#/paths/~1pets/get",
];

#[test]
fn unsupported_forms_fail_with_a_stable_code_and_plan_no_request() {
    for operation_path in UNSUPPORTED {
        let kind = match futures_block_on(dry_run(operation_path)) {
            Ok((_, url)) => panic!("expected {operation_path:?} to fail, planned request to {url}"),
            Err(kind) => kind,
        };
        assert_eq!(
            kind,
            RuntimeErrorKind::UnsupportedOperationPathForm,
            "kind for {operation_path:?}"
        );
        assert_eq!(kind.code(), "RUNTIME_UNSUPPORTED_OPERATION_PATH_FORM");
    }
}

/// The failure has to name the step and quote the value, because the whole
/// point is that the operator can find which step to rewrite.
#[test]
fn unsupported_form_message_names_the_step_and_quotes_the_path() {
    let operation_path = "{$sourceDescriptions.primary.url}#/paths/~1pets/get";
    let engine = match EngineBuilder::new(spec_with(operation_path))
        .dry_run(true)
        .build()
    {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = futures_block_on(engine.execute_collect("wf", BTreeMap::new()));
    let err = match &result.outputs {
        Ok(outputs) => panic!("expected failure, got outputs {outputs:?}"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("step \"probe\""), "message was: {err}");
    assert!(err.contains(operation_path), "message was: {err}");
    assert!(
        err.contains("a runtime expression and a JSON Pointer fragment"),
        "message was: {err}"
    );
    assert!(err.contains("not implemented"), "message was: {err}");
    assert!(err.contains("{sourceName}./path"), "message was: {err}");

    assert!(
        result.dry_run_requests().is_empty(),
        "no request may be planned for an unresolvable operationPath"
    );
}

#[test]
fn unknown_source_reference_still_fails_with_its_own_code() {
    let kind = match futures_block_on(dry_run("{missing}./pets")) {
        Ok((_, url)) => panic!("expected unknown source failure, resolved to {url}"),
        Err(kind) => kind,
    };
    assert_eq!(kind, RuntimeErrorKind::SourceDescriptionNotFound);
}
