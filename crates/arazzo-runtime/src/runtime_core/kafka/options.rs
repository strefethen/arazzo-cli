//! Caller-supplied message-transport options and the one place a password
//! lives in the runtime.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// Default bound on preparation (connect, TLS, SASL, metadata, offsets).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// A credential that never formats itself. `Debug` and `Display` print
/// `[redacted]`; only the kafka connection code reads the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// How the engine connects to brokers named by AsyncAPI documents.
///
/// The default means plaintext, no credentials, bundled roots, a 30 s
/// preparation deadline, and no server selection — a document with one
/// plaintext Kafka server works with no options at all.
#[derive(Clone)]
pub struct MessageTransportOptions {
    /// SASL username; required together with `password` when the selected
    /// server declares a supported SASL scheme.
    pub username: Option<String>,
    /// SASL password. The CLI reads it from the environment only.
    pub password: Option<Secret>,
    /// Force TLS even when the server's protocol is `kafka` (a
    /// `kafka-secure` server always uses TLS).
    pub tls: bool,
    /// Additional PEM certificate(s) trusted for broker verification, on top
    /// of the bundled Mozilla roots.
    pub ca_pem: Option<Vec<u8>>,
    /// Bound on connect, TLS, SASL, metadata, and offset snapshot per
    /// arming pass; also the client's retry-backoff deadline.
    pub connect_timeout: Duration,
    /// Source name → server key, for channels available on several eligible
    /// Kafka servers.
    pub server_selection: BTreeMap<String, String>,
}

impl Default for MessageTransportOptions {
    fn default() -> Self {
        Self {
            username: None,
            password: None,
            tls: false,
            ca_pem: None,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            server_selection: BTreeMap::new(),
        }
    }
}

impl fmt::Debug for MessageTransportOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageTransportOptions")
            .field("username", &self.username)
            .field("password", &self.password)
            .field("tls", &self.tls)
            .field("ca_pem_bytes", &self.ca_pem.as_ref().map(Vec::len))
            .field("connect_timeout", &self.connect_timeout)
            .field("server_selection", &self.server_selection)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_never_formats_its_value() {
        let secret = Secret::new("hunter2");
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(secret.to_string(), "[redacted]");
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn options_debug_hides_password_and_ca_bytes() {
        let options = MessageTransportOptions {
            username: Some("alice".into()),
            password: Some(Secret::new("hunter2")),
            ca_pem: Some(b"-----BEGIN CERTIFICATE-----".to_vec()),
            ..MessageTransportOptions::default()
        };
        let rendered = format!("{options:?}");
        assert!(rendered.contains("alice"));
        assert!(rendered.contains("[redacted]"));
        assert!(!rendered.contains("hunter2"));
        assert!(!rendered.contains("BEGIN CERTIFICATE"));
        assert!(rendered.contains("ca_pem_bytes: Some(27)"));
    }

    #[test]
    fn default_is_plaintext_without_credentials() {
        let options = MessageTransportOptions::default();
        assert!(options.username.is_none());
        assert!(options.password.is_none());
        assert!(!options.tls);
        assert!(options.ca_pem.is_none());
        assert_eq!(options.connect_timeout, Duration::from_secs(30));
        assert!(options.server_selection.is_empty());
    }
}
