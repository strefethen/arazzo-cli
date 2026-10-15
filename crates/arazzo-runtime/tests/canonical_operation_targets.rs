#![forbid(unsafe_code)]
//! Canonical target resolution against actual source-bound OpenAPI documents.
use std::{collections::BTreeMap, time::Duration};

use arazzo_runtime::{EngineBuilder, ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};
use serde_json::{json, Value};

const DOCUMENT: &str = "https://documents.invalid/api.json";

fn specification(target: StepTarget) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".into(),
        info: Info {
            title: "canonical targets".into(),
            version: "1".into(),
            ..Info::default()
        },
        source_descriptions: vec![source("api", DOCUMENT)],
        workflows: vec![Workflow {
            workflow_id: "wf".into(),
            steps: vec![Step {
                step_id: "call".into(),
                target: Some(target),
                ..Step::default()
            }],
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

fn source(name: &str, url: &str) -> SourceDescription {
    SourceDescription {
        name: name.into(),
        url: url.into(),
        type_: SourceType::OpenApi,
        ..SourceDescription::default()
    }
}

fn pointer(fragment: &str) -> StepTarget {
    StepTarget::OperationPath(format!("{{$sourceDescriptions.api.url}}#{fragment}"))
}

fn id(id: &str) -> StepTarget {
    StepTarget::OperationId(id.into())
}

fn document(server: &str) -> Value {
    json!({"openapi": "3.2.0", "$self": DOCUMENT, "info": {"title": "API", "version": "1"},
        "servers": [{"url": server}],
        "paths": {
            "/pets": {"get": {"operationId": "listPets"}, "post": {}},
            "/path": {"servers": [{"url": "https://path.invalid/v2"}],
                "put": {"operationId": "pathServer"},
                "delete": {"operationId": "operationServer", "servers": [{"url": "https://operation.invalid/v3"}]}},
            "/a~b/café": {"query": {"operationId": "queryPets"}, "additionalOperations": {"froB": {"operationId": "custom"}}},
            "/reference": {"$ref": "#/components/pathItems/Ref"}
        },
        "components": {"pathItems": {"Ref": {"get": {"operationId": "referenced"}}},
            "schemas": {"LooksLikeOperation": {"operationId": "notAnOperation"}}}
    })
}

fn builder(spec: ArazzoSpec, document: &Value) -> EngineBuilder {
    EngineBuilder::new(spec).openapi_spec(document.to_string().into_bytes(), None)
}

async fn planned(target: StepTarget, doc: &Value) -> (String, String) {
    let engine = builder(specification(target), doc)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    (requests[0].method.clone(), requests[0].url.clone())
}

fn refused(result: &ExecutionResult, kind: RuntimeErrorKind) -> String {
    assert!(
        result.dry_run_requests().is_empty(),
        "refusal must not create a planned request"
    );
    let error = match &result.outputs {
        Err(error) => error,
        Ok(outputs) => panic!("target should be refused, got outputs: {outputs:?}"),
    };
    assert_eq!(error.kind, kind);
    error.to_string()
}

#[tokio::test]
async fn pointers_and_ids_share_methods_paths_and_server_precedence() {
    let doc = document("https://root.invalid/v1");
    for (fragment, operation_id, method, url) in [
        (
            "/paths/~1pets/get",
            "listPets",
            "GET",
            "https://root.invalid/v1/pets",
        ),
        (
            "/paths/~1path/put",
            "pathServer",
            "PUT",
            "https://path.invalid/v2/path",
        ),
        (
            "/paths/~1path/delete",
            "operationServer",
            "DELETE",
            "https://operation.invalid/v3/path",
        ),
        (
            "/paths/~1a~0b~1caf%C3%A9/query",
            "queryPets",
            "QUERY",
            "https://root.invalid/v1/a~b/café",
        ),
        (
            "/paths/~1a~0b~1caf%C3%A9/additionalOperations/froB",
            "custom",
            "froB",
            "https://root.invalid/v1/a~b/café",
        ),
    ] {
        let by_pointer = planned(pointer(fragment), &doc).await;
        let by_id = planned(id(&format!("$sourceDescriptions.api.{operation_id}")), &doc).await;
        assert_eq!(by_pointer, (method.into(), url.into()));
        assert_eq!(by_pointer, by_id);
    }
    assert_eq!(
        planned(id("listPets"), &doc).await,
        planned(pointer("%2Fpaths%2F~1pets%2Fget"), &doc).await
    );
    assert_eq!(
        planned(pointer("/paths/~1pets/post"), &doc).await.0,
        "POST",
        "pointer does not require an ID or body"
    );
}

#[tokio::test]
async fn source_qualified_ids_and_pointers_never_cross_bound_sources() {
    let mut primary = document("https://primary.invalid");
    primary["paths"]["/pets"]["get"]["operationId"] = json!("primaryOnly");
    primary["paths"]["/overlap"] = json!({"get": {"operationId": "shared"}});
    let mut secondary = document("https://secondary.invalid");
    secondary["$self"] = json!("https://documents.invalid/other.json");
    secondary["paths"]["/pets"]["get"]["operationId"] = json!("secondaryOnly");
    secondary["paths"]["/overlap"] = json!({"get": {"operationId": "shared"}});
    secondary["paths"]["/secondary-only"] = json!({"get": {}});
    for target in [
        pointer("/paths/~1pets/get"),
        id("$sourceDescriptions.api.primaryOnly"),
    ] {
        let mut spec = specification(target);
        spec.source_descriptions
            .push(source("other", "https://documents.invalid/other.json"));
        let engine = builder(spec, &primary)
            .openapi_spec(secondary.to_string().into_bytes(), None)
            .dry_run(true)
            .build()
            .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
        let result = engine.execute_collect("wf", BTreeMap::new()).await;
        assert!(result.outputs.is_ok());
        assert_eq!(
            result.dry_run_requests()[0].url,
            "https://primary.invalid/pets"
        );
    }
    let mut spec = specification(id("$sourceDescriptions.api.shared"));
    spec.source_descriptions
        .push(source("other", "https://documents.invalid/other.json"));
    let engine = builder(spec, &primary)
        .openapi_spec(secondary.to_string().into_bytes(), None)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(result.outputs.is_ok());
    assert_eq!(
        result.dry_run_requests()[0].url,
        "https://primary.invalid/overlap"
    );
    for (target, kind) in [
        (
            pointer("/paths/~1secondary-only/get"),
            RuntimeErrorKind::SourceDescriptionParse,
        ),
        (
            id("$sourceDescriptions.api.secondaryOnly"),
            RuntimeErrorKind::OperationIdNotFound,
        ),
        (id("primaryOnly"), RuntimeErrorKind::OperationIdAmbiguous),
    ] {
        let mut spec = specification(target);
        spec.source_descriptions
            .push(source("other", "https://documents.invalid/other.json"));
        let engine = builder(spec, &primary)
            .openapi_spec(secondary.to_string().into_bytes(), None)
            .dry_run(true)
            .build()
            .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
        refused(&engine.execute_collect("wf", BTreeMap::new()).await, kind);
    }
}

#[tokio::test]
async fn duplicate_ids_are_ambiguous_but_pointers_select_the_actual_operation() {
    let mut doc = document("https://root.invalid");
    doc["paths"]["/pets"]["post"]["operationId"] = json!("listPets");
    let engine = builder(specification(id("listPets")), &doc)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
    refused(
        &engine.execute_collect("wf", BTreeMap::new()).await,
        RuntimeErrorKind::OperationIdAmbiguous,
    );
    assert_eq!(planned(pointer("/paths/~1pets/post"), &doc).await.0, "POST");
}

#[tokio::test]
async fn unclaimed_documents_cannot_supply_operations() {
    let bound = document("https://bound.invalid");
    let mut unclaimed = document("https://unclaimed.invalid");
    unclaimed["$self"] = json!("https://documents.invalid/unclaimed.json");
    unclaimed["paths"]["/pets"]["get"]["operationId"] = json!("unclaimedOnly");
    for target in [
        id("unclaimedOnly"),
        id("$sourceDescriptions.api.unclaimedOnly"),
    ] {
        let engine = builder(specification(target), &bound)
            .openapi_spec(unclaimed.to_string().into_bytes(), None)
            .dry_run(true)
            .build()
            .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
        refused(
            &engine.execute_collect("wf", BTreeMap::new()).await,
            RuntimeErrorKind::OperationIdNotFound,
        );
    }
    let mut spec = specification(id("unclaimedOnly"));
    spec.source_descriptions.clear();
    let engine = builder(spec, &unclaimed)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
    refused(
        &engine.execute_collect("wf", BTreeMap::new()).await,
        RuntimeErrorKind::OperationIdNotFound,
    );
}

#[test]
fn unmatched_url_cannot_become_a_request_base_and_provided_identity_wins() {
    let error = match EngineBuilder::new(specification(pointer("/paths/~1pets/get"))).build() {
        Err(error) => error,
        Ok(_) => panic!("unbound document URL must fail engine construction"),
    };
    assert_eq!(error.kind, RuntimeErrorKind::SourceDescriptionNotFound);
    assert!(error
        .to_string()
        .contains("no document is retrieved over the network"));
    let mut doc = document("https://root.invalid");
    doc["$self"] = json!("https://documents.invalid/different.json");
    assert!(builder(specification(pointer("/paths/~1pets/get")), &doc)
        .build()
        .is_err());
}

#[tokio::test]
async fn malformed_custom_and_missing_targets_refuse_before_plan_or_traffic() {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("binding hermetic capture server: {error}"));
    let doc = document(&format!("http://{}", server.server_addr()));
    for target in [
        "/pets",
        "GET /pets",
        "{api}./pets",
        "https://api.invalid/pets",
        "{$sourceDescriptions.api.url}",
        "#/paths/~1pets/get",
        "GET {$sourceDescriptions.api.url}#/paths/~1pets/get",
        "{$sourceDescriptions.api.url}#/paths/~2/get",
    ] {
        for dry_run in [true, false] {
            let engine = builder(
                specification(StepTarget::OperationPath(target.into())),
                &doc,
            )
            .dry_run(dry_run)
            .build()
            .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
            refused(
                &engine.execute_collect("wf", BTreeMap::new()).await,
                RuntimeErrorKind::UnsupportedOperationPathForm,
            );
        }
    }
    for fragment in [
        "/paths/~1missing/get",
        "/paths/~1pets",
        "/paths/~1pets/get/responses",
        "/components/schemas/LooksLikeOperation",
        "/paths/~1reference/get",
        "/components/pathItems/Ref/get",
        "",
    ] {
        for dry_run in [true, false] {
            let engine = builder(specification(pointer(fragment)), &doc)
                .dry_run(dry_run)
                .build()
                .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
            let message = refused(
                &engine.execute_collect("wf", BTreeMap::new()).await,
                RuntimeErrorKind::SourceDescriptionParse,
            );
            for context in ["api", DOCUMENT, fragment] {
                assert!(message.contains(context), "{message}");
            }
        }
    }
    assert!(server
        .recv_timeout(Duration::from_millis(30))
        .unwrap_or_else(|error| panic!("checking for unexpected traffic: {error}"))
        .is_none());
}

#[tokio::test]
async fn missing_sources_and_unsupported_server_context_refuse() {
    let doc = document("https://root.invalid");
    let target =
        StepTarget::OperationPath("{$sourceDescriptions.missing.url}#/paths/~1pets/get".into());
    let engine = builder(specification(target), &doc)
        .dry_run(true)
        .build()
        .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
    refused(
        &engine.execute_collect("wf", BTreeMap::new()).await,
        RuntimeErrorKind::SourceDescriptionNotFound,
    );
    for level in ["path", "operation"] {
        let mut doc = document("https://root.invalid");
        let servers = json!([{"url": "/relative"}]);
        if level == "path" {
            doc["paths"]["/pets"]["servers"] = servers;
        } else {
            doc["paths"]["/pets"]["get"]["servers"] = servers;
        }
        for target in [pointer("/paths/~1pets/get"), id("listPets")] {
            let engine = builder(specification(target), &doc)
                .dry_run(true)
                .build()
                .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
            refused(
                &engine.execute_collect("wf", BTreeMap::new()).await,
                RuntimeErrorKind::SourceDescriptionParse,
            );
        }
    }
}

#[tokio::test]
async fn planned_and_sent_request_use_the_same_operation_server_and_method() {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("binding hermetic capture server: {error}"));
    let endpoint = format!("http://{}", server.server_addr());
    let mut doc = document("https://unused.invalid");
    for method in ["post", "query"] {
        doc["paths"]["/pets"][method] = json!({"servers": [{"url": format!("{endpoint}/v2")}]});
    }
    doc["paths"]["/pets"]["additionalOperations"] =
        json!({"froB": {"servers": [{"url": format!("{endpoint}/v2")}]}});
    let capture = std::thread::spawn(move || {
        let mut received = Vec::new();
        for _ in 0..3 {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|error| panic!("receiving captured request: {error}"))
                .unwrap_or_else(|| panic!("expected a captured request before the timeout"));
            received.push((
                request.method().as_str().to_string(),
                request.url().to_string(),
            ));
            request
                .respond(tiny_http::Response::empty(200))
                .unwrap_or_else(|error| panic!("responding to captured request: {error}"));
        }
        received
    });
    for (fragment, method) in [
        ("/paths/~1pets/post", "POST"),
        ("/paths/~1pets/query", "QUERY"),
        ("/paths/~1pets/additionalOperations/froB", "froB"),
    ] {
        let target = pointer(fragment);
        assert_eq!(
            planned(target.clone(), &doc).await,
            (method.into(), format!("{endpoint}/v2/pets"))
        );
        let engine = builder(specification(target), &doc)
            .build()
            .unwrap_or_else(|error| panic!("building source-bound test engine: {error}"));
        let result = engine.execute_collect("wf", BTreeMap::new()).await;
        assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    }
    assert_eq!(
        capture
            .join()
            .unwrap_or_else(|error| panic!("request capture thread failed: {error:?}")),
        vec![
            ("POST".into(), "/v2/pets".into()),
            ("QUERY".into(), "/v2/pets".into()),
            ("froB".into(), "/v2/pets".into())
        ]
    );
}
