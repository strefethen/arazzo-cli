//! The persisted, versioned result of one `run` command.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use arazzo_runtime::{
    is_sensitive_key, redact_headers, redact_text_patterns, redact_url_query, ExecutionResult,
    RunStepOutcome, RunStepRecord, TraceDecision, REDACTED,
};
use humantime::format_rfc3339;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

mod paths;
pub(crate) use paths::validate_destination;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunExport {
    schema_version: SchemaVersion,
    tool: Tool,
    run: Run,
    inputs: BTreeMap<String, Value>,
    outputs: BTreeMap<String, Value>,
    workflows: BTreeMap<String, Workflow>,
}

#[derive(Debug, Serialize, JsonSchema)]
enum SchemaVersion {
    #[serde(rename = "run.v1")]
    V1,
}

#[derive(Debug, Serialize, JsonSchema)]
struct Tool {
    name: String,
    version: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Run {
    spec_path: String,
    workflow_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_id: Option<String>,
    parallel: bool,
    dry_run: bool,
    started_at: String,
    finished_at: String,
    exported_at: String,
    duration_ms: u64,
    status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ExportError>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
enum RunStatus {
    Success,
    Failure,
    DryRun,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ExportError {
    code: String,
    message: String,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
struct Workflow {
    steps: BTreeMap<String, Step>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
struct Step {
    executions: Vec<Execution>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Execution {
    sequence: u64,
    attempt: u32,
    outcome: StepOutcome,
    duration_ms: u64,
    outputs: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<Request>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<Response>,
    decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Decision {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_limit: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
enum StepOutcome {
    Success,
    Failure,
    DryRun,
}

#[derive(Debug, Serialize, JsonSchema)]
struct Request {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Response {
    status_code: i64,
    headers: BTreeMap<String, String>,
    content_type: String,
    body_bytes: u64,
}

pub struct RunMetadata<'a> {
    pub spec_path: &'a str,
    pub workflow_id: &'a str,
    pub step_id: Option<&'a str>,
    pub parallel: bool,
    pub dry_run: bool,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub duration_ms: u64,
}

pub fn build(
    meta: RunMetadata<'_>,
    inputs: &BTreeMap<String, Value>,
    result: &ExecutionResult,
) -> Result<RunExport, String> {
    let mut workflows = BTreeMap::new();
    workflows.insert(meta.workflow_id.to_string(), Workflow::default());
    for record in result.run_steps() {
        let execution = execution_from_record(record)?;
        workflows
            .entry(record.workflow_id.clone())
            .or_insert_with(Workflow::default)
            .steps
            .entry(record.step_id.clone())
            .or_insert_with(Step::default)
            .executions
            .push(execution);
    }
    let error = result.outputs.as_ref().err().map(|err| ExportError {
        code: err.code().to_string(),
        message: sanitize_runtime_error(&err.message),
    });
    let status = if error.is_some() {
        RunStatus::Failure
    } else if meta.dry_run {
        RunStatus::DryRun
    } else {
        RunStatus::Success
    };
    let mut inputs = inputs.clone();
    redact_values(&mut inputs);
    let mut outputs = result.outputs.as_ref().cloned().unwrap_or_default();
    redact_values(&mut outputs);
    Ok(RunExport {
        schema_version: SchemaVersion::V1,
        tool: Tool {
            name: "arazzo".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        run: Run {
            spec_path: meta.spec_path.to_string(),
            workflow_id: meta.workflow_id.to_string(),
            step_id: meta.step_id.map(str::to_string),
            parallel: meta.parallel,
            dry_run: meta.dry_run,
            started_at: format_rfc3339(meta.started_at).to_string(),
            finished_at: format_rfc3339(meta.finished_at).to_string(),
            exported_at: format_rfc3339(SystemTime::now()).to_string(),
            duration_ms: meta.duration_ms,
            status,
            error,
        },
        inputs,
        outputs,
        workflows,
    })
}

fn execution_from_record(record: &RunStepRecord) -> Result<Execution, String> {
    let request = record.request.as_ref().map(|request| {
        let mut headers = request.headers.clone();
        redact_header_values(&mut headers);
        let mut url = request.url.clone();
        redact_url_query(&mut url);
        redact_url_userinfo(&mut url);
        Request {
            method: request.method.clone(),
            url: redact_text_patterns(&url),
            headers,
        }
    });
    let response = record.response.as_ref().map(|response| {
        let mut headers = response.headers.clone();
        redact_header_values(&mut headers);
        Response {
            status_code: response.status_code,
            headers,
            content_type: redact_text_patterns(&response.content_type.to_string()),
            body_bytes: response.body_bytes,
        }
    });
    let mut outputs = record.outputs.clone();
    redact_values(&mut outputs);
    let decision = decision_from_record(&record.decision)?;
    Ok(Execution {
        sequence: record.seq,
        attempt: record.attempt,
        outcome: match record.outcome {
            RunStepOutcome::Success => StepOutcome::Success,
            RunStepOutcome::Failure => StepOutcome::Failure,
            RunStepOutcome::DryRun => StepOutcome::DryRun,
        },
        duration_ms: record.duration_ms,
        outputs,
        request,
        response,
        decision,
        error: record.error.as_deref().map(redact_text_patterns),
    })
}

fn decision_from_record(decision: &TraceDecision) -> Result<Decision, String> {
    let path = serde_json::to_value(&decision.path)
        .map_err(|err| format!("serializing step decision path: {err}"))?;
    let path = path
        .as_str()
        .ok_or_else(|| "step decision path did not serialize as a string".to_string())?;
    Ok(Decision {
        path: path.to_string(),
        action_type: optional_redacted_text(&decision.action_type),
        target_step_id: optional_redacted_text(&decision.target_step_id),
        target_workflow_id: optional_redacted_text(&decision.target_workflow_id),
        retry_after_seconds: decision.retry_after_seconds,
        retry_limit: decision.retry_limit,
    })
}

fn optional_redacted_text(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| redact_text_patterns(text))
}

fn redact_header_values(headers: &mut BTreeMap<String, String>) {
    redact_headers(headers);
    for (key, value) in headers {
        if !is_sensitive_key(key) {
            *value = redact_text_patterns(value);
        }
    }
}

fn redact_values(values: &mut BTreeMap<String, Value>) {
    for (key, value) in values {
        if is_sensitive_key(key) {
            *value = Value::String(REDACTED.to_string());
        } else {
            redact_value(value);
        }
    }
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if is_sensitive_key(key) {
                    *item = Value::String(REDACTED.to_string());
                } else {
                    redact_value(item);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_value(item);
            }
        }
        Value::String(text) => *text = redact_text_patterns(text),
        _ => {}
    }
}

// The shared query helper deliberately preserves URL formatting, but it does
// not cover credentials before the host. Keep this export-only policy here.
fn redact_url_userinfo(url: &mut String) {
    let Some(scheme) = url.find("://") else {
        return;
    };
    let authority_start = scheme + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |offset| authority_start + offset);
    let Some(at) = url[authority_start..authority_end].rfind('@') else {
        return;
    };
    url.replace_range(authority_start..authority_start + at, REDACTED);
}

fn sanitize_runtime_error(message: &str) -> String {
    // The ordinary runtime error may contain a response-body preview. The
    // persisted run contract retains status but never implicitly retains body.
    if message.contains("success criteria not met (status=") {
        if let Some(body) = message.find(", body=") {
            let mut safe = message[..body].to_string();
            safe.push(')');
            return redact_text_patterns(&safe);
        }
    }
    redact_text_patterns(message)
}

pub fn write_atomic(path: &Path, export: &RunExport) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(export)
        .map_err(|err| format!("serializing run export JSON: {err}"))?;
    bytes.push(b'\n');
    let parent = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| "--export-run must name a file".to_string())?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.tmp-{}-{stamp}-{sequence}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options
            .open(&temp)
            .map_err(|err| format!("creating temporary run export {}: {err}", temp.display()))?;
        file.write_all(&bytes)
            .map_err(|err| format!("writing temporary run export {}: {err}", temp.display()))?;
        file.sync_all()
            .map_err(|err| format!("syncing temporary run export {}: {err}", temp.display()))?;
        fs::rename(&temp, path).map_err(|err| {
            format!(
                "renaming temporary run export {} to {}: {err}",
                temp.display(),
                path.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
