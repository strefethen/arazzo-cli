#![forbid(unsafe_code)]
#![allow(clippy::expect_used)]

//! Generation uses real operation IDs or standard pointers, and refuses
//! duplicate IDs before selecting CRUD resource groups.

use arazzo_generate::crud::{generate_crud, GenerateOutput};
use arazzo_spec::StepTarget;
use serde_json::{json, Value};

fn document(paths: Value) -> openapiv3::OpenAPI {
    serde_json::from_value(json!({
        "openapi": "3.0.3",
        "info": {"title": "Widgets", "version": "1"},
        "servers": [{"url": "https://widgets.example.test/v1"}],
        "paths": paths
    }))
    .expect("self-contained OpenAPI 3.0 document")
}

fn operation(id: Option<&str>) -> Value {
    let mut operation = json!({"responses": {"200": {"description": "Success"}}});
    if let Some(id) = id {
        operation["operationId"] = json!(id);
    }
    operation
}

fn generate(openapi: &openapiv3::OpenAPI) -> Result<GenerateOutput, String> {
    generate_crud(
        openapi,
        "widgets.openapi.yaml",
        "../api/widgets.openapi.yaml",
    )
}

#[test]
fn representable_ids_are_preferred_without_restricting_punctuation() {
    for id in [
        "listWidgets",
        "svc.v1.list",
        "read /widgets?x=1%23:!@",
        "Read_é",
    ] {
        let openapi = document(
            json!({"/widgets": {"get": operation(Some(id)), "post": operation(Some("createWidgets"))}}),
        );
        let output = generate(&openapi).expect("unique ID generates");
        assert_eq!(
            output.spec.source_descriptions[0].url,
            "../api/widgets.openapi.yaml"
        );
        assert_eq!(
            output.spec.workflows[0].steps[1].target,
            Some(StepTarget::OperationId(format!(
                "$sourceDescriptions.widgets.{id}"
            )))
        );
    }
}

#[test]
fn absent_or_unrepresentable_ids_use_the_actual_operation_pointer() {
    for id in [
        None,
        Some("read{widgets}"),
        Some("read\\widgets"),
        Some("read\"widgets"),
    ] {
        let openapi = document(
            json!({"/widgets": {"post": operation(id), "get": operation(Some("listWidgets"))}}),
        );
        let output = generate(&openapi).expect("pointer handles absent or unrepresentable ID");
        assert_eq!(
            output.spec.workflows[0].steps.len(),
            2,
            "only the two actual CRUD operations are emitted"
        );
        assert_eq!(
            output.spec.workflows[0].steps[0].target,
            Some(StepTarget::OperationPath(
                "{$sourceDescriptions.widgets.url}#/paths/~1widgets/post".into()
            ))
        );
    }
}

#[test]
fn pointer_tokens_and_uri_fragment_bytes_are_escaped_in_order() {
    let path = "/widgets/~/%2F/é !#?";
    let openapi =
        document(json!({path: {"get": operation(None), "post": operation(Some("createWidgets"))}}));
    let output = generate(&openapi).expect("punctuated paths generate");
    assert_eq!(
        output.spec.workflows[0].steps[1].target,
        Some(StepTarget::OperationPath(
            "{$sourceDescriptions.widgets.url}#/paths/~1widgets~1~0~1%252F~1%C3%A9%20%21%23%3F/get"
                .into()
        ))
    );
}

#[test]
fn item_update_pointer_preserves_actual_method_and_parameter_path() {
    let openapi = document(
        json!({"/widgets": {"get": operation(Some("listWidgets"))}, "/widgets/{widgetId}": {"patch": operation(None)}}),
    );
    let output = generate(&openapi).expect("actual PATCH operation generates");
    assert_eq!(output.spec.workflows[0].steps.len(), 2);
    assert_eq!(
        output.spec.workflows[0].steps[1].target,
        Some(StepTarget::OperationPath(
            "{$sourceDescriptions.widgets.url}#/paths/~1widgets~1%7BwidgetId%7D/patch".into()
        ))
    );
}

#[test]
fn duplicates_are_errors_even_outside_selected_crud_operations() {
    for (path, method) in [
        ("/widgets", "post"),
        ("/health", "head"),
        ("/health", "options"),
        ("/health", "trace"),
        ("/accounts/{accountId}/widgets/{widgetId}", "get"),
    ] {
        let mut paths = json!({"/widgets": {"get": operation(Some("duplicate"))}});
        paths[path][method] = operation(Some("duplicate"));
        let error = generate(&document(paths))
            .err()
            .expect("duplicate IDs return no output");
        assert!(
            error.contains("duplicate operationId \"duplicate\""),
            "{error}"
        );
        assert!(error.contains("GET /widgets"), "{error}");
        assert!(
            error.contains(&format!("{} {path}", method.to_ascii_uppercase())),
            "{error}"
        );
    }
}

#[test]
fn duplicate_unrepresentable_ids_are_not_hidden_by_pointers() {
    let openapi = document(json!({"/widgets": {
        "get": operation(Some("read{widgets}")),
        "post": operation(Some("read{widgets}"))
    }}));
    assert!(generate(&openapi).is_err());
}

#[test]
fn operation_ids_are_case_sensitive() {
    let openapi = document(json!({"/widgets": {
        "get": operation(Some("listWidgets")),
        "post": operation(Some("ListWidgets"))
    }}));
    assert_eq!(
        generate(&openapi)
            .expect("case-distinct IDs are unique")
            .spec
            .workflows[0]
            .steps
            .len(),
        2
    );
}
