use super::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ReplayKey {
    pub(super) workflow_id: String,
    pub(super) step_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct ReplayRecord {
    pub(super) seq: u64,
    pub(super) attempt: u32,
    pub(super) request: TraceRequest,
    pub(super) response: Option<TraceResponse>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReplayState {
    pub(super) records_by_step: BTreeMap<ReplayKey, VecDeque<ReplayRecord>>,
}

impl ReplayState {
    pub(super) fn from_trace_steps(steps: &[TraceStepRecord]) -> Self {
        let mut records_by_step = BTreeMap::<ReplayKey, VecDeque<ReplayRecord>>::new();
        for step in steps {
            let Some(request) = &step.request else {
                continue;
            };
            let key = ReplayKey {
                workflow_id: step.workflow_id.clone(),
                step_id: step.step_id.clone(),
            };
            records_by_step
                .entry(key)
                .or_default()
                .push_back(ReplayRecord {
                    seq: step.seq,
                    attempt: step.attempt,
                    request: request.clone(),
                    response: step.response.clone(),
                    error: step.error.clone(),
                });
        }
        Self { records_by_step }
    }
}

pub(super) fn validate_replay_request(
    expected: &TraceRequest,
    actual: &RequestConfig,
    seq: u64,
    attempt: u32,
) -> Result<(), RuntimeError> {
    if !expected.method.eq_ignore_ascii_case(&actual.method) {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: method expected \"{}\" got \"{}\"",
                expected.method, actual.method
            ),
        ));
    }

    if expected.url != actual.url {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: url expected \"{}\" got \"{}\"",
                expected.url, actual.url
            ),
        ));
    }

    if expected.headers != actual.headers {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!(
                "replay request drift at seq {seq} attempt {attempt}: headers expected {:?} got {:?}",
                expected.headers, actual.headers
            ),
        ));
    }

    if !replay_body_matches(expected.body.as_ref(), actual.body.as_deref()) {
        return Err(RuntimeError::new(
            RuntimeErrorKind::ReplayRequestMismatch,
            format!("replay request drift at seq {seq} attempt {attempt}: request body mismatch"),
        ));
    }

    Ok(())
}

fn replay_body_matches(expected: Option<&Value>, actual: Option<&[u8]>) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(expected_value), Some(actual_bytes)) => {
            if let Ok(parsed) = serde_json::from_slice::<Value>(actual_bytes) {
                return parsed == *expected_value;
            }
            match expected_value {
                Value::String(s) => std::str::from_utf8(actual_bytes).is_ok_and(|text| text == s),
                _ => false,
            }
        }
    }
}
