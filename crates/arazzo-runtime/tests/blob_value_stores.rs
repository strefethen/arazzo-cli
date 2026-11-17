#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{
    Blob, BytesDescriptor, EngineBuilder, ExecutionResult, RuntimeError, WorkflowInputs,
};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const UNAVAILABLE: &str = "RUNTIME_VALUE_UNAVAILABLE";
const SOURCE: &str = "https://fixture.invalid/openapi.json";
const JPEG: [u8; 6] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
const UPLOAD: &str = "POST /upload/7";
const NEXT: &str = "GET /next";
const STEP_PHOTO: &str = "step \"keep\" output \"photo\"";

#[derive(Debug, Clone, PartialEq)]
struct Seen {
    line: String,
    body: Vec<u8>,
}

struct ScriptedServer {
    base_url: String,
    requests: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ScriptedServer {
    fn start(upload_statuses: &[u16]) -> Self {
        let server =
            Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("binding server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        let mut statuses = upload_statuses.to_vec();
        statuses.reverse();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(mut request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                let line = format!("{} {}", request.method(), request.url());
                let status = if line == UPLOAD {
                    statuses.pop().unwrap_or(200)
                } else {
                    200
                };
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording request"))
                    .push(Seen { line, body });
                let header = Header::from_bytes("Content-Type", "application/json")
                    .unwrap_or_else(|()| panic!("content type header"));
                let _ = request.respond(
                    TinyResponse::from_data(br#"{"ok":true}"#.to_vec())
                        .with_status_code(status)
                        .with_header(header),
                );
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

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn photo_blob() -> Blob {
    Blob::new(JPEG.to_vec())
        .with_filename("cat.jpg")
        .with_media_type("image/jpeg")
}

fn photo() -> WorkflowInputs {
    WorkflowInputs::new(
        BTreeMap::from([("caption".to_owned(), json!("cat"))]),
        BTreeMap::from([("photo".to_owned(), photo_blob())]),
    )
}

fn keep() -> Value {
    json!({"stepId":"keep", "operationId":"next",
        "outputs":{"photo":"$inputs.photo", "caption":"$inputs.caption"}})
}

fn upload(step_id: &str, payload: &str) -> Value {
    json!({"stepId":step_id, "operationId":"upload",
        "parameters":[{"name":"id", "in":"path", "value":"7"}],
        "requestBody":{"contentType":"image/jpeg", "payload":payload}})
}

fn with(mut base: Value, fields: Value) -> Value {
    for (name, value) in fields.as_object().into_iter().flatten() {
        base[name] = value.clone();
    }
    base
}

fn builder(server: &ScriptedServer, workflows: Value) -> EngineBuilder {
    let photo_input =
        json!({"type":"object", "properties":{"photo":{}, "caption":{"type":"string"}}});
    let mut workflows = workflows;
    for workflow in workflows.as_array_mut().into_iter().flatten() {
        workflow["inputs"] = photo_input.clone();
    }
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"bytes", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":workflows
    }))
    .unwrap_or_else(|error| panic!("fixture: {error}"));
    let operation = |id: &str| json!({"operationId":id, "responses":{"200":{"description":"ok"}}});
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
        "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server.base_url}],
        "paths":{"/upload/{id}":{"post":operation("upload")}, "/next":{"get":operation("next")}}});
    EngineBuilder::new(spec).openapi_spec(openapi.to_string().into_bytes(), None)
}

async fn run(server: &ScriptedServer, workflows: Value) -> ExecutionResult {
    builder(server, workflows)
        .trace(true)
        .capture_run(true)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", photo())
        .await
}

fn refused(site: &str, outcome: Result<BTreeMap<String, Value>, RuntimeError>) -> RuntimeError {
    match outcome {
        Ok(outputs) => panic!("{site}: bytes must fail the run, got {outputs:?}"),
        Err(error) => error,
    }
}

fn uploads(seen: &[Seen]) -> Vec<&[u8]> {
    seen.iter()
        .filter(|seen| seen.line == UPLOAD)
        .map(|seen| seen.body.as_slice())
        .collect()
}

#[tokio::test]
async fn workflow_and_step_outputs_carry_an_input_blob_that_a_later_step_uploads_unchanged() {
    let server = ScriptedServer::start(&[]);
    let main = json!({"workflowId":"main",
    "steps":[
        keep(),
        with(upload("send", "$steps.keep.outputs.photo"),
            json!({"outputs":{"sent":"$request.body"}})),
        upload("again", "$steps.send.outputs.sent")
    ],
    "outputs":{
        "photo":"$steps.keep.outputs.photo",
        "sent":"$steps.send.outputs.sent",
        "repeat":"$outputs.photo",
        "caption":"$steps.keep.outputs.caption"
    }});
    let result = run(&server, json!([main])).await;

    let outputs = result
        .outputs
        .as_ref()
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(
        outputs,
        &BTreeMap::from([("caption".to_owned(), json!("cat"))])
    );
    for name in ["photo", "sent", "repeat"] {
        assert_eq!(result.output_blobs.get(name), Some(&photo_blob()), "{name}");
    }
    let seen = server.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        [NEXT, UPLOAD, UPLOAD]
    );
    assert_eq!(uploads(&seen), [JPEG.as_slice(), JPEG.as_slice()]);

    let descriptor = photo_blob().descriptor();
    let presented = result
        .presented_outputs()
        .unwrap_or_else(|error| panic!("presented: {error}"));
    assert_eq!(presented["photo"], descriptor.to_value());
    assert_eq!(presented["photo"]["filename"], "cat.jpg");
    assert_eq!(presented["caption"], "cat");

    let traced = result.trace_steps();
    assert_eq!(traced[0].output_blobs["photo"], descriptor);
    assert!(!traced[0].outputs.contains_key("photo"));
    assert_eq!(traced[0].outputs["caption"], "cat");
    assert_eq!(traced[1].output_blobs["sent"], descriptor);
    let captured = result.run_steps();
    assert_eq!(captured[0].output_blobs["photo"], descriptor);
    assert!(!captured[0].outputs.contains_key("photo"));
    assert_eq!(captured[1].output_blobs["sent"], descriptor);
}

#[tokio::test]
async fn a_sensitive_blob_output_renders_only_the_redacted_marker() {
    let server = ScriptedServer::start(&[]);
    let main = json!({"workflowId":"main", "steps":[keep()],
        "outputs":{"key":"$steps.keep.outputs.photo"}});
    let secret = WorkflowInputs::new(
        BTreeMap::new(),
        BTreeMap::from([("photo".to_owned(), photo_blob().sensitive())]),
    );
    let result = builder(&server, json!([main]))
        .trace(true)
        .capture_run(true)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", secret)
        .await;

    let presented = result
        .presented_outputs()
        .unwrap_or_else(|error| panic!("presented: {error}"));
    assert_eq!(presented["key"], json!({"kind":"bytes", "redacted":true}));
    assert_eq!(result.output_blobs["key"].bytes(), JPEG);
    assert_eq!(
        result.trace_steps()[0].output_blobs["photo"],
        BytesDescriptor::redacted()
    );
    assert_eq!(
        result.run_steps()[0].output_blobs["photo"],
        BytesDescriptor::redacted()
    );
}

#[tokio::test]
async fn a_blob_output_survives_retry_retry_reference_and_goto_copies_of_the_store() {
    let server = ScriptedServer::start(&[503, 201, 200]);
    let send = with(
        upload("send", "$steps.keep.outputs.photo"),
        json!({
            "successCriteria":[{"condition":"$statusCode != 503"}],
            "onFailure":[{"name":"again", "type":"retry", "retryAfter":0, "retryLimit":1,
                "stepId":"keep"}],
            "onSuccess":[{"name":"loop", "type":"goto", "stepId":"send",
                "criteria":[{"condition":"$statusCode == 201"}]}],
            "outputs":{"sent":"$request.body"}
        }),
    );
    let main = json!({"workflowId":"main", "steps":[keep(), send],
        "outputs":{"photo":"$steps.keep.outputs.photo", "sent":"$steps.send.outputs.sent"}});
    let result = run(&server, json!([main])).await;

    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(result.output_blobs.get("photo"), Some(&photo_blob()));
    assert_eq!(result.output_blobs.get("sent"), Some(&photo_blob()));
    let seen = server.take();
    assert_eq!(
        seen.iter()
            .map(|seen| seen.line.as_str())
            .collect::<Vec<_>>(),
        [NEXT, UPLOAD, NEXT, UPLOAD, UPLOAD]
    );
    assert_eq!(uploads(&seen), [JPEG.as_slice(); 3]);
}

#[tokio::test]
async fn a_goto_workflow_inherits_the_inputs_and_returns_its_blob_output() {
    let server = ScriptedServer::start(&[]);
    let main = json!({"workflowId":"main", "steps":[{"stepId":"first", "operationId":"next",
        "onSuccess":[{"name":"hand-off", "type":"goto", "workflowId":"sender"}]}]});
    let sender = json!({"workflowId":"sender",
        "steps":[upload("send", "$inputs.photo")],
        "outputs":{"photo":"$inputs.photo"}});
    let result = run(&server, json!([main, sender])).await;

    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    assert_eq!(result.output_blobs.get("photo"), Some(&photo_blob()));
    assert_eq!(uploads(&server.take()), [JPEG.as_slice()]);
}

#[tokio::test]
async fn a_pointer_or_interpolation_into_a_blob_output_is_unavailable() {
    let pointer = format!("{STEP_PHOTO} is bytes, which a JSON Pointer cannot read");
    let whole = format!(
        "{STEP_PHOTO} is bytes, which only a whole request body payload or a whole output can carry"
    );
    let cases = [
        (
            "step output through a pointer",
            json!({"steps":[keep(), {"stepId":"read", "operationId":"next",
                "outputs":{"exif":"$steps.keep.outputs.photo#/exif"}}]}),
            format!("step read: output \"exif\": $steps.keep.outputs.photo#/exif: {pointer}"),
            vec![NEXT, NEXT],
        ),
        (
            "workflow output interpolation",
            json!({"steps":[keep()],
                "outputs":{"label":"photo {$steps.keep.outputs.photo}"}}),
            format!("workflow \"main\": output \"label\": $steps.keep.outputs.photo: {whole}"),
            vec![NEXT],
        ),
        (
            "workflow output through a pointer",
            json!({"steps":[keep()], "outputs":{"photo":"$steps.keep.outputs.photo",
                "tags":"$outputs.photo#/tags"}}),
            "workflow \"main\": output \"tags\": $outputs.photo#/tags: output \"photo\" is bytes, which a JSON Pointer cannot read".to_owned(),
            vec![NEXT],
        ),
        (
            "header parameter",
            json!({"steps":[keep(), {"stepId":"read", "operationId":"next",
                "parameters":[{"name":"X-Photo", "in":"header",
                    "value":"$steps.keep.outputs.photo"}]}]}),
            format!(
                "step read: step \"read\": parameters[0](header:X-Photo): $steps.keep.outputs.photo: {whole}; request not sent"
            ),
            vec![NEXT],
        ),
    ];
    for (site, fields, message, lines) in cases {
        let server = ScriptedServer::start(&[]);
        let main = with(json!({"workflowId":"main"}), fields);
        let error = refused(site, run(&server, json!([main])).await.outputs);
        assert_eq!(error.code(), UNAVAILABLE, "{site}: {error}");
        assert_eq!(error.message, message, "{site}");
        assert_eq!(server.take_lines(), lines, "{site}");
    }
}

#[tokio::test]
async fn a_blob_output_of_a_retry_reference_workflow_never_reads_as_null() {
    let server = ScriptedServer::start(&[503]);
    let main = json!({"workflowId":"main", "steps":[
        with(upload("send", "$inputs.photo"), json!({
            "successCriteria":[{"condition":"$statusCode == 200"}],
            "onFailure":[{"name":"recover", "type":"retry", "retryAfter":0, "retryLimit":1,
                "workflowId":"child"}]})),
        {"stepId":"read", "operationId":"next",
            "parameters":[{"name":"X-Photo", "in":"header",
                "value":"$workflows.child.outputs.photo"}]}
    ]});
    let child = json!({"workflowId":"child", "steps":[{"stepId":"only", "operationId":"next"}],
        "outputs":{"photo":"$inputs.photo"}});
    let error = refused(
        "$workflows output",
        run(&server, json!([main, child])).await.outputs,
    );
    assert_eq!(error.code(), UNAVAILABLE, "{error}");
    assert_eq!(
        error.message,
        "step read: step \"read\": parameters[0](header:X-Photo): $workflows.child.outputs.photo: workflow \"child\" output \"photo\" is bytes, which a $workflows reference cannot carry yet; request not sent"
    );
    assert_eq!(server.take_lines(), [UPLOAD, NEXT, UPLOAD]);
}
