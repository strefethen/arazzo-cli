#![forbid(unsafe_code)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_runtime::{EngineBuilder, ExecutionResult};
use arazzo_spec::{ArazzoSpec, Step};
use common::{
    logged_requests, new_request_log, record_request, start_server, MockHttpResponse, TestObserver,
};
use serde_json::{json, Value};

const CODE: &str = "RUNTIME_UNRESOLVED_REQUEST_VALUE";
const SOURCE: &str = "https://fixture.invalid/openapi.json";

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn target(parameters: Value) -> Step {
    parse(json!({"stepId":"remove", "operationId":"remove", "parameters":parameters}))
}

fn builder(base: &str, workflows: Value) -> EngineBuilder {
    let spec: ArazzoSpec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"admission", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":workflows
    }));
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":base}],
    "paths":{
        "/find":{"get":{"operationId":"find", "responses":{"200":{"description":"ok"}}}},
        "/pets/{id}":{"delete":{"operationId":"remove", "responses":{"200":{"description":"ok"}}}},
        "/recovery":{"get":{"operationId":"recovery", "responses":{"200":{"description":"ok"}}}}
    }});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .trace(true)
}

fn workflows(step: Step) -> Value {
    json!([{"workflowId":"main", "steps":[step]}])
}

fn refused(result: &ExecutionResult, observer: &TestObserver, location: &str, source: &str) {
    let error = result
        .outputs
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("request must refuse"));
    assert_eq!(error.code(), CODE, "{error}");
    assert!(error.message.contains("remove"), "{error}");
    assert!(error.message.contains(location), "{error}");
    assert!(error.message.contains(source), "{error}");
    assert!(error.message.ends_with("request not sent"), "{error}");
    assert!(result
        .dry_run_requests()
        .iter()
        .all(|request| request.step_id != "remove"));
    for event in observer.events() {
        assert!(!event.starts_with("RequestPrepared:remove:"), "{event}");
        assert!(!event.starts_with("RequestSent:remove:"), "{event}");
        assert!(!event.starts_with("CriterionEvaluated:remove:"), "{event}");
        assert!(!event.starts_with("RetryScheduled:remove:"), "{event}");
    }
    let traces = result.trace_steps();
    let attempts: Vec<_> = traces
        .iter()
        .filter(|step| step.step_id == "remove")
        .collect();
    assert_eq!(attempts.len(), 1);
    assert!(attempts
        .iter()
        .all(|step| step.request.is_none() && step.response.is_none() && step.criteria.is_empty()));
}

#[tokio::test]
async fn missing_id_never_sends_delete_in_sequential_parallel_or_run_step() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"name":"rex"}"#)
    });
    for mode in ["sequential", "parallel", "step"] {
        for source in ["$steps.find.outputs.id", "prefix-{$steps.find.outputs.id}"] {
            let observer = Arc::new(TestObserver::default());
            let engine = builder(&server.base_url, json!([{"workflowId":"main", "steps":[
                {"stepId":"find", "operationId":"find", "outputs":{"id":"$response.body#/id"}},
                {"stepId":"remove", "operationId":"remove", "dependsOn":["find"], "parameters":[{"name":"id", "in":"path", "value":source},{"name":"owner", "in":"query", "value":"$steps.find.outputs.id"}]}
            ]}])).parallel(mode == "parallel").observer(observer.clone()).build().unwrap_or_else(|error| panic!("engine: {error}"));
            let before = logged_requests(&log).len();
            let result = if mode == "step" {
                engine
                    .execute_step("main", "remove", BTreeMap::new(), false)
                    .collect()
                    .await
            } else {
                engine.execute_collect("main", BTreeMap::new()).await
            };
            refused(
                &result,
                &observer,
                "parameters[0](path:id)",
                "$steps.find.outputs.id",
            );
            if mode == "parallel" {
                assert!(result.sequential_fallbacks().is_empty());
            }
            let requests = logged_requests(&log);
            assert_eq!(requests.len(), before + 1);
            assert_eq!(requests[before].method, "GET");
            assert_eq!(requests[before].url, "/find");
        }
    }
}

#[tokio::test]
async fn nested_missing_id_keeps_its_code_and_never_sends_delete() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    for strict in [false, true] {
        for value in ["$inputs.id", "prefix-{$inputs.id}"] {
            let observer = Arc::new(TestObserver::default());
            let engine = builder(&server.base_url,json!([
                {"workflowId":"main","steps":[
                    {"stepId":"find","operationId":"find","outputs":{"id":"$response.body#/id"}},
                    {"stepId":"child","workflowId":"child","parameters":[{"name":"id","value":"$steps.find.outputs.id"}]}
                ]},
                {"workflowId":"child","inputs":{"type":"object","required":["id"],"properties":{"id":{"type":"string","enum":["ok"]}}},
                 "steps":[target(json!([{"name":"id","in":"path","value":value}]))]}
            ])).strict_inputs(strict).observer(observer.clone()).build().unwrap_or_else(|error| panic!("engine: {error}"));
            let before = logged_requests(&log).len();
            let result = engine.execute_collect("main", BTreeMap::new()).await;
            refused(&result, &observer, "parameters[0](path:id)", "$inputs.id");
            assert_eq!(logged_requests(&log).len(), before + 1);
            assert_eq!(logged_requests(&log)[before].url, "/find");
        }
    }
}

#[tokio::test]
async fn every_parameter_location_refuses_direct_nested_and_template_misses() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    for location in ["path", "query", "querystring", "header", "cookie"] {
        for (value, suffix) in [
            (json!("$inputs.absent"), ""),
            (json!("prefix-{$inputs.absent}-suffix"), ""),
            (json!({"a/b": ["$inputs.absent"]}), "/a~1b/0"),
        ] {
            for dry in [false, true] {
                let observer = Arc::new(TestObserver::default());
                let step = target(json!([{"name":"id", "in":location, "value":value}]));
                let engine = builder(&server.base_url, workflows(step))
                    .dry_run(dry)
                    .observer(observer.clone())
                    .build()
                    .unwrap_or_else(|error| panic!("engine: {error}"));
                let result = engine.execute_collect("main", BTreeMap::new()).await;
                refused(
                    &result,
                    &observer,
                    &format!("parameters[0]({location}:id){suffix}"),
                    "$inputs.absent",
                );
            }
        }
    }
    assert!(logged_requests(&log).is_empty());
}

#[tokio::test]
async fn canonical_missing_namespaces_and_syntax_are_refused_without_literal_leaks() {
    for (value, descriptor) in [
        (
            json!("$steps.unknown.outputs.id"),
            "$steps.unknown.outputs.id",
        ),
        (
            json!("$workflows.unknown.outputs.id"),
            "$workflows.unknown.outputs.id",
        ),
        (json!("$outputs.unknown"), "$outputs.unknown"),
        (
            json!("$components.parameters.unknown"),
            "$components.parameters.unknown",
        ),
        (
            json!("$sourceDescriptions.unknown.url"),
            "$sourceDescriptions.unknown.url",
        ),
        (
            json!("$sourceDescriptions.api.unknown"),
            "$sourceDescriptions.api.unknown",
        ),
        (json!("$response.body#/id"), "$response.body#/id"),
        (json!("$request.body#/id"), "$request.body#/id"),
        (json!("$request.query.id"), "$request.query.id"),
        (json!("$url"), "$url"),
        (json!("$statusCode"), "$statusCode"),
        (
            json!("secret-canary-{$inputs.absent"),
            "invalid expression syntax",
        ),
        (
            json!({"context":"$inputs.document", "selector":"$[", "type":"jsonpath"}),
            "Selector value",
        ),
        (
            json!({"context":"secret-canary-{$inputs.absent", "selector":"$", "type":"jsonpath"}),
            "Selector context",
        ),
    ] {
        let observer = Arc::new(TestObserver::default());
        let step = target(json!([{"name":"id", "in":"path", "value":value}]));
        let engine = builder("https://api.invalid", workflows(step))
            .dry_run(true)
            .observer(observer.clone())
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        refused(&result, &observer, "parameters[0](path:id)", descriptor);
        assert!(!result
            .outputs
            .as_ref()
            .err()
            .unwrap_or_else(|| panic!("refusal"))
            .message
            .contains("secret-canary"));
    }
}

#[tokio::test]
async fn payload_and_replacement_locations_preserve_leaf_order_and_escaped_pointers() {
    for (body, location, source) in [
        (
            json!({"contentType":"application/json", "payload":"$inputs.absent"}),
            "requestBody.payload",
            "$inputs.absent",
        ),
        (
            json!({"contentType":"application/json", "payload":{"a/b~":["$inputs.first", "$inputs.second"]}}),
            "requestBody.payload/a~1b~0/0",
            "$inputs.first",
        ),
        (
            json!({"contentType":"application/json", "payload":{"id":0}, "replacements":[{"target":"/id", "value":{"nested":["$inputs.absent"]}}]}),
            "requestBody.replacements[0].value/nested/0",
            "$inputs.absent",
        ),
        (
            json!({"contentType":"application/json", "payload":{"id":0}, "replacements":[{"target":"/id", "value":{"context":"$inputs.absent", "selector":"$", "type":"jsonpath"}}]}),
            "requestBody.replacements[0].value",
            "$inputs.absent",
        ),
    ] {
        let observer = Arc::new(TestObserver::default());
        let mut step = target(json!([{"name":"id","in":"path","value":"safe"}]));
        step.request_body = Some(parse(body));
        let engine = builder("https://api.invalid", workflows(step))
            .dry_run(true)
            .observer(observer.clone())
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        refused(&result, &observer, location, source);
    }
    let observer = Arc::new(TestObserver::default());
    let mut step = target(json!([
        {"name":"header", "in":"header", "value":"$inputs.later"},
        {"name":"id", "in":"path", "value":"$inputs.first"}
    ]));
    step.request_body = Some(parse(
        json!({"contentType":"application/json","payload":"$inputs.body"}),
    ));
    let engine = builder("https://api.invalid", workflows(step))
        .dry_run(true)
        .observer(observer.clone())
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"));
    refused(
        &engine.execute_collect("main", BTreeMap::new()).await,
        &observer,
        "parameters[1](path:id)",
        "$inputs.first",
    );
}

#[tokio::test]
async fn inherited_parameters_and_strict_unknown_child_inputs_use_the_guard() {
    for strict in [false, true] {
        let observer = Arc::new(TestObserver::default());
        let engine = builder("https://api.invalid",json!([
            {"workflowId":"main","steps":[{"stepId":"child","workflowId":"child","parameters":[{"name":"id","value":"$inputs.missing"}]}]},
            {"workflowId":"child","inputs":{"type":"object","required":["id"],"properties":{"id":{"type":"string","enum":["ok"]}}},
             "parameters":[{"name":"id","in":"path","value":"$inputs.id"}],
             "steps":[{"stepId":"remove","operationId":"remove"}]}
        ])).dry_run(true).strict_inputs(strict).observer(observer.clone()).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        refused(&result, &observer, "parameters[0](path:id)", "$inputs.id");
    }
}

#[tokio::test]
async fn concrete_missing_null_type_and_enum_invalid_inputs_keep_validation() {
    for inputs in [
        json!({}),
        json!({"id":null}),
        json!({"id":7}),
        json!({"id":"bad"}),
    ] {
        let engine = builder("https://api.invalid",json!([{
            "workflowId":"main","inputs":{"type":"object","required":["id"],"properties":{"id":{"type":"string","enum":["ok"]}}},
            "steps":[target(json!([{"name":"id","in":"path","value":"$inputs.id"}]))]
        }])).strict_inputs(true).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine
            .execute_collect("main", parse::<BTreeMap<String, Value>>(inputs))
            .await;
        assert_eq!(
            result
                .outputs
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("invalid input"))
                .code(),
            "RUNTIME_INPUT_VALIDATION"
        );
        assert!(result.dry_run_requests().is_empty());
    }
}

#[tokio::test]
async fn null_empty_optional_omissions_and_zero_match_selectors_stay_admitted() {
    for value in [
        json!(null),
        json!(""),
        json!("$inputs.optional"),
        json!({"context":"$inputs.document","selector":"$.absent","type":"jsonpath"}),
        json!({"context":"$inputs.document","selector":"$.nil","type":"jsonpath"}),
    ] {
        let engine = builder("https://api.invalid",json!([{
            "workflowId":"main","inputs":{"type":"object","properties":{"optional":{"type":"string"},"document":{"type":"object"}}},
            "steps":[target(json!([{"name":"id","in":"path","value":value}]))]
        }])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine
            .execute_collect(
                "main",
                BTreeMap::from([("document".into(), json!({"nil":null}))]),
            )
            .await;
        assert!(result.outputs.is_ok(), "{:?}", result.outputs);
        assert_eq!(
            result.dry_run_requests()[0].url,
            "https://api.invalid/pets/"
        );
    }
}

#[tokio::test]
async fn live_l4_null_empty_and_selector_residual_reach_loopback_unchanged() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, r#"{"id":null,"name":"rex"}"#)
    });
    for mode in ["sequential", "parallel", "step"] {
        for (output, value) in [
            (json!("$response.body#/id"), json!(null)),
            (json!("$response.body#/id"), json!("")),
            (json!("$response.body#/id"), json!("$steps.find.outputs.id")),
            (
                json!({"context":"$response.body","selector":"$.absent","type":"jsonpath"}),
                json!("$steps.find.outputs.id"),
            ),
            (
                json!({"context":"$response.body","selector":"/absent","type":"jsonpointer"}),
                json!("$steps.find.outputs.id"),
            ),
        ] {
            let engine = builder(
                &server.base_url,
                json!([{"workflowId":"main","steps":[
                    {"stepId":"find","operationId":"find","outputs":{"id":output}},
                    {"stepId":"remove","operationId":"remove","dependsOn":["find"],"parameters":[
                        {"name":"id","in":"path","value":value},
                        {"name":"owner","in":"query","value":value}
                    ]}
                ]}]),
            )
            .parallel(mode == "parallel")
            .build()
            .unwrap_or_else(|error| panic!("fixture result required: {error:?}"));
            let before = logged_requests(&log).len();
            let result = if mode == "step" {
                engine
                    .execute_step("main", "remove", BTreeMap::new(), false)
                    .collect()
                    .await
            } else {
                engine.execute_collect("main", BTreeMap::new()).await
            };
            assert!(result.outputs.is_ok(), "{mode}: {:?}", result.outputs);
            if mode == "parallel" {
                assert!(result.sequential_fallbacks().is_empty());
            }
            let requests = logged_requests(&log);
            assert_eq!(requests.len(), before + 2);
            assert_eq!(requests[before].method, "GET");
            assert_eq!(requests[before + 1].method, "DELETE");
            let expected = if value == json!("") {
                "/pets/?owner="
            } else {
                "/pets/"
            };
            assert_eq!(requests[before + 1].url, expected);
        }
    }
}

#[tokio::test]
async fn method_resolves_at_every_request_location() {
    for location in ["path", "query", "querystring", "header", "cookie"] {
        let value = if location == "querystring" {
            "method={$method}"
        } else {
            "$method"
        };
        let mut step = target(json!([{"name":"id","in":location,"value":value}]));
        step.request_body = Some(parse(
            json!({"contentType":"application/json","payload":{"method":"$method","replaced":"old"},"replacements":[{"target":"/replaced","value":"$method"}]}),
        ));
        let engine = builder("https://api.invalid", workflows(step))
            .dry_run(true)
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        assert!(result.outputs.is_ok(), "{:?}", result.outputs);
        let request = result.dry_run_requests()[0];
        assert_eq!(
            request.body,
            Some(json!({"method":"DELETE","replaced":"DELETE"}))
        );
        match location {
            "path" => assert!(request.url.ends_with("/pets/DELETE")),
            "query" => assert!(request.url.ends_with("?id=DELETE")),
            "querystring" => assert!(request.url.ends_with("?method=DELETE")),
            "header" => assert_eq!(request.headers["id"], "DELETE"),
            "cookie" => assert_eq!(request.headers["Cookie"], "id=DELETE"),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn skipped_dry_run_dependencies_stay_admitted_and_genuine_misses_dominate() {
    for mixed in [false, true] {
        let observer = Arc::new(TestObserver::default());
        let value = if mixed {
            "{$steps.find.outputs.id}-{$inputs.absent}"
        } else {
            "$steps.find.outputs.id"
        };
        let engine = builder(
            "https://api.invalid",
            json!([{"workflowId":"main","steps":[
                {"stepId":"find","operationId":"find","outputs":{"id":"$response.body#/id"}},
                target(json!([{"name":"id","in":"path","value":value}]))
            ]}]),
        )
        .dry_run(true)
        .observer(observer.clone())
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        if mixed {
            refused(
                &result,
                &observer,
                "parameters[0](path:id)",
                "$inputs.absent",
            );
            assert_eq!(result.dry_run_requests().len(), 1);
        } else {
            assert!(result.outputs.is_ok(), "{:?}", result.outputs);
            assert_eq!(result.dry_run_requests().len(), 2);
        }
    }
}

#[tokio::test]
async fn retry_is_inapplicable_while_goto_and_end_preserve_their_routing() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, "{}")
    });
    for route in ["none", "retry", "end", "goto"] {
        let retry = json!({"name":"retry","type":"retry","retryLimit":3,"retryAfter":0,"stepId":"recovery","parameters":[{"name":"x","value":"$inputs.missing"}]});
        let actions = match route {
            "none" => json!([]),
            "retry" => json!([retry]),
            "end" => json!([retry,{"name":"end","type":"end"}]),
            "goto" => json!([retry,{"name":"goto","type":"goto","stepId":"recovery"}]),
            _ => unreachable!(),
        };
        let observer = Arc::new(TestObserver::default());
        let engine = builder(&server.base_url,json!([{"workflowId":"main","steps":[
            {"stepId":"remove","operationId":"remove","parameters":[{"name":"id","in":"path","value":"$inputs.missing"}],"onFailure":actions},
            {"stepId":"recovery","operationId":"recovery"}
        ]}])).observer(observer.clone()).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let before = logged_requests(&log).len();
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        if route == "goto" {
            assert!(result.outputs.is_ok(), "{:?}", result.outputs);
            assert_eq!(logged_requests(&log).len(), before + 1);
            assert_eq!(logged_requests(&log)[before].url, "/recovery");
            assert_eq!(
                observer
                    .events()
                    .iter()
                    .filter(|event| *event == "StepStarted:remove")
                    .count(),
                1
            );
            assert!(!observer
                .events()
                .iter()
                .any(|event| event.starts_with("RequestPrepared:remove:")
                    || event.starts_with("RetryScheduled:remove:")));
        } else if route == "end" {
            assert_eq!(
                result
                    .outputs
                    .as_ref()
                    .err()
                    .unwrap_or_else(|| panic!("authored end"))
                    .code(),
                CODE
            );
            assert_eq!(logged_requests(&log).len(), before);
            assert_eq!(
                result
                    .trace_steps()
                    .iter()
                    .filter(|step| step.step_id == "remove")
                    .count(),
                1
            );
            assert!(!observer
                .events()
                .iter()
                .any(|event| event.starts_with("RequestPrepared:remove:")
                    || event.starts_with("RetryScheduled:remove:")));
        } else {
            refused(
                &result,
                &observer,
                "parameters[0](path:id)",
                "$inputs.missing",
            );
            assert_eq!(logged_requests(&log).len(), before);
        }
    }
}
