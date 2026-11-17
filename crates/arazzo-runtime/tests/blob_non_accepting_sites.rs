#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    Blob, DebugController, EngineBuilder, RuntimeError, StepBreakpoint, WorkflowInputs,
};
use arazzo_spec::{ArazzoSpec, OutputValue};
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const UNAVAILABLE: &str = "RUNTIME_VALUE_UNAVAILABLE";
const INVALID_BODY: &str = "RUNTIME_INVALID_REQUEST_BODY";
const WHOLE: &str =
    "input \"photo\" is bytes, which only a whole request body payload or a whole output can carry";
const POINTER: &str = "input \"photo\" is bytes, which a JSON Pointer cannot read";
const SENT_BODY: &str = "request body is bytes, which only a whole output can carry";
const SOURCE: &str = "https://fixture.invalid/openapi.json";
const JPEG: [u8; 6] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
const PNG: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];
const UPLOAD: &str = "POST /upload/7";

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    line: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

struct UploadServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl UploadServer {
    fn start() -> Self {
        let server =
            Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("binding server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                let content_type = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Content-Type"))
                    .map(|header| header.value.as_str().to_owned());
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen {
                        line: format!("{} {}", request.method(), request.url()),
                        content_type,
                        body,
                    });
                let (body, media_type) = if request.url() == "/photo" {
                    (PNG.to_vec(), "image/png")
                } else {
                    (br#"{"ok":true}"#.to_vec(), "application/json")
                };
                let header = Header::from_bytes("Content-Type", media_type)
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request.respond(TinyResponse::from_data(body).with_header(header));
            }
        });
        Self {
            base_url,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(
            &mut *self
                .requests
                .lock()
                .unwrap_or_else(|_| panic!("reading requests")),
        )
    }

    fn take_lines(&self) -> Vec<String> {
        self.take().into_iter().map(|seen| seen.line).collect()
    }
}

impl Drop for UploadServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap_or_else(|error| panic!("fixture: {error}"))
}

fn photo() -> WorkflowInputs {
    WorkflowInputs::new(
        BTreeMap::from([("caption".to_owned(), json!("cat"))]),
        BTreeMap::from([(
            "photo".to_owned(),
            Blob::new(JPEG.to_vec()).with_filename("cat.jpg"),
        )]),
    )
}

fn overlay(mut base: Value, fields: &Value) -> Value {
    for (name, value) in fields.as_object().into_iter().flatten() {
        base[name] = value.clone();
    }
    base
}

fn builder(base: &str, upload: &Value, main: &Value) -> EngineBuilder {
    let photo_input =
        json!({"type":"object", "properties":{"photo":{}, "caption":{"type":"string"}}});
    let step = overlay(
        json!({"stepId":"upload", "operationId":"upload",
            "parameters":[{"name":"id", "in":"path", "value":"7"}]}),
        upload,
    );
    let main = overlay(
        json!({"workflowId":"main", "inputs":photo_input,
            "steps":[step, {"stepId":"next", "operationId":"next"}]}),
        main,
    );
    let spec: ArazzoSpec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"bytes", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[
            main,
            {"workflowId":"child", "inputs":photo_input,
                "steps":[{"stepId":"only", "operationId":"next"}]}
        ]
    }));
    let operation = |id: &str| json!({"operationId":id, "responses":{"200":{"description":"ok"}}});
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":base}],
    "paths":{
        "/upload/{id}":{"post":operation("upload")},
        "/photo":{"get":operation("photo")},
        "/next":{"get":operation("next")}
    }});
    EngineBuilder::new(spec).openapi_spec(openapi.to_string().into_bytes(), None)
}

async fn run(
    server: &UploadServer,
    upload: &Value,
    main: &Value,
) -> Result<BTreeMap<String, Value>, RuntimeError> {
    builder(&server.base_url, upload, main)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", photo())
        .await
        .outputs
}

fn refused(site: &str, outcome: Result<BTreeMap<String, Value>, RuntimeError>) -> RuntimeError {
    match outcome {
        Ok(outputs) => panic!("{site}: bytes must fail the run, got {outputs:?}"),
        Err(error) => error,
    }
}

fn body(content_type: Option<&str>, payload: Value) -> Value {
    let mut request_body = json!({"payload":payload});
    if let Some(content_type) = content_type {
        request_body["contentType"] = json!(content_type);
    }
    json!({"requestBody":request_body})
}

fn selector(pointer: &str) -> Value {
    json!({"context":"$inputs.photo", "selector":pointer, "type":"jsonpointer"})
}

fn typed(kind: &str, context: &str, condition: &str) -> Value {
    let mut criterion = json!({"condition":condition, "type":kind, "context":context});
    if kind == "xpath" {
        criterion["type"] = json!({"type":"xpath", "version":"xpath-10"});
    }
    criterion
}

#[tokio::test]
async fn the_whole_blob_is_the_only_form_that_reaches_the_wire() {
    let server = UploadServer::start();
    let outputs = run(
        &server,
        &body(Some("image/jpeg"), json!("$inputs.photo")),
        &json!({}),
    )
    .await
    .unwrap_or_else(|error| panic!("upload: {error}"));
    assert!(outputs.is_empty());
    let seen = server.take();
    assert_eq!(seen[0].line, UPLOAD);
    assert_eq!(seen[0].content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(seen[0].body, JPEG);
    assert_eq!(seen[1].line, "GET /next");
}

#[tokio::test]
async fn every_request_side_site_refuses_bytes_and_sends_nothing() {
    let server = UploadServer::start();
    let path = json!({"name":"id", "in":"path", "value":"7"});
    let parameter = |parameter: Value| json!({"parameters":[path, parameter]});
    let jpeg = |payload: Value| body(Some("image/jpeg"), payload);
    let cases = [
        (
            "pointer into the blob",
            jpeg(json!("$inputs.photo#/exif")),
            "requestBody.payload: $inputs.photo#/exif",
            POINTER,
        ),
        (
            "whole-document pointer",
            jpeg(json!("$inputs.photo#")),
            "requestBody.payload: $inputs.photo#",
            POINTER,
        ),
        (
            "interpolation",
            jpeg(json!("photo={$inputs.photo}")),
            "requestBody.payload: $inputs.photo",
            WHOLE,
        ),
        (
            "interpolation of the bare expression",
            jpeg(json!("{$inputs.photo}")),
            "requestBody.payload: $inputs.photo",
            WHOLE,
        ),
        (
            "Selector Object context as the payload",
            jpeg(selector("/exif")),
            "requestBody.payload: $inputs.photo",
            WHOLE,
        ),
        (
            "pointer beside a whole reference",
            body(
                Some("multipart/form-data"),
                json!({"photo":"$inputs.photo", "exif":"$inputs.photo#/exif"}),
            ),
            "requestBody.payload/exif: $inputs.photo#/exif",
            POINTER,
        ),
        (
            "Selector Object context beside a whole reference",
            body(
                Some("multipart/form-data"),
                json!({"photo":"$inputs.photo", "tags":[selector("/tags")]}),
            ),
            "requestBody.payload/tags/0: $inputs.photo",
            WHOLE,
        ),
        (
            "path parameter",
            json!({"parameters":[{"name":"id", "in":"path", "value":"$inputs.photo"}]}),
            "parameters[0](path:id): $inputs.photo",
            WHOLE,
        ),
        (
            "query parameter",
            parameter(json!({"name":"q", "in":"query", "value":"$inputs.photo"})),
            "parameters[1](query:q): $inputs.photo",
            WHOLE,
        ),
        (
            "querystring parameter",
            parameter(json!({"name":"all", "in":"querystring", "value":"$inputs.photo"})),
            "parameters[1](querystring:all): $inputs.photo",
            WHOLE,
        ),
        (
            "header parameter",
            parameter(json!({"name":"X-Photo", "in":"header", "value":"$inputs.photo"})),
            "parameters[1](header:X-Photo): $inputs.photo",
            WHOLE,
        ),
        (
            "cookie parameter",
            parameter(json!({"name":"session", "in":"cookie", "value":"$inputs.photo"})),
            "parameters[1](cookie:session): $inputs.photo",
            WHOLE,
        ),
        (
            "parameter with no location",
            parameter(json!({"name":"unplaced", "value":"$inputs.photo"})),
            "parameters[1](unspecified:unplaced): $inputs.photo",
            WHOLE,
        ),
        (
            "interpolated header parameter",
            parameter(json!({"name":"X-Photo", "in":"header", "value":"id-{$inputs.photo}"})),
            "parameters[1](header:X-Photo): $inputs.photo",
            WHOLE,
        ),
        (
            "Selector Object context as a parameter",
            parameter(json!({"name":"q", "in":"query", "value":selector("/exif")})),
            "parameters[1](query:q): $inputs.photo",
            WHOLE,
        ),
        (
            "replacement value",
            json!({"requestBody":{"contentType":"application/json", "payload":{"photo":null},
                "replacements":[{"target":"/photo", "value":"$inputs.photo"}]}}),
            "requestBody.replacements[0].value: $inputs.photo",
            WHOLE,
        ),
        (
            "replacement value through a Selector Object",
            json!({"requestBody":{"contentType":"application/json", "payload":{"photo":null},
                "replacements":[{"target":"/photo", "value":selector("/exif")}]}}),
            "requestBody.replacements[0].value: $inputs.photo",
            WHOLE,
        ),
    ];
    for (site, upload, location, reason) in cases {
        let error = refused(site, run(&server, &upload, &json!({})).await);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(
            error.message,
            format!("step upload: step \"upload\": {location}: {reason}; request not sent"),
            "{site}"
        );
        assert_eq!(server.take_lines(), Vec::<String>::new(), "{site}");
    }

    for (site, upload, pointer) in [
        (
            "replacement target into a whole blob",
            json!({"requestBody":{"contentType":"image/jpeg", "payload":"$inputs.photo",
                "replacements":[{"target":"/exif", "value":"none"}]}}),
            "",
        ),
        (
            "replacement target beside a blob leaf",
            json!({"requestBody":{"contentType":"multipart/form-data",
                "payload":{"caption":"cat", "photo":"$inputs.photo"},
                "replacements":[{"target":"/caption", "value":"dog"}]}}),
            "/photo",
        ),
    ] {
        let error = refused(site, run(&server, &upload, &json!({})).await);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(
            error.message,
            format!("step upload: step \"upload\": requestBody.replacements[0].target: requestBody.payload{pointer} is bytes, which a replacement target cannot read; request not sent"),
            "{site}"
        );
        assert_eq!(server.take_lines(), Vec::<String>::new(), "{site}");
    }
}

#[tokio::test]
async fn bytes_under_a_content_type_that_cannot_carry_them_are_an_invalid_body() {
    let server = UploadServer::start();
    let no_text_form = |content_type: &str, pointer: &str| {
        format!("step upload: step \"upload\": requestBody.payload{pointer}: contentType {content_type:?} cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded; request not sent")
    };
    let nested = json!({"caption":"$inputs.caption", "photo":"$inputs.photo"});
    let cases = [
        (
            "omitted contentType",
            body(None, json!("$inputs.photo")),
            "step upload: step \"upload\": requestBody.payload: the payload carries bytes, so an explicit contentType is required; request not sent".to_owned(),
        ),
        (
            "omitted contentType with a blob leaf",
            body(None, nested.clone()),
            "step upload: step \"upload\": requestBody.payload/photo: the payload carries bytes, so an explicit contentType is required; request not sent".to_owned(),
        ),
        (
            "JSON",
            body(Some("application/json"), json!("$inputs.photo")),
            no_text_form("application/json", ""),
        ),
        (
            "structured-syntax JSON",
            body(Some("application/vnd.api+json"), json!("$inputs.photo")),
            no_text_form("application/vnd.api+json", ""),
        ),
        (
            "JSON with a blob leaf",
            body(Some("application/json"), nested.clone()),
            no_text_form("application/json", "/photo"),
        ),
        (
            "form",
            body(Some("application/x-www-form-urlencoded"), json!("$inputs.photo")),
            no_text_form("application/x-www-form-urlencoded", ""),
        ),
        (
            "form with a blob leaf",
            body(Some("application/x-www-form-urlencoded"), nested.clone()),
            no_text_form("application/x-www-form-urlencoded", "/photo"),
        ),
        (
            "blob leaf under a type with no encoder for it yet",
            body(Some("multipart/form-data"), nested.clone()),
            "step upload: step \"upload\": requestBody.payload/photo: bytes inside an object or array cannot be sent under contentType \"multipart/form-data\"; only a payload that is the bytes value itself can be; request not sent".to_owned(),
        ),
        (
            "blob item in an array",
            body(Some("application/octet-stream"), json!(["$inputs.photo"])),
            "step upload: step \"upload\": requestBody.payload/0: bytes inside an object or array cannot be sent under contentType \"application/octet-stream\"; only a payload that is the bytes value itself can be; request not sent".to_owned(),
        ),
    ];
    for (site, upload, message) in cases {
        let error = refused(site, run(&server, &upload, &json!({})).await);
        assert_eq!(error.code(), INVALID_BODY, "{site}: {error}");
        assert_eq!(error.message, message, "{site}");
        assert_eq!(server.take_lines(), Vec::<String>::new(), "{site}");
    }
}

#[tokio::test]
async fn every_site_after_the_response_fails_the_step_instead_of_reading_null() {
    let server = UploadServer::start();
    let criteria = |criterion: Value| json!({"successCriteria":[criterion]});
    let failing = json!({"condition":"$statusCode == 500"});
    let cases = [
        (
            "simple operand",
            criteria(json!({"condition":"$inputs.photo == 'cat'"})),
            "step upload: successCriteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "simple pointer operand after a passing criterion",
            json!({"successCriteria":[
                {"condition":"$statusCode == 200"},
                {"condition":"$inputs.photo#/exif != null"}
            ]}),
            "step upload: successCriteria[1]: $inputs.photo#/exif",
            POINTER,
        ),
        (
            "explicit regex context",
            criteria(typed("regex", "$inputs.photo", "^cat")),
            "step upload: successCriteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "explicit jsonpath context",
            criteria(typed("jsonpath", "$inputs.photo#/tags", "$[?@.id]")),
            "step upload: successCriteria[0]: $inputs.photo#/tags",
            POINTER,
        ),
        (
            "explicit xpath context",
            criteria(typed("xpath", "$inputs.photo", "//pet")),
            "step upload: successCriteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "interpolation in a typed condition",
            criteria(typed(
                "regex",
                "$response.header.Content-Type",
                "^{$inputs.photo}$",
            )),
            "step upload: successCriteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "step output through a pointer",
            json!({"outputs":{"exif":"$inputs.photo#/exif"}}),
            "step upload: output \"exif\": $inputs.photo#/exif",
            POINTER,
        ),
        (
            "Selector Object context as a step output",
            json!({"outputs":{"exif":selector("/exif")}}),
            "step upload: output \"exif\": $inputs.photo",
            WHOLE,
        ),
        (
            "onSuccess action criterion",
            json!({"onSuccess":[{"name":"stop", "type":"end",
                "criteria":[{"condition":"$inputs.photo == 1"}]}]}),
            "step upload: onSuccess[0].criteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "onFailure action criterion",
            json!({"successCriteria":[failing], "onFailure":[{"name":"again", "type":"retry",
                "retryAfter":0, "criteria":[typed("regex", "$inputs.photo", "busy")]}]}),
            "step upload: onFailure[0].criteria[0]: $inputs.photo",
            WHOLE,
        ),
        (
            "interpolated goto step",
            json!({"onSuccess":[{"name":"jump", "type":"goto", "stepId":"{$inputs.photo}"}]}),
            "step upload: action stepId: $inputs.photo",
            WHOLE,
        ),
        (
            "action parameter",
            json!({"onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"child",
                "parameters":[{"name":"photo", "value":"$inputs.photo"}]}]}),
            "step upload: action parameter \"photo\": $inputs.photo",
            WHOLE,
        ),
        (
            "retry reference parameter",
            json!({"successCriteria":[failing], "onFailure":[{"name":"recover", "type":"retry",
                "retryAfter":0, "workflowId":"child",
                "parameters":[{"name":"photo", "value":"$inputs.photo"}]}]}),
            "step upload: action parameter \"photo\": $inputs.photo",
            WHOLE,
        ),
    ];
    for (site, upload, location, reason) in cases {
        let error = refused(site, run(&server, &upload, &json!({})).await);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(error.message, format!("{location}: {reason}"), "{site}");
        assert_eq!(server.take_lines(), [UPLOAD], "{site}");
    }
}

#[tokio::test]
async fn the_default_criterion_context_over_a_bytes_response_fails_the_step() {
    let server = UploadServer::start();
    let fetch = json!({"operationId":"photo", "parameters":[],
        "successCriteria":[{"condition":"^cat", "type":"regex"}]});
    let error = refused("default context", run(&server, &fetch, &json!({})).await);
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        "step upload: successCriteria[0]: $response.body: response body is bytes (image/png), which is not yet a readable value"
    );
    assert_eq!(server.take_lines(), ["GET /photo"]);
}

#[tokio::test]
async fn request_body_after_a_whole_blob_upload_errors_everywhere_but_a_whole_output() {
    let server = UploadServer::start();
    let sent = |fields: Value| overlay(body(Some("image/jpeg"), json!("$inputs.photo")), &fields);
    let failing = json!({"condition":"$statusCode == 500"});
    let cases = [
        (
            "step output through a pointer",
            sent(json!({"outputs":{"exif":"$request.body#/exif"}})),
            "step upload: output \"exif\": $request.body#/exif",
        ),
        (
            "Selector Object context",
            sent(
                json!({"outputs":{"exif":{"context":"$request.body", "selector":"/exif",
                "type":"jsonpointer"}}}),
            ),
            "step upload: output \"exif\": $request.body",
        ),
        (
            "simple operand",
            sent(json!({"successCriteria":[{"condition":"$request.body == null"}]})),
            "step upload: successCriteria[0]: $request.body",
        ),
        (
            "explicit regex context",
            sent(json!({"successCriteria":[typed("regex", "$request.body", "^.")]})),
            "step upload: successCriteria[0]: $request.body",
        ),
        (
            "simple pointer operand after a passing criterion",
            sent(json!({"successCriteria":[
                {"condition":"$statusCode == 200"},
                {"condition":"$request.body#/a == 1"}
            ]})),
            "step upload: successCriteria[1]: $request.body#/a",
        ),
        (
            "onSuccess action criterion",
            sent(json!({"onSuccess":[{"name":"stop", "type":"end",
                "criteria":[{"condition":"$request.body == null"}]}]})),
            "step upload: onSuccess[0].criteria[0]: $request.body",
        ),
        (
            "onFailure action criterion",
            sent(
                json!({"successCriteria":[failing], "onFailure":[{"name":"again",
                "type":"retry", "retryAfter":0,
                "criteria":[typed("regex", "$request.body", "busy")]}]}),
            ),
            "step upload: onFailure[0].criteria[0]: $request.body",
        ),
        (
            "onFailure action criterion after a failed output",
            sent(
                json!({"outputs":{"sent":"$request.body#/exif"}, "onFailure":[{"name":"stop",
                "type":"end", "criteria":[{"condition":"$request.body#/a == null"}]}]}),
            ),
            "step upload: onFailure[0].criteria[0]: $request.body#/a",
        ),
        (
            "interpolated goto step",
            sent(json!({"onSuccess":[{"name":"jump", "type":"goto",
                "stepId":"{$request.body}"}]})),
            "step upload: action stepId: $request.body",
        ),
        (
            "interpolated goto workflow",
            sent(json!({"onSuccess":[{"name":"jump", "type":"goto",
                "workflowId":"{$request.body}"}]})),
            "step upload: action workflowId: $request.body",
        ),
        (
            "action parameter",
            sent(
                json!({"onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"child",
                "parameters":[{"name":"caption", "value":"$request.body"}]}]}),
            ),
            "step upload: action parameter \"caption\": $request.body",
        ),
        (
            "interpolated retry reference step",
            sent(
                json!({"successCriteria":[failing], "onFailure":[{"name":"recover",
                "type":"retry", "retryAfter":0, "stepId":"{$request.body}"}]}),
            ),
            "step upload: action stepId: $request.body",
        ),
        (
            "retry reference parameter",
            sent(
                json!({"successCriteria":[failing], "onFailure":[{"name":"recover",
                "type":"retry", "retryAfter":0, "workflowId":"child",
                "parameters":[{"name":"caption", "value":"$request.body"}]}]}),
            ),
            "step upload: action parameter \"caption\": $request.body",
        ),
    ];
    for (site, upload, location) in cases {
        let error = refused(site, run(&server, &upload, &json!({})).await);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(error.message, format!("{location}: {SENT_BODY}"), "{site}");
        let seen = server.take();
        assert_eq!(seen.len(), 1, "{site}");
        assert_eq!(seen[0].line, UPLOAD, "{site}");
        assert_eq!(seen[0].body, JPEG, "{site}");
    }

    let passing = sent(
        json!({"successCriteria":[{"condition":"$statusCode == 200"}],
        "outputs":{"method":"$method"}}),
    );
    assert!(run(&server, &passing, &json!({})).await.is_ok());
}

#[tokio::test]
async fn workflow_output_pointers_and_sub_workflow_parameters_refuse_bytes() {
    let server = UploadServer::start();
    for (output, expression, reason) in [
        (json!("$inputs.photo#/exif"), "$inputs.photo#/exif", POINTER),
        (json!("photo {$inputs.photo}"), "$inputs.photo", WHOLE),
        (selector("/exif"), "$inputs.photo", WHOLE),
    ] {
        let main = json!({"outputs":{"before":"literal", "photo":output}});
        let error = refused("workflow output", run(&server, &json!({}), &main).await);
        assert_eq!(error.code(), UNAVAILABLE, "{error}");
        assert_eq!(
            error.message,
            format!("workflow \"main\": output \"photo\": {expression}: {reason}")
        );
        assert_eq!(server.take_lines(), [UPLOAD, "GET /next"]);
    }

    let call = json!({"steps":[{"stepId":"call", "workflowId":"child",
        "parameters":[{"name":"photo", "value":"$inputs.photo"}]}]});
    let error = refused(
        "workflowId step parameter",
        run(&server, &json!({}), &call).await,
    );
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        format!("step call: parameters[0].value: $inputs.photo: {WHOLE}")
    );
    assert_eq!(server.take_lines(), Vec::<String>::new());
}

#[tokio::test]
async fn bytes_inherited_by_a_retry_reference_workflow_never_read_as_null() {
    let server = UploadServer::start();
    let main = json!({"steps":[
        {"stepId":"upload", "operationId":"upload",
            "parameters":[{"name":"id", "in":"path", "value":"7"}],
            "successCriteria":[{"condition":"$workflows.child.inputs.caption == 'cat'"}],
            "onFailure":[{"name":"recover", "type":"retry", "retryAfter":0, "retryLimit":1,
                "workflowId":"child"}]},
        {"stepId":"next", "operationId":"next",
            "parameters":[{"name":"X-Photo", "in":"header",
                "value":"$workflows.child.inputs.photo"}]}
    ]});
    let error = refused("inherited inputs", run(&server, &json!({}), &main).await);
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        "step next: step \"next\": parameters[0](header:X-Photo): $workflows.child.inputs.photo: workflow \"child\" input \"photo\" is bytes, which a $workflows reference cannot carry yet; request not sent"
    );
    assert_eq!(server.take_lines(), [UPLOAD, "GET /next", UPLOAD]);
}

#[tokio::test]
async fn a_goto_target_inherits_the_bytes_and_uploads_them_unchanged() {
    let server = UploadServer::start();
    let main = json!({"steps":[{"stepId":"first", "operationId":"next",
        "onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"sender"}]}]});
    let spec_main = overlay(json!({"workflowId":"main", "steps":[]}), &main);
    let spec: ArazzoSpec = parse(json!({
        "arazzo":"1.1.0", "info":{"title":"bytes", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[
            spec_main,
            {"workflowId":"sender", "steps":[{"stepId":"upload", "operationId":"upload",
                "parameters":[{"name":"id", "in":"path", "value":"7"}],
                "requestBody":{"contentType":"application/octet-stream",
                    "payload":"$inputs.photo"}}]}
        ]
    }));
    let operation = |id: &str| json!({"operationId":id, "responses":{"200":{"description":"ok"}}});
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
        "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server.base_url}],
        "paths":{"/upload/{id}":{"post":operation("upload")}, "/next":{"get":operation("next")}}});
    let outputs = EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", photo())
        .await
        .outputs;
    assert!(outputs.is_ok(), "{outputs:?}");
    let seen = server.take();
    assert_eq!(seen[1].line, UPLOAD);
    assert_eq!(
        seen[1].content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(seen[1].body, JPEG);
}

#[tokio::test]
async fn a_receive_step_refuses_bytes_as_correlation_id_and_carries_them_as_an_output() {
    let dir = std::env::temp_dir().join(format!("arazzo-bytes-receive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("fixture dir: {error}"));
    std::fs::write(
        dir.join("orders.asyncapi.yaml"),
        "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  local:\n    host: 127.0.0.1:9\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: orders\n    messages:\n      orderPlaced:\n        contentType: application/json\n        correlationId:\n          location: '$message.header#/id'\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n  sendOrderPlaced:\n    action: send\n    channel:\n      $ref: '#/channels/orderPlaced'\n",
    )
    .unwrap_or_else(|error| panic!("fixture asyncapi: {error}"));
    let run = |step: Value| {
        let spec: ArazzoSpec = parse(json!({
            "arazzo":"1.1.0", "info":{"title":"bytes", "version":"1"},
            "sourceDescriptions":[{"name":"orders", "url":"./orders.asyncapi.yaml",
                "type":"asyncapi"}],
            "workflows":[{"workflowId":"main", "steps":[step]}]
        }));
        let engine = EngineBuilder::new(spec)
            .source_base_dir(&dir)
            .dry_run(true)
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        async move { engine.execute_collect("main", photo()).await.outputs }
    };
    let receive = |fields: Value| {
        overlay(
            json!({"stepId":"receive", "action":"receive",
                "operationId":"$sourceDescriptions.orders.receiveOrderPlaced"}),
            &fields,
        )
    };

    let correlation = run(receive(json!({"correlationId":"order-{$inputs.photo}"}))).await;
    let whole = run(receive(json!({"correlationId":"$inputs.photo"}))).await;
    let output = run(receive(json!({"outputs":{"photo":"$inputs.photo"}}))).await;
    let send = run(json!({"stepId":"send", "action":"send",
        "operationId":"$sourceDescriptions.orders.sendOrderPlaced",
        "requestBody":{"contentType":"application/octet-stream", "payload":"$inputs.photo"}}))
    .await;
    let _ = std::fs::remove_dir_all(&dir);

    for (site, outcome, location) in [
        (
            "interpolated correlationId",
            correlation,
            "step \"receive\": correlationId",
        ),
        ("correlationId", whole, "step \"receive\": correlationId"),
    ] {
        let error = refused(site, outcome);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert!(
            error
                .message
                .ends_with(&format!("{location}: $inputs.photo: {WHOLE}")),
            "{site}: {error}"
        );
        assert!(error.message.contains("receive"), "{site}: {error}");
    }
    assert!(output.is_ok(), "message step output: {output:?}");
    let error = refused("message payload of a send step", send);
    assert_eq!(
        error.code(),
        "RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT",
        "{error}"
    );
    assert!(error.message.contains("\"send\""), "{error}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debugger_requests_report_bytes_and_scopes_list_the_slot_with_a_marker() {
    let server = UploadServer::start();
    let controller = Arc::new(DebugController::new());
    let upload = overlay(
        body(Some("image/jpeg"), json!("$inputs.photo")),
        &json!({"successCriteria":[{"condition":"$statusCode == 200"}]}),
    );
    let engine = builder(&server.base_url, &upload, &json!({}))
        .debug_controller(Arc::clone(&controller))
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"));
    controller
        .set_breakpoints(vec![
            StepBreakpoint::new("main", "upload").at_success_criterion(0)
        ])
        .unwrap_or_else(|error| panic!("breakpoint: {error}"));
    let handle = engine.execute("main", photo());
    let stopped = controller
        .wait_for_stop_count(1, Duration::from_secs(5))
        .unwrap_or_else(|error| panic!("waiting for stop: {error}"));

    let unavailable = |expression: &str, reason: &str| {
        format!("{UNAVAILABLE}: step upload: {expression}: {reason}")
    };
    let observed = [
        (
            "evaluate",
            controller.evaluate_expression("$inputs.photo").map(|_| ()),
            unavailable("$inputs.photo", WHOLE),
        ),
        (
            "watch",
            controller
                .evaluate_watch_expression("$inputs.photo#/exif")
                .map(|_| ()),
            unavailable("$inputs.photo#/exif", POINTER),
        ),
        (
            "watch list",
            controller
                .evaluate_watches(&["$statusCode".to_owned(), "$inputs.photo".to_owned()])
                .map(|_| ()),
            unavailable("$inputs.photo", WHOLE),
        ),
        (
            "condition",
            controller
                .evaluate_condition("$inputs.photo == 1")
                .map(|_| ()),
            unavailable("$inputs.photo", WHOLE),
        ),
        (
            "condition expression",
            controller
                .try_evaluate_condition_expression("$inputs.photo#/exif == 1")
                .map(|_| ()),
            unavailable("$inputs.photo#/exif", POINTER),
        ),
        (
            "output preview",
            controller
                .evaluate_output_value(&OutputValue::RuntimeExpression("$inputs.photo".to_owned()))
                .map(|_| ()),
            unavailable("$inputs.photo", WHOLE),
        ),
        (
            "Selector Object output preview",
            controller
                .evaluate_output_value(&parse(selector("/exif")))
                .map(|_| ()),
            unavailable("$inputs.photo", WHOLE),
        ),
    ];
    let caption = controller.evaluate_expression("$inputs.caption");
    let scopes = controller
        .current_scopes()
        .unwrap_or_else(|error| panic!("scopes: {error}"));
    let _ = controller.continue_execution();
    let result = handle.collect().await;

    assert!(stopped, "the criterion breakpoint must stop the run");
    for (request, outcome, expected) in observed {
        assert_eq!(outcome, Err(expected), "{request}");
    }
    assert_eq!(caption, Ok(json!("cat")));
    let marker = json!("bytes (not viewable until descriptor support)");
    assert_eq!(scopes.inputs.get("photo"), Some(&marker));
    assert_eq!(scopes.inputs.get("caption"), Some(&json!("cat")));
    assert_eq!(scopes.locals.get("requestBody"), Some(&marker));
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(server.take()[0].body, JPEG);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debugger_requests_at_every_stop_after_a_whole_blob_upload_report_the_sent_bytes() {
    let passing = json!({"successCriteria":[{"condition":"$statusCode == 200"}],
        "onSuccess":[{"name":"stop", "type":"end",
            "criteria":[{"condition":"$statusCode == 200"}]}]});
    let retried = json!({"successCriteria":[{"condition":"$statusCode == 500"}],
        "onFailure":[{"name":"again", "type":"retry", "retryAfter":0, "retryLimit":1,
            "criteria":[{"condition":"$statusCode == 200"}]}]});
    let mut delayed = retried.clone();
    delayed["onFailure"][0]["retryAfter"] = json!(0.05);
    let at = || StepBreakpoint::new("main", "upload");
    let stops = [
        ("success criterion", at().at_success_criterion(0), &passing),
        ("onSuccess action", at().at_on_success_action(0), &passing),
        (
            "onSuccess action criterion",
            at().at_on_success_criterion(0, 0),
            &passing,
        ),
        ("onFailure action", at().at_on_failure_action(0), &retried),
        (
            "onFailure action criterion",
            at().at_on_failure_criterion(0, 0),
            &retried,
        ),
        (
            "retry selected",
            at().at_on_failure_retry_selected(0),
            &retried,
        ),
        ("retry delay", at().at_on_failure_retry_delay(0), &delayed),
    ];
    for (stop, breakpoint, fields) in stops {
        let server = UploadServer::start();
        let controller = Arc::new(DebugController::new());
        let upload = overlay(body(Some("image/jpeg"), json!("$inputs.photo")), fields);
        let engine = builder(&server.base_url, &upload, &json!({}))
            .debug_controller(Arc::clone(&controller))
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        controller
            .set_breakpoints(vec![breakpoint])
            .unwrap_or_else(|error| panic!("breakpoint: {error}"));
        let handle = engine.execute("main", photo());
        let stopped = controller
            .wait_for_stop_count(1, Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("waiting for stop: {error}"));

        let observed = [
            (
                "$request.body",
                controller.evaluate_expression("$request.body").map(|_| ()),
            ),
            (
                "$request.body#/a",
                controller
                    .evaluate_watch_expression("$request.body#/a")
                    .map(|_| ()),
            ),
            (
                "$request.body",
                controller
                    .evaluate_condition("$request.body == null")
                    .map(|_| ()),
            ),
        ];
        let _ = controller.set_breakpoints(Vec::new());
        let _ = controller.continue_execution();
        let _ = handle.collect().await;

        assert!(stopped, "{stop}: the breakpoint must stop the run");
        for (expression, outcome) in observed {
            assert_eq!(
                outcome,
                Err(format!(
                    "{UNAVAILABLE}: step upload: {expression}: {SENT_BODY}"
                )),
                "{stop}"
            );
        }
        assert_eq!(server.take()[0].body, JPEG, "{stop}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breakpoint_condition_over_the_sent_bytes_ends_the_run() {
    let at = || StepBreakpoint::new("main", "upload");
    for (stop, breakpoint) in [
        ("success criterion", at().at_success_criterion(0)),
        ("onSuccess action", at().at_on_success_action(0)),
    ] {
        let server = UploadServer::start();
        let controller = Arc::new(DebugController::new());
        let upload = overlay(
            body(Some("image/jpeg"), json!("$inputs.photo")),
            &json!({"successCriteria":[{"condition":"$statusCode == 200"}],
                "onSuccess":[{"name":"stop", "type":"end"}]}),
        );
        let engine = builder(&server.base_url, &upload, &json!({}))
            .debug_controller(Arc::clone(&controller))
            .build()
            .unwrap_or_else(|error| panic!("engine: {error}"));
        controller
            .set_breakpoints(vec![breakpoint.with_condition("$request.body == null")])
            .unwrap_or_else(|error| panic!("breakpoint: {error}"));

        let error = refused(stop, engine.execute_collect("main", photo()).await.outputs);

        assert_eq!(error.code(), "RUNTIME_DEBUG_CONTROLLER", "{stop}: {error}");
        assert_eq!(
            error.message,
            format!(
                "debug controller: {UNAVAILABLE}: breakpoint condition: \"$request.body == null\" in workflow: \"main\", step: \"upload\": $request.body: {SENT_BODY}"
            ),
            "{stop}"
        );
        assert_eq!(server.take_lines(), [UPLOAD], "{stop}");
        assert!(
            controller
                .stop_events()
                .unwrap_or_else(|error| panic!("stop events: {error}"))
                .is_empty(),
            "{stop}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_breakpoint_condition_over_bytes_ends_the_run() {
    let server = UploadServer::start();
    let controller = Arc::new(DebugController::new());
    let engine = builder(
        &server.base_url,
        &json!({"successCriteria":[{"condition":"$statusCode == 200"}]}),
        &json!({}),
    )
    .debug_controller(Arc::clone(&controller))
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"));
    controller
        .set_breakpoints(vec![StepBreakpoint::new("main", "upload")
            .at_success_criterion(0)
            .with_condition("$inputs.photo == 1")])
        .unwrap_or_else(|error| panic!("breakpoint: {error}"));

    let error = refused(
        "breakpoint condition",
        engine.execute_collect("main", photo()).await.outputs,
    );

    assert_eq!(error.code(), "RUNTIME_DEBUG_CONTROLLER", "{error}");
    assert_eq!(
        error.message,
        format!(
            "debug controller: {UNAVAILABLE}: breakpoint condition: \"$inputs.photo == 1\" in workflow: \"main\", step: \"upload\": $inputs.photo: {WHOLE}"
        )
    );
    assert_eq!(server.take_lines(), [UPLOAD]);
    assert!(controller
        .stop_events()
        .unwrap_or_else(|error| panic!("stop events: {error}"))
        .is_empty());
}

#[tokio::test]
async fn replaying_a_trace_whose_request_recorded_bytes_is_refused() {
    let server = UploadServer::start();
    let recorded = builder(
        &server.base_url,
        &body(Some("image/jpeg"), json!("$inputs.photo")),
        &json!({}),
    )
    .trace(true)
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"))
    .execute_collect("main", photo())
    .await;
    assert!(recorded.outputs.is_ok(), "{:?}", recorded.outputs);
    let steps: Vec<_> = recorded.trace_steps().into_iter().cloned().collect();
    let request = steps[0]
        .request
        .as_ref()
        .unwrap_or_else(|| panic!("the upload records its request"));
    assert_eq!(request.body, None);
    assert_eq!(
        json!(request.body_blobs),
        json!({"": {"kind":"bytes", "size":6, "filename":"cat.jpg",
            "sha256":"fc16d7dcee9cae83ef3923222a81ccd8fe96c9d25fdb7f504d66f1011e0cd870"}})
    );
    let serialized = serde_json::to_string(&steps).unwrap_or_else(|error| panic!("{error}"));
    assert!(serialized.contains("\"bodyBlobs\":{\"\":{\"kind\":\"bytes\",\"size\":6,"));
    assert!(!serialized.contains("/9j/4AAQ"), "{serialized}");

    let refused = builder(&server.base_url, &json!({}), &json!({}))
        .replay_trace_steps(steps)
        .build();
    let error = match refused {
        Ok(_) => panic!("a trace that recorded bytes must not replay"),
        Err(error) => error,
    };
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        "replay trace records bytes in the request body of workflow \"main\" step \"upload\" (seq 1); replay cannot restore recorded bytes yet"
    );
}
