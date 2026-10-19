//! Real-file integration proof for the private sequential exchange fixture.
#![allow(clippy::expect_used)] // Fixture construction and expected failures use labeled assertions.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arazzo_spec::{parse_unvalidated_bytes, StepAction};

use super::controlled_exchange::{Exchange, Route};
use super::step_context::MessageSnapshot;
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-controlled-exchange-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_asyncapi(&self, extra_channel: bool) -> Result<(), Box<dyn std::error::Error>> {
        let extra = if extra_channel {
            "  alien:\n    address: /orders/alien\n    messages: {}\n"
        } else {
            ""
        };
        fs::write(
            self.0.join("events.yaml"),
            format!(
                "asyncapi: 3.0.0\ninfo:\n  title: Controlled exchange\n  version: '1'\nchannels:\n  request:\n    address: /orders/submit\n    messages: {{}}\n  reply:\n    address: /orders/replies\n    messages: {{}}\n  forward:\n    address: /orders/forward\n    messages: {{}}\n{extra}"
            ),
        )?;
        Ok(())
    }

    fn parse_spec(&self, spec: &Value) -> Result<ArazzoSpec, Box<dyn std::error::Error>> {
        let path = self.0.join("exchange.arazzo.json");
        fs::write(&path, serde_json::to_vec_pretty(spec)?)?;
        Ok(parse_unvalidated_bytes(&fs::read(path)?)?)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove isolated fixture directory");
    }
}

fn target(key: &str) -> String {
    format!("{{$sourceDescriptions.events.url}}#/channels/{key}")
}

fn spec() -> Value {
    json!({
        "arazzo": "1.1.0",
        "info": {"title": "controlled exchange", "version": "1"},
        "sourceDescriptions": [{"name":"events", "url":"events.yaml", "type":"asyncapi"}],
        "workflows": [{
            "workflowId": "exchange",
            "steps": [
                {"stepId":"send", "channelPath":target("request"), "action":"send"},
                {"stepId":"receive", "channelPath":target("reply"), "action":"receive",
                 "dependsOn":["send"],
                 "successCriteria":[{"condition":"$message.payload#/accepted == true"}],
                 "outputs":{"id":"$message.payload#/id", "tag":"$message.header.x-tag"}},
                {"stepId":"forward", "channelPath":target("forward"), "action":"send",
                 "dependsOn":["receive"]}
            ],
            "outputs":{"id":"$steps.receive.outputs.id", "tag":"$steps.receive.outputs.tag"}
        }]
    })
}

fn reply(accepted: bool) -> MessageSnapshot {
    MessageSnapshot {
        headers: BTreeMap::from([("x-tag".to_string(), "reply".to_string())]),
        payload: Some(json!({"id":42,"accepted":accepted})),
    }
}

fn inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([("order".to_string(), json!(7))])
}

fn attach(builder: EngineBuilder, exchange: &Arc<Exchange>) -> Result<Engine, RuntimeError> {
    let mut engine = builder.build()?;
    Arc::get_mut(&mut engine.inner)
        .expect("fresh Engine has sole ownership")
        .controlled_exchange = Some(Arc::clone(exchange));
    Ok(engine)
}

fn fixture(
    dir: &FixtureDir,
    doc: &Value,
    response: Option<MessageSnapshot>,
    fail_send: bool,
    wait_limit: Duration,
) -> Result<(Engine, Arc<Exchange>), Box<dyn std::error::Error>> {
    let spec = dir.parse_spec(doc)?;
    let exchange = Exchange::new(dir.path(), response, fail_send, wait_limit);
    let engine = attach(
        EngineBuilder::new(spec).source_base_dir(dir.path()),
        &exchange,
    )?;
    Ok((engine, exchange))
}

async fn collect(engine: &Engine) -> Result<ExecutionResult, tokio::time::error::Elapsed> {
    tokio::time::timeout(
        Duration::from_secs(3),
        engine.execute_collect("exchange", inputs()),
    )
    .await
}

fn routes(dir: &FixtureDir) -> Vec<Route> {
    let document = dir.path().join("events.yaml").display().to_string();
    [
        ("request", "/orders/submit", StepAction::Send),
        ("reply", "/orders/replies", StepAction::Receive),
        ("forward", "/orders/forward", StepAction::Send),
    ]
    .into_iter()
    .map(|(key, address, action)| Route {
        source: "events".to_string(),
        document: document.clone(),
        channel_key: key.to_string(),
        channel_pointer: format!("/channels/{key}"),
        address: Some(address.to_string()),
        action,
    })
    .collect()
}

#[tokio::test]
async fn controlled_exchange_immediate_reply_and_typed_chaining() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let doc = spec();
    assert!(doc["workflows"][0]["steps"]
        .as_array()
        .expect("steps array")
        .iter()
        .all(|step| step.get("requestBody").is_none()));
    let (engine, exchange) = fixture(&dir, &doc, Some(reply(true)), false, Duration::from_secs(1))?;
    let result = collect(&engine).await?;
    assert_eq!(
        result.outputs?,
        BTreeMap::from([("id".into(), json!(42)), ("tag".into(), json!("reply"))])
    );
    assert_eq!(
        *exchange.sent.lock().expect("fixture lock"),
        vec![json!({"order":7}), json!({"id":42})]
    );
    assert_eq!(*exchange.routes.lock().expect("fixture lock"), routes(&dir));
    assert_eq!(exchange.receive_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.active_waits.load(Ordering::SeqCst), 0);
    assert!(exchange.inbox_rx.try_lock().is_ok());
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_paired_success_actions_end_before_forward() -> TestResult {
    for condition in [
        "$message.payload#/id == 42",
        "$steps.receive.outputs.id == 42",
    ] {
        let dir = FixtureDir::new();
        dir.write_asyncapi(false)?;
        let mut doc = spec();
        doc["workflows"][0]["steps"][1]["onSuccess"] = json!([{
            "name":"end_on_id", "type":"end", "criteria":[{"condition":condition}]
        }]);
        let (engine, exchange) =
            fixture(&dir, &doc, Some(reply(true)), false, Duration::from_secs(1))?;
        let result = collect(&engine).await?;
        assert_eq!(result.outputs?["id"], json!(42), "{condition}");
        assert_eq!(
            *exchange.sent.lock().expect("fixture lock"),
            vec![json!({"order":7})],
            "{condition}"
        );
        assert_eq!(
            *exchange.routes.lock().expect("fixture lock"),
            routes(&dir)[..2],
            "{condition}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_failed_criterion_keeps_message_for_failure_action() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let mut doc = spec();
    doc["workflows"][0]["steps"][1]["onFailure"] = json!([{
        "name":"end_on_rejection", "type":"end",
        "criteria":[{"condition":"$message.payload#/accepted == false"}]
    }]);
    let (engine, exchange) = fixture(
        &dir,
        &doc,
        Some(reply(false)),
        false,
        Duration::from_secs(1),
    )?;
    let err = collect(&engine)
        .await?
        .outputs
        .expect_err("criterion must fail");
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed);
    assert!(err.message.contains("workflow ended by onFailure action"));
    assert_eq!(
        *exchange.sent.lock().expect("fixture lock"),
        vec![json!({"order":7})]
    );
    assert_eq!(
        *exchange.routes.lock().expect("fixture lock"),
        routes(&dir)[..2]
    );
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_resolver_failures_precede_fixture_io() -> TestResult {
    for (case, expected) in [
        ("invalid_target", "EXCHANGE_RESOLVE_InvalidTarget"),
        ("missing_source", "EXCHANGE_RESOLVE_MissingSource"),
        ("missing_document", "EXCHANGE_RESOLVE_MissingFile"),
        ("missing_channel", "EXCHANGE_RESOLVE_MissingChannel"),
        ("unexpected_channel", "EXCHANGE_UNEXPECTED_CHANNEL"),
    ] {
        let dir = FixtureDir::new();
        dir.write_asyncapi(true)?;
        let mut doc = spec();
        match case {
            "invalid_target" => {
                doc["workflows"][0]["steps"][0]["channelPath"] = json!("not-a-channel-path");
            }
            "missing_source" => {
                doc["workflows"][0]["steps"][0]["channelPath"] =
                    json!("{$sourceDescriptions.unknown.url}#/channels/request");
            }
            "missing_document" => {
                doc["sourceDescriptions"][0]["url"] = json!("absent.yaml");
            }
            "missing_channel" => {
                doc["workflows"][0]["steps"][0]["channelPath"] = json!(target("absent"));
            }
            "unexpected_channel" => {
                doc["workflows"][0]["steps"][0]["channelPath"] = json!(target("alien"));
            }
            _ => unreachable!("fixed cases"),
        }
        let (engine, exchange) =
            fixture(&dir, &doc, Some(reply(true)), false, Duration::from_secs(1))?;
        let err = collect(&engine).await?.outputs.expect_err(case);
        assert_eq!(err.kind, RuntimeErrorKind::Unspecified, "{case}");
        assert!(err.message.contains(expected), "{case}: {err}");
        assert!(
            exchange.sent.lock().expect("fixture lock").is_empty(),
            "{case}"
        );
        assert_eq!(
            exchange.receive_attempts.load(Ordering::SeqCst),
            0,
            "{case}"
        );
        assert_eq!(
            exchange.routes.lock().expect("fixture lock").len(),
            usize::from(case == "unexpected_channel")
        );
    }
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_send_failure_stops_before_receive() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let (engine, exchange) = fixture(
        &dir,
        &spec(),
        Some(reply(true)),
        true,
        Duration::from_secs(1),
    )?;
    let err = collect(&engine).await?.outputs.expect_err("send must fail");
    assert_eq!(err.kind, RuntimeErrorKind::Unspecified);
    assert!(err.message.contains("EXCHANGE_SEND_FAILURE"));
    assert!(exchange.sent.lock().expect("fixture lock").is_empty());
    assert_eq!(exchange.receive_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(
        *exchange.routes.lock().expect("fixture lock"),
        routes(&dir)[..1]
    );
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_receive_wait_expiry_releases_wait_and_lock() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let (engine, exchange) = fixture(&dir, &spec(), None, false, Duration::from_millis(20))?;
    let err = collect(&engine)
        .await?
        .outputs
        .expect_err("wait must expire");
    assert_eq!(err.kind, RuntimeErrorKind::Unspecified);
    assert!(err.message.contains("EXCHANGE_RECEIVE_WAIT_EXPIRED"));
    assert_eq!(
        *exchange.sent.lock().expect("fixture lock"),
        vec![json!({"order":7})]
    );
    assert_eq!(exchange.receive_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.active_waits.load(Ordering::SeqCst), 0);
    assert!(exchange.inbox_rx.try_lock().is_ok());
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_cancel_after_wait_registration_releases_resources() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let (engine, exchange) = fixture(&dir, &spec(), None, false, Duration::from_secs(2))?;
    let registered = exchange.receive_started.notified();
    tokio::pin!(registered);
    registered.as_mut().enable();
    let handle = engine.execute("exchange", inputs());
    tokio::time::timeout(Duration::from_secs(1), registered).await?;
    assert_eq!(exchange.active_waits.load(Ordering::SeqCst), 1);
    handle.cancel_token().cancel();
    let result = tokio::time::timeout(Duration::from_secs(1), handle.collect()).await?;
    let err = result.outputs.expect_err("invocation must cancel");
    assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
    assert_eq!(
        *exchange.sent.lock().expect("fixture lock"),
        vec![json!({"order":7})]
    );
    assert_eq!(exchange.active_waits.load(Ordering::SeqCst), 0);
    assert!(exchange.inbox_rx.try_lock().is_ok());
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_rejects_unsupported_modes_before_dispatch() -> TestResult {
    for mode in [
        "parallel", "dry_run", "trace", "capture", "replay", "debugger",
    ] {
        let dir = FixtureDir::new();
        dir.write_asyncapi(false)?;
        let spec = dir.parse_spec(&spec())?;
        let builder = EngineBuilder::new(spec).source_base_dir(dir.path());
        let builder = match mode {
            "parallel" => builder.parallel(true),
            "dry_run" => builder.dry_run(true),
            "trace" => builder.trace(true),
            "capture" => builder.capture_run(true),
            "replay" => builder.replay_trace_steps(Vec::new()),
            "debugger" => builder.debug_controller(Arc::new(DebugController::new())),
            _ => unreachable!("fixed modes"),
        };
        let exchange = Exchange::new(dir.path(), Some(reply(true)), false, Duration::from_secs(1));
        let engine = attach(builder, &exchange)?;
        let err = collect(&engine).await?.outputs.expect_err(mode);
        assert_eq!(err.kind, RuntimeErrorKind::Unspecified, "{mode}");
        assert!(
            err.message.contains("EXCHANGE_UNSUPPORTED_MODE"),
            "{mode}: {err}"
        );
        assert!(
            exchange.routes.lock().expect("fixture lock").is_empty(),
            "{mode}"
        );
        assert!(
            exchange.sent.lock().expect("fixture lock").is_empty(),
            "{mode}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn controlled_exchange_unconfigured_engine_keeps_asyncapi_refusal() -> TestResult {
    let dir = FixtureDir::new();
    dir.write_asyncapi(false)?;
    let spec = dir.parse_spec(&spec())?;
    let engine = EngineBuilder::new(spec)
        .source_base_dir(dir.path())
        .build()?;
    let err = collect(&engine)
        .await?
        .outputs
        .expect_err("async transport remains unsupported");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    Ok(())
}
