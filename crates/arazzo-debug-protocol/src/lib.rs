#![forbid(unsafe_code)]

//! Internal debug protocol models for adapter and editor integrations.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable internal debug protocol marker for compatibility checks.
pub const INTERNAL_DEBUG_PROTOCOL_VERSION: &str = "v1";

/// Envelope for one debug request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestEnvelope {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Envelope for one debug response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl ResponseEnvelope {
    pub fn ok<T>(id: u64, result: T) -> Self
    where
        T: Serialize,
    {
        let value = match serde_json::to_value(result) {
            Ok(value) => value,
            Err(err) => {
                return Self::error(
                    id,
                    "SERIALIZATION_FAILED",
                    format!("serializing response payload: {err}"),
                );
            }
        };
        Self {
            id,
            ok: true,
            result: Some(value),
            error: None,
        }
    }

    pub fn error(id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(ProtocolError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// Structured error payload for request failures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

/// `initialize` request payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    #[serde(default)]
    pub client_name: String,
    #[serde(default)]
    pub client_version: String,
    pub protocol_version: String,
}

/// `initialize` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: DebugCapabilities,
}

/// Declared adapter capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DebugCapabilities {
    pub supports_breakpoints: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_step_over: bool,
    pub supports_step_in: bool,
    pub supports_step_out: bool,
    pub supports_pause: bool,
    pub supports_watches: bool,
}

/// `ping` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub protocol_version: String,
    pub adapter_version: String,
}

/// `shutdown` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownResult {
    pub message: String,
}
