use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DapRequest {
    pub seq: u64,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub type_: String,
    pub command: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct DapBreakpoint {
    pub line: u32,
}
