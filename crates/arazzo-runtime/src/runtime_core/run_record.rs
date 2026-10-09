//! Body-free facts captured for every settled step attempt.
//!
//! Capture is independent of trace and retains only declared outputs and HTTP
//! metadata. An output explicitly declared from `$response.body` remains a full
//! typed value. Records are emitted after routing; `seq` counts capture emission
//! order, including parallel levels replayed in declaration order, and does not
//! claim wall-clock completion order. `attempt` counts each workflow/step pair
//! cumulatively from one within the top-level execution.

use super::*;

/// The attempt's own outcome, independent of its subsequent routing decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunStepOutcome {
    Success,
    Failure,
    /// Simulated dry-run execution; there was no real HTTP response.
    DryRun,
}

/// Request metadata retained for a settled attempt. No request body is stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunRequestMetadata {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

/// Response metadata retained for a settled attempt. No response body is stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunResponseMetadata {
    pub status_code: i64,
    pub headers: BTreeMap<String, String>,
    pub content_type: ContentType,
    pub body_bytes: u64,
}

/// One completed or settled step attempt in a top-level engine execution.
///
/// `outcome` reflects the attempt itself; `decision` and `error` also report
/// what routing did afterward. Failed criteria retain the runtime's existing
/// empty-output semantics. Early errors may have no request or response metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunStepRecord {
    pub seq: u64,
    pub workflow_id: String,
    pub step_id: String,
    pub attempt: u32,
    pub outcome: RunStepOutcome,
    pub duration_ms: u64,
    pub outputs: BTreeMap<String, Value>,
    pub request: Option<RunRequestMetadata>,
    pub response: Option<RunResponseMetadata>,
    pub decision: TraceDecision,
    pub error: Option<String>,
}

impl Engine {
    /// Emits one body-free record from the shared attempt protocol, if enabled.
    pub(super) async fn capture_attempt(
        &self,
        ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        number: u32,
        attempt: &step_attempt::SettledAttempt,
    ) {
        if !self.inner.capture_run {
            return;
        }
        let execution = &attempt.execution;
        let dry_run = self.inner.dry_run_mode;
        let request = execution
            .trace
            .request
            .as_ref()
            .map(|request| RunRequestMetadata {
                method: request.method.clone(),
                url: request.url.clone(),
                headers: request.headers.clone(),
            });
        let response = if dry_run {
            None
        } else {
            execution
                .result
                .response
                .as_ref()
                .map(|response| RunResponseMetadata {
                    status_code: response.status_code,
                    headers: response.headers.clone(),
                    content_type: response.content_type.clone(),
                    body_bytes: u64::try_from(response.body.len()).unwrap_or(u64::MAX),
                })
        };
        let outcome = if !execution.result.success {
            RunStepOutcome::Failure
        } else if dry_run {
            RunStepOutcome::DryRun
        } else {
            RunStepOutcome::Success
        };
        let record = RunStepRecord {
            seq: ctx.run_seq.fetch_add(1, Ordering::Relaxed) + 1,
            workflow_id: workflow_id.to_string(),
            step_id: step.step_id.clone(),
            attempt: number,
            outcome,
            duration_ms: engine_trace::duration_ms_u64(attempt.duration),
            outputs: execution.outputs.clone(),
            request,
            response,
            decision: attempt.decision.clone(),
            error: attempt.trace_error.as_deref().map(body_free_error),
        };
        let _ = ctx.event_tx.send(EngineEvent::RunStep(record)).await;
    }
}

/// `step_result_error` includes a response-body preview in its human-readable
/// failed-criterion diagnostic. Keep its status and routing context while
/// excluding that preview from the new capture event, including when a child
/// workflow's diagnostic is wrapped by its caller.
fn body_free_error(error: &str) -> String {
    let Some(criterion) = error.find("success criteria not met (status=") else {
        return error.to_string();
    };
    let Some(body) = error[criterion..].find(", body=") else {
        return error.to_string();
    };
    format!("{})", &error[..criterion + body])
}
