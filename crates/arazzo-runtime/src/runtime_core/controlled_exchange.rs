//! One concrete, test-only exchange. The one-slot inbox is registered before
//! the first send; it proves immediate-reply retention only for this fixture.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use arazzo_spec::{StepAction, ValueSource};

use super::asyncapi_target::{resolve_local_channel, ResolvedChannelTarget};
use super::step_context::{with_message, MessageSnapshot};
use super::*;

const SOURCE: &str = "events";
const DOCUMENT: &str = "events.yaml";
const REQUEST: (&str, &str) = ("request", "/orders/submit");
const REPLY: (&str, &str) = ("reply", "/orders/replies");
const FORWARD: (&str, &str) = ("forward", "/orders/forward");

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Route {
    pub source: String,
    pub document: String,
    pub channel_key: String,
    pub channel_pointer: String,
    pub address: Option<String>,
    pub action: StepAction,
}

impl From<&ResolvedChannelTarget> for Route {
    fn from(target: &ResolvedChannelTarget) -> Self {
        Self {
            source: target.source_name.clone(),
            document: target.document.clone(),
            channel_key: target.channel_key.clone(),
            channel_pointer: target.channel_pointer.clone(),
            address: target.address.clone(),
            action: target.action,
        }
    }
}

pub(super) struct Exchange {
    retrieval_dir: PathBuf,
    pub(super) routes: Mutex<Vec<Route>>,
    pub(super) sent: Mutex<Vec<Value>>,
    reply: Option<MessageSnapshot>,
    inbox_tx: mpsc::Sender<MessageSnapshot>,
    pub(super) inbox_rx: tokio::sync::Mutex<mpsc::Receiver<MessageSnapshot>>,
    pub(super) receive_started: tokio::sync::Notify,
    pub(super) active_waits: AtomicUsize,
    pub(super) receive_attempts: AtomicUsize,
    fail_send: bool,
    wait_limit: Duration,
}

struct ActiveWait<'a>(&'a AtomicUsize);

impl Drop for ActiveWait<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Exchange {
    pub(super) fn new(
        retrieval_dir: &Path,
        reply: Option<MessageSnapshot>,
        fail_send: bool,
        wait_limit: Duration,
    ) -> Arc<Self> {
        let (inbox_tx, inbox_rx) = mpsc::channel(1);
        Arc::new(Self {
            retrieval_dir: retrieval_dir.to_path_buf(),
            routes: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            reply,
            inbox_tx,
            inbox_rx: tokio::sync::Mutex::new(inbox_rx),
            receive_started: tokio::sync::Notify::new(),
            active_waits: AtomicUsize::new(0),
            receive_attempts: AtomicUsize::new(0),
            fail_send,
            wait_limit,
        })
    }

    fn expect_route(&self, target: &ResolvedChannelTarget) -> Result<(), RuntimeError> {
        let route = Route::from(target);
        self.routes
            .lock()
            .map_err(|_| RuntimeError::unspecified("EXCHANGE_ROUTE_LOCK_POISONED"))?
            .push(route.clone());
        let expected = match (target.channel_key.as_str(), target.action) {
            (key, StepAction::Send) if key == REQUEST.0 => REQUEST.1,
            (key, StepAction::Receive) if key == REPLY.0 => REPLY.1,
            (key, StepAction::Send) if key == FORWARD.0 => FORWARD.1,
            _ => {
                return Err(RuntimeError::unspecified(format!(
                    "EXCHANGE_UNEXPECTED_CHANNEL: {route:?}"
                )))
            }
        };
        if target.source_name != SOURCE
            || target.document != self.retrieval_dir.join(DOCUMENT).to_string_lossy()
            || target.channel_pointer != format!("/channels/{}", target.channel_key)
            || target.address.as_deref() != Some(expected)
        {
            return Err(RuntimeError::unspecified(format!(
                "EXCHANGE_UNEXPECTED_CHANNEL: {route:?}"
            )));
        }
        Ok(())
    }

    fn send(&self, target: &ResolvedChannelTarget, payload: Value) -> Result<(), RuntimeError> {
        if self.fail_send && target.channel_key == REQUEST.0 {
            return Err(RuntimeError::unspecified("EXCHANGE_SEND_FAILURE"));
        }
        let first = {
            let mut sent = self
                .sent
                .lock()
                .map_err(|_| RuntimeError::unspecified("EXCHANGE_SENT_LOCK_POISONED"))?;
            let first = sent.is_empty();
            sent.push(payload);
            first
        };
        if first {
            if let Some(reply) = &self.reply {
                // The reply arrives before this send returns and before the
                // engine schedules the receive step.
                self.inbox_tx
                    .try_send(reply.clone())
                    .map_err(|err| RuntimeError::unspecified(format!("EXCHANGE_INBOX: {err}")))?;
            }
        }
        Ok(())
    }

    async fn receive(&self, ctx: &ExecutionContext) -> Result<MessageSnapshot, RuntimeError> {
        // One receiver owns the fixed inbox while waiting. Dropping this
        // future on cancellation or expiry releases both the guard and lock.
        let mut inbox = self.inbox_rx.lock().await;
        self.active_waits.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveWait(&self.active_waits);
        self.receive_attempts.fetch_add(1, Ordering::SeqCst);
        self.receive_started.notify_one();
        tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => Err(ctx.cancelled_error()),
            result = tokio::time::timeout(self.wait_limit, inbox.recv()) => {
                match result {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => Err(RuntimeError::unspecified("EXCHANGE_INBOX_CLOSED")),
                    Err(_) => Err(RuntimeError::unspecified("EXCHANGE_RECEIVE_WAIT_EXPIRED")),
                }
            }
        }
    }
}

/// Guard activation before the parallel scheduler can bypass the sequential
/// dispatch hook. No production configuration or public mode is changed.
pub(super) fn require_supported_mode(engine: &Engine) -> Result<(), RuntimeError> {
    if engine.inner.parallel_mode
        || engine.inner.dry_run_mode
        || engine.inner.trace_enabled
        || engine.inner.capture_run
        || engine.inner.trace_hook.is_some()
        || engine.inner.debug_controller.is_some()
        || !engine.inner.client.is_live()
    {
        return Err(RuntimeError::unspecified("EXCHANGE_UNSUPPORTED_MODE"));
    }
    Ok(())
}

pub(super) async fn execute(
    engine: &Engine,
    exchange: &Exchange,
    ctx: &ExecutionContext,
    step: &Step,
    vars: &VarStore,
) -> Result<StepExecution, RuntimeError> {
    ctx.check_cancelled()?;
    if step.correlation_id.is_some() || step.request_body.is_some() {
        return Err(RuntimeError::unspecified("EXCHANGE_UNSUPPORTED_STEP_FORM"));
    }
    let target = resolve_local_channel(
        &engine.inner.index.spec,
        step,
        Some(&exchange.retrieval_dir),
    )
    .map_err(|err| {
        RuntimeError::unspecified(format!(
            "EXCHANGE_RESOLVE_{:?}: target={:?} source={:?} document={:?}: {}",
            err.kind, err.target, err.source_name, err.document, err.detail
        ))
    })?;
    exchange.expect_route(&target)?;

    let message = match target.action {
        StepAction::Send => {
            let template = match target.channel_key.as_str() {
                key if key == REQUEST.0 => json!({"order":"$inputs.order"}),
                key if key == FORWARD.0 => json!({"id":"$steps.receive.outputs.id"}),
                _ => return Err(RuntimeError::unspecified("EXCHANGE_UNEXPECTED_SEND")),
            };
            let template: ValueSource = serde_yaml_ng::to_value(template)
                .map(ValueSource::from)
                .map_err(|err| RuntimeError::unspecified(format!("EXCHANGE_TEMPLATE: {err}")))?;
            let eval = ExpressionEvaluator::new(engine.make_eval_context(vars, None));
            let (payload, warnings) = resolve_payload_detailed(&template, &eval);
            if !warnings.is_empty() {
                return Err(RuntimeError::unspecified(format!(
                    "EXCHANGE_PAYLOAD_DIAGNOSTIC: {warnings:?}"
                )));
            }
            exchange.send(&target, payload)?;
            None
        }
        StepAction::Receive => Some(exchange.receive(ctx).await?),
    };

    let eval = ExpressionEvaluator::new(with_message(
        engine.make_eval_context(vars, None),
        message.as_ref(),
    ));
    let mut success = true;
    for criterion in &step.success_criteria {
        let evaluated =
            evaluate_criterion_detailed(criterion, &eval, None, &engine.inner.regex_cache);
        if evaluated.error.is_some() || !evaluated.warnings.is_empty() {
            return Err(RuntimeError::unspecified(format!(
                "EXCHANGE_CRITERION_DIAGNOSTIC: {evaluated:?}"
            )));
        }
        if !evaluated.matched {
            success = false;
            break;
        }
    }
    let mut outputs = BTreeMap::new();
    if success {
        for (name, value) in &step.outputs {
            let (value, warnings) = evaluate_output_value_detailed(value, &eval, None);
            if !warnings.is_empty() {
                return Err(RuntimeError::unspecified(format!(
                    "EXCHANGE_OUTPUT_DIAGNOSTIC: {warnings:?}"
                )));
            }
            outputs.insert(name.clone(), value);
        }
    }
    Ok(StepExecution {
        result: StepResult {
            success,
            response: None,
            message,
            err: None,
            err_kind: None,
        },
        outputs,
        dry_run_request: None,
        trace: StepTraceData::default(),
    })
}
