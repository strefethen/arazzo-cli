#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{Blob, EngineBuilder, ExecutionResult, WorkflowInputs};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const INVALID_BODY: &str = "RUNTIME_INVALID_REQUEST_BODY";
const JPEG: [u8; 6] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
const TEMPLATE: &str = "{\n  \"id\": \"{$inputs.s}\",\n  \"note\": \"literal\"\n}\n";
const TEMPLATE_SENT: &str = "{\n  \"id\": \"abc\",\n  \"note\": \"literal\"\n}\n";

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    content_types: Vec<String>,
    body: Vec<u8>,
}

struct WireServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl WireServer {
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
                let content_types = request
                    .headers()
                    .iter()
                    .filter(|header| header.field.equiv("Content-Type"))
                    .map(|header| header.value.as_str().to_owned())
                    .collect();
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen {
                        content_types,
                        body,
                    });
                let header = Header::from_bytes("Content-Type", "application/json")
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ =
                    request.respond(TinyResponse::from_data(b"{}".to_vec()).with_header(header));
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
}

impl Drop for WireServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn inputs() -> WorkflowInputs {
    WorkflowInputs::new(
        BTreeMap::from([
            ("s".to_owned(), json!("abc")),
            ("raw".to_owned(), json!("{\"x\":1}")),
            ("obj".to_owned(), json!({"name":"Ann"})),
            ("nil".to_owned(), Value::Null),
        ]),
        BTreeMap::from([(
            "photo".to_owned(),
            Blob::new(JPEG.to_vec()).with_media_type("image/png"),
        )]),
    )
}

fn step(request_body: Option<Value>, parameters: Value) -> Value {
    let mut step = json!({"stepId":"send", "operationId":"send", "parameters":parameters});
    if let Some(request_body) = request_body {
        step["requestBody"] = request_body;
    }
    step
}

fn body(content_type: Option<&str>, payload: Value) -> Value {
    step(
        Some(match content_type {
            Some(content_type) => json!({"contentType":content_type, "payload":payload}),
            None => json!({"payload":payload}),
        }),
        json!([]),
    )
}

async fn run(server: &WireServer, step: &Value, dry_run: bool) -> ExecutionResult {
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"bodies", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[{"workflowId":"main",
            "inputs":{"type":"object", "properties":{"s":{"type":"string"},
                "raw":{"type":"string"}, "obj":{"type":"object"}, "nil":{},
                "photo":{}}},
            "steps":[step]}]
    }))
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
        "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server.base_url}],
        "paths":{"/send":{"post":{"operationId":"send",
            "responses":{"200":{"description":"ok"}}}}}});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .dry_run(dry_run)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", inputs())
        .await
}

#[tokio::test]
async fn every_positive_row_sends_its_stated_bytes_and_content_type_and_dry_run_agrees() {
    let server = WireServer::start();
    let selector = json!({"context":"$inputs.obj", "selector":"/name", "type":"jsonpointer"});
    let rows: Vec<(&str, Value, &str, Vec<u8>, Value)> = vec![
        (
            "JSON literal template",
            body(Some("application/json"), json!(TEMPLATE)),
            "application/json",
            TEMPLATE_SENT.into(),
            json!(TEMPLATE_SENT),
        ),
        (
            "JSON literal text with no interpolation",
            body(Some("application/json"), json!("{\"a\":1}")),
            "application/json",
            b"{\"a\":1}".to_vec(),
            json!("{\"a\":1}"),
        ),
        (
            "omitted contentType literal template",
            body(None, json!(TEMPLATE)),
            "application/json",
            TEMPLATE_SENT.into(),
            json!(TEMPLATE_SENT),
        ),
        (
            "+json literal template",
            body(Some("application/problem+json"), json!(TEMPLATE)),
            "application/problem+json",
            TEMPLATE_SENT.into(),
            json!(TEMPLATE_SENT),
        ),
        (
            "JSON interpolation of a pre-serialized string",
            body(Some("application/json"), json!("{$inputs.raw}")),
            "application/json",
            b"{\"x\":1}".to_vec(),
            json!("{\"x\":1}"),
        ),
        (
            "JSON whole expression resolving to a string",
            body(Some("application/json"), json!("$inputs.s")),
            "application/json",
            b"\"abc\"".to_vec(),
            json!("abc"),
        ),
        (
            "omitted contentType whole expression resolving to a string",
            body(None, json!("$inputs.raw")),
            "application/json",
            b"\"{\\\"x\\\":1}\"".to_vec(),
            json!("{\"x\":1}"),
        ),
        (
            "JSON Selector Object resolving to a string",
            body(Some("application/json"), selector),
            "application/json",
            b"\"Ann\"".to_vec(),
            json!("Ann"),
        ),
        (
            "JSON object",
            body(
                Some("application/json"),
                json!({"z":"$inputs.s", "a":[1, true, null]}),
            ),
            "application/json",
            b"{\"z\":\"abc\",\"a\":[1,true,null]}".to_vec(),
            json!({"z":"abc", "a":[1, true, null]}),
        ),
        (
            "form map in document order",
            body(
                Some("application/x-www-form-urlencoded"),
                json!({"z":"a b", "a":["$inputs.s", 2]}),
            ),
            "application/x-www-form-urlencoded",
            b"z=a+b&a=abc&a=2".to_vec(),
            json!({"z":"a b", "a":["abc", 2]}),
        ),
        (
            "form string",
            body(
                Some("application/x-www-form-urlencoded"),
                json!("x=a+b&s={$inputs.s}"),
            ),
            "application/x-www-form-urlencoded",
            b"x=a+b&s=abc".to_vec(),
            json!("x=a+b&s=abc"),
        ),
        (
            "other type literal string",
            body(Some("text/plain"), json!("hi {$inputs.s} é")),
            "text/plain",
            "hi abc é".into(),
            json!("hi abc é"),
        ),
        (
            "other type with a UTF-8 charset and a whole-expression string",
            body(Some("text/plain; charset=UTF-8"), json!("$inputs.s")),
            "text/plain; charset=UTF-8",
            b"abc".to_vec(),
            json!("abc"),
        ),
        (
            "other type number",
            body(Some("text/plain"), json!(42)),
            "text/plain",
            b"42".to_vec(),
            json!(42),
        ),
        (
            "other type boolean",
            body(Some("application/xml"), json!(true)),
            "application/xml",
            b"true".to_vec(),
            json!(true),
        ),
    ];
    for (row, step, content_type, wire, logical) in rows {
        let result = run(&server, &step, false).await;
        assert!(result.outputs.is_ok(), "{row}: {:?}", result.outputs);
        assert_eq!(
            server.take(),
            vec![Seen {
                content_types: vec![content_type.to_owned()],
                body: wire,
            }],
            "{row}"
        );
        let dry = run(&server, &step, true).await;
        assert!(dry.outputs.is_ok(), "{row}: {:?}", dry.outputs);
        assert_eq!(server.take(), Vec::new(), "{row}: a dry run sends nothing");
        let planned = &dry.dry_run_requests()[0];
        assert_eq!(
            planned.headers.get("Content-Type").map(String::as_str),
            Some(content_type),
            "{row}"
        );
        assert_eq!(planned.body.as_ref(), Some(&logical), "{row}");
    }
}

#[tokio::test]
async fn a_blob_is_sent_raw_under_the_step_content_type_never_its_own_media_type() {
    let server = WireServer::start();
    let upload = body(Some("application/octet-stream"), json!("$inputs.photo"));
    let result = run(&server, &upload, false).await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(
        server.take(),
        vec![Seen {
            content_types: vec!["application/octet-stream".to_owned()],
            body: JPEG.to_vec(),
        }]
    );
    let dry = run(&server, &upload, true).await;
    assert!(dry.outputs.is_ok(), "{:?}", dry.outputs);
    let planned = &dry.dry_run_requests()[0];
    assert_eq!(
        planned.headers.get("Content-Type").map(String::as_str),
        Some("application/octet-stream")
    );
    let descriptor = planned.body.clone().unwrap_or_default();
    assert_eq!(descriptor["kind"], json!("bytes"));
    assert_eq!(descriptor["mediaType"], json!("image/png"));
    assert_eq!(descriptor["size"], json!(JPEG.len()));
}

#[tokio::test]
async fn a_content_type_header_parameter_is_refused_only_beside_a_request_body() {
    let server = WireServer::start();
    let header = |name: &str| json!([{"name":name, "in":"header", "value":"text/xml"}]);
    let alone = step(None, header("Content-Type"));
    let result = run(&server, &alone, false).await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(
        server.take(),
        vec![Seen {
            content_types: vec!["text/xml".to_owned()],
            body: Vec::new(),
        }],
        "with no requestBody the header parameter is the request's own"
    );
    for name in ["Content-Type", "content-type", "CONTENT-TYPE"] {
        let beside = step(
            Some(json!({"contentType":"text/plain", "payload":"x"})),
            header(name),
        );
        for dry_run in [false, true] {
            let result = run(&server, &beside, dry_run).await;
            let error = result
                .outputs
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("{name}: must refuse"));
            assert_eq!(error.code(), INVALID_BODY, "{name}: {error}");
            assert_eq!(
                error.message,
                format!("step send: parameters[0](header:{name}): the runtime sets Content-Type from requestBody.contentType alone, so a step with a requestBody cannot also send it as a header parameter; request not sent")
            );
            assert!(result.dry_run_requests().is_empty(), "{name}");
            assert_eq!(server.take(), Vec::new(), "{name}");
        }
    }
}

#[tokio::test]
async fn every_error_row_fails_before_sending_and_names_the_step() {
    let server = WireServer::start();
    let leaf = json!({"caption":"$inputs.s", "photo":"$inputs.photo"});
    let no_text_form = |content_type: &str, pointer: &str| {
        format!("step send: step \"send\": requestBody.payload{pointer}: contentType {content_type:?} cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded; request not sent")
    };
    let structure = |kind: &str| {
        format!("step send: requestBody.payload: contentType \"application/xml\" sends only a string, number, boolean or bytes payload; got {kind}; use a JSON contentType for structured data")
    };
    let rows = [
        (
            "whole blob with omitted contentType",
            body(None, json!("$inputs.photo")),
            "step send: step \"send\": requestBody.payload: the payload carries bytes, so an explicit contentType is required; request not sent".to_owned(),
        ),
        (
            "blob map leaf with omitted contentType",
            body(None, leaf.clone()),
            "step send: step \"send\": requestBody.payload/photo: the payload carries bytes, so an explicit contentType is required; request not sent".to_owned(),
        ),
        (
            "whole blob under JSON",
            body(Some("application/json"), json!("$inputs.photo")),
            no_text_form("application/json", ""),
        ),
        (
            "blob map leaf under +json",
            body(Some("application/vnd.api+json"), leaf.clone()),
            no_text_form("application/vnd.api+json", "/photo"),
        ),
        (
            "whole blob under form",
            body(
                Some("application/x-www-form-urlencoded"),
                json!("$inputs.photo"),
            ),
            no_text_form("application/x-www-form-urlencoded", ""),
        ),
        (
            "object under another type",
            body(Some("application/xml"), json!({"key":"val"})),
            structure("object"),
        ),
        (
            "array under another type",
            body(Some("application/xml"), json!(["a"])),
            structure("array"),
        ),
        (
            "null under another type",
            body(Some("application/xml"), json!("$inputs.nil")),
            structure("null"),
        ),
        (
            "non-UTF-8 charset",
            body(Some("text/plain; charset=iso-8859-1"), json!("café")),
            "step send: requestBody.payload: contentType \"text/plain; charset=iso-8859-1\" declares charset \"iso-8859-1\"; a string payload is sent as UTF-8 and never transcoded, so declare charset=utf-8 or omit it".to_owned(),
        ),
        (
            "multipart with a string payload",
            body(Some("multipart/form-data"), json!("$inputs.s")),
            "step send: requestBody.payload: contentType \"multipart/form-data\" needs an object payload, one form-data part per entry; got string".to_owned(),
        ),
        (
            "multipart with a whole blob",
            body(Some("multipart/form-data"), json!("$inputs.photo")),
            "step send: step \"send\": requestBody.payload: contentType \"multipart/form-data\" needs an object payload, one form-data part per entry; got bytes; request not sent".to_owned(),
        ),
        (
            "multipart with a boundary parameter",
            body(
                Some("multipart/form-data; boundary=fixed"),
                json!({"caption":"cat"}),
            ),
            "step send: requestBody.payload: contentType \"multipart/form-data; boundary=fixed\" carries a boundary parameter; the runtime chooses the multipart boundary, so remove it from contentType".to_owned(),
        ),
        (
            "multipart with bytes inside a JSON part",
            body(Some("multipart/form-data"), json!({"meta":{"file":"$inputs.photo"}})),
            "step send: step \"send\": requestBody.payload/meta/file: multipart/form-data part \"meta\" is sent as application/json, which cannot carry bytes; give the bytes a part of their own; request not sent".to_owned(),
        ),
    ];
    for (row, step, message) in rows {
        for dry_run in [false, true] {
            let result = run(&server, &step, dry_run).await;
            let error = result
                .outputs
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("{row}: must refuse"));
            assert_eq!(error.code(), INVALID_BODY, "{row}: {error}");
            assert_eq!(error.message, message, "{row}");
            assert!(result.dry_run_requests().is_empty(), "{row}");
            assert_eq!(
                server.take(),
                Vec::new(),
                "{row}: nothing reaches the server"
            );
        }
    }
}

fn replaced(content_type: Option<&str>, payload: &str, target: &str) -> Value {
    let mut request_body = json!({"payload":payload,
        "replacements":[{"target":target, "value":"$inputs.s"}]});
    if let Some(content_type) = content_type {
        request_body["contentType"] = json!(content_type);
    }
    step(Some(request_body), json!([]))
}

#[tokio::test]
async fn replacements_on_a_literal_json_template_apply_as_json_pointers_to_the_parsed_text() {
    let server = WireServer::start();
    let template = "{\"a\": {\"b\": 1, \"c\": \"{$inputs.s}\"}, \"z\": [1, 2.5, null]}";
    let sent = "{\"a\":{\"b\":\"abc\",\"c\":\"abc\"},\"z\":[1,2.5,null]}";
    for (row, content_type, header) in [
        (
            "application/json",
            Some("application/json"),
            "application/json",
        ),
        (
            "+json",
            Some("application/vnd.api+json"),
            "application/vnd.api+json",
        ),
        ("omitted contentType", None, "application/json"),
    ] {
        let step = replaced(content_type, template, "/a/b");
        let result = run(&server, &step, false).await;
        assert!(result.outputs.is_ok(), "{row}: {:?}", result.outputs);
        assert_eq!(
            server.take(),
            vec![Seen {
                content_types: vec![header.to_owned()],
                body: sent.into(),
            }],
            "{row}"
        );
        let dry = run(&server, &step, true).await;
        assert!(dry.outputs.is_ok(), "{row}: {:?}", dry.outputs);
        assert_eq!(server.take(), Vec::new(), "{row}");
        assert_eq!(
            dry.dry_run_requests()[0].body.as_ref(),
            Some(&json!(sent)),
            "{row}"
        );
    }
}

#[tokio::test]
async fn replacements_on_a_literal_json_template_that_does_not_parse_fail_before_sending() {
    let server = WireServer::start();
    let unparseable = "{\"a\": {$inputs.s}}";
    for content_type in [Some("application/json"), None] {
        let step = replaced(content_type, unparseable, "/a");
        for dry_run in [false, true] {
            let result = run(&server, &step, dry_run).await;
            let error = result
                .outputs
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("{content_type:?}: must refuse"));
            assert_eq!(error.code(), INVALID_BODY, "{error}");
            assert_eq!(
                error.message,
                "step send: requestBody.payload: replacements apply to a literal JSON payload by parsing its interpolated text, which is not JSON: expected value at line 1 column 7"
            );
            assert!(result.dry_run_requests().is_empty());
            assert_eq!(server.take(), Vec::new());
        }
    }
    let verbatim = body(Some("application/json"), json!(unparseable));
    let result = run(&server, &verbatim, false).await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(
        server.take(),
        vec![Seen {
            content_types: vec!["application/json".to_owned()],
            body: b"{\"a\": abc}".to_vec(),
        }],
        "with no replacements the template is still sent as written"
    );
}
