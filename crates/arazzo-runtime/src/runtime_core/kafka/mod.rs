//! Kafka receive client: connection, arming, and next-record delivery.
//!
//! This module knows nothing about Arazzo steps. The engine's message
//! executor resolves a step to a [`KafkaEndpoint`] and a topic, connects,
//! arms the topic, and asks for the next record under a deadline and a
//! cancellation token. Everything here is bounded and cancel-first.
//!
//! Contract (decisions D4–D8 of
//! `plans/assessments/arazzo-first-asyncapi-transport-decision.md`):
//!
//! - **No consumer group, no committed offsets.** Each run is independent and
//!   leaves nothing on the broker. Close is drop.
//! - **List before arm.** [`arm`] lists all topics first (an all-topics
//!   metadata request names no topic and cannot trigger auto-creation) and
//!   refuses a topic that does not exist. What it cannot prevent is the
//!   client's own topic-specific metadata refreshes (at arming and on
//!   reconnect or leader change) carrying the protocol's default
//!   allow-auto-creation flag, so a topic deleted *during* a run may be
//!   re-created by a broker configured to allow it.
//! - **Cursors per invocation.** An [`ArmedTopic`] starts at the end offset of
//!   every partition at arming time and advances past each record it hands
//!   out. Partitions are polled round-robin; no ordering across partitions is
//!   promised. A cursor the broker has since trimmed is a transport error,
//!   never a silent re-seek.
//! - **Bounded preparation.** Connect, metadata, and offset snapshot run under
//!   a caller-supplied deadline; the client's retry backoff shares it.
//! - **Cleanup bound, stated exactly.** This module spawns no task. The
//!   client's connections close when it is dropped, except that a request
//!   whose bytes were still being written when cancellation struck is
//!   completed by the client's own detached task, which holds that connection
//!   until the write finishes or the socket fails.
//! - **Unsupported, documented.** Transactional producers: records from
//!   aborted transactions are delivered unfiltered, and the arming offset is
//!   the last *stable* offset. Duplicate header names collapse to the last
//!   value at the client boundary.

mod connection;
#[cfg(test)]
mod live_tests;
mod options;
mod receiver;

pub(super) use connection::{connect, ConnectionIdentity, KafkaEndpoint, SaslMechanism};
pub use options::{MessageTransportOptions, Secret, DEFAULT_CONNECT_TIMEOUT};
pub(super) use receiver::{arm, ArmedTopic, ReceiveOutcome, ReceivedRecord};
