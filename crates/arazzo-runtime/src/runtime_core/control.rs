use super::*;

pub(super) fn step_result_error(step_id: &str, result: &StepResult) -> RuntimeError {
    if let Some(err) = &result.err {
        let kind = result
            .err_kind
            .unwrap_or(RuntimeErrorKind::SuccessCriteriaFailed);
        return RuntimeError::new(kind, format!("step {step_id}: {err}"));
    }
    if let Some(resp) = &result.response {
        let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
        if body_preview.len() > 500 {
            let mut end = 500;
            while !body_preview.is_char_boundary(end) {
                end -= 1;
            }
            body_preview.truncate(end);
            body_preview.push_str("...");
        }
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

pub(super) async fn sleep_with_cancel(
    delay: Duration,
    cancel: &CancellationToken,
    is_timeout: &AtomicBool,
) -> Result<(), RuntimeError> {
    if delay.is_zero() {
        return Ok(());
    }

    tokio::select! {
        () = tokio::time::sleep(delay) => Ok(()),
        () = cancel.cancelled() => {
            if is_timeout.load(Ordering::Acquire) {
                Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionTimeout,
                    "execution timeout exceeded",
                ))
            } else {
                Err(RuntimeError::new(
                    RuntimeErrorKind::ExecutionCancelled,
                    "execution cancelled",
                ))
            }
        },
    }
}
