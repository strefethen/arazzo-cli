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

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PreparedReceive {
    pub workflow_id: String,
    pub step_id: String,
    pub route: Route,
}

pub(super) struct NatsTrial {
    retrieval_dir: PathBuf,
    prepared_receive: PreparedReceive,
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
    pub(super) fn prepare_receive(
        engine: &Engine,
        workflow_id: &str,
        retrieval_dir: &Path,
    ) -> Result<PreparedReceive, RuntimeError> {
        let workflow = engine
            .inner
            .index
            .spec
            .workflows
            .iter()
            .find(|workflow| workflow.workflow_id == workflow_id)
            .ok_or_else(|| {
                RuntimeError::unspecified(format!("NATS_TRIAL_UNKNOWN_WORKFLOW: {workflow_id}"))
            })?;
        let mut receivers = workflow
            .steps
            .iter()
            .filter(|step| step.action == Some(StepAction::Receive));
        let step = receivers.next().ok_or_else(|| {
            RuntimeError::unspecified(format!("NATS_TRIAL_NO_RECEIVER: {workflow_id}"))
        })?;
        if receivers.next().is_some() {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_MULTIPLE_RECEIVERS: {workflow_id}"
            )));
        }
        require_supported_step(step)?;
        let route = Route::from_target(&resolve(engine, step, retrieval_dir)?)?;
        Ok(PreparedReceive {
            workflow_id: workflow_id.to_string(),
            step_id: step.step_id.clone(),
            route,
        })
    }

    pub(super) fn workflow_id(&self) -> &str {
        &self.prepared_receive.workflow_id
    }

    pub(super) async fn connect(
        engine: &Engine,
        endpoint: &str,
        retrieval_dir: &Path,
        workflow_id: &str,
        wait_limit: Duration,
        hold_receive: bool,
    ) -> Result<Arc<Self>, RuntimeError> {
        // Complete discovery and local resolution before acquiring any client.
        let prepared_receive = Self::prepare_receive(engine, workflow_id, retrieval_dir)?;
        let route = &prepared_receive.route;
        let terminal = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let terminal_notice = Arc::new(tokio::sync::Notify::new());
        let on_terminal = Arc::clone(&terminal);
        let on_closed = Arc::clone(&closed);
        let on_notice = Arc::clone(&terminal_notice);
        let options = async_nats::ConnectOptions::new()
            .connection_timeout(IO_LIMIT)
            .max_reconnects(1usize)
            .reconnect_delay_callback(|_| Duration::from_millis(10))
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
        let setup = async {
            let subscriber =
                tokio::time::timeout(IO_LIMIT, client.subscribe(route.address.clone()))
                    .await
                    .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_SUBSCRIBE_EXPIRED"))?
                    .map_err(|err| {
                        RuntimeError::unspecified(format!("NATS_TRIAL_SUBSCRIBE: {err}"))
                    })?;
            tokio::time::timeout(IO_LIMIT, client.flush())
                .await
                .map_err(|_| RuntimeError::unspecified("NATS_TRIAL_READY_EXPIRED"))?
                .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_READY: {err}")))?;
            Ok::<_, RuntimeError>(subscriber)
        }
        .await;
        let mut subscriber = match setup {
            Ok(subscriber) => subscriber,
            Err(err) => {
                if let Err(cleanup) = terminate_client(&client, &terminal, &closed).await {
                    return Err(RuntimeError::unspecified(format!(
                        "{err}; NATS_TRIAL_SETUP_CLEANUP: {cleanup}"
                    )));
                }
                return Err(err);
            }
        };
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
            prepared_receive,
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

    fn record(&self, step: &Step, target: &ResolvedChannelTarget) -> Result<Route, RuntimeError> {
        let route = Route::from_target(target)?;
        let expected = route.action == StepAction::Receive
            || matches!(route.channel_key.as_str(), "request" | "forward");
        if !expected || route.channel_pointer != format!("/channels/{}", route.channel_key) {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_UNEXPECTED_ROUTE: {route:?}"
            )));
        }
        if route.action == StepAction::Receive
            && (step.step_id != self.prepared_receive.step_id
                || route != self.prepared_receive.route)
        {
            return Err(RuntimeError::unspecified(format!(
                "NATS_TRIAL_RECEIVE_ROUTE_CHANGED: prepared={:?} step={} resolved={route:?}",
                self.prepared_receive, step.step_id
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
        let mut pump_error = None;
        if let Some(mut pump) = self.reply_pump.lock().await.take() {
            match tokio::time::timeout(IO_LIMIT, &mut pump).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    pump_error = Some(RuntimeError::unspecified(format!(
                        "NATS_TRIAL_PUMP_JOIN: {err}"
                    )));
                }
                Err(_) => {
                    pump.abort();
                    let _ = pump.await;
                    pump_error = Some(RuntimeError::unspecified("NATS_TRIAL_PUMP_JOIN_EXPIRED"));
                }
            }
        }
        let client_result = terminate_client(&self.client, &self.terminal, &self.closed).await;
        if let Some(err) = pump_error {
            return Err(err);
        }
        client_result
            .map_err(|err| RuntimeError::unspecified(format!("NATS_TRIAL_TERMINATION: {err}")))
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
    pub(super) async fn pump_joined(&self) -> bool {
        self.reply_pump.lock().await.is_none()
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

// async-nats emits Closed on a graceful drain. On reconnect exhaustion its
// handler exits without that event; a disconnected client whose command sender
// rejects Flush has instead observably lost the handler.
async fn terminate_client(
    client: &async_nats::Client,
    terminal: &AtomicBool,
    closed: &AtomicBool,
) -> Result<(), &'static str> {
    let _ = tokio::time::timeout(IO_LIMIT, client.drain()).await;
    tokio::time::timeout(IO_LIMIT, async {
        loop {
            if closed.load(Ordering::SeqCst) {
                return;
            }
            if matches!(client.flush().await, Err(err) if err.kind() == async_nats::client::FlushErrorKind::SendError)
                && terminal.load(Ordering::SeqCst)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "NATS_TRIAL_HANDLER_TERMINATION_EXPIRED")
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

fn require_supported_step(step: &Step) -> Result<(), RuntimeError> {
    if step.correlation_id.is_some() || step.request_body.is_some() {
        return Err(RuntimeError::unspecified(
            "NATS_TRIAL_UNSUPPORTED_STEP_FORM",
        ));
    }
    Ok(())
}

pub(super) async fn execute(
    engine: &Engine,
    trial: &NatsTrial,
    ctx: &ExecutionContext,
    step: &Step,
    vars: &VarStore,
) -> Result<StepExecution, RuntimeError> {
    ctx.check_cancelled()?;
    require_supported_step(step)?;
    let target = resolve(engine, step, &trial.retrieval_dir)?;
    let route = trial.record(step, &target)?;
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
