use arazzo_runtime::{ClientConfig, Engine, EngineBuilder, TraceDecisionPath};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tiny_http::Server;

#[derive(Clone, Debug, PartialEq)]
struct Attempt {
    method: String,
    path: String,
    key: Option<String>,
    payload: String,
}

#[derive(Default, Debug)]
struct Ledger {
    attempts: Vec<Attempt>,
    operations: BTreeMap<String, Value>,
    keyed_results: BTreeMap<String, (String, Value)>,
    reservations: Vec<String>,
    released: Vec<String>,
}

struct RecoveryApi {
    base: String,
    ledger: Arc<Mutex<Ledger>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
enum LookupReply {
    Found,
    Missing,
    Unavailable,
    MissingId,
}

impl RecoveryApi {
    fn start(lookup_reply: LookupReply, cleanup_status: u16) -> Self {
        let server = Server::http("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
        let base = format!("http://{}", server.server_addr());
        let ledger = Arc::new(Mutex::new(Ledger::default()));
        let server_ledger = ledger.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Relaxed) {
                let Some(mut request) = server
                    .recv_timeout(Duration::from_millis(20))
                    .unwrap_or_else(|e| panic!("receive: {e}"))
                else {
                    continue;
                };
                let mut payload = String::new();
                request
                    .as_reader()
                    .read_to_string(&mut payload)
                    .unwrap_or_else(|e| panic!("read: {e}"));
                let key = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Idempotency-Key"))
                    .map(|h| h.value.as_str().to_string());
                let attempt = Attempt {
                    method: request.method().to_string(),
                    path: request.url().to_string(),
                    key,
                    payload,
                };
                let response = {
                    let mut ledger = server_ledger
                        .lock()
                        .unwrap_or_else(|e| panic!("ledger: {e}"));
                    ledger.attempts.push(attempt.clone());
                    match (attempt.method.as_str(), attempt.path.as_str()) {
                        ("POST", "/operations") => {
                            if let Some((prior_payload, result)) = attempt
                                .key
                                .as_ref()
                                .and_then(|key| ledger.keyed_results.get(key))
                            {
                                if prior_payload == &attempt.payload {
                                    Some((201, result.clone()))
                                } else {
                                    Some((
                                        409,
                                        json!({"error":"key reused with different payload"}),
                                    ))
                                }
                            } else {
                                let body: Value = serde_json::from_str(&attempt.payload)
                                    .unwrap_or_else(|e| panic!("payload: {e}"));
                                let business_key = body["businessKey"]
                                    .as_str()
                                    .unwrap_or_else(|| panic!("business key required"))
                                    .to_string();
                                let result = json!({"id":"operation-1"});
                                assert!(ledger
                                    .operations
                                    .insert(business_key, result.clone())
                                    .is_none());
                                if let Some(key) = &attempt.key {
                                    ledger
                                        .keyed_results
                                        .insert(key.clone(), (attempt.payload.clone(), result));
                                }
                                None
                            }
                        }
                        ("GET", path) if path.starts_with("/operations?") => {
                            let url = url::Url::parse(&format!("http://local{path}"))
                                .unwrap_or_else(|e| panic!("lookup url: {e}"));
                            let business_key = url
                                .query_pairs()
                                .find(|(name, _)| name == "businessKey")
                                .map(|(_, value)| value.into_owned())
                                .unwrap_or_else(|| panic!("lookup key required"));
                            Some(match lookup_reply {
                                LookupReply::Found => (
                                    200,
                                    ledger
                                        .operations
                                        .get(&business_key)
                                        .cloned()
                                        .unwrap_or_else(|| json!({})),
                                ),
                                LookupReply::Missing => (404, json!({"error":"not found"})),
                                LookupReply::Unavailable => {
                                    (503, json!({"error":"lookup unavailable"}))
                                }
                                LookupReply::MissingId => (200, json!({})),
                            })
                        }
                        ("POST", "/reservations") => {
                            ledger.reservations.push("reservation-1".to_string());
                            Some((201, json!({"id":"reservation-1"})))
                        }
                        ("POST", "/charges") => Some((402, json!({"error":"payment rejected"}))),
                        ("DELETE", "/reservations/reservation-1") => {
                            if cleanup_status == 204 {
                                assert_eq!(ledger.reservations, ["reservation-1"]);
                                ledger.reservations.clear();
                                ledger.released.push("reservation-1".to_string());
                            }
                            Some((cleanup_status, json!({})))
                        }
                        _ => panic!("unexpected request: {attempt:?}"),
                    }
                };
                match response {
                    None => drop(request.into_writer()),
                    Some((status, body)) => {
                        let body = if status == 204 {
                            String::new()
                        } else {
                            body.to_string()
                        };
                        let mut writer = request.into_writer();
                        write!(writer, "HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                            .unwrap_or_else(|e| panic!("respond: {e}"));
                        writer.flush().unwrap_or_else(|e| panic!("flush: {e}"));
                    }
                }
            }
        });
        Self {
            base,
            ledger,
            stop,
            thread: Some(thread),
        }
    }

    fn engine(&self, fixture: &str) -> Engine {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/recovery");
        let spec = arazzo_validate::parse(directory.join(fixture))
            .unwrap_or_else(|e| panic!("parse fixture: {e}"));
        let mut openapi: Value = serde_yaml_ng::from_str(include_str!(
            "../../../examples/recovery/recovery.openapi.yaml"
        ))
        .unwrap_or_else(|e| panic!("openapi: {e}"));
        openapi["servers"][0]["url"] = json!(self.base);
        let mut config = ClientConfig::default();
        config
            .default_headers
            .insert("Connection".to_string(), "close".to_string());
        EngineBuilder::new(spec)
            .client_config(config)
            .trace(true)
            .source_base_dir(&directory)
            .openapi_spec(
                openapi.to_string().into_bytes(),
                Some(directory.join("recovery.openapi.yaml")),
            )
            .build()
            .unwrap_or_else(|e| panic!("build: {e}"))
    }
}

impl Drop for RecoveryApi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "API server panicked");
            }
        }
    }
}

#[tokio::test]
async fn idempotent_retry_after_lost_response_has_two_attempts_and_one_effect() {
    let api = RecoveryApi::start(LookupReply::Found, 204);
    let result = api
        .engine("idempotent-retry.arazzo.yaml")
        .execute_collect(
            "createOnce",
            BTreeMap::from([
                ("key".to_string(), json!("stable-key")),
                ("businessKey".to_string(), json!("order/42")),
            ]),
        )
        .await;
    let records = result.trace_steps();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].decision.path, TraceDecisionPath::Retry);
    assert!(records[0].response.is_none());
    assert!(records[0].error.is_some());
    assert_eq!(records[1].attempt, 2);
    let outputs = result.outputs.unwrap_or_else(|e| panic!("retry: {e}"));
    assert_eq!(outputs.get("id"), Some(&json!("operation-1")));
    let ledger = api.ledger.lock().unwrap_or_else(|e| panic!("ledger: {e}"));
    assert_eq!(ledger.attempts.len(), 2);
    assert_eq!(ledger.attempts[0], ledger.attempts[1]);
    assert_eq!(ledger.attempts[0].key.as_deref(), Some("stable-key"));
    assert_eq!(
        serde_json::from_str::<Value>(&ledger.attempts[0].payload)
            .unwrap_or_else(|e| panic!("payload: {e}")),
        json!({"businessKey":"order/42"})
    );
    assert_eq!(ledger.operations.len(), 1);
    assert_eq!(ledger.keyed_results.len(), 1);
}

#[tokio::test]
async fn reconciliation_after_lost_response_never_resubmits_even_when_lookup_fails() {
    for lookup_reply in [
        LookupReply::Found,
        LookupReply::Missing,
        LookupReply::Unavailable,
        LookupReply::MissingId,
    ] {
        let api = RecoveryApi::start(lookup_reply, 204);
        let result = api
            .engine("reconcile.arazzo.yaml")
            .execute_collect(
                "submit",
                BTreeMap::from([("businessKey".to_string(), json!("order/42"))]),
            )
            .await;
        let records = result.trace_steps();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].decision.path, TraceDecisionPath::GotoWorkflow);
        assert_eq!(records[0].decision.target_workflow_id, "reconcile");
        assert!(records[0].response.is_none());
        assert!(records[0].error.is_some());
        if matches!(lookup_reply, LookupReply::Found) {
            let outputs = result.outputs.unwrap_or_else(|e| panic!("reconcile: {e}"));
            assert_eq!(outputs.get("id"), Some(&json!("operation-1")));
            assert_eq!(outputs.get("lookupStatus"), Some(&json!(200)));
        } else {
            assert!(result.outputs.is_err());
        }
        let ledger = api.ledger.lock().unwrap_or_else(|e| panic!("ledger: {e}"));
        assert_eq!(ledger.operations.len(), 1);
        assert_eq!(ledger.attempts.len(), 2);
        assert_eq!(ledger.attempts[0].method, "POST");
        assert_eq!(ledger.attempts[0].path, "/operations");
        assert_eq!(ledger.attempts[0].key, None);
        assert_eq!(ledger.attempts[1].method, "GET");
        let lookup = url::Url::parse(&format!("http://local{}", ledger.attempts[1].path))
            .unwrap_or_else(|e| panic!("url: {e}"));
        assert_eq!(lookup.path(), "/operations");
        assert_eq!(
            lookup.query_pairs().collect::<Vec<_>>(),
            [("businessKey".into(), "order/42".into())]
        );
    }
}

#[tokio::test]
async fn compensation_explicitly_releases_prior_reservation_or_reports_cleanup_failure() {
    for status in [204, 503] {
        let api = RecoveryApi::start(LookupReply::Found, status);
        let result = api
            .engine("compensate.arazzo.yaml")
            .execute_collect("purchase", BTreeMap::new())
            .await;
        if status == 204 {
            let outputs = result.outputs.unwrap_or_else(|e| panic!("compensate: {e}"));
            assert_eq!(outputs.get("cleanupStatus"), Some(&json!(204)));
            assert_eq!(outputs.get("reservationId"), Some(&json!("reservation-1")));
        } else {
            assert!(result.outputs.is_err());
        }
        let ledger = api.ledger.lock().unwrap_or_else(|e| panic!("ledger: {e}"));
        assert_eq!(
            ledger
                .attempts
                .iter()
                .map(|a| (a.method.as_str(), a.path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("POST", "/reservations"),
                ("POST", "/charges"),
                ("DELETE", "/reservations/reservation-1")
            ]
        );
        if status == 204 {
            assert!(ledger.reservations.is_empty());
            assert_eq!(ledger.released, ["reservation-1"]);
        } else {
            assert_eq!(ledger.reservations, ["reservation-1"]);
            assert!(ledger.released.is_empty());
        }
    }
}
