//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{DebugController, DebugScopes, StepCheckpoint};
use arazzo_expr::{EvalContext, ExpressionEvaluator};
use arazzo_spec::{
    classify_operation_id, classify_operation_path, ActionType, ArazzoSpec, OnAction,
    OperationIdTarget, OperationPathTarget, OutputValue, ParamLocation, Parameter, SelectorObject,
    SourceType, Step, StepTarget, SuccessCriterion, ValueSource, Workflow,
    SUPPORTED_OPERATION_ID_FORMS, SUPPORTED_OPERATION_PATH_FORMS,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Arazzo 1.1 Failure Action Object: an omitted `retryLimit` permits one retry.
const DEFAULT_RETRY_LIMIT: u64 = 1;
const MAX_CALL_DEPTH: usize = 10;
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;
pub(crate) const TRACE_BODY_PREVIEW_MAX_BYTES: usize = 2048;
/// Default maximum response body size: 10 MiB.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) mod asyncapi_receive;
#[cfg(test)]
mod asyncapi_receive_tests;
mod asyncapi_target;
#[cfg(test)]
mod asyncapi_target_tests;
mod builder;
mod client;
mod control;
#[cfg(test)]
mod controlled_exchange;
#[cfg(test)]
mod controlled_exchange_tests;
mod criteria;
mod deps;
mod document_set;
mod engine_actions;
mod engine_http;
#[cfg(test)]
mod engine_http_tests;
mod engine_impl;
mod engine_message;
mod engine_parallel;
mod engine_trace;
mod error;
mod events;
#[cfg(test)]
mod helper_tests;
mod helpers;
mod input_validation;
mod jsonpath;
mod kafka;
#[cfg(test)]
mod nats_transport_trial;
#[cfg(test)]
mod nats_transport_trial_tests;
mod operation_target;
mod payload;
mod redaction;
mod replay;
mod run_record;
mod state;
mod step_attempt;
mod step_context;
#[cfg(test)]
mod step_context_tests;
mod url;
mod xml_admission;
mod xpath;

use ::url as url_crate;
pub use builder::{relative_openapi_source_paths, EngineBuilder};
use client::HttpClient;
pub use client::{
    decide_redirect, is_loopback_host, ClientConfig, ContentType, RateLimitConfig,
    RedirectDecision, RequestConfig, Response, DEFAULT_MAX_REDIRECTS,
};
use control::{sleep_with_cancel, step_result_error};
use criteria::RegexCache;
pub(crate) use criteria::{
    evaluate_criterion, evaluate_criterion_detailed, evaluate_output_value_detailed,
    CriterionEvaluation,
};
pub(crate) use deps::{build_levels, compute_transitive_deps, extract_step_refs, parallel_blocker};
use engine_actions::{
    applicable_actions, effective_retry_limit, ActionBranch, FlowDecision, RetryReference,
    RetrySite, SelectedActionDebugContext,
};
use engine_impl::merge_workflow_params;
use engine_trace::{build_trace_response, DebugGateContext};
pub use error::{RuntimeError, RuntimeErrorKind};
pub use events::{
    DryRunRequest, EngineEvent, ExecutionEvent, ExecutionEventKind, ExecutionHandle,
    ExecutionObserver, ExecutionResult, ObserverEvent, ReceiverArmed, SequentialFallback,
    SequentialFallbackReason, StepEvent, TraceCriterionResult, TraceDecision, TraceDecisionPath,
    TraceHook, TraceMessage, TraceRedirectHop, TraceRequest, TraceResponse, TraceStepRecord,
    TransportWarning, TransportWarningKind,
};
use input_validation::{validate_inputs, InputIssueSeverity};
use jsonpath::jsonpath_condition_holds;
pub use kafka::{MessageTransportOptions, Secret, DEFAULT_CONNECT_TIMEOUT};
use payload::{
    apply_replacements, json_type_name, resolve_payload_detailed, resolve_selector,
    resolve_value_source, to_json_path, value_to_string,
};
use replay::{validate_replay_request, ReplayKey, ReplayState};
pub use run_record::{RunRequestMetadata, RunResponseMetadata, RunStepOutcome, RunStepRecord};
pub use state::Engine;
use state::ExecutionContext;
pub(crate) use state::VarStore;
use state::{
    ContextRole, EngineInner, OperationEntry, OperationIndex, OperationMatch, OperationOrigin,
    OperationServer, ServerIssue, ServerLevel, StepExecution, StepResult, StepTraceData,
    UnusableServer, WorkflowIndex,
};
use url::{encode_cookie_value, replace_path_params, UrlBuildResult};
pub(crate) use xpath::{replace_xpath, select_xpath, xpath_version_rejection};

pub use redaction::{
    is_schema_declared_secret, is_sensitive_key, redact_dry_run_request, redact_headers,
    redact_inputs, redact_json_object, redact_json_value, redact_text_patterns, redact_url,
    redacted_dry_run_request, sensitive_input_names, REDACTED,
};
