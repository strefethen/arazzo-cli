use arazzo_debug_protocol::{
    DebugCapabilities, InitializeRequest, InitializeResult, PingResult, RequestEnvelope,
    ResponseEnvelope, ShutdownResult, INTERNAL_DEBUG_PROTOCOL_VERSION,
};
use serde_json::json;

use crate::INTERNAL_DEBUG_ADAPTER_VERSION;

#[derive(Debug, Default)]
pub struct Session {
    initialized: bool,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_request(&mut self, request: RequestEnvelope) -> ResponseEnvelope {
        let id = request.id;
        match request.method.as_str() {
            "initialize" => self.handle_initialize(id, request.params),
            "ping" => self.handle_ping(id),
            "shutdown" => self.handle_shutdown(id),
            _ => ResponseEnvelope::error(
                id,
                "METHOD_NOT_SUPPORTED",
                format!("unsupported debug method: {}", request.method),
            ),
        }
    }

    fn handle_initialize(&mut self, id: u64, params: serde_json::Value) -> ResponseEnvelope {
        let init: InitializeRequest = match serde_json::from_value(params) {
            Ok(value) => value,
            Err(err) => {
                return ResponseEnvelope::error(
                    id,
                    "INVALID_INITIALIZE_PARAMS",
                    format!("invalid initialize params: {err}"),
                );
            }
        };
        if init.protocol_version != INTERNAL_DEBUG_PROTOCOL_VERSION {
            return ResponseEnvelope::error(
                id,
                "PROTOCOL_VERSION_MISMATCH",
                format!(
                    "protocol version mismatch: expected {}, received {}",
                    INTERNAL_DEBUG_PROTOCOL_VERSION, init.protocol_version
                ),
            );
        }

        self.initialized = true;
        ResponseEnvelope::ok(
            id,
            InitializeResult {
                protocol_version: INTERNAL_DEBUG_PROTOCOL_VERSION.to_string(),
                capabilities: DebugCapabilities::default(),
            },
        )
    }

    fn handle_ping(&self, id: u64) -> ResponseEnvelope {
        if !self.initialized {
            return ResponseEnvelope::error(
                id,
                "SESSION_NOT_INITIALIZED",
                "initialize must be called before ping",
            );
        }
        ResponseEnvelope::ok(
            id,
            PingResult {
                protocol_version: INTERNAL_DEBUG_PROTOCOL_VERSION.to_string(),
                adapter_version: INTERNAL_DEBUG_ADAPTER_VERSION.to_string(),
            },
        )
    }

    fn handle_shutdown(&self, id: u64) -> ResponseEnvelope {
        if !self.initialized {
            return ResponseEnvelope::error(
                id,
                "SESSION_NOT_INITIALIZED",
                "initialize must be called before shutdown",
            );
        }
        ResponseEnvelope::ok(
            id,
            ShutdownResult {
                message: "session closed".to_string(),
            },
        )
    }

    pub fn parse_request_line(&self, line: &str) -> Result<RequestEnvelope, String> {
        serde_json::from_str::<RequestEnvelope>(line)
            .map_err(|err| format!("parsing debug request JSON: {err}"))
    }

    pub fn encode_response_line(&self, response: &ResponseEnvelope) -> Result<String, String> {
        serde_json::to_string(response)
            .map(|serialized| format!("{serialized}\n"))
            .map_err(|err| format!("serializing debug response JSON: {err}"))
    }

    pub fn parse_request_or_protocol_error(
        &self,
        line: &str,
        fallback_id: u64,
    ) -> Result<RequestEnvelope, ResponseEnvelope> {
        match self.parse_request_line(line) {
            Ok(request) => Ok(request),
            Err(message) => Err(ResponseEnvelope::error(
                fallback_id,
                "INVALID_REQUEST_JSON",
                message,
            )),
        }
    }

    pub fn protocol_hello_event(&self) -> serde_json::Value {
        json!({
            "protocolVersion": INTERNAL_DEBUG_PROTOCOL_VERSION,
            "adapterVersion": INTERNAL_DEBUG_ADAPTER_VERSION,
        })
    }
}
