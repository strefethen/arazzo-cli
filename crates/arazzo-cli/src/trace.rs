use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use arazzo_runtime::{
    redact_headers, redact_json_object, redact_json_value, redact_text_patterns, redact_url_query,
    TraceStepRecord, TransportWarning,
};
use humantime::format_rfc3339;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TRACE_SCHEMA_VERSION: &str = "trace.v1";
pub const INTERNAL_TRACE_PIPELINE_VERSION: &str = "v1";
pub const TRACE_BODY_PREVIEW_DEFAULT_BYTES: usize = 2048;
pub const TRACE_MAX_BODY_BYTES_LIMIT: usize = 1024 * 1024;

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
    /// Structured transport-trust warnings recorded for this run;
    /// persists regardless of the stderr squelch (audit data).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transport_warnings: Vec<TransportWarning>,
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
    pub transport_warnings: Vec<TransportWarning>,
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
            transport_warnings: meta.transport_warnings,
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
            for hop in &mut request.redirects {
                redact_url_query(&mut hop.from);
                redact_url_query(&mut hop.to);
            }
            if let Some(body) = &mut request.body {
                redact_json_value(body);
            }
        }

        if let Some(response) = &mut step.response {
            redact_headers(&mut response.headers);
            // Try JSON redaction on both body and body_preview regardless of
            // content type — catches JSON with incorrect content-type headers.
            // For non-JSON bodies, fall back to regex-based pattern redaction.
            for text in [&mut response.body_preview, &mut response.body]
                .into_iter()
                .flatten()
            {
                if let Ok(mut value) = serde_json::from_str::<Value>(text) {
                    redact_json_value(&mut value);
                    if let Ok(serialized) = serde_json::to_string(&value) {
                        *text = serialized;
                    }
                } else {
                    *text = redact_text_patterns(text);
                }
            }
            // Truncate body_preview to max_body_bytes (at a char boundary to
            // avoid splitting multi-byte UTF-8 sequences).
            if let Some(preview) = &mut response.body_preview {
                if preview.len() > max_body_bytes {
                    let mut end = max_body_bytes;
                    while !preview.is_char_boundary(end) {
                        end -= 1;
                    }
                    preview.truncate(end);
                    preview.push_str("...");
                }
            }
            // Truncate body to max_body_bytes
            if let Some(body) = &mut response.body {
                if body.len() > max_body_bytes {
                    let mut end = max_body_bytes;
                    while !body.is_char_boundary(end) {
                        end -= 1;
                    }
                    body.truncate(end);
                    body.push_str("...");
                }
            }
        }

        redact_json_object(&mut step.outputs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arazzo_runtime::{
        ContentType, TraceDecision, TraceDecisionPath, TraceRequest, TraceResponse, REDACTED,
    };

    fn make_trace_file(response: TraceResponse) -> TraceFile {
        TraceFile {
            schema_version: TRACE_SCHEMA_VERSION.to_string(),
            tool: TraceTool {
                name: "test".to_string(),
                version: "0.0.0".to_string(),
            },
            run: TraceRun {
                spec_path: "test.yaml".to_string(),
                workflow_id: "wf".to_string(),
                parallel: false,
                dry_run: false,
                timeout_ms: 0,
                started_at: String::new(),
                finished_at: String::new(),
                duration_ms: 0,
                status: "success".to_string(),
                error: None,
                transport_warnings: Vec::new(),
            },
            inputs: BTreeMap::new(),
            steps: vec![TraceStepRecord {
                seq: 1,
                workflow_id: "wf".to_string(),
                step_id: "s1".to_string(),
                attempt: 1,
                kind: "http".to_string(),
                operation_path: "/test".to_string(),
                workflow_id_ref: String::new(),
                duration_ms: 0,
                request: Some(TraceRequest {
                    method: "GET".to_string(),
                    url: "https://example.com/test".to_string(),
                    headers: BTreeMap::new(),
                    body: None,
                    redirects: Vec::new(),
                }),
                response: Some(response),
                criteria: Vec::new(),
                warnings: Vec::new(),
                decision: TraceDecision::with_path(TraceDecisionPath::Next),
                outputs: BTreeMap::new(),
                error: None,
            }],
        }
    }

    #[test]
    fn redact_trace_redacts_both_body_and_body_preview() {
        let json_body = r#"{"token":"secret123","name":"Alice"}"#.to_string();
        let response = TraceResponse {
            status_code: 200,
            content_type: ContentType::Json,
            headers: BTreeMap::new(),
            body_bytes: json_body.len() as u64,
            body_preview: Some(json_body.clone()),
            body: Some(json_body),
            body_lossy: false,
        };

        let mut trace = make_trace_file(response);
        redact_trace_file(&mut trace, 2048);

        let resp = trace.steps[0]
            .response
            .as_ref()
            .unwrap_or_else(|| panic!("response missing"));
        // Both body and body_preview should have "token" redacted
        let preview_str = resp
            .body_preview
            .as_deref()
            .unwrap_or_else(|| panic!("preview missing"));
        let preview: Value =
            serde_json::from_str(preview_str).unwrap_or_else(|e| panic!("parse preview: {e}"));
        assert_eq!(preview["token"], Value::String(REDACTED.to_string()));
        assert_eq!(preview["name"], Value::String("Alice".to_string()));

        let body_str = resp
            .body
            .as_deref()
            .unwrap_or_else(|| panic!("body missing"));
        let body: Value =
            serde_json::from_str(body_str).unwrap_or_else(|e| panic!("parse body: {e}"));
        assert_eq!(body["token"], Value::String(REDACTED.to_string()));
        assert_eq!(body["name"], Value::String("Alice".to_string()));
    }

    #[test]
    fn redact_trace_redacts_json_body_regardless_of_content_type() {
        // Body is valid JSON but content type says "other"
        let json_body = r#"{"password":"hunter2","user":"bob"}"#.to_string();
        let response = TraceResponse {
            status_code: 200,
            content_type: ContentType::Other("text/plain".to_string()),
            headers: BTreeMap::new(),
            body_bytes: json_body.len() as u64,
            body_preview: Some(json_body.clone()),
            body: Some(json_body),
            body_lossy: false,
        };

        let mut trace = make_trace_file(response);
        redact_trace_file(&mut trace, 2048);

        let resp = trace.steps[0]
            .response
            .as_ref()
            .unwrap_or_else(|| panic!("response missing"));
        let body_str = resp
            .body
            .as_deref()
            .unwrap_or_else(|| panic!("body missing"));
        let body: Value =
            serde_json::from_str(body_str).unwrap_or_else(|e| panic!("parse body: {e}"));
        assert_eq!(body["password"], Value::String(REDACTED.to_string()));
        assert_eq!(body["user"], Value::String("bob".to_string()));
    }

    #[test]
    fn redact_trace_redacts_non_json_body_patterns() {
        // Plain text body with Bearer token and key=value secrets
        let plain_body =
            "Hello\ntoken: Bearer eyJhbGciOiJIUzI1NiJ9.test\npassword=hunter2\nuser=bob"
                .to_string();
        let response = TraceResponse {
            status_code: 200,
            content_type: ContentType::Other("text/plain".to_string()),
            headers: BTreeMap::new(),
            body_bytes: plain_body.len() as u64,
            body_preview: Some(plain_body.clone()),
            body: Some(plain_body),
            body_lossy: false,
        };

        let mut trace = make_trace_file(response);
        redact_trace_file(&mut trace, 4096);

        let resp = trace.steps[0]
            .response
            .as_ref()
            .unwrap_or_else(|| panic!("response missing"));
        let body = resp
            .body
            .as_deref()
            .unwrap_or_else(|| panic!("body missing"));
        // JWT should be redacted (via bearer or kv pattern)
        assert!(
            !body.contains("eyJhbGci"),
            "JWT should be redacted, got: {body}"
        );
        // password value should be redacted
        assert!(
            !body.contains("hunter2"),
            "password value should be redacted, got: {body}"
        );
        // Non-sensitive value should survive
        assert!(body.contains("user=bob"), "user=bob should survive: {body}");
    }

    #[test]
    fn redact_text_patterns_unit() {
        assert_eq!(
            redact_text_patterns("Bearer abc123.xyz"),
            "Bearer [REDACTED]"
        );
        assert_eq!(
            redact_text_patterns("token=secret123 name=alice"),
            "token=[REDACTED] name=alice"
        );
        assert_eq!(
            redact_text_patterns("password: hunter2"),
            "password: [REDACTED]"
        );
        assert_eq!(redact_text_patterns("no secrets here"), "no secrets here");
    }
}
