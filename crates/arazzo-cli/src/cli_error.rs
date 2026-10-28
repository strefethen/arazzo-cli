//! The one owner of CLI error presentation.
//!
//! A migrated command returns a [`CliError`] (or [`CliFailure`]) and never
//! prints it; `main` decides how. With `--json` (or `test --format json`)
//! the error becomes one `{"kind":"error","error":…,"code":…}` object on
//! stdout, followed by any command-specific extra fields. Without it, the
//! message goes to stderr verbatim.

use std::io::Write;

use arazzo_validate::Error as ValidateError;
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Map, Value};

/// A clap usage error (unknown flag, missing argument) under `--json`.
pub const CLI_USAGE: &str = "CLI_USAGE";
/// A JSON result or error object could not be written to stdout.
pub const CLI_OUTPUT_WRITE: &str = "CLI_OUTPUT_WRITE";
/// A malformed or duplicate offline validate server-selection flag.
pub const VALIDATE_ASYNCAPI_SERVER_FLAG: &str = "VALIDATE_ASYNCAPI_SERVER_FLAG";

/// A command failure that carries a code. There is no conversion from a bare
/// `String`: every construction names its code.
#[derive(Debug)]
pub struct CliError {
    code: &'static str,
    message: String,
    extra: Map<String, Value>,
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            extra: Map::new(),
        }
    }

    /// Attaches a command-specific field, serialized after `kind`, `error`,
    /// and `code` in JSON mode and ignored otherwise. `kind`, `error`, and
    /// `code` themselves cannot be overridden.
    #[allow(dead_code)] // first used by the `run` and `validate` migrations
    pub fn with_field(mut self, key: &str, value: Value) -> Self {
        debug_assert!(
            !matches!(key, "kind" | "error" | "code"),
            "reserved error-object key: {key}"
        );
        self.extra.insert(key.to_string(), value);
        self
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Serialize for CliError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3 + self.extra.len()))?;
        map.serialize_entry("kind", "error")?;
        map.serialize_entry("error", &self.message)?;
        map.serialize_entry("code", self.code)?;
        for (key, value) in &self.extra {
            if !matches!(key.as_str(), "kind" | "error" | "code") {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

/// How a migrated command fails.
#[derive(Debug)]
pub enum CliFailure {
    /// `main` prints the error and exits non-zero.
    Error(CliError),
    /// The command already printed its complete result (for example a `test`
    /// run with failing cases) and must exit non-zero. Nothing more is
    /// printed.
    ResultsReported,
}

impl From<CliError> for CliFailure {
    fn from(err: CliError) -> Self {
        Self::Error(err)
    }
}

/// The four codes one command uses for `arazzo_validate::parse` failures.
pub struct SpecErrorCodes {
    pub read_file: &'static str,
    pub parse_yaml: &'static str,
    pub validation: &'static str,
    pub component_resolution: &'static str,
}

/// Maps a spec load failure to a [`CliError`] carrying the command's code for
/// that variant and the error's display text.
pub fn spec_error(err: &ValidateError, codes: &SpecErrorCodes) -> CliError {
    let code = match err {
        ValidateError::ReadFile(_) => codes.read_file,
        ValidateError::ParseYaml(_) => codes.parse_yaml,
        ValidateError::Validation(_) => codes.validation,
        ValidateError::ComponentResolution(_) => codes.component_resolution,
    };
    CliError::new(code, err.to_string())
}

/// Writes `err` as one pretty-printed JSON object and a newline.
pub fn write_json(mut out: impl Write, err: &CliError) -> std::io::Result<()> {
    serde_json::to_writer_pretty(&mut out, err)?;
    writeln!(out)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODES: SpecErrorCodes = SpecErrorCodes {
        read_file: "X_READ",
        parse_yaml: "X_YAML",
        validation: "X_VALID",
        component_resolution: "X_COMP",
    };

    #[test]
    fn spec_error_maps_every_variant_to_its_code() {
        let read = ValidateError::ReadFile(std::io::Error::other("gone"));
        assert_eq!(spec_error(&read, &CODES).code(), "X_READ");
        assert_eq!(spec_error(&read, &CODES).message(), read.to_string());

        let yaml = match serde_yaml_ng::from_str::<Value>("a: [") {
            Err(err) => ValidateError::ParseYaml(err),
            Ok(value) => panic!("expected a YAML error, got {value}"),
        };
        assert_eq!(spec_error(&yaml, &CODES).code(), "X_YAML");

        let validation = match arazzo_validate::parse_bytes(
            b"arazzo: 1.0.1\ninfo: {title: t, version: '1'}\nsourceDescriptions: []\nworkflows: []\n",
        ) {
            Err(err @ ValidateError::Validation(_)) => err,
            other => panic!("expected a validation error, got {other:?}"),
        };
        assert_eq!(spec_error(&validation, &CODES).code(), "X_VALID");

        let comp = ValidateError::ComponentResolution("bad ref".to_string());
        let mapped = spec_error(&comp, &CODES);
        assert_eq!(mapped.code(), "X_COMP");
        assert_eq!(mapped.message(), "bad ref");
    }

    #[test]
    fn error_object_leads_with_kind_error_code_then_extra_fields() {
        let err = CliError::new("X_CODE", "boom").with_field("warnings", Value::Array(vec![]));
        let mut buf = Vec::new();
        assert!(write_json(&mut buf, &err).is_ok());
        let text = String::from_utf8_lossy(&buf);
        assert_eq!(
            text,
            "{\n  \"kind\": \"error\",\n  \"error\": \"boom\",\n  \"code\": \"X_CODE\",\n  \"warnings\": []\n}\n"
        );
    }
}
