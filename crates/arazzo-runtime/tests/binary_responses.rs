#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    BytesDescriptor, EngineBuilder, ExecutionResult, RuntimeErrorKind, TraceStepRecord,
};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const UNAVAILABLE: &str = "RUNTIME_VALUE_UNAVAILABLE";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nPLANTED-PNG-PIXELS\x00\xff";
const PNG_MARKER: &str = "PLANTED-PNG-PIXELS";
const PNG_BASE64: &str = "iVBORw0KGgpQTEFOVEVELVBORy1QSVhFTFMA/w==";
const PNG_REASON: &str = "response body is bytes (image/png), which is not yet a readable value";
const UNTYPED_REASON: &str =
    "response has no Content-Type header, so its body is bytes, which is not yet a readable value";

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    line: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

struct PhotoServer {
    base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl PhotoServer {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (record, stopped) = (Arc::clone(&seen), Arc::clone(&stop));
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
                    .map(|header| header.value.to_string());
                let line = format!("{} {}", request.method(), request.url());
                let (body_out, media_type) = match request.url() {
                    "/photo" => (PNG.to_vec(), Some("image/png")),
                    "/raw" => (PNG.to_vec(), None),
                    _ => (br#"{"ok":true}"#.to_vec(), Some("application/json")),
                };
                record
                    .lock()
                    .unwrap_or_else(|_| panic!("recording"))
                    .push(Seen {
                        line,
                        content_type,
                        body,
                    });
                let mut response = TinyResponse::from_data(body_out);
                if let Some(media_type) = media_type {
                    response = response.with_header(
                        Header::from_bytes("Content-Type", media_type)
                            .unwrap_or_else(|()| panic!("content type header")),
                    );
                }
                let _ = request.respond(response);
            }
        });
        Self {
            base_url,
            seen,
            stop,
            worker: Some(worker),
        }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap_or_else(|_| panic!("reading")))
    }

    fn lines(&self) -> Vec<String> {
        self.take().into_iter().map(|seen| seen.line).collect()
    }
}

impl Drop for PhotoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn builder(base_url: &str, steps: Value, outputs: Value) -> EngineBuilder {
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"binary responses", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[{"workflowId":"main", "steps":steps, "outputs":outputs}]
    }))
    .unwrap_or_else(|error| panic!("spec: {error}"));
    let operation = |id: &str| json!({"operationId":id, "responses":{"200":{"description":"ok"}}});
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":base_url}],
    "paths":{
        "/photo":{"get":operation("photo")},
        "/raw":{"get":operation("raw")},
        "/upload":{"post":operation("upload")}
    }});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .trace(true)
}

async fn run(base_url: &str, steps: Value, outputs: Value) -> ExecutionResult {
    builder(base_url, steps, outputs)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", BTreeMap::new())
        .await
}

fn download(operation: &str, extra: &Value) -> Value {
    let mut step = json!({"stepId":"download", "operationId":operation,
        "successCriteria":[{"condition":"$statusCode == 200"}]});
    for (key, value) in extra.as_object().into_iter().flatten() {
        step[key] = value.clone();
    }
    step
}

fn upload(content_type: &str) -> Value {
    json!({"stepId":"upload", "operationId":"upload",
        "requestBody":{"contentType":content_type, "payload":"$steps.download.outputs.photo"}})
}

fn whole_output() -> Value {
    json!({"outputs":{"photo":"$response.body"}})
}

fn owned_trace(result: &ExecutionResult) -> Vec<TraceStepRecord> {
    result.trace_steps().into_iter().cloned().collect()
}

#[tokio::test]
async fn a_downloaded_png_is_a_blob_output_that_a_later_step_uploads_byte_for_byte() {
    let server = PhotoServer::start();
    let result = run(
        &server.base_url,
        json!([download("photo", &whole_output()), upload("image/png")]),
        json!({"photo":"$steps.download.outputs.photo"}),
    )
    .await;
    let outputs = result
        .outputs
        .as_ref()
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert!(outputs.is_empty(), "{outputs:?}");

    let seen = server.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        ["GET /photo", "POST /upload"]
    );
    assert_eq!(seen[1].content_type.as_deref(), Some("image/png"));
    assert_eq!(seen[1].body, PNG);

    let blob = &result.output_blobs["photo"];
    assert_eq!(blob.bytes(), PNG);
    assert_eq!(blob.media_type(), Some("image/png"));
    assert_eq!(blob.filename(), None);
    let descriptor = BytesDescriptor::of(PNG, Some("image/png".to_owned()), None);
    assert_eq!(blob.descriptor(), descriptor);

    let presented = result
        .presented_outputs()
        .unwrap_or_else(|error| panic!("presented: {error}"));
    assert_eq!(presented["photo"], descriptor.to_value());
    let traced = result.trace_steps();
    assert_eq!(traced[0].output_blobs["photo"], descriptor);
    assert!(!traced[0].outputs.contains_key("photo"));
    let response = traced[0]
        .response
        .as_ref()
        .unwrap_or_else(|| panic!("download response is traced"));
    assert_eq!(response.body_blob.as_ref(), Some(&descriptor));
    assert_eq!(response.body, None);
    assert_eq!(
        traced[1]
            .request
            .as_ref()
            .map(|request| request.body_blobs.len()),
        Some(1)
    );

    let serialized = [
        serde_json::to_string(&presented),
        serde_json::to_string(&traced),
    ]
    .map(|text| text.unwrap_or_else(|error| panic!("serialize: {error}")));
    let sha256 = descriptor.to_value()["sha256"].clone();
    let sha256 = sha256.as_str().unwrap_or_else(|| panic!("descriptor hash"));
    for text in &serialized {
        assert!(text.contains(sha256), "{text}");
        assert!(!text.contains(PNG_MARKER), "{text}");
        assert!(!text.contains(PNG_BASE64), "{text}");
    }
}

#[tokio::test]
async fn a_response_without_content_type_is_a_blob_and_a_pointer_names_the_missing_header() {
    let server = PhotoServer::start();
    let result = run(
        &server.base_url,
        json!([
            download("raw", &whole_output()),
            upload("application/octet-stream")
        ]),
        json!({"photo":"$steps.download.outputs.photo"}),
    )
    .await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    let seen = server.take();
    assert_eq!(seen[1].line, "POST /upload");
    assert_eq!(
        seen[1].content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(seen[1].body, PNG);
    let blob = &result.output_blobs["photo"];
    assert_eq!(blob.bytes(), PNG);
    assert_eq!(blob.media_type(), None);
    assert!(result
        .presented_outputs()
        .is_ok_and(|presented| presented["photo"].get("mediaType").is_none()));

    let pointer = run(
        &server.base_url,
        json!([download(
            "raw",
            &json!({"outputs":{"a":"$response.body#/a"}})
        )]),
        json!({}),
    )
    .await;
    let error = pointer
        .outputs
        .err()
        .unwrap_or_else(|| panic!("a pointer into bytes must fail"));
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        format!("step download: output \"a\": $response.body#/a: {UNTYPED_REASON}")
    );
    assert!(error.message.contains("no Content-Type header"));
}

#[tokio::test]
async fn every_use_but_a_whole_output_fails_and_status_criteria_still_pass() {
    let server = PhotoServer::start();
    let regex = |context: Option<&str>| {
        let mut criterion = json!({"condition":"^PNG", "type":"regex"});
        if let Some(context) = context {
            criterion["context"] = json!(context);
        }
        json!({"successCriteria":[{"condition":"$statusCode == 200"}, criterion]})
    };
    let cases = [
        (
            "pointer",
            json!({"outputs":{"a":"$response.body#/a"}}),
            "step download: output \"a\": $response.body#/a",
        ),
        (
            "whole-document pointer",
            json!({"outputs":{"a":"$response.body#"}}),
            "step download: output \"a\": $response.body#",
        ),
        (
            "interpolation",
            json!({"onSuccess":[{"name":"jump", "type":"goto",
                "stepId":"after-{$response.body}"}]}),
            "step download: action stepId: $response.body",
        ),
        (
            "simple condition",
            json!({"successCriteria":[{"condition":"$statusCode == 200"},
                {"condition":"$response.body != null"}]}),
            "step download: successCriteria[1]: $response.body",
        ),
        (
            "explicit criterion context",
            regex(Some("$response.body")),
            "step download: successCriteria[1]: $response.body",
        ),
        (
            "default criterion context",
            regex(None),
            "step download: successCriteria[1]: $response.body",
        ),
    ];
    for (site, extra, location) in cases {
        let mut step = download("photo", &json!({}));
        for (key, value) in extra.as_object().into_iter().flatten() {
            step[key] = value.clone();
        }
        let result = run(&server.base_url, json!([step]), json!({})).await;
        let error = result
            .outputs
            .err()
            .unwrap_or_else(|| panic!("{site}: bytes must fail the step"));
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(error.message, format!("{location}: {PNG_REASON}"), "{site}");
        assert_eq!(server.lines(), ["GET /photo"], "{site}");
    }

    let passed = run(
        &server.base_url,
        json!([download(
            "photo",
            &json!({"outputs":{"status":"$statusCode", "photo":"$response.body"}})
        )]),
        json!({"status":"$steps.download.outputs.status"}),
    )
    .await;
    let outputs = passed
        .outputs
        .unwrap_or_else(|error| panic!("$statusCode criterion must pass: {error}"));
    assert_eq!(outputs["status"], json!(200));
}

#[tokio::test]
async fn replaying_a_binary_download_never_invents_a_blob() {
    let server = PhotoServer::start();
    let live = run(
        &server.base_url,
        json!([download("photo", &whole_output())]),
        json!({"photo":"$steps.download.outputs.photo"}),
    )
    .await;
    assert!(
        live.output_blobs.contains_key("photo"),
        "{:?}",
        live.outputs
    );
    assert_eq!(server.lines(), ["GET /photo"]);

    let replayed = builder(
        &server.base_url,
        json!([download("photo", &whole_output())]),
        json!({"photo":"$steps.download.outputs.photo"}),
    )
    .replay_trace_steps(owned_trace(&live))
    .build()
    .unwrap_or_else(|error| panic!("replay engine: {error}"))
    .execute_collect("main", BTreeMap::new())
    .await;
    assert!(replayed.output_blobs.is_empty());
    let error = replayed
        .outputs
        .err()
        .unwrap_or_else(|| panic!("replay must not produce a blob"));
    assert_eq!(error.kind, RuntimeErrorKind::ValueUnavailable, "{error}");
    assert_eq!(
        error.message,
        "step download: output \"photo\": $response.body: response body is bytes (image/png), which a replay trace records only as a descriptor, so replay cannot restore it"
    );
    assert!(server.lines().is_empty());

    let round_trip = run(
        &server.base_url,
        json!([download("photo", &whole_output()), upload("image/png")]),
        json!({}),
    )
    .await;
    assert!(round_trip.outputs.is_ok(), "{:?}", round_trip.outputs);
    let refused = builder(
        &server.base_url,
        json!([download("photo", &whole_output()), upload("image/png")]),
        json!({}),
    )
    .replay_trace_steps(owned_trace(&round_trip))
    .build();
    let error = refused
        .err()
        .unwrap_or_else(|| panic!("a trace that uploaded bytes must be refused"));
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert!(
        error
            .message
            .contains("replay cannot restore recorded bytes yet"),
        "{error}"
    );
}
