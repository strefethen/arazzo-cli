//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::{DebugController, DebugScopes, StepCheckpoint};
use arazzo_expr::{is_truthy, EvalContext, ExpressionEvaluator};
use arazzo_spec::{
    ActionType, ArazzoSpec, OnAction, OutputValue, ParamLocation, Parameter, SelectorObject, Step,
    StepTarget, SuccessCriterion, ValueSource, Workflow,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const MAX_RETRIES_PER_STEP: usize = 3;
const MAX_CALL_DEPTH: usize = 10;
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;
pub(crate) const TRACE_BODY_PREVIEW_MAX_BYTES: usize = 2048;
/// Default maximum response body size: 10 MiB.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

mod builder;
mod client;
mod control;
mod criteria;
mod deps;
mod engine_actions;
mod engine_http;
mod engine_impl;
mod engine_parallel;
mod engine_trace;
mod error;
mod events;
#[cfg(test)]
mod helper_tests;
mod helpers;
mod input_validation;
mod jsonpath;
mod payload;
mod redaction;
mod replay;
mod state;
mod url;
mod xpath;

use ::url as url_crate;
use builder::parse_openapi_into_index;
pub use builder::{relative_openapi_source_paths, EngineBuilder};
use client::HttpClient;
pub use client::{
    decide_redirect, is_loopback_host, ClientConfig, ContentType, RateLimitConfig,
    RedirectDecision, RequestConfig, Response, DEFAULT_MAX_REDIRECTS,
};
use control::{sleep_with_cancel, step_result_error};
use criteria::RegexCache;
pub(crate) use criteria::{
    evaluate_criterion, evaluate_criterion_detailed, evaluate_output_expression,
    evaluate_output_value_detailed, CriterionEvaluation,
};
use deps::can_execute_parallel;
#[cfg(test)]
use deps::has_control_flow;
pub(crate) use deps::{build_levels, compute_transitive_deps, extract_step_refs};
use engine_actions::{ActionBranch, FlowDecision, SelectedActionDebugContext, StepDecisionContext};
use engine_impl::merge_workflow_params;
use engine_trace::{build_trace_response, DebugGateContext};
pub use error::{RuntimeError, RuntimeErrorKind};
pub use events::{
    DryRunRequest, EngineEvent, ExecutionEvent, ExecutionEventKind, ExecutionHandle,
    ExecutionObserver, ExecutionResult, ObserverEvent, StepEvent, TraceCriterionResult,
    TraceDecision, TraceDecisionPath, TraceHook, TraceRedirectHop, TraceRequest, TraceResponse,
    TraceStepRecord, TransportWarning, TransportWarningKind,
};
use input_validation::{validate_inputs, InputIssueSeverity};
use jsonpath::{evaluate_jsonpath_condition, JsonPathOutcome};
use payload::{
    apply_replacements, resolve_payload_detailed, resolve_selector, resolve_value_source,
    to_json_path, value_to_string,
};
use replay::{validate_replay_request, ReplayKey, ReplayState};
pub use state::Engine;
use state::ExecutionContext;
pub(crate) use state::VarStore;
use state::{
    EngineInner, OperationEntry, OperationOrigin, StepExecution, StepResult, StepTraceData,
    WorkflowIndex,
};
pub(crate) use url::parse_method;
use url::{encode_cookie_value, parse_source_prefix, replace_path_params, UrlBuildResult};
pub(crate) use xpath::{extract_xpath, select_xpath};

pub use redaction::{
    is_sensitive_key, redact_dry_run_request, redact_headers, redact_json_object,
    redact_json_value, redact_text_patterns, redact_url_query, redacted_dry_run_request, REDACTED,
};
