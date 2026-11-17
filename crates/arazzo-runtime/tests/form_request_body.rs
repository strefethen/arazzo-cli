#![forbid(unsafe_code)]

mod common;

use arazzo_runtime::{EngineBuilder, ExecutionResult};
use common::{
    logged_requests, new_request_log, record_request, start_server, MockHttpResponse, TestObserver,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};

const FORM: &str = "application/x-www-form-urlencoded";
const SOURCE: &str = "https://fixture.invalid/openapi.json";

fn builder(base: &str, body: Value) -> EngineBuilder {
    let spec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"forms","version":"1"},
        "sourceDescriptions":[{"name":"api","url":SOURCE,"type":"openapi"}],
        "workflows":[{"workflowId":"main","steps":[{"stepId":"send","operationId":"send",
        "requestBody":body,"outputs":{"body":"$request.body","name":"$request.body#/name"}}],
        "outputs":{"body":"$steps.send.outputs.body","name":"$steps.send.outputs.name"}}]
    }))
    .unwrap_or_else(|e| panic!("spec: {e}"));
    let api = json!({"openapi":"3.2.0","$self":SOURCE,"info":{"title":"forms","version":"1"},
        "servers":[{"url":base}],"paths":{"/send":{"post":{"operationId":"send",
        "requestBody":{"content":{FORM:{"schema":{"type":"object"}}}},
        "responses":{"200":{"description":"ok"}}}}}});
    EngineBuilder::new(spec)
        .openapi_spec(api.to_string().into_bytes(), None)
        .trace(true)
}

async fn exercise(
    body: Value,
    inputs: BTreeMap<String, Value>,
    dry: bool,
) -> (
    ExecutionResult,
    Vec<common::RecordedRequest>,
    Arc<TestObserver>,
) {
    let log = new_request_log();
    let capture = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&capture, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    let observer = Arc::new(TestObserver::default());
    let engine = builder(&server.base_url, body)
        .dry_run(dry)
        .observer(observer.clone())
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));
    let result = engine.execute_collect("main", inputs).await;
    drop(server);
    (result, logged_requests(&log), observer)
}

#[tokio::test]
async fn form_wire_matrix() {
    let cases = vec![
        (json!({"name":"Alice Smith"}),"name=Alice+Smith"),
        (json!({"empty":""}),"empty="), (json!({"nickname":null}),""),
        (json!({"q":"a+b&c=d e"}),"q=a%2Bb%26c%3Dd+e"),
        (json!({"city":"São Paulo ☕"}),"city=S%C3%A3o+Paulo+%E2%98%95"),
        (json!({"a b":"c~d"}),"a+b=c%7Ed"),
        (json!({"s":"\"quoted\""}),"s=%22quoted%22"),
        (json!({"n":42,"ok":true,"pi":1.5}),"n=42&ok=true&pi=1.5"),
        (json!({"tag":["a","b",null,"c d"]}),"tag=a&tag=b&tag=c+d"),
        (json!({"tag":[]}),""), (json!({}),""), (json!({"a":null,"b":null}),""),
        (json!({"meta":{"x":null,"y":[1,"two"]}}),"meta=%7B%22x%22%3Anull%2C%22y%22%3A%5B1%2C%22two%22%5D%7D"),
        (json!({"pts":[{"x":1},[1,2],{"n":null},[null]]}),"pts=%7B%22x%22%3A1%7D&pts=%5B1%2C2%5D&pts=%7B%22n%22%3Anull%7D&pts=%5Bnull%5D"),
        (json!("client_id=synthetic-client&scope=read+write"),"client_id=synthetic-client&scope=read+write"),
        (json!(""),""),
        (json!({"address":{"zip":"99999+1234"},"city":"São Paulo ☕","client_id":"synthetic-client","empty":"","grant_type":"authorization_code","n":42,"nickname":null,"ok":true,"q":"a+b&c=d e","tag":["a","b",null,"c d"]}),"address=%7B%22zip%22%3A%2299999%2B1234%22%7D&city=S%C3%A3o+Paulo+%E2%98%95&client_id=synthetic-client&empty=&grant_type=authorization_code&n=42&ok=true&q=a%2Bb%26c%3Dd+e&tag=a&tag=b&tag=c+d")
    ];
    for (logical, wire) in cases {
        for ct in [FORM, "Application/X-WWW-Form-Urlencoded; charset=UTF-8"] {
            let (result, requests, _) = exercise(
                json!({"contentType":ct,"payload":logical}),
                BTreeMap::new(),
                false,
            )
            .await;
            assert!(result.outputs.is_ok(), "{:?}", result.outputs);
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].body.as_bytes(), wire.as_bytes());
            assert_eq!(
                common::header_value(&requests[0].headers, "Content-Type").as_deref(),
                Some(ct)
            );
            assert_eq!(
                result.trace_steps()[0].request.as_ref().map(|r| &r.body),
                Some(&Some(logical.clone()))
            );
            assert_eq!(
                result.outputs.as_ref().ok().and_then(|o| o.get("body")),
                Some(&logical)
            );
        }
        let (result, requests, _) = exercise(
            json!({"contentType":FORM,"payload":logical}),
            BTreeMap::new(),
            true,
        )
        .await;
        assert!(result.outputs.is_ok(), "{:?}", result.outputs);
        assert!(requests.is_empty());
        assert_eq!(result.dry_run_requests()[0].body, Some(logical));
    }
}

#[tokio::test]
async fn form_fields_follow_document_order() {
    let document = format!(
        r#"
arazzo: 1.1.0
info: {{title: forms, version: "1"}}
sourceDescriptions:
  - {{name: api, url: "{SOURCE}", type: openapi}}
workflows:
  - workflowId: main
    steps:
      - stepId: send
        operationId: send
        requestBody:
          contentType: {FORM}
          payload:
            z: 1
            a: 2
            m: {{y: 1, b: 2}}
            tag: [q, c]
"#
    );
    let spec =
        arazzo_validate::parse_bytes(document.as_bytes()).unwrap_or_else(|e| panic!("parse: {e}"));
    let log = new_request_log();
    let capture = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&capture, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    let api = json!({"openapi":"3.2.0","$self":SOURCE,"info":{"title":"forms","version":"1"},
        "servers":[{"url":server.base_url}],"paths":{"/send":{"post":{"operationId":"send",
        "requestBody":{"content":{FORM:{"schema":{"type":"object"}}}},
        "responses":{"200":{"description":"ok"}}}}}});
    let engine = EngineBuilder::new(spec)
        .openapi_spec(api.to_string().into_bytes(), None)
        .build()
        .unwrap_or_else(|e| panic!("build: {e}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    drop(server);
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let requests = logged_requests(&log);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body, "z=1&a=2&m=%7B%22y%22%3A1%2C%22b%22%3A2%7D&tag=q&tag=c",
        "fields must follow document order"
    );
    assert_ne!(
        requests[0].body, "a=2&m=%7B%22b%22%3A2%2C%22y%22%3A1%7D&tag=q&tag=c&z=1",
        "sorted order must not be produced"
    );
}

#[tokio::test]
async fn refused_shapes_and_unresolved_are_zero_send() {
    for (value, kind) in [
        (json!([]), "array"),
        (json!(["SECRET"]), "array"),
        (json!(42), "number"),
        (json!(true), "boolean"),
        (json!(false), "boolean"),
        (Value::Null, "null"),
    ] {
        for dry in [false, true] {
            let (result, requests, observer) = exercise(
                json!({"contentType":FORM,"payload":"$inputs.body"}),
                BTreeMap::from([("body".into(), value.clone())]),
                dry,
            )
            .await;
            let error = result
                .outputs
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("must refuse {kind}"));
            assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY");
            assert_eq!(error.message,format!("step send: application/x-www-form-urlencoded request body must be an object or string; got {kind}"));
            assert!(requests.is_empty());
            assert!(result.dry_run_requests().is_empty());
            assert!(observer
                .events()
                .iter()
                .all(|e| !e.starts_with("RequestSent:")));
        }
    }
    for dry in [false, true] {
        let (result, requests, _) = exercise(
            json!({"contentType":FORM,"payload":"$inputs.missing"}),
            BTreeMap::new(),
            dry,
        )
        .await;
        assert_eq!(
            result.outputs.as_ref().err().map(|e| e.code()),
            Some("RUNTIME_UNRESOLVED_REQUEST_VALUE")
        );
        assert!(requests.is_empty());
        let (result, requests, _) =
            exercise(json!({"contentType":FORM}), BTreeMap::new(), dry).await;
        assert!(result.outputs.is_ok());
        if !dry {
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].body, "");
            assert!(common::header_value(&requests[0].headers, "Content-Type").is_none());
        }
    }
}

#[tokio::test]
async fn evaluation_replacements_and_controls() {
    let inputs = BTreeMap::from([
        ("text".into(), json!("a+b&c=d e")),
        ("data".into(), json!({"name":"old"})),
    ]);
    let body = json!({"contentType":FORM,"payload":{"context":"$inputs.data","selector":"","type":"jsonpointer"},
        "replacements":[{"target":"/name","targetSelectorType":"jsonpointer","value":"$inputs.text"}]});
    for dry in [false, true] {
        let (result, requests, _) = exercise(body.clone(), inputs.clone(), dry).await;
        assert_eq!(
            result.outputs.as_ref().ok().and_then(|o| o.get("name")),
            Some(&json!("a+b&c=d e"))
        );
        if dry {
            assert_eq!(
                result.dry_run_requests()[0].body,
                Some(json!({"name":"a+b&c=d e"}))
            );
        } else {
            assert_eq!(requests[0].body, "name=a%2Bb%26c%3Dd+e");
        }
    }
    let (result, requests, _) = exercise(
        json!({"contentType":FORM,"payload":"name={$inputs.text}"}),
        inputs,
        false,
    )
    .await;
    assert!(result.outputs.is_ok());
    assert_eq!(requests[0].body, "name=a+b&c=d e");
    for ct in [
        "",
        "application/json",
        "application/x-www-form-urlencoded-extra",
        "text/application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        let (_, requests, _) = exercise(
            json!({"contentType":ct,"payload":{"name":"a b"}}),
            BTreeMap::new(),
            false,
        )
        .await;
        assert_eq!(requests[0].body, r#"{"name":"a b"}"#);
    }
    let (_, requests, _) =
        exercise(json!({"payload":{"name":"a b"}}), BTreeMap::new(), false).await;
    assert_eq!(requests[0].body, r#"{"name":"a b"}"#);
    let (_, requests, _) = exercise(
        json!({"contentType":"application/xml","payload":"<name>a b</name>"}),
        BTreeMap::new(),
        false,
    )
    .await;
    assert_eq!(requests[0].body, "<name>a b</name>");
}
