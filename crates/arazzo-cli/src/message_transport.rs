//! Kafka/AsyncAPI transport flags shared by `run` and `test`: converts the
//! parsed flags and the environment-only password into the engine's
//! `MessageTransportOptions`, refusing malformed or partial combinations
//! before an engine is built. `transport.rs` owns the HTTP transport-trust
//! flags and stays untouched.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arazzo_runtime::{ExecutionObserver, MessageTransportOptions, ObserverEvent, Secret};

use crate::cli_error::CliError;

/// Environment variable holding the SASL username (also `--kafka-username`).
pub const USERNAME_ENV: &str = "ARAZZO_KAFKA_USERNAME";
/// Environment variable holding the SASL password. There is no flag: a
/// password on the command line would be visible to every process listing.
pub const PASSWORD_ENV: &str = "ARAZZO_KAFKA_PASSWORD";

/// Kafka/AsyncAPI flags accepted by `run` and `test`.
#[derive(Debug, Clone)]
pub struct MessageTransportFlags {
    pub username: Option<String>,
    /// Read from [`PASSWORD_ENV`] only; never printed.
    pub password: Option<Secret>,
    pub tls: bool,
    pub ca_cert: Option<PathBuf>,
    pub connect_timeout: Duration,
    /// Raw `--asyncapi-server <SOURCE=KEY>` values.
    pub server_selection: Vec<String>,
}

/// `--json` codes for flag-conversion failures; `run` and `test` use their
/// own prefixes like the spec and OpenAPI read errors do.
#[derive(Debug, Clone, Copy)]
pub struct MessageTransportCodes {
    pub credentials: &'static str,
    pub ca_cert_read: &'static str,
    pub server_flag: &'static str,
}

pub const RUN_MESSAGE_CODES: MessageTransportCodes = MessageTransportCodes {
    credentials: "RUN_KAFKA_CREDENTIALS",
    ca_cert_read: "RUN_KAFKA_CA_CERT_READ",
    server_flag: "RUN_ASYNCAPI_SERVER_FLAG",
};

pub const TEST_MESSAGE_CODES: MessageTransportCodes = MessageTransportCodes {
    credentials: "TEST_KAFKA_CREDENTIALS",
    ca_cert_read: "TEST_KAFKA_CA_CERT_READ",
    server_flag: "TEST_ASYNCAPI_SERVER_FLAG",
};

impl MessageTransportFlags {
    /// Reads the password from the environment once. The value is wrapped
    /// before it can reach any `Debug` output.
    pub fn password_from_env() -> Option<Secret> {
        std::env::var_os(PASSWORD_ENV).map(|raw| Secret::new(raw.to_string_lossy().into_owned()))
    }

    /// Validates the flags and reads the CA file. Fails before any engine
    /// exists, so a typo never costs a broker round trip.
    pub fn into_options(
        self,
        codes: &MessageTransportCodes,
    ) -> Result<MessageTransportOptions, CliError> {
        match (&self.username, &self.password) {
            (Some(_), None) => {
                return Err(CliError::new(
                    codes.credentials,
                    format!(
                        "Kafka credentials are incomplete: --kafka-username/{USERNAME_ENV} was \
                         supplied without {PASSWORD_ENV} (the password is read from the \
                         environment only)"
                    ),
                ));
            }
            (None, Some(_)) => {
                return Err(CliError::new(
                    codes.credentials,
                    format!(
                        "Kafka credentials are incomplete: {PASSWORD_ENV} is set without a \
                         username (pass --kafka-username or set {USERNAME_ENV})"
                    ),
                ));
            }
            _ => {}
        }
        let ca_pem = match &self.ca_cert {
            Some(path) => Some(fs::read(path).map_err(|err| {
                CliError::new(
                    codes.ca_cert_read,
                    format!("reading --kafka-ca-cert \"{}\": {err}", path.display()),
                )
            })?),
            None => None,
        };
        let server_selection = parse_server_selection(&self.server_selection)
            .map_err(|message| CliError::new(codes.server_flag, message))?;
        Ok(MessageTransportOptions {
            username: self.username,
            password: self.password,
            tls: self.tls,
            ca_pem,
            connect_timeout: self.connect_timeout,
            server_selection,
        })
    }
}

/// `<source>=<key>` pairs; a source may be named once.
pub(crate) fn parse_server_selection(
    values: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let mut selection = BTreeMap::new();
    for value in values {
        let Some((source, key)) = value.split_once('=') else {
            return Err(format!(
                "invalid --asyncapi-server \"{value}\": expected <source>=<server-key>"
            ));
        };
        let (source, key) = (source.trim(), key.trim());
        if source.is_empty() || key.is_empty() {
            return Err(format!(
                "invalid --asyncapi-server \"{value}\": source and server key must both be non-empty"
            ));
        }
        if selection
            .insert(source.to_string(), key.to_string())
            .is_some()
        {
            return Err(format!(
                "duplicate --asyncapi-server for source \"{source}\": name each source once"
            ));
        }
    }
    Ok(selection)
}

/// `--verbose` observer: prints the armed line the moment a receiver is
/// ready, so a user (or a script) knows when producing is safe. Stderr only.
pub struct ArmedLinePrinter;

impl ExecutionObserver for ArmedLinePrinter {
    fn on_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::MessageReceiverArmed {
            topic,
            host,
            partitions,
            ..
        } = event
        {
            eprintln!("Kafka receiver armed: {topic} on {host} ({partitions} partitions)");
        }
    }
}

impl ArmedLinePrinter {
    pub fn shared() -> Arc<dyn ExecutionObserver> {
        Arc::new(Self)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // Test assertions.
mod tests {
    use super::*;

    fn flags() -> MessageTransportFlags {
        MessageTransportFlags {
            username: None,
            password: None,
            tls: false,
            ca_cert: None,
            connect_timeout: Duration::from_secs(30),
            server_selection: Vec::new(),
        }
    }

    #[test]
    fn defaults_convert_to_default_options() {
        let options = flags().into_options(&RUN_MESSAGE_CODES).expect("defaults");
        assert!(options.username.is_none() && options.password.is_none());
        assert!(!options.tls && options.ca_pem.is_none());
        assert_eq!(options.connect_timeout, Duration::from_secs(30));
        assert!(options.server_selection.is_empty());
    }

    #[test]
    fn partial_credentials_are_refused_with_the_command_code() {
        let mut user_only = flags();
        user_only.username = Some("arazzo".into());
        let err = user_only.into_options(&RUN_MESSAGE_CODES).unwrap_err();
        assert_eq!(err.code(), "RUN_KAFKA_CREDENTIALS");
        assert!(err.message().contains(PASSWORD_ENV), "{}", err.message());

        let mut password_only = flags();
        password_only.password = Some(Secret::new("hunter2"));
        let err = password_only.into_options(&TEST_MESSAGE_CODES).unwrap_err();
        assert_eq!(err.code(), "TEST_KAFKA_CREDENTIALS");
        assert!(!err.message().contains("hunter2"));
    }

    #[test]
    fn server_selection_rejects_malformed_and_duplicate_values() {
        for bad in ["orders", "=kafka", "orders=", "orders= "] {
            let mut f = flags();
            f.server_selection = vec![bad.to_string()];
            let err = f.into_options(&RUN_MESSAGE_CODES).unwrap_err();
            assert_eq!(err.code(), "RUN_ASYNCAPI_SERVER_FLAG", "{bad}");
        }
        let mut dup = flags();
        dup.server_selection = vec!["orders=kafka".into(), "orders=kafka-secure".into()];
        let err = dup.into_options(&RUN_MESSAGE_CODES).unwrap_err();
        assert!(err.message().contains("duplicate"), "{}", err.message());

        let mut ok = flags();
        ok.server_selection = vec!["orders=kafka-secure".into(), "shop=kafka".into()];
        let options = ok.into_options(&RUN_MESSAGE_CODES).expect("valid");
        assert_eq!(
            options.server_selection.get("orders").map(String::as_str),
            Some("kafka-secure")
        );
    }

    #[test]
    fn missing_ca_file_is_a_read_error() {
        let mut f = flags();
        f.ca_cert = Some(PathBuf::from("/nonexistent/arazzo-cli/missing.pem"));
        let err = f.into_options(&RUN_MESSAGE_CODES).unwrap_err();
        assert_eq!(err.code(), "RUN_KAFKA_CA_CERT_READ");
    }

    #[test]
    fn flags_debug_never_shows_the_password() {
        let mut f = flags();
        f.password = Some(Secret::new("hunter2"));
        assert!(!format!("{f:?}").contains("hunter2"));
    }
}
