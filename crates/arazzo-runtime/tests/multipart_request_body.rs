#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    Blob, BytesDescriptor, EngineBuilder, ExecutionResult, TraceStepRecord, WorkflowInputs,
};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server, StatusCode};

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const MARKER: &[u8] = b"PLANTED-MULTIPART-BYTES-41c9";

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    path: String,
    content_type: String,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    name: String,
    filename: Option<String>,
    media_type: Option<String>,
    headers: Vec<String>,
    bytes: Vec<u8>,
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
        let flaky = AtomicUsize::new(0);
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
                    .map(|header| header.value.as_str().to_owned())
                    .unwrap_or_default();
                let path = request.url().to_owned();
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen {
                        path: path.clone(),
                        content_type,
                        body,
                    });
                let header = |name: &str, value: &str| {
                    Header::from_bytes(name, value).unwrap_or_else(|()| panic!("header {name}"))
                };
                let response = match path.as_str() {
                    "/redirect" => TinyResponse::from_string("")
                        .with_status_code(StatusCode(307))
                        .with_header(header("Location", "/upload")),
                    "/flaky" if flaky.fetch_add(1, Ordering::Relaxed) == 0 => {
                        TinyResponse::from_string("busy").with_status_code(StatusCode(503))
                    }
                    _ => TinyResponse::from_string(r#"{"id":"p1"}"#)
                        .with_header(header("Content-Type", "application/json")),
                };
                let _ = request.respond(response);
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

fn jpeg() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.extend_from_slice(MARKER);
    bytes.extend_from_slice(&[0x00, 0x0D, 0x0A, 0xFF, 0xD9]);
    bytes
}

fn raw() -> Vec<u8> {
    [MARKER, b"-raw"].concat()
}

fn photo() -> Blob {
    Blob::new(jpeg())
        .with_media_type("image/jpeg")
        .with_filename("c.jpg")
}

fn inputs(caption: &str) -> WorkflowInputs {
    WorkflowInputs::new(
        BTreeMap::from([
            ("caption".to_owned(), json!(caption)),
            ("meta".to_owned(), json!({"z": 1, "a": [true]})),
            ("tags".to_owned(), json!(["t1", null, "t2"])),
        ]),
        BTreeMap::from([
            ("photo".to_owned(), photo()),
            ("raw".to_owned(), Blob::new(raw())),
            (
                "odd".to_owned(),
                Blob::new(b"odd".to_vec()).with_filename("a\"b\r\n.jpg"),
            ),
        ]),
    )
}

fn payload() -> Value {
    json!({
        "caption": "$inputs.caption",
        "count": 3,
        "flag": true,
        "meta": "$inputs.meta",
        "tags": ["a", "b", ["x", "y"]],
        "none": null,
        "empty": [],
        "listed": "$inputs.tags",
        "raw": "$inputs.raw",
        "photo": "$inputs.photo"
    })
}

fn upload(operation: &str, payload: Value) -> Value {
    json!({"stepId":"upload", "operationId":operation,
        "requestBody":{"contentType":"multipart/form-data", "payload":payload},
        "successCriteria":[{"condition":"$statusCode == 200"}],
        "outputs":{"id":"$response.body#/id"}})
}

fn builder(server: &WireServer, step: Value) -> EngineBuilder {
    builder_with(server, json!([step]))
}

fn builder_with(server: &WireServer, steps: Value) -> EngineBuilder {
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"multipart", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[{"workflowId":"main",
            "inputs":{"type":"object", "properties":{"caption":{"type":"string"},
                "meta":{"type":"object"}, "tags":{"type":"array"},
                "photo":{}, "raw":{}, "odd":{}}},
            "steps":steps, "outputs":{"id":"$steps.upload.outputs.id"}}]
    }))
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    let operation = |id: &str| {
        json!({"post":{"operationId":id,
        "responses":{"200":{"description":"ok"}}}})
    };
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
        "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server.base_url}],
        "paths":{"/upload":operation("upload"), "/flaky":operation("flaky"),
            "/redirect":operation("redirected")}});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .trace(true)
}

async fn run(builder: EngineBuilder, caption: &str) -> ExecutionResult {
    builder
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", inputs(caption))
        .await
}

async fn parse(seen: &Seen) -> Vec<Field> {
    let boundary = multer::parse_boundary(&seen.content_type)
        .unwrap_or_else(|error| panic!("{:?}: {error}", seen.content_type));
    let body = seen.body.clone();
    let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut fields = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .unwrap_or_else(|error| panic!("parsing multipart: {error}"))
    {
        let header = |name: &str| {
            field
                .headers()
                .get(name)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        };
        let media_type = header("content-type");
        let headers = field
            .headers()
            .keys()
            .map(|name| name.as_str().to_owned())
            .collect();
        let name = field.name().unwrap_or_default().to_owned();
        let filename = field.file_name().map(str::to_owned);
        let bytes = field
            .bytes()
            .await
            .unwrap_or_else(|error| panic!("reading part: {error}"))
            .to_vec();
        fields.push(Field {
            name,
            filename,
            media_type,
            headers,
            bytes,
        });
    }
    fields
}

fn text(name: &str, value: &str) -> Field {
    Field {
        name: name.to_owned(),
        filename: None,
        media_type: None,
        headers: vec!["content-disposition".to_owned()],
        bytes: value.as_bytes().to_vec(),
    }
}

fn typed(name: &str, filename: Option<&str>, media_type: &str, bytes: &[u8]) -> Field {
    Field {
        name: name.to_owned(),
        filename: filename.map(str::to_owned),
        media_type: Some(media_type.to_owned()),
        headers: vec!["content-disposition".to_owned(), "content-type".to_owned()],
        bytes: bytes.to_vec(),
    }
}

fn expected_fields() -> Vec<Field> {
    vec![
        text("caption", "cat"),
        text("count", "3"),
        text("flag", "true"),
        typed("meta", None, "application/json", br#"{"z":1,"a":[true]}"#),
        text("tags", "a"),
        text("tags", "b"),
        typed("tags", None, "application/json", br#"["x","y"]"#),
        text("listed", "t1"),
        text("listed", "t2"),
        typed("raw", Some("raw"), "application/octet-stream", &raw()),
        typed("photo", Some("c.jpg"), "image/jpeg", &jpeg()),
    ]
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn trace_of(result: &ExecutionResult) -> Vec<TraceStepRecord> {
    result.trace_steps().into_iter().cloned().collect()
}

#[tokio::test]
async fn every_d4_row_reaches_an_independent_parser_in_document_order() {
    let server = WireServer::start();
    let result = run(builder(&server, upload("upload", payload())), "cat").await;
    assert_eq!(
        result.outputs.as_ref().ok(),
        Some(&BTreeMap::from([("id".to_owned(), json!("p1"))])),
        "{:?}",
        result.outputs
    );
    let seen = server.take();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0]
            .content_type
            .starts_with("multipart/form-data; boundary="),
        "{}",
        seen[0].content_type
    );
    assert_eq!(parse(&seen[0]).await, expected_fields());

    let traces = trace_of(&result);
    let request = traces[0]
        .request
        .as_ref()
        .unwrap_or_else(|| panic!("the request is traced"));
    assert_eq!(request.headers["Content-Type"], "multipart/form-data");
    assert_eq!(
        request.body,
        Some(
            json!({"caption":"cat", "count":3, "flag":true, "meta":{"z":1, "a":[true]},
            "tags":["a", "b", ["x", "y"]], "listed":["t1", "t2"], "raw":null, "photo":null})
        )
    );
    assert_eq!(
        request.body_blobs,
        BTreeMap::from([
            ("/photo".to_owned(), photo().descriptor()),
            ("/raw".to_owned(), BytesDescriptor::of(&raw(), None, None)),
        ])
    );
    let serialized =
        serde_json::to_vec(&traces).unwrap_or_else(|error| panic!("serializing trace: {error}"));
    assert!(!contains(&serialized, MARKER), "the trace holds no bytes");
    assert!(
        !contains(&serialized, b"boundary"),
        "the trace holds no boundary"
    );
}

#[tokio::test]
async fn a_dry_run_shows_parts_and_descriptors_and_never_bytes_or_the_boundary() {
    let server = WireServer::start();
    let result = run(
        builder(&server, upload("upload", payload())).dry_run(true),
        "cat",
    )
    .await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert!(server.take().is_empty());
    let planned = result.dry_run_requests()[0].clone();
    assert_eq!(planned.headers["Content-Type"], "multipart/form-data");
    assert_eq!(
        planned.body,
        Some(
            json!({"caption":"cat", "count":3, "flag":true, "meta":{"z":1, "a":[true]},
            "tags":["a", "b", ["x", "y"]], "listed":["t1", "t2"],
            "raw":BytesDescriptor::of(&raw(), None, None).to_value(),
            "photo":photo().descriptor().to_value()})
        )
    );
    let serialized = serde_json::to_vec(&json!({"dry": planned, "events": result
        .execution_events()}))
    .unwrap_or_else(|error| panic!("serializing dry run: {error}"));
    assert!(!contains(&serialized, MARKER));
    assert!(!contains(&serialized, b"boundary"));
}

#[tokio::test]
async fn names_and_filenames_percent_escape_quote_cr_and_lf() {
    let server = WireServer::start();
    let step = upload(
        "upload",
        json!({"q\"uote":"1", "c\rr":"2", "l\nf":"3", "file":"$inputs.odd"}),
    );
    let result = run(builder(&server, step), "cat").await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let seen = server.take();
    assert_eq!(
        parse(&seen[0]).await,
        vec![
            text("q%22uote", "1"),
            text("c%0Dr", "2"),
            text("l%0Af", "3"),
            typed(
                "file",
                Some("a%22b%0D%0A.jpg"),
                "application/octet-stream",
                b"odd"
            ),
        ]
    );
}

#[tokio::test]
async fn every_retry_attempt_gets_a_fresh_boundary_and_the_same_parts() {
    let server = WireServer::start();
    let mut step = upload("flaky", payload());
    step["onFailure"] = json!([{"name":"again", "type":"retry", "retryAfter":0, "retryLimit":2}]);
    let result = run(builder(&server, step), "cat").await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let seen = server.take();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].content_type, seen[1].content_type);
    for attempt in &seen {
        assert_eq!(parse(attempt).await, expected_fields());
    }
    let traces = trace_of(&result);
    assert_eq!(traces.len(), 2);
    for trace in &traces {
        let request = trace
            .request
            .as_ref()
            .unwrap_or_else(|| panic!("each attempt is traced"));
        assert_eq!(request.headers["Content-Type"], "multipart/form-data");
    }
}

#[tokio::test]
async fn a_307_resend_within_one_attempt_sends_identical_bytes() {
    let server = WireServer::start();
    let result = run(builder(&server, upload("redirected", payload())), "cat").await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let seen = server.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.path.as_str())
            .collect::<Vec<_>>(),
        ["/redirect", "/upload"]
    );
    assert_eq!(seen[0].content_type, seen[1].content_type);
    assert_eq!(seen[0].body, seen[1].body);
    assert_eq!(parse(&seen[1]).await, expected_fields());
}

#[tokio::test]
async fn a_multipart_run_replays_and_a_changed_part_is_drift() {
    let server = WireServer::start();
    let live = run(builder(&server, upload("upload", payload())), "cat").await;
    assert!(live.outputs.is_ok(), "{:?}", live.outputs);
    assert_eq!(server.take().len(), 1);

    let replayed = run(
        builder(&server, upload("upload", payload())).replay_trace_steps(trace_of(&live)),
        "cat",
    )
    .await;
    assert_eq!(replayed.outputs.ok(), live.outputs.as_ref().ok().cloned());
    assert!(server.take().is_empty(), "replay never touches the wire");

    let drifted = run(
        builder(&server, upload("upload", payload())).replay_trace_steps(trace_of(&live)),
        "dog",
    )
    .await;
    let error = drifted
        .outputs
        .err()
        .unwrap_or_else(|| panic!("a changed caption is drift"));
    assert_eq!(error.code(), "RUNTIME_REPLAY_REQUEST_MISMATCH", "{error}");
    assert!(error.message.contains("body mismatch"), "{error}");
}

#[tokio::test]
async fn request_body_after_a_multipart_request_is_unavailable() {
    let server = WireServer::start();
    for expression in ["$request.body", "$request.body#/caption"] {
        let mut step = upload("upload", payload());
        step["outputs"]["sent"] = json!(expression);
        let result = run(builder(&server, step), "cat").await;
        let error = result
            .outputs
            .err()
            .unwrap_or_else(|| panic!("{expression} must not resolve"));
        assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE", "{error}");
        assert!(
            error
                .message
                .contains("request body is multipart/form-data"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn bytes_inside_a_json_part_fail_before_sending() {
    let server = WireServer::start();
    let step = upload("upload", json!({"meta":{"file":"$inputs.photo"}}));
    let result = run(builder(&server, step), "cat").await;
    let error = result
        .outputs
        .err()
        .unwrap_or_else(|| panic!("bytes cannot be JSON"));
    assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY", "{error}");
    assert!(
        error.message.contains(
            "requestBody.payload/meta/file: multipart/form-data part \"meta\" is sent as application/json, which cannot carry bytes"
        ),
        "{error}"
    );
    assert!(server.take().is_empty());
}

#[tokio::test]
async fn a_dry_run_marks_parts_fed_by_a_skipped_response_beside_a_file_part() {
    let server = WireServer::start();
    let create = json!({"stepId":"create", "operationId":"upload",
        "successCriteria":[{"condition":"$statusCode == 200"}],
        "outputs":{"id":"$response.body#/id"}});
    let step = upload(
        "upload",
        json!({"albumId":"$steps.create.outputs.id", "caption":"c",
            "tags":["t", "$steps.create.outputs.id"], "photo":"$inputs.photo"}),
    );
    let result = run(
        builder_with(&server, json!([create, step])).dry_run(true),
        "cat",
    )
    .await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert!(server.take().is_empty());
    let planned = result.dry_run_requests()[1].clone();
    assert_eq!(
        planned.body,
        Some(
            json!({"albumId":"<unresolved:$response.body#/id>", "caption":"c",
            "tags":["t", "<unresolved:$response.body#/id>"],
            "photo":photo().descriptor().to_value()})
        )
    );
    assert_eq!(
        planned.warnings,
        [
            "dry-run unresolved main/upload/requestBody.payload/albumId: main/create.outputs.id depends on $response.body#/id; transport was skipped",
            "dry-run unresolved main/upload/requestBody.payload/tags/1: main/create.outputs.id depends on $response.body#/id; transport was skipped",
            "dry-run unresolved output main/upload.outputs.id: $response.body#/id requires a response; transport was skipped",
        ]
    );
}

#[tokio::test]
async fn a_live_file_part_whose_bytes_carry_only_a_descriptor_fails_before_sending() {
    let server = WireServer::start();
    let inputs = WorkflowInputs::new(
        BTreeMap::new(),
        BTreeMap::from([(
            "photo".to_owned(),
            Blob::from_descriptor(photo().descriptor()),
        )]),
    );
    let result = builder(
        &server,
        upload("upload", json!({"caption":"c", "photo":"$inputs.photo"})),
    )
    .build()
    .unwrap_or_else(|error| panic!("engine: {error}"))
    .execute_collect("main", inputs)
    .await;
    let error = result
        .outputs
        .err()
        .unwrap_or_else(|| panic!("a descriptor-only file part has nothing to send"));
    assert_eq!(error.code(), "RUNTIME_INTERNAL_ERROR", "{error}");
    assert!(
        error.message.contains("carry only their descriptor"),
        "{error}"
    );
    assert!(server.take().is_empty(), "nothing reaches the wire");
}

#[tokio::test]
async fn request_content_type_after_a_multipart_request_is_the_logical_header_live_and_on_replay() {
    let server = WireServer::start();
    let expected = BTreeMap::from([("id".to_owned(), json!("multipart/form-data"))]);
    for payload in [payload(), json!({"caption":"$inputs.caption"})] {
        let mut step = upload("upload", payload);
        step["outputs"]["id"] = json!("$request.header.Content-Type");
        let live = run(builder(&server, step.clone()), "cat").await;
        assert_eq!(
            live.outputs.as_ref().ok(),
            Some(&expected),
            "{:?}",
            live.outputs
        );
        assert!(server.take()[0]
            .content_type
            .starts_with("multipart/form-data; boundary="));
        let replayed = run(
            builder(&server, step).replay_trace_steps(trace_of(&live)),
            "cat",
        )
        .await;
        assert_eq!(
            replayed.outputs.as_ref().ok(),
            Some(&expected),
            "{:?}",
            replayed.outputs
        );
    }
}
