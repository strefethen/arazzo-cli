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
