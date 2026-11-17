use std::fmt;
use std::io;
use std::time::Duration;

use serde_json::{json, Value};

pub(crate) const LIMIT_CODE: &str = "MCP_LIMIT_EXCEEDED";
pub(crate) const LIMIT_MESSAGE: &str = "Operation exceeded a server resource limit";
pub(crate) const DEFAULT_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McpLimits {
    pub max_request_bytes: usize,
    pub max_pending_requests: usize,
    pub max_result_bytes: usize,
    pub max_response_body_bytes: usize,
    pub max_document_bytes: usize,
    pub max_http_timeout: Duration,
    pub max_execution_timeout: Duration,
}

impl Default for McpLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: 256 * 1024,
            max_pending_requests: 16,
            max_result_bytes: 1024 * 1024,
            max_response_body_bytes: 4 * 1024 * 1024,
            max_document_bytes: DEFAULT_DOCUMENT_BYTES,
            max_http_timeout: Duration::from_secs(30),
            max_execution_timeout: Duration::from_secs(300),
        }
    }
}

impl McpLimits {
    pub fn validate(&self) -> Result<(), String> {
        let below_one = [
            ("max_request_bytes", self.max_request_bytes == 0),
            ("max_pending_requests", self.max_pending_requests == 0),
            ("max_result_bytes", self.max_result_bytes == 0),
            ("max_response_body_bytes", self.max_response_body_bytes == 0),
            ("max_document_bytes", self.max_document_bytes == 0),
            ("max_http_timeout", self.max_http_timeout.as_secs() == 0),
            (
                "max_execution_timeout",
                self.max_execution_timeout.as_secs() == 0,
            ),
        ];
        match below_one.iter().find(|(_, zero)| *zero) {
            Some((name, _)) if name.ends_with("timeout") => {
                Err(format!("MCP limit {name} must be at least 1 second"))
            }
            Some((name, _)) => Err(format!("MCP limit {name} must be at least 1")),
            None => Ok(()),
        }
    }
}

impl fmt::Display for McpLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "limits max_request_bytes={} max_pending_requests={} max_result_bytes={} max_response_body_bytes={} max_document_bytes={} max_http_timeout={}s max_execution_timeout={}s",
            self.max_request_bytes,
            self.max_pending_requests,
            self.max_result_bytes,
            self.max_response_body_bytes,
            self.max_document_bytes,
            self.max_http_timeout.as_secs(),
            self.max_execution_timeout.as_secs(),
        )
    }
}

pub(crate) fn exceeded_tool_result() -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": json!({"code": LIMIT_CODE, "message": LIMIT_MESSAGE}).to_string(),
        }],
        "isError": true,
    })
}

pub(crate) enum CappedText {
    Within(String),
    Exceeded,
}

pub(crate) fn pretty_text(value: &Value, cap: usize) -> Result<CappedText, String> {
    let mut buffer = CappedBuffer {
        bytes: Vec::new(),
        cap,
    };
    match serde_json::to_writer_pretty(&mut buffer, value) {
        Ok(()) => String::from_utf8(buffer.bytes)
            .map(CappedText::Within)
            .map_err(|err| format!("serializing result: {err}")),
        Err(err) if err.is_io() => Ok(CappedText::Exceeded),
        Err(err) => Err(format!("serializing result: {err}")),
    }
}

struct CappedBuffer {
    bytes: Vec<u8>,
    cap: usize,
}

impl io::Write for CappedBuffer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.cap - self.bytes.len() {
            return Err(io::Error::other(LIMIT_MESSAGE));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
