use super::*;

pub(super) fn cancellation_error(is_timeout: &AtomicBool) -> RuntimeError {
    if is_timeout.load(Ordering::Acquire) {
        RuntimeError::new(
            RuntimeErrorKind::ExecutionTimeout,
            "execution timeout exceeded",
        )
    } else {
        RuntimeError::new(RuntimeErrorKind::ExecutionCancelled, "execution cancelled")
    }
}

pub(super) fn step_result_error(step_id: &str, result: &StepResult) -> RuntimeError {
    if let Some(err) = &result.err {
        let kind = result
            .err_kind
            .unwrap_or(RuntimeErrorKind::SuccessCriteriaFailed);
        return RuntimeError::new(kind, format!("step {step_id}: {err}"));
    }
    if let Some(resp) = &result.response {
        let body_preview = match resp.body_view() {
            BodyView::Absent => String::new(),
            BodyView::Text(text) => {
                let mut preview = redacted_body_text(&text);
                truncate_with_ellipsis(&mut preview, 500);
                preview
            }
            BodyView::Bytes { descriptor, .. } => {
                descriptor.map_or_else(|| "<bytes>".to_owned(), |d| d.summary())
            }
        };
        return RuntimeError::new(
            RuntimeErrorKind::SuccessCriteriaFailed,
            format!(
                "step {step_id}: success criteria not met (status={}, body={})",
                resp.status_code, body_preview
            ),
        );
    }
    RuntimeError::new(
        RuntimeErrorKind::SuccessCriteriaFailed,
        format!("step {step_id}: success criteria not met"),
    )
}

fn redacted_body_text(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(mut value) => {
            redact_json_value(&mut value);
            value.to_string()
        }
        Err(_) => redact_text_patterns(text),
    }
}

pub(super) async fn sleep_with_cancel(
    delay: Duration,
    cancel: &CancellationToken,
    is_timeout: &AtomicBool,
) -> Result<(), RuntimeError> {
    if cancel.is_cancelled() {
        return Err(cancellation_error(is_timeout));
    }
    if delay.is_zero() {
        return Ok(());
    }

    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(cancellation_error(is_timeout)),
        () = tokio::time::sleep(delay) => Ok(()),
    }
}
