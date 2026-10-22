//! Owned loopback Core NATS fixtures for the private transport trial.
#![allow(clippy::expect_used)]

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::ops::Deref;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

use arazzo_spec::parse_unvalidated_bytes;
use futures_util::{FutureExt, StreamExt};

use super::nats_transport_trial::NatsTrial;
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const SERVER_VERSION: &str = "nats-server: v2.15.0";
const LIMIT: Duration = Duration::from_secs(3);

struct Fixture {
    dir: PathBuf,
    child: Option<Child>,
    endpoint: String,
    prefix: String,
}

impl Fixture {
    fn new(live: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let binary = if live { Some(server_binary()?) } else { None };
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("arazzo-nats-trial-{}-{serial}", std::process::id()));
        fs::create_dir(&dir)?;
        let prefix = format!("arazzo.trial.{}.{}", std::process::id(), serial);
        let mut fixture = Self {
            dir,
            child: None,
            endpoint: String::new(),
            prefix,
        };
        if !live {
            return Ok(fixture);
        }
        let socket = TcpListener::bind("127.0.0.1:0")?;
        let port = socket.local_addr()?.port();
        drop(socket);
        let endpoint = format!("127.0.0.1:{port}");
        let child = Command::new(binary.expect("live binary validated"))
            .args(["-a", "127.0.0.1", "-p", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        fixture.child = Some(child);
        fixture.endpoint = endpoint;
        let started = std::time::Instant::now();
        loop {
            if TcpStream::connect(&fixture.endpoint).is_ok() {
                break;
            }
            if let Some(status) = fixture.child.as_mut().expect("child").try_wait()? {
                return Err(format!("NATS_TRIAL_SERVER_EXIT: {status}").into());
            }
            if started.elapsed() > LIMIT {
                return Err("NATS_TRIAL_SERVER_START_EXPIRED".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(fixture)
    }

    fn subject(&self, suffix: &str) -> String {
        format!("{}.{}", self.prefix, suffix)
    }
    fn write_docs(&self, changed: bool) -> Result<Value, Box<dyn std::error::Error>> {
        let request = self.subject(if changed { "wrong" } else { "request" });
        fs::write(self.dir.join("events.yaml"), format!(
            "asyncapi: 3.0.0\ninfo:\n  title: NATS trial\n  version: '1'\nchannels:\n  request:\n    address: {request}\n    messages: {{}}\n  reply:\n    address: {}\n    messages: {{}}\n  forward:\n    address: {}\n    messages: {{}}\n", self.subject("reply"), self.subject("forward")
        ))?;
        let doc = spec();
        fs::write(
            self.dir.join("exchange.arazzo.json"),
            serde_json::to_vec_pretty(&doc)?,
        )?;
        Ok(doc)
    }
    fn parse(&self, doc: &Value) -> Result<ArazzoSpec, Box<dyn std::error::Error>> {
        fs::write(
            self.dir.join("exchange.arazzo.json"),
            serde_json::to_vec_pretty(doc)?,
        )?;
        Ok(parse_unvalidated_bytes(&fs::read(
            self.dir.join("exchange.arazzo.json"),
        )?)?)
    }
    fn kill_server(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(child) = &mut self.child {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            wait_child(child)?;
        }
        self.child = None;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = wait_child(&mut child);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn wait_child(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if started.elapsed() > LIMIT {
            return Err("NATS_TRIAL_CHILD_REAP_EXPIRED".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn server_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
    server_binary_from(std::env::var_os("NATS_SERVER_BIN"))
}
fn server_binary_from(
    bin: Option<std::ffi::OsString>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let bin = bin.ok_or("NATS_TRIAL_MISSING_SERVER_BIN")?;
    let path = PathBuf::from(bin);
    if !path.is_absolute() || !path.is_file() {
        return Err("NATS_TRIAL_INVALID_SERVER_BIN".into());
    }
    let mut child = Command::new(&path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Err(err) = wait_child(&mut child) {
        let _ = child.kill();
        let _ = wait_child(&mut child);
        return Err(err);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != SERVER_VERSION
    {
        return Err("NATS_TRIAL_WRONG_SERVER_VERSION".into());
    }
    Ok(path)
}
fn target(key: &str) -> String {
    format!("{{$sourceDescriptions.events.url}}#/channels/{key}")
}
fn spec() -> Value {
    json!({"arazzo":"1.1.0", "info":{"title":"NATS trial","version":"1"},
      "sourceDescriptions":[{"name":"events","url":"events.yaml","type":"asyncapi"}],
      "workflows":[{"workflowId":"exchange","steps":[
        {"stepId":"send","channelPath":target("request"),"action":"send"},
        {"stepId":"receive","channelPath":target("reply"),"action":"receive", "dependsOn":["send"],
          "successCriteria":[{"condition":"$message.payload#/accepted == true"}],
          "outputs":{"id":"$message.payload#/id","tag":"$message.header.x-tag"}},
        {"stepId":"forward","channelPath":target("forward"),"action":"send","dependsOn":["receive"]}
      ],"outputs":{"id":"$steps.receive.outputs.id","tag":"$steps.receive.outputs.tag"}}]})
}
fn inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([("order".into(), json!(7))])
}
#[derive(Clone)]
struct FixtureClient {
    client: async_nats::Client,
    closed: Arc<AtomicBool>,
    notice: Arc<tokio::sync::Notify>,
}
impl Deref for FixtureClient {
    type Target = async_nats::Client;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
async fn client(endpoint: &str) -> Result<FixtureClient, Box<dyn std::error::Error>> {
    let closed = Arc::new(AtomicBool::new(false));
    let notice = Arc::new(tokio::sync::Notify::new());
    let flag = Arc::clone(&closed);
    let on_notice = Arc::clone(&notice);
    let options = async_nats::ConnectOptions::new()
        .connection_timeout(LIMIT)
        .event_callback(move |event| {
            let flag = Arc::clone(&flag);
            let notice = Arc::clone(&on_notice);
            async move {
                if matches!(event, async_nats::Event::Closed) {
                    flag.store(true, Ordering::SeqCst);
                    notice.notify_one();
                }
            }
        });
    Ok(tokio::time::timeout(LIMIT, options.connect(endpoint))
        .await
        .map(|result| {
            result.map(|client| FixtureClient {
                client,
                closed,
                notice,
            })
        })??)
}
async fn attach(
    fixture: &Fixture,
    doc: &Value,
    early: bool,
    wait: Duration,
) -> Result<(Engine, Arc<NatsTrial>), Box<dyn std::error::Error>> {
    let spec = fixture.parse(doc)?;
    let receive = spec.workflows[0].steps[1].clone();
    let mut engine = EngineBuilder::new(spec)
        .source_base_dir(&fixture.dir)
        .build()?;
    let trial = NatsTrial::connect(
        &engine,
        &fixture.endpoint,
        &fixture.dir,
        &receive,
        wait,
        early,
    )
    .await?;
    Arc::get_mut(&mut engine.inner)
        .expect("fresh Engine")
        .nats_trial = Some(Arc::clone(&trial));
    Ok((engine, trial))
}
async fn collect(engine: &Engine) -> Result<ExecutionResult, tokio::time::error::Elapsed> {
    tokio::time::timeout(LIMIT, engine.execute_collect("exchange", inputs())).await
}
async fn ready_subscription(
    client: &async_nats::Client,
    subject: String,
) -> Result<async_nats::Subscriber, Box<dyn std::error::Error>> {
    let sub = tokio::time::timeout(LIMIT, client.subscribe(subject)).await??;
    tokio::time::timeout(LIMIT, client.flush()).await??;
    Ok(sub)
}
async fn close_client(client: &FixtureClient) -> TestResult {
    tokio::time::timeout(LIMIT, client.drain()).await??;
    tokio::time::timeout(LIMIT, async {
        while !client.closed.load(Ordering::SeqCst) {
            client.notice.notified().await;
        }
    })
    .await?;
    Ok(())
}
async fn join_fixture_task(mut task: tokio::task::JoinHandle<()>) -> TestResult {
    match tokio::time::timeout(LIMIT, &mut task).await {
        Ok(result) => {
            result?;
            Ok(())
        }
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("NATS_TRIAL_FIXTURE_TASK_JOIN_EXPIRED".into())
        }
    }
}
fn assert_clean(trial: &NatsTrial) {
    assert_eq!(trial.active_waits.load(Ordering::SeqCst), 0);
    assert!(trial.receiver_free());
}

#[test]
fn nats_transport_trial_prerequisites_fail_without_connecting() -> TestResult {
    assert!(server_binary_from(None)
        .expect_err("missing prerequisite")
        .to_string()
        .contains("MISSING"));
    assert!(server_binary_from(Some("relative-nats".into()))
        .expect_err("invalid prerequisite")
        .to_string()
        .contains("INVALID"));
    let fake = std::env::current_exe()?;
    assert!(server_binary_from(Some(fake.into_os_string()))
        .expect_err("wrong version")
        .to_string()
        .contains("WRONG"));
    Ok(())
}

#[tokio::test]
async fn nats_transport_trial_mode_and_unconfigured_guards() -> TestResult {
    let fixture = Fixture::new(false)?;
    let doc = fixture.write_docs(false)?;
    let spec = fixture.parse(&doc)?;
    let engine = EngineBuilder::new(spec)
        .source_base_dir(&fixture.dir)
        .build()?;
    let err = collect(&engine)
        .await?
        .outputs
        .expect_err("unconfigured transport");
    assert_eq!(err.kind, RuntimeErrorKind::UnsupportedAsyncApiTransport);
    for mode in [
        "parallel", "dry_run", "trace", "capture", "replay", "debugger",
    ] {
        let spec = fixture.parse(&doc)?;
        let builder = EngineBuilder::new(spec).source_base_dir(&fixture.dir);
        let builder = match mode {
            "parallel" => builder.parallel(true),
            "dry_run" => builder.dry_run(true),
            "trace" => builder.trace(true),
            "capture" => builder.capture_run(true),
            "replay" => builder.replay_trace_steps(Vec::new()),
            "debugger" => builder.debug_controller(Arc::new(DebugController::new())),
            _ => unreachable!(),
        };
        let engine = builder.build()?;
        let err = super::nats_transport_trial::require_supported_mode(&engine).expect_err(mode);
        assert!(
            err.message.contains("NATS_TRIAL_UNSUPPORTED_MODE"),
            "{mode}: {err}"
        );
    }
    let mut engine = EngineBuilder::new(fixture.parse(&doc)?)
        .source_base_dir(&fixture.dir)
        .build()?;
    Arc::get_mut(&mut engine.inner)
        .expect("fresh Engine")
        .controlled_exchange = Some(super::controlled_exchange::Exchange::new(
        &fixture.dir,
        None,
        false,
        LIMIT,
    ));
    let err =
        super::nats_transport_trial::require_supported_mode(&engine).expect_err("dual fixture");
    assert!(err.message.contains("NATS_TRIAL_DUAL_FIXTURE"));
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_exchange_and_early_reply() -> TestResult {
    let mut fixture = Fixture::new(true)?;
    let doc = fixture.write_docs(false)?;
    let responder = client(&fixture.endpoint).await?;
    let observer = client(&fixture.endpoint).await?;
    let mut request = ready_subscription(&responder, fixture.subject("request")).await?;
    let mut forward = ready_subscription(&observer, fixture.subject("forward")).await?;
    let (engine, trial) = attach(&fixture, &doc, true, LIMIT).await?;
    let responder_task = tokio::spawn({
        let responder = responder.clone();
        let reply = fixture.subject("reply");
        async move {
            let request = tokio::time::timeout(LIMIT, request.next())
                .await
                .expect("request deadline")
                .expect("request message");
            assert_eq!(
                serde_json::from_slice::<Value>(&request.payload).expect("request JSON"),
                json!({"order":7})
            );
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("x-tag", "reply");
            responder
                .publish_with_headers(
                    reply,
                    headers,
                    serde_json::to_vec(&json!({"id":42,"accepted":true}))
                        .expect("JSON")
                        .into(),
                )
                .await
                .expect("reply publish");
            responder.flush().await.expect("reply flush");
        }
    });
    let handle = engine.execute("exchange", inputs());
    let cancel = handle.cancel_token().clone();
    let body = std::panic::AssertUnwindSafe(async {
        trial.wait_receive_gate().await?;
        trial.wait_buffered().await?;
        // The send step has returned because the scheduler entered the receive
        // gate, while the subscriber pump already holds the broker reply.
        assert_eq!(trial.receive_attempts.load(Ordering::SeqCst), 0);
        trial.release_receive();
        let result = tokio::time::timeout(LIMIT, handle.collect()).await?;
        assert_eq!(
            result.outputs?,
            BTreeMap::from([("id".into(), json!(42)), ("tag".into(), json!("reply"))])
        );
        let forwarded = tokio::time::timeout(LIMIT, forward.next())
            .await?
            .expect("forward message");
        assert_eq!(
            serde_json::from_slice::<Value>(&forwarded.payload)?,
            json!({"id":42})
        );
        assert_eq!(trial.receive_attempts.load(Ordering::SeqCst), 1);
        let routes = trial.routes.lock().expect("routes").clone();
        assert_eq!(
            routes
                .iter()
                .map(|r| r.channel_key.as_str())
                .collect::<Vec<_>>(),
            vec!["request", "reply", "forward"]
        );
        assert!(routes.iter().all(|r| r.source == "events"
            && r.document == fixture.dir.join("events.yaml").display().to_string()
            && r.address == fixture.subject(&r.channel_key)));
        assert_clean(&trial);
        Ok(()) as TestResult
    })
    .catch_unwind()
    .await;
    cancel.cancel();
    trial.release_receive();
    let task_result = join_fixture_task(responder_task).await;
    let trial_close = trial.close().await;
    let responder_close = close_client(&responder).await;
    let observer_close = close_client(&observer).await;
    let server_reap = fixture.kill_server();
    let body = match body {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    body?;
    task_result?;
    trial_close?;
    assert!(trial.closed());
    responder_close?;
    observer_close?;
    server_reap?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ReplyCase {
    SuccessEnd,
    FailureEnd,
    Malformed,
    MultiHeader,
}

async fn reply_case(case: ReplyCase) -> TestResult {
    let mut fixture = Fixture::new(true)?;
    let mut doc = fixture.write_docs(false)?;
    match case {
        ReplyCase::SuccessEnd => {
            doc["workflows"][0]["steps"][1]["onSuccess"] = json!([{
                "name":"end_on_id","type":"end","criteria":[{"condition":"$message.payload#/id == 42"}]
            }]);
        }
        ReplyCase::FailureEnd => {
            doc["workflows"][0]["steps"][1]["onFailure"] = json!([{
                "name":"end_on_rejection","type":"end","criteria":[{"condition":"$message.payload#/accepted == false"}]
            }]);
        }
        _ => {}
    }
    let responder = client(&fixture.endpoint).await?;
    let observer = client(&fixture.endpoint).await?;
    let mut request = ready_subscription(&responder, fixture.subject("request")).await?;
    let mut forward = ready_subscription(&observer, fixture.subject("forward")).await?;
    let (engine, trial) = attach(&fixture, &doc, false, LIMIT).await?;
    let reply_subject = fixture.subject("reply");
    let responder_task = tokio::spawn({
        let responder = responder.clone();
        async move {
            let request = tokio::time::timeout(LIMIT, request.next())
                .await
                .expect("request deadline")
                .expect("request");
            assert_eq!(
                serde_json::from_slice::<Value>(&request.payload).expect("request JSON"),
                json!({"order":7})
            );
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("x-tag", "reply");
            if matches!(case, ReplyCase::MultiHeader) {
                headers.append("x-tag", "second");
            }
            let payload: Vec<u8> = if matches!(case, ReplyCase::Malformed) {
                b"{".to_vec()
            } else {
                serde_json::to_vec(
                    &json!({"id":42,"accepted":!matches!(case, ReplyCase::FailureEnd)}),
                )
                .expect("reply JSON")
            };
            responder
                .publish_with_headers(reply_subject, headers, payload.into())
                .await
                .expect("reply publish");
            responder.flush().await.expect("reply flush");
        }
    });
    let handle = engine.execute("exchange", inputs());
    let cancel = handle.cancel_token().clone();
    let body = std::panic::AssertUnwindSafe(async {
        let result = tokio::time::timeout(LIMIT, handle.collect()).await?;
        match case {
            ReplyCase::SuccessEnd => {
                assert_eq!(result.outputs?["id"], json!(42));
                assert_eq!(trial.routes.lock().expect("routes").len(), 2);
            }
            ReplyCase::FailureEnd => {
                let err = result.outputs.expect_err("criterion failure");
                assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed);
                assert!(err.message.contains("workflow ended by onFailure action"));
            }
            ReplyCase::Malformed => {
                assert!(result
                    .outputs
                    .expect_err("malformed reply")
                    .message
                    .contains("NATS_TRIAL_MALFORMED_REPLY"));
            }
            ReplyCase::MultiHeader => {
                assert!(result
                    .outputs
                    .expect_err("multi-value header")
                    .message
                    .contains("NATS_TRIAL_MULTI_VALUE_HEADER"));
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), forward.next())
                .await
                .is_err()
        );
        assert_clean(&trial);
        Ok(()) as TestResult
    })
    .catch_unwind()
    .await;
    cancel.cancel();
    let task_result = join_fixture_task(responder_task).await;
    let trial_close = trial.close().await;
    let responder_close = close_client(&responder).await;
    let observer_close = close_client(&observer).await;
    let server_reap = fixture.kill_server();
    let body = match body {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    body?;
    task_result?;
    trial_close?;
    assert!(trial.closed());
    responder_close?;
    observer_close?;
    server_reap?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_actions_and_reply_shape_errors() -> TestResult {
    for case in [
        ReplyCase::SuccessEnd,
        ReplyCase::FailureEnd,
        ReplyCase::Malformed,
        ReplyCase::MultiHeader,
    ] {
        reply_case(case).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_resolver_and_changed_address() -> TestResult {
    for case in [
        "invalid",
        "missing",
        "changed",
        "bad_address",
        "request_body",
        "correlation",
    ] {
        let mut fixture = Fixture::new(true)?;
        let mut doc = fixture.write_docs(case == "changed")?;
        match case {
            "invalid" => {
                doc["workflows"][0]["steps"][0]["channelPath"] = json!("not-a-channel-path")
            }
            "missing" => doc["workflows"][0]["steps"][0]["channelPath"] = json!(target("absent")),
            "bad_address" => {
                let path = fixture.dir.join("events.yaml");
                let text = fs::read_to_string(&path)?;
                fs::write(path, text.replace(&fixture.subject("request"), "bad.*"))?;
            }
            "request_body" => {
                doc["workflows"][0]["steps"][0]["requestBody"] = json!({"payload":{"order":7}})
            }
            "correlation" => doc["workflows"][0]["steps"][0]["correlationId"] = json!("fixed"),
            _ => {}
        }
        let probe = client(&fixture.endpoint).await?;
        let mut original = ready_subscription(&probe, fixture.subject("request")).await?;
        let mut changed = ready_subscription(&probe, fixture.subject("wrong")).await?;
        let (engine, trial) = attach(&fixture, &doc, false, Duration::from_millis(150)).await?;
        let handle = engine.execute("exchange", inputs());
        let cancel = handle.cancel_token().clone();
        let body = std::panic::AssertUnwindSafe(async {
            let err = tokio::time::timeout(LIMIT, handle.collect())
                .await?
                .outputs
                .expect_err(case);
            if case == "changed" {
                assert!(err.message.contains("NATS_TRIAL_RECEIVE_WAIT_EXPIRED"));
                let message = tokio::time::timeout(LIMIT, changed.next())
                    .await?
                    .expect("changed subject message");
                assert_eq!(
                    serde_json::from_slice::<Value>(&message.payload)?,
                    json!({"order":7})
                );
                assert_eq!(
                    trial.routes.lock().expect("routes")[0].address,
                    fixture.subject("wrong")
                );
            } else {
                let marker = match case {
                    "bad_address" => "NATS_TRIAL_UNSUPPORTED_ADDRESS",
                    "request_body" | "correlation" => "NATS_TRIAL_UNSUPPORTED_STEP_FORM",
                    _ => "NATS_TRIAL_RESOLVE_",
                };
                assert!(err.message.contains(marker), "{case}: {err}");
                assert!(trial.routes.lock().expect("routes").is_empty());
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), original.next())
                    .await
                    .is_err()
            );
            if case != "changed" {
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), changed.next())
                        .await
                        .is_err()
                );
            }
            assert_clean(&trial);
            Ok(()) as TestResult
        })
        .catch_unwind()
        .await;
        cancel.cancel();
        let trial_close = trial.close().await;
        let probe_close = close_client(&probe).await;
        let server_reap = fixture.kill_server();
        let body = match body {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        body?;
        trial_close?;
        probe_close?;
        server_reap?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_wait_expiry_and_cancellation() -> TestResult {
    for cancel in [false, true] {
        let mut fixture = Fixture::new(true)?;
        let doc = fixture.write_docs(false)?;
        let probe = client(&fixture.endpoint).await?;
        let mut request = ready_subscription(&probe, fixture.subject("request")).await?;
        let mut forward = ready_subscription(&probe, fixture.subject("forward")).await?;
        let (engine, trial) = attach(
            &fixture,
            &doc,
            false,
            Duration::from_millis(if cancel { 2000 } else { 100 }),
        )
        .await?;
        let registered = trial.receive_started.notified();
        tokio::pin!(registered);
        registered.as_mut().enable();
        let handle = engine.execute("exchange", inputs());
        let cancel_token = handle.cancel_token().clone();
        let body = std::panic::AssertUnwindSafe(async {
            tokio::time::timeout(LIMIT, registered).await?;
            assert_eq!(trial.active_waits.load(Ordering::SeqCst), 1);
            let request = tokio::time::timeout(LIMIT, request.next())
                .await?
                .expect("request crossed broker");
            assert_eq!(
                serde_json::from_slice::<Value>(&request.payload)?,
                json!({"order":7})
            );
            if cancel {
                cancel_token.cancel();
            }
            let err = tokio::time::timeout(LIMIT, handle.collect())
                .await?
                .outputs
                .expect_err("receive failure");
            if cancel {
                assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
            } else {
                assert!(err.message.contains("NATS_TRIAL_RECEIVE_WAIT_EXPIRED"));
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), forward.next())
                    .await
                    .is_err()
            );
            assert_clean(&trial);
            Ok(()) as TestResult
        })
        .catch_unwind()
        .await;
        cancel_token.cancel();
        let trial_close = trial.close().await;
        let probe_close = close_client(&probe).await;
        let server_reap = fixture.kill_server();
        let body = match body {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        body?;
        trial_close?;
        probe_close?;
        server_reap?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_terminal_server_loss() -> TestResult {
    let mut fixture = Fixture::new(true)?;
    let doc = fixture.write_docs(false)?;
    let probe = client(&fixture.endpoint).await?;
    let mut request = ready_subscription(&probe, fixture.subject("request")).await?;
    let mut forward = ready_subscription(&probe, fixture.subject("forward")).await?;
    let (engine, trial) = attach(&fixture, &doc, false, LIMIT).await?;
    let registered = trial.receive_started.notified();
    tokio::pin!(registered);
    registered.as_mut().enable();
    let handle = engine.execute("exchange", inputs());
    let cancel = handle.cancel_token().clone();
    let body = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(LIMIT, registered).await?;
        let _ = tokio::time::timeout(LIMIT, request.next())
            .await?
            .expect("request crossed broker");
        fixture.kill_server()?;
        let err = tokio::time::timeout(LIMIT, handle.collect())
            .await?
            .outputs
            .expect_err("server lost");
        assert!(err.message.contains("NATS_TRIAL_DISCONNECTED"), "{err}");
        assert!(trial.terminal());
        assert!(!matches!(
            tokio::time::timeout(Duration::from_millis(100), forward.next()).await,
            Ok(Some(_))
        ));
        assert_clean(&trial);
        Ok(()) as TestResult
    })
    .catch_unwind()
    .await;
    cancel.cancel();
    let server_reap = fixture.kill_server();
    let close = tokio::time::timeout(LIMIT, trial.close()).await;
    let _probe_close = close_client(&probe).await;
    let body = match body {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    body?;
    server_reap?;
    let close = close?;
    match close {
        Ok(()) => assert!(trial.closed()),
        Err(err) => assert!(
            err.message.contains("NATS_TRIAL_DRAIN")
                || err.message.contains("NATS_TRIAL_CLOSED_NOT_OBSERVED"),
            "{err}"
        ),
    }
    drop(engine);
    drop(probe);
    Ok(())
}

#[tokio::test]
#[ignore = "requires explicit NATS_SERVER_BIN pointing to pinned nats-server v2.15.0"]
async fn nats_transport_trial_live_changed_receive_route_rejects_old_reply() -> TestResult {
    let mut fixture = Fixture::new(true)?;
    let doc = fixture.write_docs(false)?;
    let probe = client(&fixture.endpoint).await?;
    let mut request = ready_subscription(&probe, fixture.subject("request")).await?;
    let mut forward = ready_subscription(&probe, fixture.subject("forward")).await?;
    let (engine, trial) = attach(&fixture, &doc, false, LIMIT).await?;

    // The registered old reply genuinely crossed NATS into the fixed inbox.
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("x-tag", "reply");
    probe
        .publish_with_headers(
            fixture.subject("reply"),
            headers,
            serde_json::to_vec(&json!({"id":42,"accepted":true}))?.into(),
        )
        .await?;
    probe.flush().await?;
    trial.wait_buffered().await?;
    assert_eq!(trial.buffered(), 1);

    // Change only the local AsyncAPI receive address after preregistration.
    let path = fixture.dir.join("events.yaml");
    let text = fs::read_to_string(&path)?;
    fs::write(
        &path,
        text.replace(&fixture.subject("reply"), &fixture.subject("changed_reply")),
    )?;
    let handle = engine.execute("exchange", inputs());
    let cancel = handle.cancel_token().clone();
    let body = std::panic::AssertUnwindSafe(async {
        let err = tokio::time::timeout(LIMIT, handle.collect())
            .await?
            .outputs
            .expect_err("receive route changed");
        assert!(
            err.message.contains("NATS_TRIAL_RECEIVE_ROUTE_CHANGED"),
            "{err}"
        );
        let sent = tokio::time::timeout(LIMIT, request.next())
            .await?
            .expect("request crossed broker");
        assert_eq!(
            serde_json::from_slice::<Value>(&sent.payload)?,
            json!({"order":7})
        );
        assert_eq!(trial.buffered(), 1, "old reply must remain unconsumed");
        assert_eq!(trial.receive_attempts.load(Ordering::SeqCst), 0);
        assert_eq!(trial.routes.lock().expect("routes").len(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), forward.next())
                .await
                .is_err()
        );
        assert_clean(&trial);
        Ok(()) as TestResult
    })
    .catch_unwind()
    .await;
    cancel.cancel();
    let trial_close = trial.close().await;
    let probe_close = close_client(&probe).await;
    let server_reap = fixture.kill_server();
    let body = match body {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    body?;
    trial_close?;
    probe_close?;
    server_reap?;
    Ok(())
}
