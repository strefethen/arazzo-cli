//! Live client tests against the pinned broker started by
//! `scripts/kafka/broker.sh up [--secure]`. Every test is `#[ignore]`; the
//! ordinary suite never touches a broker. Run with:
//!
//! ```sh
//! scripts/kafka/broker.sh up --secure
//! ARAZZO_KAFKA_BOOTSTRAP=127.0.0.1:19092 \
//! ARAZZO_KAFKA_SECURE_BOOTSTRAP=127.0.0.1:19093 \
//! ARAZZO_KAFKA_CA_PEM=/tmp/arazzo-kafka/ca.pem \
//!   cargo test -p arazzo-runtime --lib kafka -- --ignored --test-threads=1
//! scripts/kafka/broker.sh down
//! ```
//!
//! A missing variable is a failed test, never a skip.
#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture steps use labelled failures.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rskafka::client::partition::{Compression, UnknownTopicHandling};
use rskafka::client::Client;
use rskafka::record::Record;
use tokio_util::sync::CancellationToken;

use super::{
    arm, connect, ArmedTopic, ConnectionIdentity, KafkaEndpoint, MessageTransportOptions,
    ReceiveOutcome, SaslMechanism, Secret,
};
use crate::runtime_core::error::RuntimeErrorKind;

const SCRAM_USER: &str = "arazzo";
const SCRAM_PASSWORD: &str = "arazzo-secret";

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} must be set for the live Kafka tests (see scripts/kafka/broker.sh)")
    })
}

fn broker_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/kafka/broker.sh")
        .canonicalize()
        .expect("scripts/kafka/broker.sh exists")
}

fn script(args: &[&str], stdin: Option<&str>) -> String {
    let mut command = Command::new(broker_script());
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn broker.sh");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().expect("broker.sh completes");
    assert!(
        output.status.success(),
        "broker.sh {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn unique_topic(label: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    format!(
        "arazzo-live-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn endpoint(host: &str, tls_required: bool, sasl: Option<SaslMechanism>) -> KafkaEndpoint {
    KafkaEndpoint {
        identity: ConnectionIdentity {
            source_name: "orders".into(),
            server_key: if tls_required {
                "secure".into()
            } else {
                "local".into()
            },
            host: host.to_string(),
        },
        tls_required,
        sasl,
    }
}

fn plain_options() -> MessageTransportOptions {
    MessageTransportOptions {
        connect_timeout: Duration::from_secs(10),
        ..MessageTransportOptions::default()
    }
}

fn secure_options(password: &str, ca: Option<Vec<u8>>) -> MessageTransportOptions {
    MessageTransportOptions {
        username: Some(SCRAM_USER.into()),
        password: Some(Secret::new(password)),
        ca_pem: ca,
        connect_timeout: Duration::from_secs(10),
        ..MessageTransportOptions::default()
    }
}

async fn plain_client() -> (String, Arc<Client>) {
    let host = required_env("ARAZZO_KAFKA_BOOTSTRAP");
    let client = connect(
        &endpoint(&host, false, None),
        &plain_options(),
        "arazzo-live",
        &CancellationToken::new(),
    )
    .await
    .expect("connect to the plaintext listener");
    (host, client)
}

/// Creates the topic and waits until every partition has a leader that
/// answers an offset query: creation is asynchronous on the broker, and a
/// producer or receiver that follows immediately can otherwise see
/// `UnknownTopicOrPartition` or a missing leader.
async fn create_topic(client: &Client, topic: &str, partitions: i32) {
    client
        .controller_client()
        .unwrap()
        .create_topic(topic, partitions, 1, 10_000)
        .await
        .expect("create topic");
    let deadline = Instant::now() + Duration::from_secs(20);
    for partition in 0..partitions {
        loop {
            let ready = match client
                .partition_client(topic, partition, UnknownTopicHandling::Retry)
                .await
            {
                Ok(pc) => pc
                    .get_offset(rskafka::client::partition::OffsetAt::Latest)
                    .await
                    .is_ok(),
                Err(_) => false,
            };
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "topic {topic} partition {partition} never became ready"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

async fn delete_topic(client: &Client, topic: &str) {
    let _ = client
        .controller_client()
        .unwrap()
        .delete_topic(topic, 10_000)
        .await;
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

async fn produce(
    client: &Client,
    topic: &str,
    partition: i32,
    key: Option<&str>,
    value: &str,
    headers: &[(&str, &str)],
) -> i64 {
    let pc = client
        .partition_client(topic, partition, UnknownTopicHandling::Error)
        .await
        .expect("partition client");
    let record = Record {
        key: key.map(|k| k.as_bytes().to_vec()),
        value: Some(value.as_bytes().to_vec()),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>(),
        timestamp: rskafka::chrono::DateTime::from_timestamp_millis(now_ms()).unwrap(),
    };
    let offsets = pc
        .produce(vec![record], Compression::NoCompression)
        .await
        .expect("produce");
    offsets[0]
}

async fn arm_topic(client: &Client, host: &str, topic: &str) -> ArmedTopic {
    arm(
        client,
        host,
        topic,
        Instant::now() + Duration::from_secs(10),
        &CancellationToken::new(),
    )
    .await
    .expect("arm")
}

fn established_connections() -> usize {
    script(&["connections"], None)
        .lines()
        .next()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_arm_refuses_a_missing_topic_without_creating_it() {
    let (host, client) = plain_client().await;
    let topic = unique_topic("missing");
    let err = arm(
        &client,
        &host,
        &topic,
        Instant::now() + Duration::from_secs(10),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, RuntimeErrorKind::MessageTransport);
    assert!(err.message.contains(&topic), "{}", err.message);
    assert!(
        err.message.contains("never creates topics"),
        "{}",
        err.message
    );
    // The harness broker allows auto-creation; the listing must not have
    // created the topic.
    let topics = client.list_topics().await.unwrap();
    assert!(
        !topics.iter().any(|t| t.name == topic),
        "topic was auto-created by arming"
    );
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_records_before_arming_are_skipped_and_records_after_are_delivered() {
    let (host, client) = plain_client().await;
    let topic = unique_topic("arming");
    create_topic(&client, &topic, 2).await;
    produce(&client, &topic, 1, None, r#"{"id":1,"old":true}"#, &[]).await;

    let mut armed = arm_topic(&client, &host, &topic).await;
    assert_eq!(armed.partition_count(), 2);
    assert_eq!(armed.cursors(), vec![(0, 0), (1, 1)]);

    let before = now_ms();
    let offset = produce(
        &client,
        &topic,
        1,
        Some("k"),
        r#"{"id":42}"#,
        &[("content-type", "application/json")],
    )
    .await;
    let outcome = armed
        .receive_next(
            Some(Instant::now() + Duration::from_secs(10)),
            &CancellationToken::new(),
        )
        .await;
    let ReceiveOutcome::Received(record) = outcome else {
        panic!("expected a record, got {outcome:?}");
    };
    assert_eq!(record.topic, topic);
    assert_eq!(record.partition, 1);
    assert_eq!(record.offset, offset);
    assert_eq!(record.key.as_deref(), Some(&b"k"[..]));
    assert_eq!(record.value.as_deref(), Some(&br#"{"id":42}"#[..]));
    assert_eq!(
        record.headers.get("content-type").map(Vec::as_slice),
        Some(&b"application/json"[..])
    );
    assert!(
        record.timestamp_ms >= before - 1_000,
        "timestamp {} before {before}",
        record.timestamp_ms
    );
    assert_eq!(armed.cursors(), vec![(0, 0), (1, offset + 1)]);
    delete_topic(&client, &topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_consecutive_receives_return_consecutive_records() {
    let (host, client) = plain_client().await;
    let topic = unique_topic("consecutive");
    create_topic(&client, &topic, 1).await;
    let mut armed = arm_topic(&client, &host, &topic).await;
    let first = produce(&client, &topic, 0, None, r#"{"n":1}"#, &[]).await;
    let second = produce(&client, &topic, 0, None, r#"{"n":2}"#, &[]).await;
    assert_eq!(second, first + 1);
    for expected in [first, second] {
        let outcome = armed
            .receive_next(
                Some(Instant::now() + Duration::from_secs(10)),
                &CancellationToken::new(),
            )
            .await;
        let ReceiveOutcome::Received(record) = outcome else {
            panic!("expected a record, got {outcome:?}");
        };
        assert_eq!(record.offset, expected);
    }
    delete_topic(&client, &topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_every_enabled_codec_decodes_and_zstd_is_refused_explicitly() {
    let (host, client) = plain_client().await;
    let topic = unique_topic("codecs");
    create_topic(&client, &topic, 1).await;
    let mut armed = arm_topic(&client, &host, &topic).await;
    for codec in ["gzip", "snappy", "lz4"] {
        let payload = format!(r#"{{"codec":"{codec}"}}"#);
        script(
            &["produce", &topic, "--codec", codec],
            Some(&format!("codec:{codec}\t{payload}\n")),
        );
        let outcome = armed
            .receive_next(
                Some(Instant::now() + Duration::from_secs(10)),
                &CancellationToken::new(),
            )
            .await;
        let ReceiveOutcome::Received(record) = outcome else {
            panic!("{codec}: expected a record, got {outcome:?}");
        };
        assert_eq!(record.value.as_deref(), Some(payload.as_bytes()), "{codec}");
        assert_eq!(
            record.headers.get("codec").map(Vec::as_slice),
            Some(codec.as_bytes()),
            "{codec}"
        );
    }
    // The contracted `produce <topic> --key <k>` form: every line carries the
    // fixed key after its headers, byte for byte — a backslash sequence in
    // the key must not be interpreted (round-2 review regression).
    script(
        &["produce", &topic, "--key", "order\\next"],
        Some("kind:keyed\t{\"keyed\":true}\n"),
    );
    let outcome = armed
        .receive_next(
            Some(Instant::now() + Duration::from_secs(10)),
            &CancellationToken::new(),
        )
        .await;
    let ReceiveOutcome::Received(record) = outcome else {
        panic!("keyed: expected a record, got {outcome:?}");
    };
    assert_eq!(record.key.as_deref(), Some(&b"order\\next"[..]));
    assert_eq!(record.value.as_deref(), Some(&br#"{"keyed":true}"#[..]));
    assert_eq!(
        record.headers.get("kind").map(Vec::as_slice),
        Some(&b"keyed"[..])
    );

    // zstd is compiled out (unpatched advisories in the only zstd line rskafka
    // 0.6.0 accepts); a zstd batch must fail visibly, never hang or vanish.
    script(
        &["produce", &topic, "--codec", "zstd"],
        Some("codec:zstd\t{\"codec\":\"zstd\"}\n"),
    );
    let outcome = armed
        .receive_next(
            Some(Instant::now() + Duration::from_secs(10)),
            &CancellationToken::new(),
        )
        .await;
    let ReceiveOutcome::Transport(err) = outcome else {
        panic!("zstd: expected an explicit transport error, got {outcome:?}");
    };
    assert_eq!(err.kind, RuntimeErrorKind::MessageTransport);
    assert!(
        err.message.to_ascii_lowercase().contains("zstd"),
        "{}",
        err.message
    );
    delete_topic(&client, &topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_deadline_and_cancellation_are_distinct_and_prompt() {
    let (host, client) = plain_client().await;
    let topic = unique_topic("deadline");
    create_topic(&client, &topic, 2).await;
    let mut armed = arm_topic(&client, &host, &topic).await;

    let started = Instant::now();
    let outcome = armed
        .receive_next(
            Some(started + Duration::from_millis(1_200)),
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(outcome, ReceiveOutcome::TimedOut), "{outcome:?}");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1_150) && elapsed <= Duration::from_millis(1_450),
        "deadline 1 200 ms must end within one fetch wait (250 ms): {elapsed:?}"
    );

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        token.cancel();
    });
    let started = Instant::now();
    let outcome = armed.receive_next(None, &cancel).await;
    assert!(matches!(outcome, ReceiveOutcome::Cancelled), "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_millis(700),
        "{:?}",
        started.elapsed()
    );
    delete_topic(&client, &topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_BOOTSTRAP (scripts/kafka/broker.sh up)"]
async fn kafka_dropping_the_client_closes_its_connections_while_the_runtime_lives() {
    let baseline = established_connections();
    let (host, client) = plain_client().await;
    let topic = unique_topic("cleanup");
    create_topic(&client, &topic, 1).await;
    let mut armed = arm_topic(&client, &host, &topic).await;
    let _ = armed
        .receive_next(
            Some(Instant::now() + Duration::from_millis(300)),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        established_connections() > baseline,
        "no connection observed while armed"
    );
    delete_topic(&client, &topic).await;
    drop(armed);
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if established_connections() <= baseline {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "connections did not close within 5 s of drop"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_SECURE_BOOTSTRAP and ARAZZO_KAFKA_CA_PEM (scripts/kafka/broker.sh up --secure)"]
async fn kafka_scram_over_tls_connects_and_receives() {
    let secure_host = required_env("ARAZZO_KAFKA_SECURE_BOOTSTRAP");
    let ca = std::fs::read(required_env("ARAZZO_KAFKA_CA_PEM")).expect("read CA bundle");
    let (plain_host, producer) = plain_client().await;
    let topic = unique_topic("secure");
    create_topic(&producer, &topic, 1).await;

    for mechanism in [SaslMechanism::ScramSha256, SaslMechanism::ScramSha512] {
        let client = connect(
            &endpoint(&secure_host, true, Some(mechanism)),
            &secure_options(SCRAM_PASSWORD, Some(ca.clone())),
            "arazzo-live-secure",
            &CancellationToken::new(),
        )
        .await
        .unwrap_or_else(|err| panic!("{}: {}", mechanism.as_str(), err.message));
        let mut armed = arm_topic(&client, &secure_host, &topic).await;
        let offset = produce(&producer, &topic, 0, None, r#"{"secure":true}"#, &[]).await;
        let outcome = armed
            .receive_next(
                Some(Instant::now() + Duration::from_secs(10)),
                &CancellationToken::new(),
            )
            .await;
        let ReceiveOutcome::Received(record) = outcome else {
            panic!("{}: expected a record, got {outcome:?}", mechanism.as_str());
        };
        assert_eq!(record.offset, offset);
    }
    let _ = plain_host;
    delete_topic(&producer, &topic).await;
}

#[tokio::test]
#[ignore = "needs ARAZZO_KAFKA_SECURE_BOOTSTRAP and ARAZZO_KAFKA_CA_PEM (scripts/kafka/broker.sh up --secure)"]
async fn kafka_wrong_password_and_untrusted_ca_fail_naming_the_stage() {
    let secure_host = required_env("ARAZZO_KAFKA_SECURE_BOOTSTRAP");
    let ca = std::fs::read(required_env("ARAZZO_KAFKA_CA_PEM")).expect("read CA bundle");

    let wrong = connect(
        &endpoint(&secure_host, true, Some(SaslMechanism::ScramSha256)),
        &secure_options("not-the-password", Some(ca)),
        "arazzo-live-secure",
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(wrong.kind, RuntimeErrorKind::MessageTransport);
    assert!(wrong.message.contains("SCRAM-SHA-256"), "{}", wrong.message);
    assert!(
        !wrong.message.contains("not-the-password"),
        "{}",
        wrong.message
    );
    assert!(!wrong.message.contains(SCRAM_PASSWORD), "{}", wrong.message);

    let untrusted = connect(
        &endpoint(&secure_host, true, Some(SaslMechanism::ScramSha256)),
        &secure_options(SCRAM_PASSWORD, None),
        "arazzo-live-secure",
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(untrusted.kind, RuntimeErrorKind::MessageTransport);
    assert!(untrusted.message.contains("TLS"), "{}", untrusted.message);
    assert!(
        untrusted
            .message
            .to_ascii_lowercase()
            .contains("certificate"),
        "{}",
        untrusted.message
    );
    assert!(
        !untrusted.message.contains(SCRAM_PASSWORD),
        "{}",
        untrusted.message
    );
}
