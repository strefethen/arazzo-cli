/// Runtime error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeErrorKind {
    Unspecified,
    ExecutionTimeout,
    ExecutionCancelled,
    WorkflowNotFound,
    StepNotFound,
    OperationIdNotFound,
    UnsupportedAsyncApiTransport,
    MaxCallDepthExceeded,
    RetryLimitExceeded,
    DependencyCycle,
    GotoTargetNotFound,
    GotoTargetMissing,
    InvalidHttpMethod,
    HttpClientBuild,
    HttpRequest,
    HttpResponseRead,
    RateLimiterLockPoisoned,
    ParallelThreadPanic,
    JsonParse,
    SourceDescriptionParse,
    SourceDescriptionNotFound,
    SubWorkflowFailed,
    SuccessCriteriaFailed,
    DebugController,
    StepMissingDependency,
    InputValidation,
    InternalError,
    ResponseTooLarge,
    ReplayTraceExhausted,
    ReplayRequestMismatch,
    ReplayResponseMissing,
    IterationLimitExceeded,
}

impl RuntimeErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unspecified => "RUNTIME_UNSPECIFIED",
            Self::ExecutionTimeout => "RUNTIME_EXECUTION_TIMEOUT",
            Self::ExecutionCancelled => "RUNTIME_EXECUTION_CANCELLED",
            Self::WorkflowNotFound => "RUNTIME_WORKFLOW_NOT_FOUND",
            Self::StepNotFound => "RUNTIME_STEP_NOT_FOUND",
            Self::OperationIdNotFound => "RUNTIME_OPERATION_ID_NOT_FOUND",
            Self::UnsupportedAsyncApiTransport => "RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT",
            Self::MaxCallDepthExceeded => "RUNTIME_MAX_CALL_DEPTH_EXCEEDED",
            Self::RetryLimitExceeded => "RUNTIME_RETRY_LIMIT_EXCEEDED",
            Self::DependencyCycle => "RUNTIME_DEPENDENCY_CYCLE",
            Self::GotoTargetNotFound => "RUNTIME_GOTO_TARGET_NOT_FOUND",
            Self::GotoTargetMissing => "RUNTIME_GOTO_TARGET_MISSING",
            Self::InvalidHttpMethod => "RUNTIME_INVALID_HTTP_METHOD",
            Self::HttpClientBuild => "RUNTIME_HTTP_CLIENT_BUILD",
            Self::HttpRequest => "RUNTIME_HTTP_REQUEST",
            Self::HttpResponseRead => "RUNTIME_HTTP_RESPONSE_READ",
            Self::RateLimiterLockPoisoned => "RUNTIME_RATE_LIMITER_LOCK_POISONED",
            Self::ParallelThreadPanic => "RUNTIME_PARALLEL_THREAD_PANIC",
            Self::JsonParse => "RUNTIME_JSON_PARSE",
            Self::SourceDescriptionParse => "RUNTIME_SOURCE_DESCRIPTION_PARSE",
            Self::SourceDescriptionNotFound => "RUNTIME_SOURCE_DESCRIPTION_NOT_FOUND",
            Self::SubWorkflowFailed => "RUNTIME_SUB_WORKFLOW_FAILED",
            Self::SuccessCriteriaFailed => "RUNTIME_SUCCESS_CRITERIA_FAILED",
            Self::DebugController => "RUNTIME_DEBUG_CONTROLLER",
            Self::StepMissingDependency => "STEP_MISSING_DEPENDENCY",
            Self::InputValidation => "RUNTIME_INPUT_VALIDATION",
            Self::InternalError => "RUNTIME_INTERNAL_ERROR",
            Self::ResponseTooLarge => "RUNTIME_RESPONSE_TOO_LARGE",
            Self::ReplayTraceExhausted => "RUNTIME_REPLAY_TRACE_EXHAUSTED",
            Self::ReplayRequestMismatch => "RUNTIME_REPLAY_REQUEST_MISMATCH",
            Self::ReplayResponseMissing => "RUNTIME_REPLAY_RESPONSE_MISSING",
            Self::IterationLimitExceeded => "RUNTIME_ITERATION_LIMIT_EXCEEDED",
        }
    }
}

#[derive(Debug)]
pub struct RuntimeError {
    pub kind: RuntimeErrorKind,
    pub message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl RuntimeError {
    pub fn new(kind: RuntimeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        kind: RuntimeErrorKind,
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn unspecified(message: impl Into<String>) -> Self {
        Self::new(RuntimeErrorKind::Unspecified, message)
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<reqwest::Error> for RuntimeError {
    fn from(err: reqwest::Error) -> Self {
        let kind = if err.is_timeout() {
            RuntimeErrorKind::ExecutionTimeout
        } else {
            RuntimeErrorKind::HttpRequest
        };
        Self::with_source(kind, err.to_string(), err)
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(err: serde_json::Error) -> Self {
        Self::with_source(
            RuntimeErrorKind::JsonParse,
            format!("JSON parse error: {err}"),
            err,
        )
    }
}
