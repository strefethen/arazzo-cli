#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use arazzo_runtime::{EngineBuilder, RuntimeError};
use arazzo_spec::ArazzoSpec;
use serde_json::{json, Value};
use tiny_http::{Header, Response as TinyResponse, Server};

const SOURCE: &str = "https://fixture.invalid/openapi.json";
const UNAVAILABLE: &str = "RUNTIME_VALUE_UNAVAILABLE";

type Recorded = Arc<Mutex<Vec<(String, Option<String>, Vec<u8>)>>>;

fn route(name: &str) -> (u16, Option<&'static str>, Vec<u8>) {
    let latin_xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><p>caf\xE9</p>".to_vec();
    match name {
        "json" => (200, Some("application/json"), br#"{"a":[1]}"#.to_vec()),
        "json-bom-charset" => (
            200,
            Some("application/json; charset=iso-8859-1"),
            "\u{feff}{\"name\":\"caf\u{e9}\"}".as_bytes().to_vec(),
        ),
        "problem" => (
            400,
            Some("Application/Problem+JSON"),
            br#"{"title":"bad"}"#.to_vec(),
        ),
        "text-number" => (200, Some("text/plain"), b"123".to_vec()),
        "latin1" => (
            200,
            Some("text/plain; charset=iso-8859-1"),
            b"caf\xE9".to_vec(),
        ),
        "form" => (
            200,
            Some("application/x-www-form-urlencoded"),
            b"a=1&b=2".to_vec(),
        ),
        "charset-other" => (
            200,
            Some("application/javascript; charset=\"utf-8\""),
            b"x()".to_vec(),
        ),
        "xml-decl" => (200, Some("application/xml"), latin_xml),
        "xml-charset" => (
            200,
            Some("text/xml; charset=windows-1252"),
            b"<p>\x80</p>".to_vec(),
        ),
        "xml-bom" => (
            200,
            Some("application/atom+xml"),
            b"\xFF\xFE<\0a\0/\0>\0".to_vec(),
        ),
        "xml-utf8" => (
            200,
            Some("application/rss+xml"),
            "<r>\u{e9}</r>".as_bytes().to_vec(),
        ),
        "empty" => (200, Some("application/json"), Vec::new()),
        "no-content" => (204, Some("application/json"), Vec::new()),
        "bad-json" => (200, Some("application/json"), b"{".to_vec()),
        "unknown-charset" => (200, Some("text/plain; charset=klingon"), b"x".to_vec()),
        "invalid-bytes" => (200, Some("text/plain; charset=utf-8"), b"caf\xE9".to_vec()),
        "bytes" => (200, Some("image/png"), b"\x89PNG\r\n".to_vec()),
        "no-type" => (200, None, br#"{"a":1}"#.to_vec()),
        "bad-type" => (200, Some("json"), br#"{"a":1}"#.to_vec()),
        _ => (200, Some("application/json"), b"{}".to_vec()),
    }
}

struct FixtureServer {
    base_url: String,
    recorded: Recorded,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|error| panic!("server: {error}"));
        let base_url = format!("http://{}", server.server_addr());
        let recorded: Recorded = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (Arc::clone(&recorded), Arc::clone(&stop));
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
                let url = request.url().to_owned();
                seen.lock().unwrap_or_else(|_| panic!("recording")).push((
                    url.clone(),
                    content_type,
                    body,
                ));
                let (status, media_type, body) = route(url.trim_start_matches("/r/"));
                let mut response = TinyResponse::from_data(body).with_status_code(status);
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
            recorded,
            stop,
            worker: Some(worker),
        }
    }

    fn requests(&self) -> Vec<(String, Option<String>, Vec<u8>)> {
        std::mem::take(&mut *self.recorded.lock().unwrap_or_else(|_| panic!("reading")))
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

async fn run(
    server: &FixtureServer,
    mut step: Value,
    outputs: Value,
) -> Result<BTreeMap<String, Value>, RuntimeError> {
    step["stepId"] = json!("fetch");
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"decoding", "version":"1"},
        "sourceDescriptions":[{"name":"api", "url":SOURCE, "type":"openapi"}],
        "workflows":[{"workflowId":"main", "steps":[step], "outputs":outputs}]
    }))
    .unwrap_or_else(|error| panic!("spec: {error}"));
    let openapi = json!({"openapi":"3.2.0", "$self":SOURCE,
    "info":{"title":"fixture", "version":"1"}, "servers":[{"url":server.base_url}],
    "paths":{
        "/r/{name}":{"get":{"operationId":"fetch",
            "parameters":[{"name":"name", "in":"path", "required":true, "schema":{"type":"string"}}],
            "responses":{"200":{"description":"ok"}}}},
        "/echo":{"post":{"operationId":"echo", "responses":{"200":{"description":"ok"}}}}
    }});
    EngineBuilder::new(spec)
        .openapi_spec(openapi.to_string().into_bytes(), None)
        .build()
        .unwrap_or_else(|error| panic!("engine: {error}"))
        .execute_collect("main", BTreeMap::new())
        .await
        .outputs
}

fn fetch(name: &str, extra: Value) -> Value {
    let mut step = json!({"operationId":"fetch",
        "parameters":[{"name":"name", "in":"path", "value":name}]});
    for (key, value) in extra.as_object().into_iter().flatten() {
        step[key] = value.clone();
    }
    step
}

async fn body_of(server: &FixtureServer, name: &str) -> Result<Value, RuntimeError> {
    run(
        server,
        fetch(name, json!({"outputs":{"body":"$response.body"}})),
        json!({"body":"$steps.fetch.outputs.body"}),
    )
    .await
    .map(|mut outputs| outputs.remove("body").unwrap_or(Value::Null))
}

#[tokio::test]
async fn every_decoding_row_produces_the_stated_body() {
    let server = FixtureServer::start();
    let cases = [
        ("json", json!({"a":[1]})),
        ("json-bom-charset", json!({"name":"caf\u{e9}"})),
        ("problem", json!({"title":"bad"})),
        ("text-number", json!("123")),
        ("latin1", json!("caf\u{e9}")),
        ("form", json!("a=1&b=2")),
        ("charset-other", json!("x()")),
        (
            "xml-decl",
            json!("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><p>caf\u{e9}</p>"),
        ),
        ("xml-charset", json!("<p>\u{20ac}</p>")),
        ("xml-bom", json!("<a/>")),
        ("xml-utf8", json!("<r>\u{e9}</r>")),
        ("empty", Value::Null),
        ("no-content", Value::Null),
    ];
    for (name, expected) in cases {
        let body = body_of(&server, name)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(body, expected, "{name}");
    }
}

#[tokio::test]
async fn failing_rows_are_unavailable_only_when_the_body_is_referenced() {
    let server = FixtureServer::start();
    let cases = [
        (
            "bad-json",
            "response body is not valid JSON (application/json): EOF while parsing an object at line 1 column 1",
        ),
        (
            "unknown-charset",
            "response charset \"klingon\" is not a supported encoding label",
        ),
        ("invalid-bytes", "response body is not valid UTF-8 text"),
        (
            "bytes",
            "response body is bytes (image/png), which is not yet a readable value",
        ),
        (
            "no-type",
            "response has no Content-Type header, so its body is bytes, which is not yet a readable value",
        ),
        (
            "bad-type",
            "response Content-Type \"json\" is not a valid media type, so its body is bytes, which is not yet a readable value",
        ),
    ];
    for (name, reason) in cases {
        let error = match body_of(&server, name).await {
            Ok(body) => panic!("{name}: referencing the body must fail, got {body}"),
            Err(error) => error,
        };
        assert_eq!(error.code(), UNAVAILABLE, "{name}: {error}");
        assert_eq!(
            error.message,
            format!("step fetch: output \"body\": $response.body: {reason}"),
            "{name}"
        );
        let pointer = run(
            &server,
            fetch(name, json!({"outputs":{"a":"$response.body#/a"}})),
            json!({}),
        )
        .await;
        assert!(
            pointer.is_err_and(|error| error.code() == UNAVAILABLE),
            "{name}: pointer"
        );
        let status = run(
            &server,
            fetch(
                name,
                json!({"successCriteria":[{"condition":"$statusCode == 200"}],
                    "outputs":{"status":"$statusCode"}}),
            ),
            json!({"status":"$steps.fetch.outputs.status"}),
        )
        .await
        .unwrap_or_else(|error| panic!("{name}: status criteria must pass: {error}"));
        assert_eq!(status["status"], json!(200), "{name}");
    }
}

#[tokio::test]
async fn default_and_explicit_xpath_contexts_see_the_same_decoded_text() {
    let server = FixtureServer::start();
    let xpath = json!({"type":"xpath", "version":"xpath-10"});
    for name in ["xml-decl", "xml-charset"] {
        let expected = if name == "xml-decl" {
            "caf\u{e9}"
        } else {
            "\u{20ac}"
        };
        let condition = format!("//p = '{expected}'");
        let outputs = run(
            &server,
            fetch(
                name,
                json!({"successCriteria":[
                        {"context":"$response.body", "condition":condition, "type":xpath},
                        {"condition":condition, "type":xpath}
                    ],
                    "outputs":{"text":"//p"}}),
            ),
            json!({"text":"$steps.fetch.outputs.text"}),
        )
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(outputs["text"], json!(expected), "{name}");
    }
    for name in ["bytes", "invalid-bytes"] {
        for context in [Some("$response.body"), None] {
            let mut criterion = json!({"condition":"count(//p) >= 0", "type":xpath});
            if let Some(context) = context {
                criterion["context"] = json!(context);
            }
            let result = run(
                &server,
                fetch(name, json!({"successCriteria":[criterion]})),
                json!({}),
            )
            .await;
            assert!(
                result.is_err_and(|error| error.code() == UNAVAILABLE),
                "{name} {context:?}"
            );
        }
    }
}

#[tokio::test]
async fn request_serialization_classifies_json_and_form_through_the_parser() {
    let server = FixtureServer::start();
    let cases = [
        (
            json!({"contentType":"application/json", "payload":{"a":1}}),
            "application/json",
            r#"{"a":1}"#,
        ),
        (
            json!({"contentType":"application/vnd.api+json", "payload":{"data":"text"}}),
            "application/vnd.api+json",
            r#"{"data":"text"}"#,
        ),
        (
            json!({"contentType":"Application/X-WWW-Form-Urlencoded; charset=UTF-8",
                "payload":{"a":"b c"}}),
            "Application/X-WWW-Form-Urlencoded; charset=UTF-8",
            "a=b+c",
        ),
        (
            json!({"contentType":"text/plain", "payload":"hello"}),
            "text/plain",
            "hello",
        ),
        (
            json!({"contentType":"application/json-patch", "payload":"raw"}),
            "application/json-patch",
            "raw",
        ),
    ];
    for (request_body, content_type, wire) in cases {
        run(
            &server,
            json!({"operationId":"echo", "requestBody":request_body}),
            json!({}),
        )
        .await
        .unwrap_or_else(|error| panic!("{content_type}: {error}"));
        let requests = server.requests();
        assert_eq!(requests.len(), 1, "{content_type}");
        let (url, sent_type, body) = &requests[0];
        assert_eq!(url, "/echo");
        assert_eq!(sent_type.as_deref(), Some(content_type));
        assert_eq!(String::from_utf8_lossy(body), wire, "{content_type}");
    }
}
