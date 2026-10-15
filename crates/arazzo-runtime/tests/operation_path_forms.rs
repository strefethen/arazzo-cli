#![forbid(unsafe_code)]

//! Standard operation pointers resolve real source-bound operations. Removed
//! shortcut forms are explicit rejection cases and never become request URLs.

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, RuntimeErrorKind};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};

/// Distinct server bases prove which bound document supplied the operation.
const PRIMARY_BASE: &str = "https://primary.example.com/v1";
const SECONDARY_BASE: &str = "https://secondary.example.com/v2";

fn spec_with(operation_path: &str) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: "operationPath forms".to_string(),
            version: "1.0.0".to_string(),
            ..Info::default()
        },
        source_descriptions: vec![
            SourceDescription {
                name: "primary".to_string(),
                url: "https://descriptions.example.test/primary.yaml".to_string(),
                type_: SourceType::OpenApi,
                ..SourceDescription::default()
            },
            SourceDescription {
                name: "secondary".to_string(),
                url: "https://descriptions.example.test/secondary.yaml".to_string(),
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

/// Explicit document catalog, independent of the target under test.
fn builder(operation_path: &str) -> EngineBuilder {
    let mut builder = EngineBuilder::new(spec_with(operation_path)).dry_run(true);
    for (name, base) in [("primary", PRIMARY_BASE), ("secondary", SECONDARY_BASE)] {
        let document = serde_json::json!({
            "openapi": "3.0.3",
            "$self": format!("https://descriptions.example.test/{name}.yaml"),
            "info": {"title": "pointer fixture", "version": "1"},
            "servers": [{"url": base}],
            "paths": {
                "/pets": {
                    "get": {"responses": {"200": {"description": "OK"}}},
                    "post": {"responses": {"200": {"description": "OK"}}}
                },
                "/pets/{petId}": {
                    "parameters": [{"name": "petId", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "get": {"responses": {"200": {"description": "OK"}}}
                },
                "/pets/7": {"delete": {"responses": {"200": {"description": "OK"}}}}
            }
        });
        builder = builder.openapi_spec(document.to_string().into_bytes(), None);
    }
    builder
}

/// Resolves one `operationPath` to the `(method, url)` a dry run would send.
async fn dry_run(operation_path: &str) -> Result<(String, String), RuntimeErrorKind> {
    let engine = match builder(operation_path).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine for {operation_path:?}: {err}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(err) = &result.outputs {
        assert!(
            result.dry_run_requests().is_empty(),
            "refused operationPath must plan no request"
        );
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
fn pointer_uses_the_primary_document_server() {
    let (method, url) = expect_url("{$sourceDescriptions.primary.url}#/paths/~1pets/get");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://primary.example.com/v1/pets");
}

#[test]
fn pointer_routes_to_the_named_source() {
    let (method, url) = expect_url("{$sourceDescriptions.secondary.url}#/paths/~1pets/get");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://secondary.example.com/v2/pets");
}

#[test]
fn pointer_uses_the_actual_operation_method() {
    let (method, url) = expect_url("{$sourceDescriptions.secondary.url}#/paths/~1pets/post");
    assert_eq!(method, "POST");
    assert_eq!(url, "https://secondary.example.com/v2/pets");
}

#[test]
fn pointer_can_select_delete_without_a_method_override() {
    let (method, url) = expect_url("{$sourceDescriptions.primary.url}#/paths/~1pets~17/delete");
    assert_eq!(method, "DELETE");
    assert_eq!(url, "https://primary.example.com/v1/pets/7");
}

#[test]
fn pointer_keeps_path_parameters_for_parameter_substitution() {
    let (method, url) =
        expect_url("{$sourceDescriptions.primary.url}#/paths/~1pets~1%7BpetId%7D/get");
    assert_eq!(method, "GET");
    assert_eq!(url, "https://primary.example.com/v1/pets/{petId}");
}

/// Every shape this runtime cannot resolve, and the exact text it used to
/// paste into a request URL instead of failing.
const UNSUPPORTED: &[&str] = &[
    "/pets",
    "{secondary}./pets",
    "POST {secondary}./pets",
    "https://elsewhere.example.com/pets?page=2",
    "DELETE https://elsewhere.example.com/pets/7",
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
    let operation_path = "POST {primary}./pets";
    let engine = match builder(operation_path).build() {
        Ok(engine) => engine,
        Err(err) => panic!("building engine: {err}"),
    };
    let result = futures_block_on(engine.execute_collect("wf", BTreeMap::new()));
    let err = match &result.outputs {
        Ok(outputs) => panic!("expected failure, got outputs {outputs:?}"),
        Err(err) => err.to_string(),
    };

    assert!(err.contains("step probe:"), "message was: {err}");
    assert!(err.contains(operation_path), "message was: {err}");
    assert!(
        err.contains("$sourceDescriptions.<name>.url"),
        "message was: {err}"
    );
    assert!(err.contains("JSON Pointer"), "message was: {err}");

    assert!(
        result.dry_run_requests().is_empty(),
        "no request may be planned for an unresolvable operationPath"
    );
}

#[test]
fn unknown_source_reference_still_fails_with_its_own_code() {
    let kind = match futures_block_on(dry_run(
        "{$sourceDescriptions.missing.url}#/paths/~1pets/get",
    )) {
        Ok((_, url)) => panic!("expected unknown source failure, resolved to {url}"),
        Err(kind) => kind,
    };
    assert_eq!(kind, RuntimeErrorKind::SourceDescriptionNotFound);
}
