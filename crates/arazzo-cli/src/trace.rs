use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use arazzo_runtime::TraceStepRecord;
use humantime::format_rfc3339;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TRACE_SCHEMA_VERSION: &str = "trace.v1";
pub const INTERNAL_TRACE_PIPELINE_VERSION: &str = "v1";
pub const TRACE_REDACTED: &str = "[REDACTED]";
pub const TRACE_BODY_PREVIEW_DEFAULT_BYTES: usize = 2048;
pub const TRACE_MAX_BODY_BYTES_LIMIT: usize = 1024 * 1024;

const TRACE_SENSITIVE_KEYS: [&str; 18] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "pwd",
    "session",
    "sessionid",
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceFile {
    pub schema_version: String,
    pub tool: TraceTool,
    pub run: TraceRun,
    pub inputs: BTreeMap<String, Value>,
    pub steps: Vec<TraceStepRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TraceTool {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRun {
    pub spec_path: String,
    pub workflow_id: String,
    pub parallel: bool,
    pub dry_run: bool,
    pub timeout_ms: u64,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TraceRunMetadata {
    pub spec_path: String,
    pub workflow_id: String,
    pub parallel: bool,
    pub dry_run: bool,
    pub timeout_ms: u64,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub duration_ms: u64,
    pub run_error: Option<String>,
}

pub fn parse_trace_max_body_bytes(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|err| format!("invalid trace max body bytes \"{raw}\": {err}"))?;
    if value > TRACE_MAX_BODY_BYTES_LIMIT {
        return Err(format!(
            "trace max body bytes must be <= {TRACE_MAX_BODY_BYTES_LIMIT}"
        ));
    }
    Ok(value)
}

pub fn read_trace_file(path: &Path) -> Result<TraceFile, String> {
    let data =
        fs::read(path).map_err(|err| format!("reading trace file {}: {err}", path.display()))?;
    let trace: TraceFile = serde_json::from_slice(&data)
        .map_err(|err| format!("parsing trace JSON {}: {err}", path.display()))?;
    if trace.schema_version != TRACE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported trace schema version \"{}\" (expected \"{}\")",
            trace.schema_version, TRACE_SCHEMA_VERSION
        ));
    }
    Ok(trace)
}

pub fn build_trace_file(
    meta: TraceRunMetadata,
    inputs: BTreeMap<String, Value>,
    steps: Vec<TraceStepRecord>,
) -> TraceFile {
    TraceFile {
        schema_version: TRACE_SCHEMA_VERSION.to_string(),
        tool: TraceTool {
            name: "arazzo".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        run: TraceRun {
            spec_path: meta.spec_path,
            workflow_id: meta.workflow_id,
            parallel: meta.parallel,
            dry_run: meta.dry_run,
            timeout_ms: meta.timeout_ms,
            started_at: format_rfc3339(meta.started_at).to_string(),
            finished_at: format_rfc3339(meta.finished_at).to_string(),
            duration_ms: meta.duration_ms,
            status: if meta.run_error.is_some() {
                "failure".to_string()
            } else {
                "success".to_string()
            },
            error: meta.run_error,
        },
        inputs,
        steps,
    }
}

pub fn prepare_trace_for_write(trace: &mut TraceFile, max_body_bytes: usize) {
    redact_trace_file(trace, max_body_bytes);
}

pub fn write_trace_file_atomic(path: &Path, trace: &TraceFile) -> Result<(), String> {
    let parent = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)
        .map_err(|err| format!("creating trace directory {}: {err}", parent.display()))?;

    let stamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(delta) => delta.as_nanos(),
        Err(_) => 0,
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trace.json");
    let tmp_name = format!(".{file_name}.tmp-{}-{stamp}", std::process::id());
    let tmp_path = parent.join(tmp_name);

    {
        let mut file = fs::File::create(&tmp_path)
            .map_err(|err| format!("creating temp trace file {}: {err}", tmp_path.display()))?;
        serde_json::to_writer_pretty(&mut file, trace).map_err(|err| {
            format!(
                "serializing trace JSON to temp file {}: {err}",
                tmp_path.display()
            )
        })?;
        use std::io::Write;
        writeln!(file)
            .map_err(|err| format!("writing trace newline to {}: {err}", tmp_path.display()))?;
        file.sync_all()
            .map_err(|err| format!("syncing temp trace file {}: {err}", tmp_path.display()))?;
    }

    fs::rename(&tmp_path, path).map_err(|err| {
        // Intentional: best-effort cleanup of temp file; the rename error
        // is the one we propagate.
        if fs::remove_file(&tmp_path).is_err() {
            // Temp file may already be gone; rename error remains the primary failure.
        }
        format!(
            "renaming temp trace file {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

fn redact_trace_file(trace: &mut TraceFile, max_body_bytes: usize) {
    redact_json_object(&mut trace.inputs);
    for step in &mut trace.steps {
        if let Some(request) = &mut step.request {
            redact_headers(&mut request.headers);
            redact_url_query(&mut request.url);
            if let Some(body) = &mut request.body {
                redact_json_value(body);
            }
        }

        if let Some(response) = &mut step.response {
            redact_headers(&mut response.headers);
            if matches!(response.content_type, arazzo_runtime::ContentType::Json) {
                if let Some(preview) = &mut response.body_preview {
                    if let Ok(mut value) = serde_json::from_str::<Value>(preview) {
                        redact_json_value(&mut value);
                        if let Ok(serialized) = serde_json::to_string(&value) {
                            *preview = serialized;
                        }
                    }
                }
            }
            if let Some(preview) = &mut response.body_preview {
                let mut bytes = preview.as_bytes().to_vec();
                if bytes.len() > max_body_bytes {
                    bytes.truncate(max_body_bytes);
                    let mut text = String::from_utf8_lossy(&bytes).to_string();
                    text.push_str("...");
                    *preview = text;
                }
            }
        }

        redact_json_object(&mut step.outputs);
    }
}

fn redact_headers(headers: &mut BTreeMap<String, String>) {
    for (name, value) in headers {
        if is_sensitive_key(name) {
            *value = TRACE_REDACTED.to_string();
        }
    }
}

fn redact_url_query(url: &mut String) {
    let mut parsed = match url::Url::parse(url) {
        Ok(value) => value,
        Err(_) => return,
    };

    let mut pairs = Vec::<(String, String)>::new();
    for (key, value) in parsed.query_pairs() {
        if is_sensitive_key(&key) {
            pairs.push((key.to_string(), TRACE_REDACTED.to_string()));
        } else {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    if pairs.is_empty() {
        return;
    }

    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    *url = parsed.to_string();
}

fn redact_json_object(map: &mut BTreeMap<String, Value>) {
    for (key, value) in map {
        if is_sensitive_key(key) {
            *value = Value::String(TRACE_REDACTED.to_string());
        } else {
            redact_json_value(value);
        }
    }
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if is_sensitive_key(key) {
                    *nested = Value::String(TRACE_REDACTED.to_string());
                } else {
                    redact_json_value(nested);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    TRACE_SENSITIVE_KEYS.iter().any(|key| lower == *key)
}
