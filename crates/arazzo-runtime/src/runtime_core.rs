//! Workflow execution runtime for the Rust implementation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
mod blob;
mod builder;
mod bytes_descriptor;
mod client;
mod control;
#[cfg(test)]
mod controlled_exchange;
#[cfg(test)]
mod controlled_exchange_tests;
mod criteria;
mod deps;
mod document_set;
mod dry_run;
mod engine_actions;
mod engine_http;
#[cfg(test)]
mod engine_http_tests;
mod engine_impl;
mod engine_message;
mod engine_parallel;
mod engine_trace;
mod error;
mod eval_context;
mod events;
mod form_body;
#[cfg(test)]
mod helper_tests;
mod helpers;
mod input_validation;
mod jsonpath;
mod kafka;
mod multipart_body;
#[cfg(test)]
mod nats_transport_trial;
#[cfg(test)]
mod nats_transport_trial_tests;
mod operation_target;
mod payload;
#[cfg(test)]
mod payload_provenance_tests;
mod redaction;
mod regex_cache;
mod replay;
mod replay_projection;
mod request_admission;
mod request_body;
mod response_body;
mod run_record;
mod source_document;
mod state;
mod step_attempt;
mod step_context;
#[cfg(test)]
mod step_context_tests;
mod url;
mod value_provenance;
#[cfg(test)]
mod value_provenance_tests;
#[cfg(test)]
mod value_unavailable_tests;
mod xml_admission;
mod xpath;

use ::url as url_crate;
pub use blob::{Blob, WorkflowInputs, WorkflowOutputs};
use blob::{BlobSources, Slot};
pub use builder::{relative_openapi_source_paths, EngineBuilder};
pub use bytes_descriptor::{BytesDescriptor, ContentDescriptor, RedactedDescriptor};
use client::HttpClient;
pub use client::{
    decide_redirect, is_loopback_host, ClientConfig, RateLimitConfig, RedirectDecision,
    RequestConfig, Response, DEFAULT_MAX_REDIRECTS,
};
use control::{sleep_with_cancel, step_result_error};
#[cfg(test)]
pub(crate) use criteria::evaluate_criterion;
#[cfg(test)]
use criteria::evaluate_provenanced_output;
use criteria::evaluate_step_output;
pub(crate) use criteria::{
    evaluate_criterion_detailed, evaluate_output_value_detailed, CriterionEvaluation,
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
    DryRunRequest, EngineEvent, ExecutionCancellation, ExecutionEvent, ExecutionEventKind,
    ExecutionHandle, ExecutionObserver, ExecutionOutcome, ExecutionResult, ObserverEvent,
    ReceiverArmed, SequentialFallback, SequentialFallbackReason, StepEvent, TraceCriterionResult,
    TraceDecision, TraceDecisionPath, TraceHook, TraceMessage, TraceRedirectHop, TraceRequest,
    TraceResponse, TraceStepRecord, TransportWarning, TransportWarningKind, WorkflowOutputWarning,
};
#[cfg(test)]
use input_validation::validate_inputs;
use input_validation::InputIssueSeverity;
use jsonpath::jsonpath_condition_holds;
pub use kafka::{MessageTransportOptions, Secret, DEFAULT_CONNECT_TIMEOUT};
#[cfg(test)]
use payload::resolve_payload_detailed;
use payload::{
    apply_replacements, json_type_name, resolve_selector, to_json_path, value_to_string,
};
use regex_cache::RegexCache;
use replay::{ReplayKey, ReplayState};
use response_body::{truncate_with_ellipsis, BodyView};
pub use run_record::{RunRequestMetadata, RunResponseMetadata, RunStepOutcome, RunStepRecord};
pub use source_document::SourceDocumentLoader;
pub use state::Engine;
use state::ExecutionContext;
pub(crate) use state::VarStore;
use state::{
    ContextRole, EngineInner, OperationEntry, OperationIndex, OperationMatch, OperationOrigin,
    OperationServer, ServerIssue, ServerLevel, StepExecution, StepResult, StepTraceData,
    UnusableServer, WorkflowIndex,
};
use url::{encode_cookie_value, replace_path_params, UrlBuildResult};
use value_provenance::{
    ProvenancedValues, ValueProducer, ValueProducerSite, ValueProducerTransport, ValueProvenance,
};
pub(crate) use xpath::{replace_xpath, select_xpath, xpath_version_rejection};

pub use redaction::{
    is_schema_declared_secret, is_sensitive_key, redact_dry_run_request, redact_headers,
    redact_inputs, redact_json_object, redact_json_value, redact_text_patterns,
    redact_trace_request, redact_url, redact_url_components, redacted_dry_run_request,
    sensitive_input_names, REDACTED,
};
