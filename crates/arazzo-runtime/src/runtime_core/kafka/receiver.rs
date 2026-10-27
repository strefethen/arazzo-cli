//! Arming a topic and delivering its next record.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rskafka::client::error::{Error as KafkaError, ProtocolError};
use rskafka::client::partition::{OffsetAt, PartitionClient, UnknownTopicHandling};
use rskafka::client::Client;
use rskafka::record::RecordAndOffset;
use tokio_util::sync::CancellationToken;

use crate::runtime_core::error::{RuntimeError, RuntimeErrorKind};

/// Longest a single fetch asks the broker to hold the request open. Short so
/// that deadlines and cancellation are honoured promptly.
const FETCH_WAIT: Duration = Duration::from_millis(250);
/// Largest batch one fetch may return.
const MAX_FETCH_BYTES: i32 = 1024 * 1024;

/// One record as the broker delivered it, before any JSON interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedRecord {
    pub(crate) topic: String,
    pub(crate) partition: i32,
    pub(crate) offset: i64,
    pub(crate) timestamp_ms: i64,
    pub(crate) key: Option<Vec<u8>>,
    pub(crate) value: Option<Vec<u8>>,
    /// Header names are UTF-8 at the wire boundary; duplicate names have
    /// already collapsed to the last value (client limitation, documented).
    pub(crate) headers: BTreeMap<String, Vec<u8>>,
}

/// How a wait ended.
#[derive(Debug)]
pub(crate) enum ReceiveOutcome {
    Received(ReceivedRecord),
    /// The caller's deadline passed with nothing fetched.
    TimedOut,
    /// The cancellation token fired.
    Cancelled,
    /// A fetch failed; the cursor is unusable after this.
    Transport(RuntimeError),
}

struct PartitionCursor {
    partition: i32,
    client: Arc<PartitionClient>,
    next_offset: i64,
}

/// A topic whose partition end offsets were snapshotted at arming. Hands out
/// records at or after those offsets, advancing one partition cursor per
/// record. Owned by the invocation that armed it.
pub(crate) struct ArmedTopic {
    host: String,
    topic: String,
    partitions: Vec<PartitionCursor>,
    next_index: usize,
}

impl std::fmt::Debug for ArmedTopic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArmedTopic")
            .field("host", &self.host)
            .field("topic", &self.topic)
            .field("cursors", &self.cursors())
            .finish()
    }
}

fn transport(message: String) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::MessageTransport, message)
}

/// Runs `fut` under the remaining time to `deadline`, cancel-first.
async fn bounded<T>(
    what: &str,
    host: &str,
    deadline: Instant,
    cancel: &CancellationToken,
    fut: impl std::future::Future<Output = Result<T, KafkaError>>,
) -> Result<T, RuntimeError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(transport(format!(
            "{what} on Kafka broker {host} did not start before the preparation deadline \
             (--kafka-connect-timeout)"
        )));
    }
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(RuntimeError::new(
            RuntimeErrorKind::ExecutionCancelled,
            format!("{what} on Kafka broker {host} was cancelled"),
        )),
        result = tokio::time::timeout(remaining, fut) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(transport(format!("{what} on Kafka broker {host} failed: {err}"))),
            Err(_) => Err(transport(format!(
                "{what} on Kafka broker {host} did not complete before the preparation deadline \
                 (--kafka-connect-timeout)"
            ))),
        },
    }
}

/// Confirms `topic` exists without naming it in a metadata request, then
/// snapshots every partition's end offset.
pub(crate) async fn arm(
    client: &Client,
    host: &str,
    topic: &str,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<ArmedTopic, RuntimeError> {
    // An all-topics listing names no topic, so a broker that allows
    // auto-creation has nothing to create. Refusing here is the runtime's
    // guarantee that it never asks for a topic that does not exist.
    let topics = bounded(
        "listing topics",
        host,
        deadline,
        cancel,
        client.list_topics(),
    )
    .await?;
    let listed = topics
        .into_iter()
        .find(|t| t.name == topic)
        .ok_or_else(|| {
            transport(format!(
            "topic \"{topic}\" does not exist on Kafka broker {host}; this runtime never creates \
             topics"
        ))
        })?;
    // `Topic::partitions` is an ordered set, so cursors come out sorted.
    let partition_ids: Vec<i32> = listed.partitions.into_iter().collect();
    if partition_ids.is_empty() {
        return Err(transport(format!(
            "topic \"{topic}\" on Kafka broker {host} reports no partitions"
        )));
    }

    let mut partitions = Vec::with_capacity(partition_ids.len());
    for partition in partition_ids {
        let partition_client = bounded(
            &format!("binding partition {partition} of topic \"{topic}\""),
            host,
            deadline,
            cancel,
            client.partition_client(topic, partition, UnknownTopicHandling::Error),
        )
        .await?;
        let partition_client = Arc::new(partition_client);
        let next_offset = bounded(
            &format!("reading the end offset of partition {partition} of topic \"{topic}\""),
            host,
            deadline,
            cancel,
            partition_client.get_offset(OffsetAt::Latest),
        )
        .await?;
        partitions.push(PartitionCursor {
            partition,
            client: partition_client,
            next_offset,
        });
    }

    Ok(ArmedTopic {
        host: host.to_string(),
        topic: topic.to_string(),
        partitions,
        next_index: 0,
    })
}

impl ArmedTopic {
    pub(crate) fn partition_count(&self) -> usize {
        self.partitions.len()
    }

    /// Current `(partition, next offset)` pairs, for diagnostics and tests.
    pub(crate) fn cursors(&self) -> Vec<(i32, i64)> {
        self.partitions
            .iter()
            .map(|cursor| (cursor.partition, cursor.next_offset))
            .collect()
    }

    /// Waits for the next record at or after the cursors. Partitions are
    /// polled round-robin from where the previous call stopped; the first
    /// non-empty batch wins and only that partition's cursor advances.
    ///
    /// Bounded exactly by the caller's `deadline` and `cancel`, nothing else
    /// (D8): a broker that stops answering mid-fetch is a wait the step
    /// `timeout` or the run's execution timeout ends, because rskafka 0.6.0
    /// has no request timeout of its own and this module adds no policy of
    /// its own.
    pub(crate) async fn receive_next(
        &mut self,
        deadline: Option<Instant>,
        cancel: &CancellationToken,
    ) -> ReceiveOutcome {
        loop {
            if cancel.is_cancelled() {
                return ReceiveOutcome::Cancelled;
            }
            let remaining = deadline.map(|at| at.saturating_duration_since(Instant::now()));
            if remaining.is_some_and(|left| left.is_zero()) {
                return ReceiveOutcome::TimedOut;
            }
            let wait = remaining.map_or(FETCH_WAIT, |left| left.min(FETCH_WAIT));
            let wait_ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);

            let index = self.next_index % self.partitions.len();
            self.next_index = (index + 1) % self.partitions.len();
            let cursor = &mut self.partitions[index];
            let offset = cursor.next_offset;
            let fetch = cursor
                .client
                .fetch_records(offset, 1..MAX_FETCH_BYTES, wait_ms);

            let fetched = match remaining {
                Some(left) => tokio::select! {
                    biased;
                    () = cancel.cancelled() => return ReceiveOutcome::Cancelled,
                    result = tokio::time::timeout(left, fetch) => match result {
                        Ok(result) => result,
                        Err(_) => return ReceiveOutcome::TimedOut,
                    },
                },
                None => tokio::select! {
                    biased;
                    () = cancel.cancelled() => return ReceiveOutcome::Cancelled,
                    result = fetch => result,
                },
            };
            match fetched {
                Ok((records, _high_watermark)) => {
                    if let Some(record) = earliest(records) {
                        cursor.next_offset = record.offset + 1;
                        return ReceiveOutcome::Received(ReceivedRecord {
                            topic: self.topic.clone(),
                            partition: cursor.partition,
                            offset: record.offset,
                            timestamp_ms: record.record.timestamp.timestamp_millis(),
                            key: record.record.key,
                            value: record.record.value,
                            headers: record.record.headers,
                        });
                    }
                    // Empty: fall through to the next partition.
                }
                Err(KafkaError::ServerError {
                    protocol_error: ProtocolError::OffsetOutOfRange,
                    ..
                }) => {
                    return ReceiveOutcome::Transport(transport(format!(
                        "offset {offset} of partition {} of topic \"{}\" on Kafka broker {} is no \
                         longer available (retention removed records this run had not read)",
                        cursor.partition, self.topic, self.host
                    )));
                }
                Err(err) => {
                    return ReceiveOutcome::Transport(transport(format!(
                        "fetching partition {} of topic \"{}\" on Kafka broker {} failed: {err}",
                        cursor.partition, self.topic, self.host
                    )));
                }
            }
        }
    }
}

fn earliest(records: Vec<RecordAndOffset>) -> Option<RecordAndOffset> {
    records.into_iter().min_by_key(|record| record.offset)
}
