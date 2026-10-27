//! Endpoint description, option mapping, and the bounded connect.
//!
//! [`plan`] is pure and decides everything that can be decided without a
//! socket — bootstrap address, TLS, SASL mechanism, and every configuration
//! refusal — so the hermetic tests cover the mapping table completely.
//! [`connect`] executes a plan under the preparation deadline.

use std::sync::Arc;
use std::time::Duration;

use rskafka::client::{Client, ClientBuilder, Credentials, SaslConfig};
use rskafka::BackoffConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::CertificateDer;
use tokio_util::sync::CancellationToken;

use super::options::MessageTransportOptions;
use crate::runtime_core::error::{RuntimeError, RuntimeErrorKind};

/// Kafka's conventional plaintext/TLS port, appended when the authored host
/// has none.
pub(crate) const DEFAULT_PORT: u16 = 9092;

/// SASL mechanisms this runtime can perform, named as the AsyncAPI Security
/// Scheme `type` values that select them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaslMechanism {
    /// `type: plain` → SASL/PLAIN (RFC 4616).
    Plain,
    /// `type: scramSha256` → SCRAM-SHA-256 (RFC 7677).
    ScramSha256,
    /// `type: scramSha512` → SCRAM-SHA-512.
    ScramSha512,
}

impl SaslMechanism {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "PLAIN",
            Self::ScramSha256 => "SCRAM-SHA-256",
            Self::ScramSha512 => "SCRAM-SHA-512",
        }
    }
}

/// Which broker a receive target talks to. Two documents or servers that
/// happen to share a topic name never share state because this is part of
/// every cursor key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ConnectionIdentity {
    pub(crate) source_name: String,
    pub(crate) server_key: String,
    /// `host[:port]` as authored after server-variable substitution; the
    /// default port is appended by [`plan`], not here.
    pub(crate) host: String,
}

/// Everything the document decided about a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KafkaEndpoint {
    pub(crate) identity: ConnectionIdentity,
    /// `protocol: kafka-secure` on the selected server.
    pub(crate) tls_required: bool,
    /// First supported scheme in the server's `security` list.
    pub(crate) sasl: Option<SaslMechanism>,
}

/// A connect decision: what [`connect`] will do, inspectable without a
/// socket.
pub(crate) struct ConnectionPlan {
    pub(crate) bootstrap: String,
    pub(crate) tls: Option<Arc<rustls::ClientConfig>>,
    pub(crate) mechanism: Option<SaslMechanism>,
    sasl: Option<SaslConfig>,
    pub(crate) deadline: Duration,
}

impl std::fmt::Debug for ConnectionPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPlan")
            .field("bootstrap", &self.bootstrap)
            .field("tls", &self.tls.is_some())
            .field("mechanism", &self.mechanism)
            .field("deadline", &self.deadline)
            .finish()
    }
}

fn config_error(message: String) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::MessageTransportConfig, message)
}

/// The client stops retrying this long before the outer connect timeout:
/// one second earlier, but never less than half the timeout.
fn retry_deadline(connect_timeout: Duration) -> Duration {
    connect_timeout
        .saturating_sub(Duration::from_secs(1))
        .max(connect_timeout / 2)
}

/// Appends the default port when the authored host has none. Bracketed IPv6
/// literals keep their brackets.
pub(crate) fn bootstrap_address(host: &str) -> String {
    let has_port = if let Some(rest) = host.strip_prefix('[') {
        rest.contains("]:")
    } else {
        host.matches(':').count() == 1
    };
    if has_port {
        host.to_string()
    } else {
        format!("{host}:{DEFAULT_PORT}")
    }
}

/// Decides the connection without opening one.
pub(crate) fn plan(
    endpoint: &KafkaEndpoint,
    options: &MessageTransportOptions,
) -> Result<ConnectionPlan, RuntimeError> {
    let host = &endpoint.identity.host;
    let server = format!(
        "server \"{}\" of sourceDescription \"{}\" ({host})",
        endpoint.identity.server_key, endpoint.identity.source_name
    );

    let credentials = match (&options.username, &options.password) {
        (Some(username), Some(password)) => Some(Credentials::new(
            username.clone(),
            password.expose().to_string(),
        )),
        (None, None) => None,
        (Some(_), None) => {
            return Err(config_error(
                "Kafka credentials are incomplete: a username was supplied without a password \
                 (set ARAZZO_KAFKA_PASSWORD)"
                    .to_string(),
            ));
        }
        (None, Some(_)) => {
            return Err(config_error(
                "Kafka credentials are incomplete: a password was supplied without a username \
                 (set ARAZZO_KAFKA_USERNAME or pass --kafka-username)"
                    .to_string(),
            ));
        }
    };

    let (mechanism, sasl) = match (endpoint.sasl, credentials) {
        (Some(mechanism), Some(credentials)) => {
            let config = match mechanism {
                SaslMechanism::Plain => SaslConfig::Plain(credentials),
                SaslMechanism::ScramSha256 => SaslConfig::ScramSha256(credentials),
                SaslMechanism::ScramSha512 => SaslConfig::ScramSha512(credentials),
            };
            (Some(mechanism), Some(config))
        }
        (Some(mechanism), None) => {
            return Err(config_error(format!(
                "{server} requires SASL {} credentials; set ARAZZO_KAFKA_USERNAME and \
                 ARAZZO_KAFKA_PASSWORD",
                mechanism.as_str()
            )));
        }
        (None, Some(_)) => {
            return Err(config_error(format!(
                "Kafka credentials were supplied but {server} declares no supported SASL \
                 security scheme (plain, scramSha256, scramSha512); remove the credentials or \
                 declare the scheme in the AsyncAPI document"
            )));
        }
        (None, None) => (None, None),
    };

    // A supplied CA bundle is validated whether or not this endpoint uses
    // TLS: a bad file is a configuration error the user must see, not one
    // that waits for the first secure server to surface it.
    let roots = root_store(options.ca_pem.as_deref())?;
    let tls = if endpoint.tls_required || options.tls {
        Some(Arc::new(tls_config(roots)?))
    } else {
        None
    };

    Ok(ConnectionPlan {
        bootstrap: bootstrap_address(host),
        tls,
        mechanism,
        sasl,
        deadline: options.connect_timeout,
    })
}

fn root_store(ca_pem: Option<&[u8]>) -> Result<rustls::RootCertStore, RuntimeError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = ca_pem {
        let mut added = 0usize;
        for cert in CertificateDer::pem_slice_iter(pem) {
            let cert = cert.map_err(|err| {
                config_error(format!(
                    "Kafka CA certificate bundle is not valid PEM: {err}"
                ))
            })?;
            roots.add(cert).map_err(|err| {
                config_error(format!(
                    "Kafka CA certificate bundle contains an unusable certificate: {err}"
                ))
            })?;
            added += 1;
        }
        if added == 0 {
            return Err(config_error(
                "Kafka CA certificate bundle contains no CERTIFICATE blocks".to_string(),
            ));
        }
    }
    Ok(roots)
}

fn tls_config(roots: rustls::RootCertStore) -> Result<rustls::ClientConfig, RuntimeError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| config_error(format!("Kafka TLS configuration failed: {err}")))
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
}

/// Opens a bootstrap connection under the plan's deadline. Cancellation wins
/// over a connection that is about to succeed.
pub(crate) async fn connect(
    endpoint: &KafkaEndpoint,
    options: &MessageTransportOptions,
    client_id: &str,
    cancel: &CancellationToken,
) -> Result<Arc<Client>, RuntimeError> {
    let plan = plan(endpoint, options)?;
    let host = &endpoint.identity.host;
    let mut builder = ClientBuilder::new(vec![plan.bootstrap.clone()])
        .client_id(client_id)
        // Bounded retries: a freshly created topic can still be electing its
        // leader when arming starts, and the client's default backoff grows by
        // a factor of three (0.1 s, 0.3 s, 0.9 s, 2.7 s, 8.1 s, …), so one
        // unlucky retry would overshoot a short connect timeout. Cap each
        // pause at one second, and stop retrying before the outer timeout so
        // the client's own error — which names the cause (refused connection,
        // untrusted certificate, failed authentication) — is what the user
        // sees, not the outer "did not complete" message.
        .backoff_config(BackoffConfig {
            init_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(1),
            base: 2.0,
            deadline: Some(retry_deadline(plan.deadline)),
        });
    if let Some(tls) = plan.tls.clone() {
        builder = builder.tls_config(tls);
    }
    if let Some(sasl) = plan.sasl {
        builder = builder.sasl_config(sasl);
    }
    let stage = match (plan.tls.is_some(), plan.mechanism) {
        (true, Some(mechanism)) => format!("TLS + SASL {}", mechanism.as_str()),
        (true, None) => "TLS".to_string(),
        (false, Some(mechanism)) => format!("SASL {}", mechanism.as_str()),
        (false, None) => "plaintext".to_string(),
    };

    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(RuntimeError::new(
            RuntimeErrorKind::ExecutionCancelled,
            format!("connecting to Kafka broker {host} was cancelled"),
        )),
        result = tokio::time::timeout(plan.deadline, builder.build()) => match result {
            Ok(Ok(client)) => Ok(Arc::new(client)),
            Ok(Err(err)) => Err(RuntimeError::new(
                RuntimeErrorKind::MessageTransport,
                format!("connecting to Kafka broker {host} ({stage}) failed: {err}"),
            )),
            Err(_) => Err(RuntimeError::new(
                RuntimeErrorKind::MessageTransport,
                format!(
                    "connecting to Kafka broker {host} ({stage}) did not complete within {:?} \
                     (--kafka-connect-timeout)",
                    plan.deadline
                ),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Mapping-table assertions on known-good inputs.

    use super::*;
    use crate::runtime_core::kafka::options::Secret;

    fn endpoint(tls_required: bool, sasl: Option<SaslMechanism>) -> KafkaEndpoint {
        KafkaEndpoint {
            identity: ConnectionIdentity {
                source_name: "orders".into(),
                server_key: "local".into(),
                host: "broker.example".into(),
            },
            tls_required,
            sasl,
        }
    }

    fn with_credentials() -> MessageTransportOptions {
        MessageTransportOptions {
            username: Some("arazzo".into()),
            password: Some(Secret::new("arazzo-secret")),
            ..MessageTransportOptions::default()
        }
    }

    #[test]
    fn default_port_is_appended_only_when_absent() {
        assert_eq!(bootstrap_address("broker"), "broker:9092");
        assert_eq!(bootstrap_address("broker:19092"), "broker:19092");
        assert_eq!(bootstrap_address("[::1]"), "[::1]:9092");
        assert_eq!(bootstrap_address("[::1]:19092"), "[::1]:19092");
    }

    #[test]
    fn plaintext_without_credentials_or_sasl() {
        let plan = plan(&endpoint(false, None), &MessageTransportOptions::default()).unwrap();
        assert_eq!(plan.bootstrap, "broker.example:9092");
        assert!(plan.tls.is_none());
        assert_eq!(plan.mechanism, None);
        assert_eq!(plan.deadline, Duration::from_secs(30));
    }

    #[test]
    fn plain_scheme_with_credentials_selects_plain() {
        let plan = plan(
            &endpoint(false, Some(SaslMechanism::Plain)),
            &with_credentials(),
        )
        .unwrap();
        assert_eq!(plan.mechanism, Some(SaslMechanism::Plain));
        assert!(matches!(plan.sasl, Some(SaslConfig::Plain(_))));
        assert!(plan.tls.is_none());
    }

    #[test]
    fn scram_schemes_select_their_mechanism() {
        for (mechanism, expect) in [
            (SaslMechanism::ScramSha256, "SCRAM-SHA-256"),
            (SaslMechanism::ScramSha512, "SCRAM-SHA-512"),
        ] {
            let plan = plan(&endpoint(false, Some(mechanism)), &with_credentials()).unwrap();
            assert_eq!(plan.mechanism.map(SaslMechanism::as_str), Some(expect));
        }
    }

    #[test]
    fn sasl_scheme_without_credentials_is_a_config_error_naming_the_variables() {
        let err = plan(
            &endpoint(false, Some(SaslMechanism::ScramSha256)),
            &MessageTransportOptions::default(),
        )
        .unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::MessageTransportConfig);
        assert!(err.message.contains("ARAZZO_KAFKA_USERNAME"));
        assert!(err.message.contains("ARAZZO_KAFKA_PASSWORD"));
        assert!(err.message.contains("SCRAM-SHA-256"));
    }

    #[test]
    fn credentials_without_a_scheme_are_a_config_error() {
        let err = plan(&endpoint(false, None), &with_credentials()).unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::MessageTransportConfig);
        assert!(err.message.contains("declares no supported SASL"));
        assert!(!err.message.contains("arazzo-secret"));
    }

    #[test]
    fn half_credentials_are_a_config_error() {
        let username_only = MessageTransportOptions {
            username: Some("arazzo".into()),
            ..MessageTransportOptions::default()
        };
        let err = plan(&endpoint(false, Some(SaslMechanism::Plain)), &username_only).unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::MessageTransportConfig);
        assert!(err.message.contains("without a password"));

        let password_only = MessageTransportOptions {
            password: Some(Secret::new("x")),
            ..MessageTransportOptions::default()
        };
        let err = plan(&endpoint(false, Some(SaslMechanism::Plain)), &password_only).unwrap_err();
        assert!(err.message.contains("without a username"));
    }

    #[test]
    fn tls_is_enabled_by_kafka_secure_or_by_option() {
        let plan_required =
            plan(&endpoint(true, None), &MessageTransportOptions::default()).unwrap();
        assert!(plan_required.tls.is_some());
        let opted_in = MessageTransportOptions {
            tls: true,
            ..MessageTransportOptions::default()
        };
        let plan_opted = plan(&endpoint(false, None), &opted_in).unwrap();
        assert!(plan_opted.tls.is_some());
    }

    #[test]
    fn bad_or_empty_ca_bundle_is_a_config_error() {
        for (pem, needle) in [
            (b"not a pem".to_vec(), "no CERTIFICATE blocks"),
            (
                b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_vec(),
                "unusable certificate",
            ),
        ] {
            let options = MessageTransportOptions {
                ca_pem: Some(pem),
                ..MessageTransportOptions::default()
            };
            let err = plan(&endpoint(true, None), &options).unwrap_err();
            assert_eq!(
                err.kind,
                RuntimeErrorKind::MessageTransportConfig,
                "{needle}"
            );
            assert!(err.message.contains(needle), "{}", err.message);
        }
    }

    #[test]
    fn bad_ca_bundle_is_refused_even_for_a_plaintext_endpoint() {
        let options = MessageTransportOptions {
            ca_pem: Some(b"not a pem".to_vec()),
            ..MessageTransportOptions::default()
        };
        let err = plan(&endpoint(false, None), &options).unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::MessageTransportConfig);
        assert!(
            err.message.contains("no CERTIFICATE blocks"),
            "{}",
            err.message
        );
    }

    #[test]
    fn plan_debug_never_shows_credentials() {
        let plan = plan(
            &endpoint(true, Some(SaslMechanism::Plain)),
            &with_credentials(),
        )
        .unwrap();
        let rendered = format!("{plan:?}");
        assert!(!rendered.contains("arazzo-secret"));
        assert!(rendered.contains("mechanism: Some(Plain)"));
    }

    #[tokio::test]
    async fn connect_to_a_closed_port_fails_within_the_deadline() {
        let endpoint = KafkaEndpoint {
            identity: ConnectionIdentity {
                source_name: "orders".into(),
                server_key: "local".into(),
                host: "127.0.0.1:1".into(),
            },
            tls_required: false,
            sasl: None,
        };
        let options = MessageTransportOptions {
            connect_timeout: Duration::from_millis(300),
            ..MessageTransportOptions::default()
        };
        let started = std::time::Instant::now();
        let err = connect(&endpoint, &options, "test", &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::MessageTransport);
        assert!(err.message.contains("127.0.0.1:1"), "{}", err.message);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn connect_returns_promptly_when_cancelled() {
        let endpoint = KafkaEndpoint {
            identity: ConnectionIdentity {
                source_name: "orders".into(),
                server_key: "local".into(),
                host: "127.0.0.1:1".into(),
            },
            tls_required: false,
            sasl: None,
        };
        let options = MessageTransportOptions {
            connect_timeout: Duration::from_secs(30),
            ..MessageTransportOptions::default()
        };
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token.cancel();
        });
        let started = std::time::Instant::now();
        let err = connect(&endpoint, &options, "test", &cancel)
            .await
            .unwrap_err();
        assert_eq!(err.kind, RuntimeErrorKind::ExecutionCancelled);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }
}
