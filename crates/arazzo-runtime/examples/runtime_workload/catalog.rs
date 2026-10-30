use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;

pub(crate) const CATALOG_VERSION: u32 = 1;
pub(crate) const INVOCATION_HEADER: &str = "x-arazzo-workload-invocation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct HarnessError {
    pub stage: String,
    pub code: String,
    pub message: String,
}

impl HarnessError {
    pub fn new(stage: &str, code: &str, message: impl fmt::Display) -> Self {
        Self {
            stage: stage.into(),
            code: code.into(),
            message: message.to_string(),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.stage, self.code, self.message)
    }
}

impl std::error::Error for HarnessError {}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Family {
    Sources,
    Expressions,
    Dag,
    Bodies,
    Retries,
    Cancellation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlKind {
    MissingSource,
    MalformedSource,
    FalseCriterion,
    Cycle,
    SuccessEnd,
    OverCap,
    RetryExhaustion,
    Watchdog,
    CompleteChunked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Transfer {
    Fixed,
    Chunked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stall {
    None,
    Headers,
    BetweenChunks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResponsePolicy {
    pub transfer: Transfer,
    pub stall: Stall,
    pub body_bytes: u64,
    pub failure_attempts: u32,
    pub exhaust_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HandleMode {
    Collect,
    ResultOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputPolicy {
    Scalar,
    FullBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub tokio_threads: u32,
    pub http_timeout_ms: u64,
    pub channel_capacity: usize,
    pub response_cap: u64,
    pub burst: u32,
    pub rate: u32,
    pub parallel: bool,
    pub trace: bool,
    pub capture: bool,
    pub handle_mode: HandleMode,
    pub output_policy: OutputPolicy,
    pub invocation_header: String,
    pub observer: bool,
    pub trace_hook: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tokio_threads: 2,
            http_timeout_ms: 5000,
            channel_capacity: 1024,
            response_cap: 10_485_760,
            burst: 20,
            rate: 0,
            parallel: false,
            trace: false,
            capture: false,
            handle_mode: HandleMode::Collect,
            output_policy: OutputPolicy::Scalar,
            invocation_header: INVOCATION_HEADER.into(),
            observer: false,
            trace_hook: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Dimensions {
    pub source_files: usize,
    pub operations_per_file: usize,
    pub steps: usize,
    pub criteria_per_step: usize,
    pub pointer_depth: usize,
    pub scalar_outputs_per_step: usize,
    pub width: usize,
    pub diamond: bool,
    pub serialized_body_bytes: usize,
    pub json_nodes: usize,
    pub watchdog_timeout_ms: u64,
    pub retry_limit: u32,
    pub retry_after_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedOperation {
    pub source: usize,
    pub operation: usize,
    pub operation_id: String,
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ExpectedValue {
    Scalar {
        value: Value,
    },
    FullBody {
        payload_index: usize,
        bytes: usize,
        pointer_depth: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedCriterion {
    pub step_id: String,
    pub index: usize,
    pub attempt: u32,
    pub passed: bool,
    pub criterion_type: String,
    pub condition: String,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedAttempt {
    pub step_id: String,
    pub attempt: u32,
    pub status: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedCase {
    pub workflow_ids: Vec<String>,
    pub operations: Vec<ExpectedOperation>,
    pub outputs: BTreeMap<String, ExpectedValue>,
    pub settled_step_outputs: BTreeMap<String, BTreeMap<String, ExpectedValue>>,
    pub criteria: Vec<ExpectedCriterion>,
    pub lifecycle_steps: Vec<String>,
    pub attempt_sequence: Vec<ExpectedAttempt>,
    pub attempts: BTreeMap<String, u32>,
    pub required_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
    pub terminal_kind: String,
    pub sequential_fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseSpec {
    pub case_id: String,
    pub family: Family,
    pub control: Option<ControlKind>,
    pub dimensions: Dimensions,
    pub settings: Settings,
    pub response_policy: ResponsePolicy,
    pub workflow_id: String,
    pub expected: ExpectedCase,
}

pub(crate) fn operation(source: usize, index: usize) -> ExpectedOperation {
    ExpectedOperation {
        source,
        operation: index,
        operation_id: format!("operation_{source}_{index}"),
        method: "GET".into(),
        path: format!("/s{source}/op{index}"),
    }
}

pub(crate) fn step_operation(case: &CaseSpec, index: usize) -> ExpectedOperation {
    let d = &case.dimensions;
    if case.family == Family::Sources {
        if index + 1 == d.steps {
            operation(d.source_files - 1, d.operations_per_file - 1)
        } else {
            operation(
                index % d.source_files,
                index * d.operations_per_file / d.steps,
            )
        }
    } else {
        operation(0, index)
    }
}

pub(crate) fn predecessors(case: &CaseSpec, index: usize) -> Vec<usize> {
    let d = &case.dimensions;
    if case.family != Family::Dag && case.family != Family::Cancellation {
        return Vec::new();
    }
    if d.diamond {
        match index {
            1 | 2 => vec![0],
            3 => vec![1, 2],
            _ => Vec::new(),
        }
    } else if index >= d.width {
        (0..d.width).collect()
    } else {
        Vec::new()
    }
}

pub(crate) fn scalar(payload_index: usize, field: usize) -> Value {
    json!((payload_index as u64 + 1) * 100 + field as u64)
}

pub(crate) fn output_predecessors(case: &CaseSpec, index: usize) -> Vec<usize> {
    let all = predecessors(case, index);
    match all.as_slice() {
        [] => Vec::new(),
        [one] => vec![*one],
        [first, .., last] => vec![*first, *last],
    }
}

fn base(id: String, family: Family, steps: usize) -> CaseSpec {
    CaseSpec {
        case_id: id,
        family,
        control: None,
        dimensions: Dimensions {
            source_files: 1,
            operations_per_file: steps,
            steps,
            criteria_per_step: 1,
            pointer_depth: 12,
            scalar_outputs_per_step: 8,
            width: 0,
            diamond: false,
            serialized_body_bytes: 1024,
            json_nodes: 21,
            watchdog_timeout_ms: 0,
            retry_limit: 0,
            retry_after_ms: 0,
        },
        settings: Settings::default(),
        response_policy: ResponsePolicy {
            transfer: Transfer::Fixed,
            stall: Stall::None,
            body_bytes: 1024,
            failure_attempts: 0,
            exhaust_path: None,
        },
        workflow_id: "workload".into(),
        expected: ExpectedCase {
            workflow_ids: vec!["workload".into()],
            operations: Vec::new(),
            outputs: BTreeMap::new(),
            settled_step_outputs: BTreeMap::new(),
            criteria: Vec::new(),
            lifecycle_steps: Vec::new(),
            attempt_sequence: Vec::new(),
            attempts: BTreeMap::new(),
            required_paths: Vec::new(),
            forbidden_paths: Vec::new(),
            terminal_kind: "success".into(),
            sequential_fallback: None,
        },
    }
}

fn complete(mut c: CaseSpec) -> CaseSpec {
    c.response_policy.body_bytes = c.dimensions.serialized_body_bytes as u64;
    c.expected.operations = (0..c.dimensions.source_files)
        .flat_map(|s| (0..c.dimensions.operations_per_file).map(move |o| operation(s, o)))
        .collect();
    let no_http = matches!(
        c.control,
        Some(ControlKind::MissingSource | ControlKind::MalformedSource | ControlKind::Cycle)
    );
    for i in 0..c.dimensions.steps {
        let op = step_operation(&c, i);
        let forbidden = no_http
            || (c.family == Family::Cancellation && i >= c.dimensions.width)
            || (matches!(
                c.control,
                Some(ControlKind::FalseCriterion | ControlKind::SuccessEnd)
            ) && i > 0)
            || (c.control == Some(ControlKind::RetryExhaustion) && !c.settings.parallel && i > 0);
        if forbidden {
            c.expected.forbidden_paths.push(op.path);
            continue;
        }
        c.expected.required_paths.push(op.path.clone());
        let attempts = c.response_policy.failure_attempts + 1;
        c.expected.attempts.insert(op.path.clone(), attempts);
        c.expected.lifecycle_steps.push(format!("step_{i}"));
        if c.response_policy.stall != Stall::None || c.control == Some(ControlKind::OverCap) {
            continue;
        }
        for attempt in 1..=attempts {
            c.expected.attempt_sequence.push(ExpectedAttempt {
                step_id: format!("step_{i}"),
                attempt,
                status: if attempt <= c.response_policy.failure_attempts
                    || c.response_policy.exhaust_path.as_deref() == Some(op.path.as_str())
                {
                    503
                } else {
                    200
                },
            });
            for index in 0..c.dimensions.criteria_per_step {
                let passed = attempt > c.response_policy.failure_attempts
                    && c.response_policy.exhaust_path.as_deref() != Some(op.path.as_str())
                    && !(c.control == Some(ControlKind::FalseCriterion) && i == 0 && index == 0);
                c.expected.criteria.push(ExpectedCriterion {
                    step_id: format!("step_{i}"),
                    index,
                    attempt,
                    passed,
                    criterion_type: "simple".into(),
                    condition: if c.control == Some(ControlKind::FalseCriterion)
                        && i == 0
                        && index == 0
                    {
                        "$statusCode == 999".into()
                    } else if c.family == Family::Expressions {
                        let pointer = (0..11)
                            .map(|d| format!("d{d}"))
                            .collect::<Vec<_>>()
                            .join("/");
                        format!(
                            "$statusCode == 200 && $response.body#/{pointer}/v{} == {}",
                            index % 8,
                            (op.operation + 1) * 100 + index % 8
                        )
                    } else {
                        "$statusCode == 200".into()
                    },
                    context: String::new(),
                });
            }
        }
        if c.response_policy.exhaust_path.as_deref() == Some(op.path.as_str())
            || c.control == Some(ControlKind::FalseCriterion)
        {
            continue;
        }
        let mut step_outputs = BTreeMap::new();
        for field in 0..c.dimensions.scalar_outputs_per_step {
            step_outputs.insert(
                format!("value_{field}"),
                ExpectedValue::Scalar {
                    value: scalar(op.operation, field),
                },
            );
        }
        if c.settings.output_policy == OutputPolicy::FullBody {
            step_outputs.insert(
                "body".into(),
                ExpectedValue::FullBody {
                    payload_index: op.operation,
                    bytes: c.dimensions.serialized_body_bytes,
                    pointer_depth: c.dimensions.pointer_depth,
                },
            );
        }
        for p in output_predecessors(&c, i) {
            step_outputs.insert(
                format!("predecessor_{p}"),
                ExpectedValue::Scalar {
                    value: scalar(step_operation(&c, p).operation, 0),
                },
            );
        }
        if c.expected.terminal_kind == "success" {
            c.expected.outputs.extend(
                step_outputs
                    .iter()
                    .map(|(name, value)| (format!("step_{i}_{name}"), value.clone())),
            );
        }
        c.expected
            .settled_step_outputs
            .insert(format!("step_{i}"), step_outputs);
    }
    c
}

pub(crate) fn catalog() -> Vec<CaseSpec> {
    let mut all = Vec::new();
    for files in [1, 8] {
        for ops in [10, 100] {
            let mut c = base(format!("sources_f{files}_o{ops}"), Family::Sources, 10);
            c.dimensions.source_files = files;
            c.dimensions.operations_per_file = ops;
            all.push(complete(c));
        }
    }
    for (steps, criteria) in [(8, 1), (8, 16), (8, 64), (64, 16)] {
        let mut c = base(
            format!("expressions_s{steps}_c{criteria}"),
            Family::Expressions,
            steps,
        );
        c.dimensions.criteria_per_step = criteria;
        c.settings.trace = true;
        all.push(complete(c));
    }
    for (width, diamond) in [(1, false), (8, false), (40, false), (128, false), (2, true)] {
        for parallel in [false, true] {
            for rate in [0, 10] {
                let shape = if diamond {
                    "diamond".into()
                } else {
                    format!("w{width}")
                };
                let mut c = base(
                    format!("dag_{shape}_p{parallel}_r{rate}"),
                    Family::Dag,
                    if diamond { 4 } else { width * 2 },
                );
                c.dimensions.width = width;
                c.dimensions.diamond = diamond;
                c.settings.parallel = parallel;
                c.settings.rate = rate;
                c.settings.capture = true;
                all.push(complete(c));
            }
        }
    }
    for (steps, bytes) in [(1, 1024), (1, 1_048_576), (1, 8_388_608), (8, 1_048_576)] {
        for output in [OutputPolicy::Scalar, OutputPolicy::FullBody] {
            for handle in [HandleMode::Collect, HandleMode::ResultOnly] {
                for transfer in [Transfer::Fixed, Transfer::Chunked] {
                    let mut c = base(
                        format!("bodies_s{steps}_b{bytes}_{output:?}_{handle:?}_{transfer:?}")
                            .to_lowercase(),
                        Family::Bodies,
                        steps,
                    );
                    c.dimensions.serialized_body_bytes = bytes;
                    c.settings.parallel = steps == 8;
                    c.settings.output_policy = output;
                    c.settings.handle_mode = handle;
                    c.response_policy.transfer = transfer;
                    all.push(complete(c));
                }
            }
        }
    }
    for steps in [1, 8] {
        for limit in [0, 1, 3] {
            for parallel in [false, true] {
                for delay in [0, 100] {
                    let mut c = base(
                        format!("retries_s{steps}_l{limit}_p{parallel}_d{delay}"),
                        Family::Retries,
                        steps,
                    );
                    c.dimensions.retry_limit = limit;
                    c.dimensions.retry_after_ms = delay;
                    c.response_policy.failure_attempts = limit;
                    c.settings.parallel = parallel;
                    c.settings.capture = true;
                    all.push(complete(c));
                }
            }
        }
    }
    for width in [1, 8] {
        for stall in [Stall::Headers, Stall::BetweenChunks] {
            for handle in [HandleMode::Collect, HandleMode::ResultOnly] {
                let mut c = base(
                    format!("cancellation_w{width}_{stall:?}_{handle:?}").to_lowercase(),
                    Family::Cancellation,
                    width * 2,
                );
                c.dimensions.width = width;
                c.settings.parallel = true;
                c.settings.handle_mode = handle;
                c.response_policy.transfer = Transfer::Chunked;
                c.response_policy.stall = stall;
                c.expected.terminal_kind = "ExecutionCancelled".into();
                all.push(complete(c));
            }
        }
    }
    for (name, kind) in [
        ("missing_source", ControlKind::MissingSource),
        ("malformed_source", ControlKind::MalformedSource),
        ("false_criterion", ControlKind::FalseCriterion),
        ("cycle", ControlKind::Cycle),
        ("success_end", ControlKind::SuccessEnd),
        ("over_cap_fixed", ControlKind::OverCap),
        ("over_cap_chunked", ControlKind::OverCap),
        ("watchdog", ControlKind::Watchdog),
        ("complete_chunked", ControlKind::CompleteChunked),
    ] {
        let family = match kind {
            ControlKind::Cycle | ControlKind::SuccessEnd => Family::Dag,
            ControlKind::FalseCriterion => Family::Expressions,
            ControlKind::OverCap | ControlKind::CompleteChunked => Family::Bodies,
            ControlKind::RetryExhaustion => Family::Retries,
            ControlKind::Watchdog => Family::Cancellation,
            _ => Family::Sources,
        };
        let mut c = base(
            format!("control_{name}"),
            family,
            if matches!(kind, ControlKind::CompleteChunked | ControlKind::OverCap) {
                1
            } else {
                2
            },
        );
        c.control = Some(kind);
        match kind {
            ControlKind::MissingSource => c.expected.terminal_kind = "SourceDescriptionLoad".into(),
            ControlKind::MalformedSource => {
                c.expected.terminal_kind = "SourceDescriptionParse".into()
            }
            ControlKind::FalseCriterion => {
                c.settings.trace = true;
                c.expected.terminal_kind = "SuccessCriteriaFailed".into();
            }
            ControlKind::Cycle => {
                c.dimensions.width = 1;
                c.settings.parallel = true;
                c.expected.terminal_kind = "DependencyCycle".into();
            }
            ControlKind::SuccessEnd => {
                c.dimensions.width = 1;
                c.settings.parallel = true;
                c.settings.capture = true;
                c.expected.sequential_fallback = Some("controlFlowAction".into());
            }
            ControlKind::OverCap => {
                c.settings.response_cap = 1023;
                c.response_policy.transfer = if name.ends_with("chunked") {
                    Transfer::Chunked
                } else {
                    Transfer::Fixed
                };
                c.expected.terminal_kind = "ResponseTooLarge".into();
            }
            ControlKind::RetryExhaustion => {
                c.dimensions.retry_limit = 3;
                c.response_policy.failure_attempts = 3;
                c.response_policy.exhaust_path = Some(operation(0, 1).path);
                c.settings.parallel = true;
                c.settings.capture = true;
                c.expected.terminal_kind = "RetryLimitExceeded".into();
            }
            ControlKind::Watchdog => {
                c.dimensions.watchdog_timeout_ms = 100;
                c.dimensions.width = 1;
                c.settings.parallel = true;
                c.response_policy.stall = Stall::Headers;
                c.expected.terminal_kind = "ExecutionTimeout".into();
            }
            ControlKind::CompleteChunked => c.response_policy.transfer = Transfer::Chunked,
        }
        all.push(complete(c));
    }
    for steps in [1, 8] {
        for limit in [0, 1, 3] {
            for parallel in [false, true] {
                for delay in [0, 100] {
                    let mut c = base(
                        format!("control_retry_exhaustion_s{steps}_l{limit}_p{parallel}_d{delay}"),
                        Family::Retries,
                        steps,
                    );
                    c.control = Some(ControlKind::RetryExhaustion);
                    c.dimensions.retry_limit = limit;
                    c.dimensions.retry_after_ms = delay;
                    c.response_policy.failure_attempts = limit;
                    c.response_policy.exhaust_path = Some(operation(0, 0).path);
                    c.settings.parallel = parallel;
                    c.settings.capture = true;
                    c.expected.terminal_kind = "RetryLimitExceeded".into();
                    all.push(complete(c));
                }
            }
        }
    }
    all
}

pub(crate) fn case(id: &str) -> Result<CaseSpec, HarnessError> {
    catalog()
        .into_iter()
        .find(|c| c.case_id == id)
        .ok_or_else(|| HarnessError::new("catalog", "unknown_case", id))
}
