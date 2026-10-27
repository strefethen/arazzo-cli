//! AsyncAPI receive steps on Kafka: arming at invocation start, waiting for
//! the next message, and turning it into `$message.*` for criteria, outputs,
//! actions and debugger gates. The HTTP counterpart is `engine_http.rs`.
//!
//! Receivers belong to the invocation (`ExecutionContext::message_receivers`),
//! never to the `Engine`: cursors are keyed by connection identity plus topic,
//! shared by every workflow the invocation enters (sub-workflows, `goto`
//! re-entry, retries) and by parallel attempt contexts. Decisions D5–D9 of
//! `plans/assessments/arazzo-first-asyncapi-transport-decision.md`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arazzo_spec::{Step, StepTarget};
use rskafka::client::Client;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::asyncapi_receive::{project_receive_target, ReceiveTarget};
use super::engine_trace::DebugGateContext;
use super::kafka::{
    arm, connect, ArmedTopic, ConnectionIdentity, ReceiveOutcome, ReceivedRecord, SaslMechanism,
};
use super::step_context::{with_message, MessageSnapshot};
use super::{
    evaluate_criterion_detailed, evaluate_output_value_detailed, DryRunRequest, Engine,
    EngineEvent, ExecutionContext, ExpressionEvaluator, ObserverEvent, ReceiverArmed, RuntimeError,
    RuntimeErrorKind, StepExecution, StepResult, StepTraceData, TraceCriterionResult, TraceMessage,
    TransportWarning, TransportWarningKind, VarStore, TRACE_BODY_PREVIEW_MAX_BYTES,
};

type ArmKey = (ConnectionIdentity, String);

/// The invocation's armed topics and the broker connections behind them.
/// The registry locks are held only to look entries up; each armed topic has
/// its own lock, held across a wait, so an unanswered topic never blocks a
/// different one.
#[derive(Default)]
pub struct MessageReceivers {
    clients: Mutex<BTreeMap<ConnectionIdentity, Arc<Client>>>,
    armed: Mutex<BTreeMap<ArmKey, Arc<Mutex<ArmedTopic>>>>,
}

impl std::fmt::Debug for MessageReceivers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageReceivers").finish_non_exhaustive()
    }
}

/// The dispatch predicate: a step with `channelPath`, or any step that
/// declares `action`, is an AsyncAPI step and never an HTTP request.
pub(super) fn is_message_step(step: &Step) -> bool {
    matches!(step.target, Some(StepTarget::ChannelPath(_))) || step.action.is_some()
}

fn client_id() -> String {
    format!("arazzo-cli/{}", env!("CARGO_PKG_VERSION"))
}

impl Engine {
    fn project(&self, step: &Step) -> Result<ReceiveTarget, RuntimeError> {
        let selection = step_source_name(step)
            .and_then(|source| self.inner.message_options.server_selection.get(source))
            .map(String::as_str);
        project_receive_target(
            &self.inner.index.spec,
            step,
            self.inner.source_base_dir.as_deref(),
            selection,
        )
    }

    /// Arms every receive step among `steps` for this invocation: admits all
    /// of them first (so an unsupported later target never lets an earlier
    /// one connect), then connects and snapshots offsets under one
    /// preparation deadline. Dry run admits and stops; replay refuses.
    pub(super) async fn arm_receivers<'a>(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        steps: impl Iterator<Item = &'a Step>,
    ) -> Result<(), RuntimeError> {
        #[cfg(test)]
        if self.inner.controlled_exchange.is_some() || self.inner.nats_trial.is_some() {
            // The test-only exchange hooks own channel steps.
            return Ok(());
        }
        let receive_steps: Vec<&Step> = steps.filter(|step| is_message_step(step)).collect();
        if receive_steps.is_empty() {
            return Ok(());
        }
        if self.inner.replay_mode {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedAsyncApiTransport,
                format!(
                    "workflow \"{workflow_id}\" step \"{}\" is an AsyncAPI receive step; replay \
                     has no recorded message to inject",
                    receive_steps[0].step_id
                ),
            ));
        }
        let mut projected = Vec::with_capacity(receive_steps.len());
        for step in receive_steps {
            projected.push((step.step_id.clone(), self.project(step)?));
        }
        if self.inner.dry_run_mode {
            return Ok(());
        }

        let options = &self.inner.message_options;
        let deadline = Instant::now() + options.connect_timeout;
        for (step_id, target) in projected {
            let key: ArmKey = (target.endpoint.identity.clone(), target.topic.clone());
            if exec_ctx
                .message_receivers
                .armed
                .lock()
                .await
                .contains_key(&key)
            {
                continue;
            }
            let client = {
                let mut clients = exec_ctx.message_receivers.clients.lock().await;
                match clients.get(&target.endpoint.identity) {
                    Some(client) => Arc::clone(client),
                    None => {
                        exec_ctx.check_cancelled()?;
                        // One preparation budget for the whole pass: a later
                        // connection only gets what the earlier ones left.
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return Err(RuntimeError::new(
                                RuntimeErrorKind::MessageTransport,
                                format!(
                                    "preparation deadline ({:?}, --kafka-connect-timeout) was \
                                     exhausted before connecting to Kafka broker {}",
                                    options.connect_timeout, target.endpoint.identity.host
                                ),
                            ));
                        }
                        let budget = super::MessageTransportOptions {
                            connect_timeout: remaining,
                            ..options.clone()
                        };
                        let client =
                            connect(&target.endpoint, &budget, &client_id(), &exec_ctx.cancel)
                                .await
                                .map_err(|err| classify_cancellation(exec_ctx, err))?;
                        if target.endpoint.sasl == Some(SaslMechanism::Plain)
                            && !(target.endpoint.tls_required || options.tls)
                        {
                            self.warn_cleartext_sasl(exec_ctx, &target).await;
                        }
                        clients.insert(target.endpoint.identity.clone(), Arc::clone(&client));
                        client
                    }
                }
            };
            let host = target.endpoint.identity.host.clone();
            let armed = arm(&client, &host, &target.topic, deadline, &exec_ctx.cancel)
                .await
                .map_err(|err| classify_cancellation(exec_ctx, err))?;
            let partitions = u32::try_from(armed.partition_count()).unwrap_or(u32::MAX);
            exec_ctx
                .message_receivers
                .armed
                .lock()
                .await
                .insert(key, Arc::new(Mutex::new(armed)));
            let event = ReceiverArmed {
                workflow_id: workflow_id.to_string(),
                step_id,
                source_name: target.source_name.clone(),
                server_key: target.server_key.clone(),
                host: host.clone(),
                topic: target.topic.clone(),
                partitions,
            };
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::MessageReceiverArmed {
                    workflow_id: event.workflow_id.clone(),
                    step_id: event.step_id.clone(),
                    source_name: event.source_name.clone(),
                    server_key: event.server_key.clone(),
                    host: event.host.clone(),
                    topic: event.topic.clone(),
                    partitions,
                },
            )
            .await;
            let _ = exec_ctx
                .event_tx
                .send(EngineEvent::ReceiverArmed(event))
                .await;
        }
        Ok(())
    }

    async fn warn_cleartext_sasl(&self, exec_ctx: &ExecutionContext, target: &ReceiveTarget) {
        let host = &target.endpoint.identity.host;
        let warning = TransportWarning {
            kind: TransportWarningKind::CleartextCredentials,
            hosts: vec![host.clone()],
            message: format!(
                "SASL PLAIN credentials sent to Kafka broker {host} over a plaintext connection \
                 (server \"{}\" of sourceDescription \"{}\" is `kafka`, not `kafka-secure`, and \
                 --kafka-tls is not set)",
                target.server_key, target.source_name
            ),
        };
        if self.inner.client.transport_warnings_enabled() {
            eprintln!("warning: {}", warning.message);
        }
        let _ = exec_ctx
            .event_tx
            .send(EngineEvent::TransportWarning(warning))
            .await;
    }

    /// Executes one receive step: waits for the next message on its armed
    /// topic (or simulates one in a dry run) and evaluates criteria and
    /// outputs against it.
    pub(super) async fn execute_message_step(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
    ) -> Result<StepExecution, RuntimeError> {
        exec_ctx.check_cancelled()?;
        // The step deadline covers everything below, including waiting for
        // the topic's lock (D8).
        let entered = Instant::now();
        let deadline = step.timeout.map(|ms| entered + Duration::from_millis(ms));
        let target = self.project(step)?;
        let host = target.endpoint.identity.host.clone();

        if self.inner.dry_run_mode {
            let snapshot = MessageSnapshot {
                headers: BTreeMap::new(),
                payload: Some(json!({})),
            };
            let trace_message = TraceMessage {
                transport: "kafka".to_string(),
                host: host.clone(),
                topic: target.topic.clone(),
                partition: 0,
                offset: 0,
                timestamp_ms: 0,
                key: None,
                headers: BTreeMap::new(),
                payload_bytes: 2,
                payload_preview: Some("{}".to_string()),
                payload: Some("{}".to_string()),
                payload_lossy: false,
                simulated: true,
            };
            // Like the HTTP dry run, criteria are not evaluated against a
            // simulated message; outputs are, so dependent steps resolve.
            let mut execution = self
                .evaluate_message(
                    exec_ctx,
                    workflow_id,
                    step,
                    vars,
                    depth,
                    snapshot,
                    trace_message,
                    false,
                )
                .await?;
            execution.dry_run_request = Some(DryRunRequest {
                step_id: step.step_id.clone(),
                method: "RECEIVE".to_string(),
                url: format!("kafka://{host}/{}", target.topic),
                headers: BTreeMap::new(),
                body: None,
                warnings: execution.trace.warnings.clone(),
            });
            return Ok(execution);
        }

        let key: ArmKey = (target.endpoint.identity.clone(), target.topic.clone());
        let armed = exec_ctx
            .message_receivers
            .armed
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    format!(
                        "step \"{}\": topic \"{}\" on {host} was not armed for this invocation",
                        step.step_id, target.topic
                    ),
                )
            })?;

        let mut guard = match deadline {
            Some(at) => {
                let remaining = at.saturating_duration_since(Instant::now());
                tokio::select! {
                    biased;
                    () = exec_ctx.cancel.cancelled() => return Err(exec_ctx.cancelled_error()),
                    locked = tokio::time::timeout(remaining, armed.lock()) => match locked {
                        Ok(guard) => guard,
                        Err(_) => return Err(receive_timeout(step, &target, &host)),
                    },
                }
            }
            None => tokio::select! {
                biased;
                () = exec_ctx.cancel.cancelled() => return Err(exec_ctx.cancelled_error()),
                guard = armed.lock() => guard,
            },
        };
        let record = match guard.receive_next(deadline, &exec_ctx.cancel).await {
            ReceiveOutcome::Received(record) => record,
            ReceiveOutcome::TimedOut => return Err(receive_timeout(step, &target, &host)),
            ReceiveOutcome::Cancelled => return Err(exec_ctx.cancelled_error()),
            ReceiveOutcome::Transport(err) => return Err(err),
        };
        drop(guard);

        let (snapshot, trace_message) = snapshot_from_record(step, &host, record)?;
        self.evaluate_message(
            exec_ctx,
            workflow_id,
            step,
            vars,
            depth,
            snapshot,
            trace_message,
            true,
        )
        .await
    }

    /// Criteria, outputs, observer events and debugger gates for one received
    /// (or simulated) message — the same sequence `evaluate_step_response`
    /// runs for an HTTP response.
    #[allow(clippy::too_many_arguments)]
    async fn evaluate_message(
        &self,
        exec_ctx: &ExecutionContext,
        workflow_id: &str,
        step: &Step,
        vars: &VarStore,
        depth: usize,
        snapshot: MessageSnapshot,
        trace_message: TraceMessage,
        evaluate_criteria: bool,
    ) -> Result<StepExecution, RuntimeError> {
        let eval = ExpressionEvaluator::new(with_message(
            self.make_eval_context(vars, None),
            Some(&snapshot),
        ));
        let mut checkpoint_outputs = BTreeMap::<String, Value>::new();
        let mut criteria = Vec::new();
        let mut warnings = Vec::new();
        let criteria_to_check: &[arazzo_spec::SuccessCriterion] = if evaluate_criteria {
            &step.success_criteria
        } else {
            &[]
        };
        for (index, criterion) in criteria_to_check.iter().enumerate() {
            let evaluation =
                evaluate_criterion_detailed(criterion, &eval, None, &self.inner.regex_cache);
            for warning in &evaluation.warnings {
                warnings.push(format!("successCriteria[{index}]: {warning}"));
            }
            criteria.push(TraceCriterionResult {
                index,
                type_: evaluation.type_name.clone(),
                condition: evaluation.condition.clone(),
                context: evaluation.context_expr.clone(),
                result: evaluation.matched,
                warnings: evaluation.warnings.iter().map(|w| w.to_string()).collect(),
            });
            self.emit_observer_event(
                exec_ctx,
                ObserverEvent::CriterionEvaluated {
                    workflow_id: workflow_id.to_string(),
                    step_id: step.step_id.clone(),
                    index,
                    condition: evaluation.condition.clone(),
                    passed: evaluation.matched,
                },
            )
            .await;
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: None,
                request: None,
                message: Some(&snapshot),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_success_criterion(&gate, index, &evaluation)
                .await?;
            if !evaluation.matched {
                return Ok(StepExecution {
                    result: StepResult {
                        success: false,
                        response: None,
                        message: Some(snapshot),
                        err: None,
                        err_kind: None,
                    },
                    outputs: BTreeMap::new(),
                    dry_run_request: None,
                    trace: StepTraceData {
                        request: None,
                        response: None,
                        criteria,
                        warnings,
                        message: Some(trace_message),
                    },
                });
            }
        }

        let mut outputs = BTreeMap::new();
        for (name, expr) in &step.outputs {
            let (value, expr_warnings) = evaluate_output_value_detailed(expr, &eval, None);
            outputs.insert(name.clone(), value.clone());
            checkpoint_outputs.insert(name.clone(), value);
            for warning in expr_warnings {
                warnings.push(format!("output \"{name}\": {warning}"));
            }
            let gate = DebugGateContext {
                workflow_id,
                step_id: &step.step_id,
                vars,
                response: None,
                request: None,
                message: Some(&snapshot),
                current_outputs: &checkpoint_outputs,
                depth,
            };
            self.debug_gate_output(&gate, name, expr).await?;
        }

        Ok(StepExecution {
            result: StepResult {
                success: true,
                response: None,
                message: Some(snapshot),
                err: None,
                err_kind: None,
            },
            outputs,
            dry_run_request: None,
            trace: StepTraceData {
                request: None,
                response: None,
                criteria,
                warnings,
                message: Some(trace_message),
            },
        })
    }
}

/// The kafka module reports a fired token as `ExecutionCancelled`; the
/// invocation knows whether that was the execution timeout.
fn classify_cancellation(exec_ctx: &ExecutionContext, err: RuntimeError) -> RuntimeError {
    if err.kind == RuntimeErrorKind::ExecutionCancelled {
        exec_ctx.cancelled_error()
    } else {
        err
    }
}

fn step_source_name(step: &Step) -> Option<&str> {
    match &step.target {
        Some(StepTarget::ChannelPath(target)) => match arazzo_spec::classify_operation_path(target)
        {
            arazzo_spec::OperationPathTarget::SourcePointer { source_name, .. } => {
                Some(source_name)
            }
            arazzo_spec::OperationPathTarget::Malformed(_) => None,
        },
        Some(StepTarget::OperationId(target)) => match arazzo_spec::classify_operation_id(target) {
            arazzo_spec::OperationIdTarget::SourceQualified { source_name, .. } => {
                Some(source_name)
            }
            _ => None,
        },
        _ => None,
    }
}

fn receive_timeout(step: &Step, target: &ReceiveTarget, host: &str) -> RuntimeError {
    RuntimeError::new(
        RuntimeErrorKind::MessageReceiveTimeout,
        format!(
            "step \"{}\": no message arrived on topic \"{}\" of Kafka broker {host} within the \
             step timeout of {} ms",
            step.step_id,
            target.topic,
            step.timeout.unwrap_or(0)
        ),
    )
}

/// Turns a record into the `$message.*` snapshot and its trace record: the
/// value must be JSON (or absent → explicit null), header values must be
/// UTF-8. Anything else is `RUNTIME_MESSAGE_MALFORMED`, a failed attempt
/// `onFailure` may route.
fn snapshot_from_record(
    step: &Step,
    host: &str,
    record: ReceivedRecord,
) -> Result<(MessageSnapshot, TraceMessage), RuntimeError> {
    let where_ = format!(
        "step \"{}\": record at offset {} of partition {} of topic \"{}\" on Kafka broker {host}",
        step.step_id, record.offset, record.partition, record.topic
    );
    let payload = match &record.value {
        None => Value::Null,
        Some(bytes) => serde_json::from_slice(bytes).map_err(|err| {
            RuntimeError::new(
                RuntimeErrorKind::MessageMalformed,
                format!("{where_} is not JSON: {err}"),
            )
        })?,
    };
    let mut headers = BTreeMap::new();
    for (name, bytes) in &record.headers {
        let value = String::from_utf8(bytes.clone()).map_err(|_| {
            RuntimeError::new(
                RuntimeErrorKind::MessageMalformed,
                format!("{where_} has a non-UTF-8 header value for \"{name}\""),
            )
        })?;
        headers.insert(name.clone(), value);
    }
    let payload_bytes = record.value.as_ref().map_or(0, Vec::len);
    let (payload_text, payload_lossy) = match &record.value {
        None => (None, false),
        Some(bytes) => (
            Some(String::from_utf8_lossy(bytes).to_string()),
            std::str::from_utf8(bytes).is_err(),
        ),
    };
    let payload_preview = record.value.as_ref().map(|bytes| {
        let max = TRACE_BODY_PREVIEW_MAX_BYTES.min(bytes.len());
        let mut preview = String::from_utf8_lossy(&bytes[..max]).to_string();
        if bytes.len() > max {
            preview.push_str("...");
        }
        preview
    });
    let trace_message = TraceMessage {
        transport: "kafka".to_string(),
        host: host.to_string(),
        topic: record.topic.clone(),
        partition: record.partition,
        offset: record.offset,
        timestamp_ms: record.timestamp_ms,
        key: record
            .key
            .as_ref()
            .map(|k| String::from_utf8_lossy(k).to_string()),
        headers: headers.clone(),
        payload_bytes: u64::try_from(payload_bytes).unwrap_or(u64::MAX),
        payload_preview,
        payload: payload_text,
        payload_lossy,
        simulated: false,
    };
    Ok((
        MessageSnapshot {
            headers,
            payload: Some(payload),
        },
        trace_message,
    ))
}
