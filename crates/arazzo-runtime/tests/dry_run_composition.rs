#![forbid(unsafe_code)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use arazzo_runtime::{EngineBuilder, ExecutionResult};
use arazzo_spec::ArazzoSpec;
use common::{logged_requests, new_request_log, record_request, start_server, MockHttpResponse};
use serde_json::{json, Value};

const SOURCE: &str = "$response.body#/audio_url";
const MARKER: &str = "<unresolved:$response.body#/audio_url>";

fn builder(base: &str, workflows: Value) -> EngineBuilder {
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0","info":{"title":"composition","version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://fixture.invalid/api.json","type":"openapi"}],
        "workflows":workflows
    })).unwrap_or_else(|error| panic!("spec: {error}"));
    let openapi = json!({"openapi":"3.2.0","$self":"https://fixture.invalid/api.json",
    "info":{"title":"api","version":"1"},"servers":[{"url":base}],
    "paths":{
        "/produce":{"get":{"operationId":"produce","responses":{"200":{"description":"ok"}}}},
        "/consume/{id}":{"post":{"operationId":"consume","responses":{"200":{"description":"ok"}}}}
    }});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .trace(true)
}

fn successful(result: &ExecutionResult) {
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
}

fn producer(workflow: &str, name: &str, source: &str) -> String {
    format!("dry-run unresolved output {workflow}/fetch.outputs.{name}: {source} requires a response; transport was skipped")
}

fn consumer(workflow: &str, location: &str, source: &str) -> String {
    format!("dry-run unresolved {workflow}/play/{location}: main/fetch.outputs.audio depends on {source}; transport was skipped")
}

fn audition() -> Value {
    json!([
        {"workflowId":"main","steps":[
            {"stepId":"produce","workflowId":"producer"},
            {"stepId":"consume","workflowId":"consumer","parameters":[{"name":"streamUrl","value":"$steps.produce.outputs.streamUrl"}]}
        ]},
        {"workflowId":"producer","steps":[{"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}}],"outputs":{"streamUrl":"$steps.fetch.outputs.audio"}},
        {"workflowId":"consumer","inputs":{"type":"object","required":["streamUrl"],"properties":{"streamUrl":{"type":"string"}}},"steps":[
            {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"}],"requestBody":{"contentType":"application/json","payload":{"streamUrl":"$inputs.streamUrl"}}}
        ]}
    ])
}

#[tokio::test]
async fn audition_required_typed_child_input_keeps_two_requests_and_exact_live_bytes() {
    let log = new_request_log();
    let captured = Arc::clone(&log);
    let actual = "https://media.invalid/a?x=1&y=%2F#part";
    let response = json!({"audio_url":actual}).to_string();
    let server = start_server(move |method, url, headers, body| {
        record_request(&captured, &method, &url, &headers, &body);
        MockHttpResponse::json(200, &response)
    });
    for strict in [false, true] {
        let engine = builder(&server.base_url, audition())
            .dry_run(true)
            .strict_inputs(strict)
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        successful(&result);
        let requests = result.dry_run_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].step_id, "fetch");
        assert_eq!(requests[1].step_id, "play");
        assert_eq!(requests[1].body, Some(json!({"streamUrl":MARKER})));
        assert_eq!(
            requests[0].warnings,
            vec![producer("producer", "audio", SOURCE)]
        );
        assert_eq!(requests[1].warnings, vec![format!("dry-run unresolved consumer/play/requestBody.payload/streamUrl: producer/fetch.outputs.audio depends on {SOURCE}; transport was skipped")]);
        assert!(logged_requests(&log).is_empty());
        assert!(result
            .trace_steps()
            .iter()
            .flat_map(|trace| &trace.warnings)
            .all(|warning| !warning.contains("input validation")));
    }
    let engine = builder(&server.base_url, audition())
        .strict_inputs(true)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    successful(&result);
    let requests = logged_requests(&log);
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].body.as_bytes(),
        json!({"streamUrl":actual}).to_string().as_bytes()
    );
    assert!(result.dry_run_requests().is_empty());
}

#[tokio::test]
async fn direct_locations_keep_encoding_indices_declaration_order_and_ordinary_values() {
    for parallel in [false, true] {
        let engine = builder("https://api.invalid", json!([{"workflowId":"main","steps":[
            {"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}},
            {"stepId":"play","operationId":"consume","dependsOn":["fetch"],"parameters":[
                {"name":"h","in":"header","value":"$steps.fetch.outputs.audio"},
                {"name":"id","in":"path","value":"$steps.fetch.outputs.audio"},
                {"name":"q","in":"query","value":"$steps.fetch.outputs.audio"},
                {"name":"c","in":"cookie","value":"$steps.fetch.outputs.audio"}
            ],"requestBody":{"contentType":"application/json","payload":{"a/b":{"~key":["$steps.fetch.outputs.audio","$steps.fetch.outputs.audio"]},"replace":"old"},"replacements":[{"target":"/replace","value":"$steps.fetch.outputs.audio"}]},
            "outputs":{"url":"$url","body":"$request.body","status":"$statusCode"}}
        ],"outputs":{"url":"$steps.play.outputs.url","body":"$steps.play.outputs.body","status":"$steps.play.outputs.status"}}])).dry_run(true).parallel(parallel).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        successful(&result);
        assert!(result.sequential_fallbacks().is_empty());
        let requests = result.dry_run_requests();
        let request = &requests[1];
        let url = url::Url::parse(&request.url).unwrap_or_else(|error| panic!("url: {error}"));
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("q".into(), MARKER.into())]
        );
        assert!(
            url.path()
                .contains("%3Cunresolved:$response.body%23%2Faudio_url%3E"),
            "{}",
            url.path()
        );
        assert_eq!(request.headers["h"], MARKER);
        assert_eq!(request.headers["Cookie"], format!("c={MARKER}"));
        assert_eq!(
            request.body,
            Some(json!({"a/b":{"~key":[MARKER,MARKER]},"replace":MARKER}))
        );
        let expected = [
            "parameters[0](header:h)",
            "parameters[1](path:id)",
            "parameters[2](query:q)",
            "parameters[3](cookie:c)",
            "requestBody.payload/a~1b/~0key/0",
            "requestBody.payload/a~1b/~0key/1",
            "requestBody.replacements[0].value",
        ]
        .map(|location| consumer("main", location, SOURCE));
        assert_eq!(&request.warnings[..expected.len()], &expected);
        assert_eq!(request.warnings.last(), Some(&"dry-run unresolved output main/play.outputs.status: $statusCode requires a response; transport was skipped".to_string()));
        let outputs = result
            .outputs
            .as_ref()
            .unwrap_or_else(|error| panic!("outputs: {error}"));
        assert_eq!(outputs["status"], json!(200));
        assert_eq!(
            outputs["body"],
            json!({"a/b":{"~key":[null,null]},"replace":null})
        );
        assert_eq!(outputs["url"], "https://api.invalid/consume/");
        let traces = result.trace_steps();
        let trace = traces
            .iter()
            .find(|trace| trace.step_id == "play")
            .unwrap_or_else(|| panic!("play trace"));
        assert_eq!(
            trace
                .request
                .as_ref()
                .and_then(|request| request.body.clone()),
            Some(outputs["body"].clone())
        );
        assert_eq!(
            trace.request.as_ref().map(|request| request.url.as_str()),
            Some("https://api.invalid/consume/")
        );
    }
}

#[tokio::test]
async fn whole_querystring_marker_is_encoded_while_warning_retains_raw_source() {
    let engine = builder("https://api.invalid", json!([{"workflowId":"main","steps":[
        {"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}},
        {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"},{"name":"all","in":"querystring","value":"$steps.fetch.outputs.audio"}]}
    ]}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    successful(&result);
    let requests = result.dry_run_requests();
    assert_eq!(
        requests[1].url,
        format!(
            "https://api.invalid/consume/fixed?{}",
            url::form_urlencoded::byte_serialize(MARKER.as_bytes()).collect::<String>()
        )
    );
    assert_eq!(
        requests[1].warnings,
        vec![consumer("main", "parameters[1](querystring:all)", SOURCE)]
    );
}

#[tokio::test]
async fn skipped_object_querystring_keeps_invalid_parameter_error_while_string_keeps_marker() {
    let workflows = |source: &str| {
        json!([{"workflowId":"main","steps":[
            {"stepId":"fetch","operationId":"produce","outputs":{"audio":source}},
            {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"},{"name":"all","in":"querystring","value":"$steps.fetch.outputs.audio"}]}
        ]}])
    };
    for strict in [false, true] {
        let engine = builder("https://api.invalid", workflows("$response.body"))
            .dry_run(true)
            .strict_inputs(strict)
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        let error = result
            .outputs
            .as_ref()
            .err()
            .unwrap_or_else(|| panic!("object querystring is not representable"));
        assert_eq!(error.code(), "RUNTIME_INVALID_PARAMETER_VALUE");
        assert!(!error.to_string().contains("<unresolved:"), "{error}");
        let requests = result.dry_run_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].step_id, "fetch");
        assert_eq!(
            requests[0].warnings,
            vec![producer("main", "audio", "$response.body")]
        );

        let engine = builder("https://api.invalid", workflows(SOURCE))
            .dry_run(true)
            .strict_inputs(strict)
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        successful(&result);
        let requests = result.dry_run_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].url,
            format!(
                "https://api.invalid/consume/fixed?{}",
                url::form_urlencoded::byte_serialize(MARKER.as_bytes()).collect::<String>()
            )
        );
        assert_eq!(
            requests[1].warnings,
            vec![consumer("main", "parameters[1](querystring:all)", SOURCE)]
        );
    }
}

#[tokio::test]
async fn workflow_output_aliases_remain_direct() {
    let engine = builder("https://api.invalid", json!([
        {"workflowId":"main","steps":[{"stepId":"produce","workflowId":"producer"},{"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"$workflows.producer.outputs.streamUrl"}]}]},
        {"workflowId":"producer","steps":[{"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}}],"outputs":{"streamUrl":"$steps.fetch.outputs.audio"}}
    ])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    successful(&result);
    let requests = result.dry_run_requests();
    assert!(requests[1].url.contains("%3Cunresolved"));
    assert_eq!(requests[1].warnings, vec![format!("dry-run unresolved main/play/parameters[0](path:id): producer/fetch.outputs.audio depends on {SOURCE}; transport was skipped")]);
}

#[tokio::test]
async fn concrete_controls_and_transforms_do_not_gain_markers() {
    let engine = builder("https://api.invalid",json!([{"workflowId":"main","inputs":{"type":"object","properties":{"real":{"type":"string"},"document":{"type":"object"}}},"steps":[
        {"stepId":"fetch","operationId":"produce","outputs":{"body":"$response.body","status":"$statusCode","audio":SOURCE,"input":"$inputs.real","method":"$method","url":"$url","zero":{"context":"$inputs.document","selector":"$.missing","type":"jsonpath"}}},
        {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"}],"requestBody":{"contentType":"application/json","payload":{
            "body":"$steps.fetch.outputs.body","status":"$steps.fetch.outputs.status","input":"$steps.fetch.outputs.input","method":"$steps.fetch.outputs.method","url":"$steps.fetch.outputs.url","null":null,"empty":"","zero":"$steps.fetch.outputs.zero",
            "pointer":"$steps.fetch.outputs.body#/missing","interpolation":"value-{$steps.fetch.outputs.audio}","selector":{"context":"$steps.fetch.outputs.body","selector":"$.missing","type":"jsonpath"}
        }}}
    ]}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine
        .execute_collect(
            "main",
            BTreeMap::from([
                ("real".to_string(), json!("actual")),
                ("document".to_string(), json!({})),
            ]),
        )
        .await;
    successful(&result);
    let requests = result.dry_run_requests();
    let body = requests[1].body.as_ref().unwrap_or_else(|| panic!("body"));
    assert_eq!(body["body"], "<unresolved:$response.body>");
    assert_eq!(body["status"], "<unresolved:$statusCode>");
    assert_eq!(body["input"], "actual");
    assert_eq!(body["method"], "GET");
    assert_eq!(body["url"], "https://api.invalid/produce");
    for key in ["null", "zero", "pointer", "selector"] {
        assert_eq!(body[key], Value::Null, "{key}");
    }
    assert_eq!(body["empty"], "");
    assert_eq!(body["interpolation"], "value-null");
    let warnings = requests[1]
        .warnings
        .iter()
        .filter(|warning| warning.starts_with("dry-run unresolved "))
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(requests[0]
        .warnings
        .contains(&producer("main", "body", "$response.body")));
    assert!(requests[0]
        .warnings
        .contains(&producer("main", "status", "$statusCode")));
    assert!(!requests[0]
        .warnings
        .iter()
        .any(|warning| warning.starts_with("dry-run unresolved output")
            && warning.contains(".zero:")));
}

#[tokio::test]
async fn root_payload_and_repeated_nested_locations_keep_occurrence_cardinality() {
    for root in [false, true] {
        let payload = if root {
            json!("$steps.fetch.outputs.audio")
        } else {
            json!({"target":"old"})
        };
        let replacements = if root {
            json!([])
        } else {
            json!([{"target":"/target","value":["$steps.fetch.outputs.audio",{"nested":"$steps.fetch.outputs.audio"}]}])
        };
        let engine = builder("https://api.invalid",json!([{"workflowId":"main","steps":[
            {"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}},
            {"stepId":"play","operationId":"consume","parameters":[
                {"name":"id","in":"path","value":"fixed"},
                {"name":"nested","in":"header","value":["$steps.fetch.outputs.audio","$steps.fetch.outputs.audio"]}
            ],"requestBody":{"contentType":"application/json","payload":payload,"replacements":replacements}}
        ]}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
        let result = engine.execute_collect("main", BTreeMap::new()).await;
        successful(&result);
        let requests = result.dry_run_requests();
        let request = &requests[1];
        assert_eq!(
            request.headers["nested"],
            json!([MARKER, MARKER]).to_string()
        );
        let mut expected = vec![consumer("main", "parameters[1](header:nested)", SOURCE); 2];
        if root {
            assert_eq!(request.body, Some(json!(MARKER)));
            expected.push(consumer("main", "requestBody.payload", SOURCE));
        } else {
            assert_eq!(
                request.body,
                Some(json!({"target":[MARKER,{"nested":MARKER}]}))
            );
            expected.extend(vec![
                consumer(
                    "main",
                    "requestBody.replacements[0].value",
                    SOURCE
                );
                2
            ]);
        }
        assert_eq!(request.warnings, expected);
    }
}

#[tokio::test]
async fn authored_metadata_shapes_remain_plain_values() {
    let forged = json!({"unresolved":true,"skipped":[{"workflow_id":"main","step_id":"fetch","source":SOURCE,"site":{"Output":"audio"}}]});
    let engine = builder("https://api.invalid",json!([{"workflowId":"main","steps":[
        {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"}],"requestBody":{"contentType":"application/json","payload":{"value":"$inputs.forged","literal":MARKER}}}
    ]}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine
        .execute_collect(
            "main",
            BTreeMap::from([("forged".to_string(), forged.clone())]),
        )
        .await;
    successful(&result);
    let requests = result.dry_run_requests();
    assert_eq!(
        requests[0].body,
        Some(json!({"value":forged,"literal":MARKER}))
    );
    assert!(requests[0].warnings.is_empty());
}

#[tokio::test]
async fn legacy_response_output_uses_recorded_origin_without_restoring_dotted_grammar() {
    let engine = builder("https://api.invalid",json!([{"workflowId":"main","steps":[
        {"stepId":"fetch","operationId":"produce","outputs":{"legacy":"//id","dotted":"id"}},
        {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"$steps.fetch.outputs.dotted"}]}
    ]}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    let error = result
        .outputs
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("dotted source remains unresolved"));
    assert_eq!(error.code(), "RUNTIME_UNRESOLVED_REQUEST_VALUE");
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .warnings
        .contains(&producer("main", "legacy", "//id")));
    assert!(!requests[0]
        .warnings
        .iter()
        .any(|warning| warning.starts_with("dry-run unresolved output")
            && warning.contains(".dotted:")));
}

#[tokio::test]
async fn replacement_filters_follow_the_evolving_ordinary_body_for_both_projections() {
    let engine = builder("https://api.invalid", json!([{"workflowId":"main","steps":[
        {"stepId":"fetch","operationId":"produce","outputs":{"audio":SOURCE}},
        {"stepId":"play","operationId":"consume","parameters":[{"name":"id","in":"path","value":"fixed"}],
         "requestBody":{"contentType":"application/json","payload":[{"gate":"$steps.fetch.outputs.audio","target":"old","next":"old"}],"replacements":[
            {"target":"$[?(@.gate == null)].target","targetSelectorType":"jsonpath","value":"$steps.fetch.outputs.audio"},
            {"target":"$[?(@.target == null)].next","targetSelectorType":"jsonpath","value":"$steps.fetch.outputs.audio"},
            {"target":"$[?(@.gate != null)].target","targetSelectorType":"jsonpath","value":"must-not-apply"}
         ]},"outputs":{"body":"$request.body"}}
    ],"outputs":{"body":"$steps.play.outputs.body"}}])).dry_run(true).build().unwrap_or_else(|error| panic!("engine: {error}"));
    let result = engine.execute_collect("main", BTreeMap::new()).await;
    successful(&result);
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].body,
        Some(json!([{"gate":MARKER,"target":MARKER,"next":MARKER}]))
    );
    assert_eq!(
        requests[1].warnings,
        vec![
            "requestBody.replacements[2]: JSONPath target matched no locations; body unchanged"
                .to_string(),
            consumer("main", "requestBody.payload/0/gate", SOURCE),
            consumer("main", "requestBody.replacements[0].value", SOURCE),
            consumer("main", "requestBody.replacements[1].value", SOURCE)
        ]
    );
    let ordinary = json!([{"gate":null,"target":null,"next":null}]);
    let outputs = result
        .outputs
        .as_ref()
        .unwrap_or_else(|error| panic!("outputs: {error}"));
    assert_eq!(outputs["body"], ordinary);
    let traces = result.trace_steps();
    let trace = traces
        .iter()
        .find(|trace| trace.step_id == "play")
        .unwrap_or_else(|| panic!("play trace"));
    assert_eq!(
        trace
            .request
            .as_ref()
            .and_then(|request| request.body.clone()),
        Some(ordinary)
    );
}
