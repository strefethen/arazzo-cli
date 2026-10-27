//! `run`/`test` with Kafka receive steps through the built binary.
//!
//! Hermetic cases point the AsyncAPI document at `127.0.0.1:1` (nothing
//! listens) and prove the flag refusals, the dry run, the password discipline,
//! and the replay refusal without a broker. Live cases are `#[ignore]` and
//! need the harness broker:
//!
//! ```sh
//! scripts/kafka/broker.sh up --secure
//! ARAZZO_KAFKA_BOOTSTRAP=127.0.0.1:19092 ARAZZO_KAFKA_SECURE_BOOTSTRAP=127.0.0.1:19093 \
//! ARAZZO_KAFKA_CA_PEM=/tmp/arazzo-kafka/ca.pem \
//!   cargo test -p arazzo-cli --test cli_kafka_receive -- --ignored --test-threads=1
//! ```
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture setup and assertions.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const USERNAME_ENV: &str = "ARAZZO_KAFKA_USERNAME";
const PASSWORD_ENV: &str = "ARAZZO_KAFKA_PASSWORD";
const PASSWORD_LITERAL: &str = "s3cret-value-never-printed";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arazzo-cli-kafka-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create fixture dir");
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, text).expect("write fixture");
        path
    }

    /// One plaintext `kafka` server.
    fn asyncapi(&self, host: &str, topic: &str) {
        self.write(
            "orders.asyncapi.yaml",
            &format!(
                "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  kafka:\n    host: {host}\n    protocol: kafka\nchannels:\n  orderPlaced:\n    address: {topic}\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n"
            ),
        );
    }

    /// A plaintext `kafka` server and a `kafka-secure` SCRAM server, so the
    /// run must pick one with `--asyncapi-server`.
    fn asyncapi_two_servers(&self, plain_host: &str, secure_host: &str, topic: &str) {
        self.write(
            "orders.asyncapi.yaml",
            &format!(
                "asyncapi: 3.0.0\ninfo:\n  title: Orders\n  version: 1.0.0\nservers:\n  kafka:\n    host: {plain_host}\n    protocol: kafka\n  kafka-secure:\n    host: {secure_host}\n    protocol: kafka-secure\n    security:\n      - $ref: '#/components/securitySchemes/scram'\nchannels:\n  orderPlaced:\n    address: {topic}\n    messages:\n      orderPlaced:\n        contentType: application/json\noperations:\n  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\ncomponents:\n  securitySchemes:\n    scram:\n      type: scramSha256\n"
            ),
        );
    }

    fn openapi(&self, base_url: &str) {
        self.write(
            "shop.openapi.yaml",
            &format!(
                "openapi: 3.0.3\ninfo:\n  title: Shop\n  version: 1.0.0\nservers:\n  - url: {base_url}\npaths:\n  /events/{{id}}:\n    get:\n      operationId: getEvent\n      parameters:\n        - name: id\n          in: path\n          required: true\n          schema:\n            type: integer\n      responses:\n        '200':\n          description: ok\n"
            ),
        );
    }

    fn arazzo(&self, workflows_yaml: &str) -> PathBuf {
        self.write(
            "spec.arazzo.yaml",
            &format!(
                "arazzo: 1.1.0\ninfo:\n  title: Kafka receive CLI fixture\n  version: 1.0.0\nsourceDescriptions:\n  - name: orders\n    url: ./orders.asyncapi.yaml\n    type: asyncapi\n  - name: shop\n    url: ./shop.openapi.yaml\n    type: openapi\nworkflows:\n{workflows_yaml}"
            ),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

const RECEIVE_THEN_HTTP: &str = "  - workflowId: followup\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        successCriteria:\n          - condition: $message.payload#/id > 0\n        outputs:\n          id: $message.payload#/id\n      - stepId: fetch-event\n        operationId: $sourceDescriptions.shop.getEvent\n        dependsOn: [await-order]\n        parameters:\n          - name: id\n            in: path\n            value: $steps.await-order.outputs.id\n        successCriteria:\n          - condition: $statusCode == 200\n    outputs:\n      orderId: $steps.await-order.outputs.id\n";

const RECEIVE_WITH_STEP_TIMEOUT: &str = "  - workflowId: followup\n    steps:\n      - stepId: await-order\n        operationId: $sourceDescriptions.orders.receiveOrderPlaced\n        action: receive\n        timeout: 1500\n        outputs:\n          id: $message.payload#/id\n";

/// The CLI with every Kafka environment variable cleared, so a developer's
/// shell cannot leak into the hermetic cases.
fn cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arazzo-cli"));
    command.env_remove(USERNAME_ENV).env_remove(PASSWORD_ENV);
    command
}

fn run_json(args: &[&str], envs: &[(&str, &str)]) -> (Output, Value) {
    let mut command = cli();
    command.arg("--json").args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("execute CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str::<Value>(stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "stdout is not one JSON object ({err}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, value)
}

fn assert_error(value: &Value, code: &str) {
    assert_eq!(value["kind"], "error", "{value}");
    assert_eq!(value["code"], code, "{value}");
    assert!(value["error"].is_string(), "{value}");
}

struct HttpServer {
    base_url: String,
    hits: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
    handle: Option<thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl HttpServer {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("bind test server");
        let base_url = format!("http://{}", server.server_addr());
        let hits = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (hits_t, paths_t, stop_t) = (Arc::clone(&hits), Arc::clone(&paths), Arc::clone(&stop));
        let handle = thread::spawn(move || {
            while !stop_t.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(100)) {
                    Ok(Some(request)) => {
                        hits_t.fetch_add(1, Ordering::SeqCst);
                        paths_t.lock().unwrap().push(request.url().to_string());
                        let _ = request.respond(
                            tiny_http::Response::from_string("{\"ok\":true}").with_header(
                                tiny_http::Header::from_bytes("Content-Type", "application/json")
                                    .unwrap(),
                            ),
                        );
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url,
            hits,
            paths,
            handle: Some(handle),
            stop,
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

// ───────────────────────── hermetic ─────────────────────────

#[test]
fn a_dead_broker_fails_with_the_transport_code_within_the_connect_timeout() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let started = Instant::now();
    let (output, value) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--kafka-connect-timeout",
            "300ms",
        ],
        &[],
    );
    let elapsed = started.elapsed();
    assert!(!output.status.success());
    assert_error(&value, "RUNTIME_MESSAGE_TRANSPORT");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    // `run_json` parsed the whole of stdout as one JSON value, so nothing
    // else (no YAML, no prose) shared the stream.
}

#[test]
fn partial_credentials_are_refused_before_any_connection() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let started = Instant::now();
    let (_, value) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--kafka-username",
            "arazzo",
        ],
        &[],
    );
    assert_error(&value, "RUN_KAFKA_CREDENTIALS");
    assert!(value["error"].as_str().unwrap().contains(PASSWORD_ENV));

    let (_, value) = run_json(
        &["run", spec.to_str().unwrap(), "followup"],
        &[(PASSWORD_ENV, PASSWORD_LITERAL)],
    );
    assert_error(&value, "RUN_KAFKA_CREDENTIALS");
    assert!(!value.to_string().contains(PASSWORD_LITERAL));

    // The username may also arrive through its environment variable.
    let (_, value) = run_json(
        &["run", spec.to_str().unwrap(), "followup"],
        &[(USERNAME_ENV, "arazzo")],
    );
    assert_error(&value, "RUN_KAFKA_CREDENTIALS");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "refusals must not wait on the broker: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_password_flag_is_unknown() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let (output, value) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--kafka-password",
            "nope",
        ],
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_error(&value, "CLI_USAGE");
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("--kafka-password"));
}

#[test]
fn asyncapi_server_and_ca_cert_flags_are_validated_first() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let spec = spec.to_str().unwrap();
    let (_, value) = run_json(
        &["run", spec, "followup", "--asyncapi-server", "orders"],
        &[],
    );
    assert_error(&value, "RUN_ASYNCAPI_SERVER_FLAG");
    let (_, value) = run_json(
        &[
            "run",
            spec,
            "followup",
            "--asyncapi-server",
            "orders=kafka",
            "--asyncapi-server",
            "orders=kafka-secure",
        ],
        &[],
    );
    assert_error(&value, "RUN_ASYNCAPI_SERVER_FLAG");
    assert!(value["error"].as_str().unwrap().contains("duplicate"));
    let missing = fixture.path("missing.pem");
    let (_, value) = run_json(
        &[
            "run",
            spec,
            "followup",
            "--kafka-ca-cert",
            missing.to_str().unwrap(),
        ],
        &[],
    );
    assert_error(&value, "RUN_KAFKA_CA_CERT_READ");
}

#[test]
fn dry_run_lists_the_receive_without_connecting() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let started = Instant::now();
    let (output, value) = run_json(
        &["run", spec.to_str().unwrap(), "followup", "--dry-run"],
        &[],
    );
    assert!(output.status.success(), "{value}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(value["kind"], "dryRun", "{value}");
    let requests = value["requests"].as_array().expect("requests");
    assert_eq!(requests[0]["method"], "RECEIVE");
    assert_eq!(requests[0]["url"], "kafka://127.0.0.1:1/orders.placed");
    assert_eq!(requests[1]["method"], "GET");

    let output = cli()
        .args(["run", spec.to_str().unwrap(), "followup", "--dry-run"])
        .output()
        .expect("execute CLI");
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("RECEIVE kafka://127.0.0.1:1/orders.placed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn the_password_never_reaches_any_artefact() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let trace = fixture.path("trace.json");
    let (output, value) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--verbose",
            "--kafka-connect-timeout",
            "300ms",
            "--trace",
            trace.to_str().unwrap(),
        ],
        &[(USERNAME_ENV, "arazzo"), (PASSWORD_ENV, PASSWORD_LITERAL)],
    );
    // The plaintext fixture server declares no SASL scheme, so credentials
    // are refused as a configuration error before any connection (D4); the
    // refusal message must not echo the password either.
    assert_error(&value, "RUNTIME_MESSAGE_TRANSPORT_CONFIG");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains(PASSWORD_LITERAL), "stdout: {stdout}");
    assert!(!stderr.contains(PASSWORD_LITERAL), "stderr: {stderr}");
    if let Ok(text) = fs::read_to_string(&trace) {
        assert!(!text.contains(PASSWORD_LITERAL), "trace: {text}");
    }
}

#[test]
fn test_reports_the_transport_code_as_a_case_error_and_refuses_partial_credentials() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    fixture.arazzo(RECEIVE_THEN_HTTP);
    let dir = fixture.dir.to_str().unwrap().to_string();
    let started = Instant::now();
    let (output, value) = run_json(
        &[
            "test",
            &dir,
            "--format",
            "json",
            "--kafka-connect-timeout",
            "300ms",
        ],
        &[],
    );
    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(value["kind"], "results", "{value}");
    let case = &value["suites"][0]["tests"][0];
    assert_eq!(case["status"], "error", "{value}");
    assert_eq!(case["errorCode"], "RUNTIME_MESSAGE_TRANSPORT", "{value}");

    let (_, value) = run_json(
        &[
            "test",
            &dir,
            "--format",
            "json",
            "--kafka-username",
            "arazzo",
        ],
        &[],
    );
    assert_error(&value, "TEST_KAFKA_CREDENTIALS");
    let (_, value) = run_json(
        &[
            "test",
            &dir,
            "--format",
            "json",
            "--asyncapi-server",
            "orders",
        ],
        &[],
    );
    assert_error(&value, "TEST_ASYNCAPI_SERVER_FLAG");
}

#[test]
fn replay_refuses_a_trace_containing_a_receive_step() {
    let fixture = Fixture::new();
    fixture.asyncapi("127.0.0.1:1", "orders.placed");
    fixture.openapi("http://127.0.0.1:1");
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let trace = fixture.path("dry.trace.json");
    let (output, _) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--dry-run",
            "--trace",
            trace.to_str().unwrap(),
        ],
        &[],
    );
    assert!(output.status.success());
    let recorded: Value = serde_json::from_str(&fs::read_to_string(&trace).unwrap()).unwrap();
    assert!(
        recorded["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["message"]["simulated"] == true),
        "the dry-run trace carries the simulated receive record: {recorded}"
    );
    let (output, value) = run_json(&["replay", trace.to_str().unwrap()], &[]);
    assert!(!output.status.success());
    assert_error(&value, "RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT");
}

// ───────────────────────── live ─────────────────────────

fn bootstrap() -> String {
    std::env::var("ARAZZO_KAFKA_BOOTSTRAP")
        .expect("ARAZZO_KAFKA_BOOTSTRAP must be set (scripts/kafka/broker.sh up --secure)")
}

fn broker_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/kafka/broker.sh")
}

fn broker(args: &[&str], stdin: Option<&str>) -> String {
    let mut command = Command::new(broker_script());
    command.args(args);
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn broker.sh");
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .expect("write producer input");
    }
    let output = child.wait_with_output().expect("broker.sh");
    assert!(
        output.status.success(),
        "broker.sh {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn unique_topic(label: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let topic = format!(
        "arazzo-cli-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    broker(&["topic", &topic], None);
    topic
}

fn connections() -> usize {
    broker(&["connections"], None)
        .lines()
        .next()
        .unwrap_or("0")
        .trim()
        .parse()
        .expect("connection count")
}

/// A running `arazzo-cli run`, with stderr followed live so the test can
/// act on the armed line.
struct LiveRun {
    child: std::process::Child,
    stdout: thread::JoinHandle<Vec<u8>>,
    stderr: thread::JoinHandle<Vec<String>>,
    armed: mpsc::Receiver<String>,
    started: Instant,
}

impl LiveRun {
    fn spawn(args: &[&str], envs: &[(&str, &str)]) -> Self {
        Self::spawn_mode(true, args, envs)
    }

    fn spawn_mode(json: bool, args: &[&str], envs: &[(&str, &str)]) -> Self {
        let mut command = cli();
        if json {
            command.arg("--json");
        }
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        let started = Instant::now();
        let mut child = command.spawn().expect("spawn arazzo-cli");
        let mut out = child.stdout.take().unwrap();
        let stdout = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            bytes
        });
        let err = child.stderr.take().unwrap();
        let (tx, armed) = mpsc::channel();
        let stderr = thread::spawn(move || {
            let mut lines = Vec::new();
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if line.starts_with("Kafka receiver armed:") {
                    let _ = tx.send(line.clone());
                }
                lines.push(line);
            }
            lines
        });
        Self {
            child,
            stdout,
            stderr,
            armed,
            started,
        }
    }

    fn wait_armed(&self) -> String {
        self.armed
            .recv_timeout(Duration::from_secs(20))
            .expect("the armed line within 20 s")
    }

    /// Raw stdout text and stderr lines, for text-mode runs.
    fn finish_raw(mut self) -> (std::process::ExitStatus, String, String, Duration) {
        let status = self.child.wait().expect("wait for arazzo-cli");
        let elapsed = self.started.elapsed();
        let stdout = self.stdout.join().unwrap();
        let stderr = self.stderr.join().unwrap().join("\n");
        (
            status,
            String::from_utf8_lossy(&stdout).into_owned(),
            stderr,
            elapsed,
        )
    }

    fn finish(self) -> (std::process::ExitStatus, Value, String, Duration) {
        let (status, text, stderr, elapsed) = self.finish_raw();
        let value = serde_json::from_str::<Value>(text.trim())
            .unwrap_or_else(|err| panic!("stdout not JSON ({err}): {text}\nstderr: {stderr}"));
        (status, value, stderr, elapsed)
    }
}

fn produce(topic: &str, headers: &str, payload: &str) {
    broker(
        &["produce", topic],
        Some(&format!("{headers}\t{payload}\n")),
    );
}

#[test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up --secure)"]
fn kafka_cli_receive_then_http_end_to_end_with_a_redacted_trace() {
    let bootstrap = bootstrap();
    let topic = unique_topic("followup");
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    let server = HttpServer::start();
    fixture.openapi(&server.base_url);
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let trace = fixture.path("trace.json");

    let run = LiveRun::spawn(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--verbose",
            "--trace",
            trace.to_str().unwrap(),
        ],
        &[],
    );
    let armed = run.wait_armed();
    assert!(
        armed.contains(&topic) && armed.contains(&bootstrap),
        "armed line names topic and host: {armed}"
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);
    produce(
        &topic,
        "source:cli-test,authorization:cli-header-secret",
        r#"{"id":42,"status":"placed"}"#,
    );
    let (status, value, stderr, _) = run.finish();
    assert!(status.success(), "{value}\n{stderr}");
    assert_eq!(value["kind"], "success", "{value}");
    assert_eq!(
        value["outputs"]["orderId"],
        serde_json::json!(42),
        "{value}"
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    assert_eq!(server.paths.lock().unwrap()[0], "/events/42");

    let text = fs::read_to_string(&trace).expect("trace written");
    let recorded: Value = serde_json::from_str(&text).unwrap();
    let step = recorded["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["stepId"] == "await-order")
        .expect("receive step record");
    assert_eq!(step["message"]["topic"], topic.as_str());
    assert_eq!(step["message"]["partition"], 0);
    assert!(step["message"]["offset"].is_number(), "{step}");
    assert_eq!(step["message"]["headers"]["source"], "cli-test");
    assert_ne!(
        step["message"]["headers"]["authorization"],
        "cli-header-secret"
    );
    assert!(!text.contains("cli-header-secret"), "header leaked: {text}");
    assert!(step["request"].is_null(), "receive steps carry no request");

    // Text mode: the verbose step listing names the record the step consumed.
    let run = LiveRun::spawn_mode(
        false,
        &["run", spec.to_str().unwrap(), "followup", "--verbose"],
        &[],
    );
    run.wait_armed();
    produce(&topic, "source:cli-test", r#"{"id":43}"#);
    let (status, stdout, stderr, _) = run.finish_raw();
    assert!(status.success(), "{stdout}\n{stderr}");
    let line = stdout
        .lines()
        .find(|line| line.contains("[msg] RECEIVE "))
        .unwrap_or_else(|| panic!("no receive step line in stdout: {stdout}"));
    assert!(
        line.contains(&format!("kafka://{bootstrap}/{topic} partition=0 offset=")),
        "{line}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("GET ") && line.contains("/events/43")),
        "{stdout}"
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
}

#[test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up --secure)"]
fn kafka_cli_criterion_failure_exits_with_its_code_and_sends_no_http() {
    let bootstrap = bootstrap();
    let topic = unique_topic("criterion");
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    let server = HttpServer::start();
    fixture.openapi(&server.base_url);
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let run = LiveRun::spawn(
        &["run", spec.to_str().unwrap(), "followup", "--verbose"],
        &[],
    );
    run.wait_armed();
    produce(&topic, "source:cli-test", r#"{"id":0}"#);
    let (status, value, _, _) = run.finish();
    assert!(!status.success());
    assert_error(&value, "RUNTIME_SUCCESS_CRITERIA_FAILED");
    assert_eq!(server.hits.load(Ordering::SeqCst), 0);
}

#[test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up --secure)"]
fn kafka_cli_timeouts_exit_with_their_codes_and_leave_the_broker_clean() {
    let bootstrap = bootstrap();
    let topic = unique_topic("timeouts");
    let fixture = Fixture::new();
    fixture.asyncapi(&bootstrap, &topic);
    fixture.openapi("http://127.0.0.1:1");
    let baseline = connections();

    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let run = LiveRun::spawn(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--verbose",
            "--execution-timeout",
            "2s",
        ],
        &[],
    );
    run.wait_armed();
    let (status, value, _, elapsed) = run.finish();
    assert!(!status.success());
    assert_error(&value, "RUNTIME_EXECUTION_TIMEOUT");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert!(
        broker(&["groups"], None).is_empty(),
        "no consumer group may exist"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = connections();
        if now <= baseline {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "connections did not return to baseline {baseline} (now {now})"
        );
        thread::sleep(Duration::from_millis(200));
    }

    let spec = fixture.arazzo(RECEIVE_WITH_STEP_TIMEOUT);
    let run = LiveRun::spawn(
        &["run", spec.to_str().unwrap(), "followup", "--verbose"],
        &[],
    );
    run.wait_armed();
    let (status, value, _, elapsed) = run.finish();
    assert!(!status.success());
    assert_error(&value, "RUNTIME_MESSAGE_RECEIVE_TIMEOUT");
    assert!(elapsed < Duration::from_secs(6), "{elapsed:?}");
}

#[test]
#[ignore = "needs ARAZZO_KAFKA_SECURE_BOOTSTRAP and ARAZZO_KAFKA_CA_PEM (scripts/kafka/broker.sh up --secure)"]
fn kafka_cli_scram_over_tls_succeeds_and_a_wrong_password_fails_cleanly() {
    let bootstrap = bootstrap();
    let secure =
        std::env::var("ARAZZO_KAFKA_SECURE_BOOTSTRAP").expect("ARAZZO_KAFKA_SECURE_BOOTSTRAP");
    let ca_pem = std::env::var("ARAZZO_KAFKA_CA_PEM").expect("ARAZZO_KAFKA_CA_PEM");
    let topic = unique_topic("secure");
    let fixture = Fixture::new();
    fixture.asyncapi_two_servers(&bootstrap, &secure, &topic);
    let server = HttpServer::start();
    fixture.openapi(&server.base_url);
    let spec = fixture.arazzo(RECEIVE_THEN_HTTP);
    let trace = fixture.path("secure.trace.json");

    let run = LiveRun::spawn(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--verbose",
            "--asyncapi-server",
            "orders=kafka-secure",
            "--kafka-ca-cert",
            &ca_pem,
            "--trace",
            trace.to_str().unwrap(),
        ],
        &[(USERNAME_ENV, "arazzo"), (PASSWORD_ENV, "arazzo-secret")],
    );
    let armed = run.wait_armed();
    assert!(
        armed.contains(&secure),
        "armed on the secure listener: {armed}"
    );
    produce(&topic, "source:cli-test", r#"{"id":7}"#);
    let (status, value, stderr, _) = run.finish();
    assert!(status.success(), "{value}\n{stderr}");
    assert_eq!(value["outputs"]["orderId"], serde_json::json!(7));
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    for artefact in [
        value.to_string(),
        stderr,
        fs::read_to_string(&trace).unwrap(),
    ] {
        assert!(
            !artefact.contains("arazzo-secret"),
            "password leaked: {artefact}"
        );
    }

    let wrong = fixture.path("wrong.trace.json");
    let (output, value) = run_json(
        &[
            "run",
            spec.to_str().unwrap(),
            "followup",
            "--verbose",
            "--asyncapi-server",
            "orders=kafka-secure",
            "--kafka-ca-cert",
            &ca_pem,
            "--trace",
            wrong.to_str().unwrap(),
        ],
        &[(USERNAME_ENV, "arazzo"), (PASSWORD_ENV, PASSWORD_LITERAL)],
    );
    assert!(!output.status.success());
    assert_error(&value, "RUNTIME_MESSAGE_TRANSPORT");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let trace_text = fs::read_to_string(&wrong).unwrap_or_default();
    for artefact in [value.to_string(), stderr, trace_text] {
        assert!(
            !artefact.contains(PASSWORD_LITERAL),
            "password leaked: {artefact}"
        );
    }
    assert_eq!(
        server.hits.load(Ordering::SeqCst),
        1,
        "no further HTTP request"
    );
}
