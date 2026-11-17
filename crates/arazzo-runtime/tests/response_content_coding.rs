#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{EngineBuilder, ExecutionResult, TraceStepRecord};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const BOMB_DECODED_BYTES: usize = 1 << 20;
const BOMB_LIMIT: usize = 64 * 1024;

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(bytes)
        .unwrap_or_else(|error| panic!("gzip: {error}"));
    encoder
        .finish()
        .unwrap_or_else(|error| panic!("gzip: {error}"))
}

fn brotli(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut encoded, 4096, 11, 22);
        writer
            .write_all(bytes)
            .unwrap_or_else(|error| panic!("brotli: {error}"));
    }
    encoded
}

fn route(name: &str) -> (&'static str, Option<&'static str>, Vec<u8>) {
    match name {
        "gzip-json" => (
            "application/json",
            Some("gzip"),
            gzip(br#"{"greeting":"hello","n":[1,2]}"#),
        ),
        "br-text" => ("text/plain", Some("br"), brotli("caf\u{e9}".as_bytes())),
        "bomb" => (
            "application/json",
            Some("gzip"),
            gzip(&vec![b' '; BOMB_DECODED_BYTES]),
        ),
        "compress" => ("application/json", Some("compress"), b"\x1f\x9d{}".to_vec()),
        _ => ("application/json", None, br#"{"plain":true}"#.to_vec()),
    }
}

type Seen = Arc<Mutex<Vec<Option<String>>>>;

struct FixtureServer {
    base_url: String,
    accept_encoding: Seen,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let accept_encoding: Seen = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&accept_encoding), Arc::clone(&stop));
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(20)) else {
                    continue;
                };
                let accept = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Accept-Encoding"))
                    .map(|header| header.value.to_string());
                seen.lock()
                    .unwrap_or_else(|_| panic!("recording"))
                    .push(accept);
                let (media_type, coding, body) = route(request.url().trim_start_matches("/r/"));
                let mut response = TinyResponse::from_data(body).with_header(
                    Header::from_bytes("Content-Type", media_type)
                        .unwrap_or_else(|()| panic!("content type header")),
                );
                if let Some(coding) = coding {
                    response = response.with_header(
                        Header::from_bytes("Content-Encoding", coding)
                            .unwrap_or_else(|()| panic!("content encoding header")),
                    );
                }
                let _ = request.respond(response);
            }
        });
        Self {
            base_url,
            accept_encoding,
            stop,
            worker: Some(worker),
        }
    }

    fn accept_encodings(&self) -> Vec<Option<String>> {
        std::mem::take(
            &mut *self
                .accept_encoding
                .lock()
                .unwrap_or_else(|_| panic!("reading")),
        )
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn spec(name: &str, mut extra: Value) -> ArazzoSpec {
    let mut step = json!({"stepId":"fetch", "operationId":"fetch",
        "parameters":[{"name":"name", "in":"path", "value":name}]});
    let outputs = extra
        .as_object_mut()
        .and_then(|extra| extra.remove("workflowOutputs"))
        .unwrap_or_else(|| json!({}));
    for (key, value) in extra.as_object().into_iter().flatten() {
        step[key] = value.clone();
    }
    serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"content coding", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[{"workflowId":"main", "steps":[step], "outputs":outputs}]
    }))
    .unwrap_or_else(|error| panic!("spec: {error}"))
}

fn builder(server_url: &str, spec: ArazzoSpec) -> EngineBuilder {
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server_url}],
    "paths":{"/r/{name}":{"get":{"operationId":"fetch",
        "parameters":[{"name":"name", "in":"path", "required":true, "schema":{"type":"string"}}],
        "responses":{"200":{"description":"ok"}}}}}});
    EngineBuilder::new(spec).openapi_spec(openapi.to_string().into_bytes(), None)
}

async fn execute(server: &FixtureServer, name: &str, extra: Value) -> ExecutionResult {
    builder(&server.base_url, spec(name, extra))
        .trace(true)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", BTreeMap::new())
        .await
}

fn body_outputs() -> Value {
    json!({"outputs":{"body":"$response.body"},
        "workflowOutputs":{"body":"$steps.fetch.outputs.body"}})
}

#[tokio::test]
async fn gzip_and_brotli_bodies_reach_the_media_type_table_decoded() {
    let server = FixtureServer::start();
    for (name, expected) in [
        ("gzip-json", json!({"greeting":"hello","n":[1,2]})),
        ("br-text", json!("caf\u{e9}")),
    ] {
        let outputs = execute(&server, name, body_outputs())
            .await
            .outputs
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(outputs["body"], expected, "{name}");
    }
}

#[tokio::test]
async fn the_response_cap_counts_decoded_bytes() {
    let server = FixtureServer::start();
    let (_, _, wire) = route("bomb");
    assert!(wire.len() < BOMB_LIMIT / 16, "fixture is small on the wire");
    let error = builder(&server.base_url, spec("bomb", body_outputs()))
        .max_response_bytes(BOMB_LIMIT)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", BTreeMap::new())
        .await
        .outputs
        .err()
        .unwrap_or_else(|| panic!("a gzip body decoding past the cap must fail"));
    assert_eq!(error.code(), "RUNTIME_RESPONSE_TOO_LARGE", "{error}");
}

#[tokio::test]
async fn an_undecoded_coding_is_unavailable_only_when_the_body_is_referenced() {
    let server = FixtureServer::start();
    let error = execute(&server, "compress", body_outputs())
        .await
        .outputs
        .err()
        .unwrap_or_else(|| panic!("referencing a compress body must fail"));
    assert_eq!(error.code(), "RUNTIME_VALUE_UNAVAILABLE", "{error}");
    assert_eq!(
        error.message,
        "step fetch: output \"body\": $response.body: response Content-Encoding \"compress\" is not a coding the runtime decodes, so its body is bytes, which is not yet a readable value"
    );
    let outputs = execute(
        &server,
        "compress",
        json!({"successCriteria":[{"condition":"$statusCode == 200"}],
            "outputs":{"status":"$statusCode", "coding":"$response.header.Content-Encoding"},
            "workflowOutputs":{"status":"$steps.fetch.outputs.status",
                "coding":"$steps.fetch.outputs.coding"}}),
    )
    .await
    .outputs
    .unwrap_or_else(|error| panic!("status criteria must pass: {error}"));
    assert_eq!(outputs["status"], json!(200));
    assert_eq!(outputs["coding"], json!("compress"));
}

#[tokio::test]
async fn decoded_responses_hide_their_coding_and_length_headers() {
    let server = FixtureServer::start();
    let headers = json!({"outputs":{
            "type":"$response.header.Content-Type",
            "coding":"$response.header.Content-Encoding",
            "length":"$response.header.Content-Length"},
        "workflowOutputs":{
            "type":"$steps.fetch.outputs.type",
            "coding":"$steps.fetch.outputs.coding",
            "length":"$steps.fetch.outputs.length"}});
    let decoded = execute(&server, "gzip-json", headers.clone())
        .await
        .outputs
        .unwrap_or_else(|error| panic!("gzip-json: {error}"));
    assert_eq!(
        decoded,
        BTreeMap::from([
            ("type".to_owned(), json!("application/json")),
            ("coding".to_owned(), Value::Null),
            ("length".to_owned(), Value::Null),
        ])
    );
    let plain = execute(&server, "plain", headers)
        .await
        .outputs
        .unwrap_or_else(|error| panic!("plain: {error}"));
    assert_eq!(plain["length"], json!("14"));
    assert_eq!(plain["coding"], Value::Null);
}

#[tokio::test]
async fn the_transport_offers_gzip_and_br_but_traces_only_logical_headers() {
    let server = FixtureServer::start();
    let result = execute(&server, "gzip-json", body_outputs()).await;
    assert_eq!(server.accept_encodings(), vec![Some("gzip,br".to_owned())]);
    let traces = result.trace_steps();
    let request = traces[0]
        .request
        .as_ref()
        .unwrap_or_else(|| panic!("traced request"));
    assert!(
        !request
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("accept-encoding")),
        "{:?}",
        request.headers
    );
    let response = traces[0]
        .response
        .as_ref()
        .unwrap_or_else(|| panic!("traced response"));
    assert!(
        !response
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("content-encoding")),
        "{:?}",
        response.headers
    );
}

#[tokio::test]
async fn a_gzip_run_replays_from_its_trace() {
    let server = FixtureServer::start();
    let mut recorded: Vec<TraceStepRecord> = Vec::new();
    let names = ["gzip-json", "compress"];
    for name in names {
        let live = execute(&server, name, body_outputs()).await;
        recorded.extend(live.trace_steps().into_iter().cloned());
    }
    assert_eq!(server.accept_encodings().len(), names.len());
    let expected = [
        Ok(json!({"greeting":"hello","n":[1,2]})),
        Err("response Content-Encoding \"compress\" is not a coding the runtime decodes, so its body is bytes, which is not yet a readable value"),
    ];
    for ((name, record), expected) in names.into_iter().zip(recorded).zip(expected) {
        let replayed = builder(&server.base_url, spec(name, body_outputs()))
            .trace(true)
            .replay_trace_steps(vec![record])
            .build()
            .unwrap_or_else(|error| panic!("replay engine: {error}"))
            .execute_collect("main", BTreeMap::new())
            .await
            .outputs;
        assert!(
            server.accept_encodings().is_empty(),
            "{name}: replay is offline"
        );
        match (replayed, expected) {
            (Ok(outputs), Ok(body)) => assert_eq!(outputs["body"], body, "{name}"),
            (Err(error), Err(reason)) => {
                assert!(error.message.ends_with(reason), "{name}: {error}");
            }
            (other, expected) => panic!("{name}: replay mismatch: {other:?} vs {expected:?}"),
        }
    }
}
