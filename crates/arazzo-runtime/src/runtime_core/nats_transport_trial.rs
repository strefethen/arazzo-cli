//! One concrete, cfg(test) Core NATS trial. Addresses come only from the local
//! channel resolver; connection and payload configuration remain fixture-owned.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arazzo_spec::{StepAction, ValueSource};
use futures_util::StreamExt;

use super::asyncapi_target::{resolve_local_channel, ResolvedChannelTarget};
use super::step_context::{with_message, MessageSnapshot};
use super::*;

const IO_LIMIT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Route {
    pub source: String,
    pub document: String,
    pub channel_key: String,
    pub channel_pointer: String,
    pub address: String,
    pub action: StepAction,
}

impl Route {
    fn from_target(target: &ResolvedChannelTarget) -> Result<Self, RuntimeError> {
        let address = target.address.as_deref().ok_or_else(|| {
            RuntimeError::unspecified("NATS_TRIAL_UNSUPPORTED_ADDRESS: missing channel address")
        })?;
        if address.is_empty()
            || address.contains(|c: char| c.is_ascii_whitespace() || c == '*' || c == '>')
            || address.starts_with('.')
            || address.ends_with('.')
        {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_UNSUPPORTED_ADDRESS: {address:?}"
            )));
        }
        Ok(Self {
            source: target.source_name.clone(),
            document: target.document.clone(),
            channel_key: target.channel_key.clone(),
            channel_pointer: target.channel_pointer.clone(),
            address: address.to_string(),
            action: target.action,
        })
    }
}

pub(super) struct NatsTrial {
    retrieval_dir: PathBuf,
    registered_reply: Route,
    client: async_nats::Client,
    reply: tokio::sync::Mutex<mpsc::Receiver<async_nats::Message>>,
    reply_pump: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pump_stop: tokio_util::sync::CancellationToken,
    reply_buffered: Arc<tokio::sync::Notify>,
    buffered_count: Arc<AtomicUsize>,
    receive_gate: tokio::sync::Notify,
    receive_gate_entered: tokio::sync::Notify,
    terminal: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    terminal_notice: Arc<tokio::sync::Notify>,
    pub(super) routes: Mutex<Vec<Route>>,
    pub(super) receive_started: tokio::sync::Notify,
    pub(super) active_waits: AtomicUsize,
    pub(super) receive_attempts: AtomicUsize,
    wait_limit: Duration,
    hold_receive: bool,
}

struct ActiveWait<'a>(&'a AtomicUsize);
impl Drop for ActiveWait<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl NatsTrial {
    pub(super) async fn connect(
        engine: &Engine,
        endpoint: &str,
        retrieval_dir: &Path,
        receive_step: &Step,
        wait_limit: Duration,
        hold_receive: bool,
    ) -> Result<Arc<Self>, RuntimeError> {
        let target = resolve(engine, receive_step, retrieval_dir)?;
        let route = Route::from_target(&target)?;
        if route.action != StepAction::Receive || route.channel_key != "reply" {
            return Err(RuntimeError::unspecified(
                "NATS_TRIAL_UNEXPECTED_RECEIVE_ROUTE",
            ));
        }
        let terminal = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let terminal_notice = Arc::new(tokio::sync::Notify::new());
        let on_terminal = Arc::clone(&terminal);
        let on_closed = Arc::clone(&closed);
        let on_notice = Arc::clone(&terminal_notice);
        let options = async_nats::ConnectOptions::new()
            .connection_timeout(IO_LIMIT)
            .max_reconnects(1usize)
            .event_callback(move |event| {
                let flag = Arc::clone(&on_terminal);
                let closed = Arc::clone(&on_closed);
                let notice = Arc::clone(&on_notice);
                async move {
                    if matches!(
                        event,
                        async_nats::Event::Disconnected | async_nats::Event::Closed
                    ) {
                        flag.store(true, Ordering::SeqCst);
                        notice.notify_one();
                    }
                    if matches!(event, async_nats::Event::Closed) {
                        closed.store(true, Ordering::SeqCst);
                        notice.notify_one();
                    }
                }
            });
        let client = tokio::time::timeout(IO_LIMIT, options.connect(endpoint))
            .await
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_CONNECT_EXPIRED"))?
            .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_CONNECT: {err}")))?;
        let mut subscriber =
            tokio::time::timeout(IO_LIMIT, client.subscribe(route.address.clone()))
                .await
                .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_SUBSCRIBE_EXPIRED"))?
                .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_SUBSCRIBE: {err}")))?;
        tokio::time::timeout(IO_LIMIT, client.flush())
            .await
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_READY_EXPIRED"))?
            .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_READY: {err}")))?;
        let (reply_tx, reply) = mpsc::channel(1);
        let pump_stop = tokio_util::sync::CancellationToken::new();
        let stop = pump_stop.clone();
        let reply_buffered = Arc::new(tokio::sync::Notify::new());
        let buffered_notice = Arc::clone(&reply_buffered);
        let buffered_count = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&buffered_count);
        let pump = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => break,
                    next = subscriber.next() => match next {
                        Some(message) => {
                            if reply_tx.send(message).await.is_err() { break; }
                            count.fetch_add(1, Ordering::SeqCst);
                            buffered_notice.notify_one();
                        }
                        None => break,
                    }
                }
            }
            let _ = tokio::time::timeout(IO_LIMIT, subscriber.unsubscribe()).await;
        });
        Ok(Arc::new(Self {
            retrieval_dir: retrieval_dir.to_path_buf(),
            registered_reply: route,
            client,
            reply: tokio::sync::Mutex::new(reply),
            reply_pump: tokio::sync::Mutex::new(Some(pump)),
            pump_stop,
            reply_buffered,
            buffered_count,
            receive_gate: tokio::sync::Notify::new(),
            receive_gate_entered: tokio::sync::Notify::new(),
            terminal,
            closed,
            terminal_notice,
            routes: Mutex::new(Vec::new()),
            receive_started: tokio::sync::Notify::new(),
            active_waits: AtomicUsize::new(0),
            receive_attempts: AtomicUsize::new(0),
            wait_limit,
            hold_receive,
        }))
    }

    fn record(&self, target: &ResolvedChannelTarget) -> Result<Route, RuntimeError> {
        let route = Route::from_target(target)?;
        let expected = matches!(
            (route.channel_key.as_str(), route.action),
            ("request", StepAction::Send)
                | ("reply", StepAction::Receive)
                | ("forward", StepAction::Send)
        );
        if !expected || route.channel_pointer != format!("/channels/{}", route.channel_key) {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_UNEXPECTED_ROUTE: {route:?}"
            )));
        }
        if route.action == StepAction::Receive && route != self.registered_reply {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_RECEIVE_ROUTE_CHANGED: registered={:?} resolved={route:?}",
                self.registered_reply
            )));
        }
        self.routes
            .lock()
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_ROUTE_LOCK"))?
            .push(route.clone());
        Ok(route)
    }

    async fn send(&self, route: &Route, payload: &Value) -> Result<(), RuntimeError> {
        self.ensure_connected()?;
        let bytes = serde_json::to_vec(payload)
            .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_JSON_ENCODE: {err}")))?;
        tokio::time::timeout(
            IO_LIMIT,
            self.client.publish(route.address.clone(), bytes.into()),
        )
        .await
        .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_PUBLISH_EXPIRED"))?
        .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_PUBLISH: {err}")))?;
        tokio::time::timeout(IO_LIMIT, self.client.flush())
            .await
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_FLUSH_EXPIRED"))?
            .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_FLUSH: {err}")))?;
        self.ensure_connected()?;
        Ok(())
    }

    async fn receive(&self, ctx: &ExecutionContext) -> Result<MessageSnapshot, RuntimeError> {
        self.ensure_connected()?;
        let mut receiver = self.reply.lock().await;
        if self.hold_receive {
            self.receive_gate_entered.notify_one();
            tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => return Err(ctx.cancelled_error()),
                () = self.terminal_notice.notified() => return Err(disconnected()),
                result = tokio::time::timeout(self.wait_limit, self.receive_gate.notified()) => {
                    if result.is_err() { return Err(RuntimeError::unspecified("NATS_TRIAL_RECEIVE_GATE_EXPIRED")); }
                }
            }
        }
        self.active_waits.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveWait(&self.active_waits);
        self.receive_attempts.fetch_add(1, Ordering::SeqCst);
        self.receive_started.notify_one();
        let message = tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => return Err(ctx.cancelled_error()),
                () = self.terminal_notice.notified() => return Err(disconnected()),
                result = tokio::time::timeout(self.wait_limit, receiver.recv()) => match result {
                    Ok(Some(message)) => message,
                    Ok(None) => return Err(disconnected()),
                    Err(_) => return Err(RuntimeError::unspecified("NATS_TRIAL_RECEIVE_WAIT_EXPIRED")),
                },
        };
        ctx.check_cancelled()?;
        self.ensure_connected()?;
        let payload = serde_json::from_slice(&message.payload).map_err(|err| {
            RuntimeError::unspecified(format!("NATS_TRIAL_MALFORMED_REPLY: {err}"))
        })?;
        let mut headers = BTreeMap::new();
        if let Some(map) = &message.headers {
            for (name, values) in map.iter() {
                let [value] = values.as_slice() else {
                    return Err(RuntimeError::unspecified(format!(
                        "NATS_TRIAL_MULTI_VALUE_HEADER: {name}"
                    )));
                };
                headers.insert(name.to_string().to_ascii_lowercase(), value.to_string());
            }
        }
        Ok(MessageSnapshot {
            headers,
            payload: Some(payload),
        })
    }

    fn ensure_connected(&self) -> Result<(), RuntimeError> {
        if self.terminal.load(Ordering::SeqCst)
            || self.client.connection_state() != async_nats::connection::State::Connected
        {
            Err(disconnected())
        } else {
            Ok(())
        }
    }

    pub(super) async fn close(&self) -> Result<(), RuntimeError> {
        self.pump_stop.cancel();
        if let Some(mut pump) = self.reply_pump.lock().await.take() {
            match tokio::time::timeout(IO_LIMIT, &mut pump).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    return Err(RuntimeError::unspecified(format!(
                        "NATS_TRIAL_PUMP_JOIN: {err}"
                    )))
                }
                Err(_) => {
                    pump.abort();
                    let _ = pump.await;
                    return Err(RuntimeError::unspecified("NATS_TRIAL_PUMP_JOIN_EXPIRED"));
                }
            }
        }
        // Drain can stall after a server death. Bound it, then drop the client
        // when the sole fixture-owned Arc is released.
        match tokio::time::timeout(IO_LIMIT, self.client.drain()).await {
            Ok(Ok(())) => {
                if tokio::time::timeout(IO_LIMIT, async {
                    while !self.closed.load(Ordering::SeqCst) {
                        self.terminal_notice.notified().await;
                    }
                })
                .await
                .is_ok()
                {
                    Ok(())
                } else {
                    Err(RuntimeError::unspecified("NATS_TRIAL_CLOSED_NOT_OBSERVED"))
                }
            }
            Ok(Err(err)) => Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_DRAIN: {err}"
            ))),
            Err(_) => Err(RuntimeError::unspecified("NATS_TRIAL_DRAIN_EXPIRED")),
        }
    }

    pub(super) fn terminal(&self) -> bool {
        self.terminal.load(Ordering::SeqCst)
    }
    pub(super) fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    pub(super) fn receiver_free(&self) -> bool {
        self.reply.try_lock().is_ok()
    }
    pub(super) fn buffered(&self) -> usize {
        self.buffered_count.load(Ordering::SeqCst)
    }
    pub(super) async fn wait_buffered(&self) -> Result<(), RuntimeError> {
        if self.buffered() > 0 {
            return Ok(());
        }
        tokio::time::timeout(IO_LIMIT, self.reply_buffered.notified())
            .await
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_BUFFER_EXPIRED"))
    }
    pub(super) fn release_receive(&self) {
        self.receive_gate.notify_one();
    }
    pub(super) async fn wait_receive_gate(&self) -> Result<(), RuntimeError> {
        tokio::time::timeout(IO_LIMIT, self.receive_gate_entered.notified())
            .await
            .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_GATE_ENTRY_EXPIRED"))
    }
}

impl Drop for NatsTrial {
    fn drop(&mut self) {
        self.pump_stop.cancel();
        if let Ok(mut pump) = self.reply_pump.try_lock() {
            if let Some(handle) = pump.take() {
                handle.abort();
            }
        }
    }
}

fn disconnected() -> RuntimeError {
    RuntimeError::unspecified("NATS_TRIAL_DISCONNECTED")
}

fn resolve(
    engine: &Engine,
    step: &Step,
    dir: &Path,
) -> Result<ResolvedChannelTarget, RuntimeError> {
    resolve_local_channel(&engine.inner.index.spec, step, Some(dir)).map_err(|err| {
        RuntimeError::unspecified(format!(
            "NATS_TRIAL_RESOLVE_{:?}: target={:?} source={:?} document={:?}: {}",
            err.kind, err.target, err.source_name, err.document, err.detail
        ))
    })
}

pub(super) fn require_supported_mode(engine: &Engine) -> Result<(), RuntimeError> {
    if engine.inner.controlled_exchange.is_some() {
        return Err(RuntimeError::unspecified("NATS_TRIAL_DUAL_FIXTURE"));
    }
    super::controlled_exchange::require_supported_mode(engine)
        .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_UNSUPPORTED_MODE"))
}

pub(super) async fn execute(
    engine: &Engine,
    trial: &NatsTrial,
    ctx: &ExecutionContext,
    step: &Step,
    vars: &VarStore,
) -> Result<StepExecution, RuntimeError> {
    ctx.check_cancelled()?;
    if step.correlation_id.is_some() || step.request_body.is_some() {
        return Err(RuntimeError::unspecified(
            "NATS_TRIAL_UNSUPPORTED_STEP_FORM",
        ));
    }
    let target = resolve(engine, step, &trial.retrieval_dir)?;
    let route = trial.record(&target)?;
    let message = match route.action {
        StepAction::Send => {
            let template = match route.channel_key.as_str() {
                "request" => json!({"order":"$inputs.order"}),
                "forward" => json!({"id":"$steps.receive.outputs.id"}),
                _ => return Err(RuntimeError::unspecified("NATS_TRIAL_UNEXPECTED_SEND")),
            };
            let template: ValueSource = serde_yaml_ng::to_value(template)
                .map(ValueSource::from)
                .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_TEMPLATE: {err}")))?;
            let eval = ExpressionEvaluator::new(engine.make_eval_context(vars, None));
            let (payload, warnings) = resolve_payload_detailed(&template, &eval);
            if !warnings.is_empty() {
                return Err(RuntimeError::unspecified(format!(
                    "NATS_TRIAL_PAYLOAD_DIAGNOSTIC: {warnings:?}"
                )));
            }
            trial.send(&route, &payload).await?;
            None
        }
        StepAction::Receive => Some(trial.receive(ctx).await?),
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
                "NATS_TRIAL_CRITERION_DIAGNOSTIC: {evaluated:?}"
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
                    "NATS_TRIAL_OUTPUT_DIAGNOSTIC: {warnings:?}"
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
