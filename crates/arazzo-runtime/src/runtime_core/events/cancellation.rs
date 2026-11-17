use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutcome {
    Running,
    Completed,
    Cancelled,
    Dropped,
    TimedOut,
}

#[derive(Debug)]
struct Arbitration {
    outcome: Mutex<ExecutionOutcome>,
    worker: CancellationToken,
    settled: CancellationToken,
    is_timeout: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub struct ExecutionCancellation {
    inner: Arc<Arbitration>,
}

impl Default for ExecutionCancellation {
    fn default() -> Self {
        Self {
            inner: Arc::new(Arbitration {
                outcome: Mutex::new(ExecutionOutcome::Running),
                worker: CancellationToken::new(),
                settled: CancellationToken::new(),
                is_timeout: Arc::new(AtomicBool::new(false)),
            }),
        }
    }
}

impl ExecutionCancellation {
    pub fn cancel(&self) {
        self.claim(ExecutionOutcome::Cancelled);
    }

    pub fn outcome(&self) -> ExecutionOutcome {
        *self
            .inner
            .outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.worker.is_cancelled()
    }

    pub(crate) fn claim(&self, next: ExecutionOutcome) -> ExecutionOutcome {
        let mut outcome = self
            .inner
            .outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *outcome == ExecutionOutcome::Running {
            *outcome = next;
            if matches!(
                next,
                ExecutionOutcome::Cancelled
                    | ExecutionOutcome::Dropped
                    | ExecutionOutcome::TimedOut
            ) {
                self.inner
                    .is_timeout
                    .store(next == ExecutionOutcome::TimedOut, Ordering::Release);
                self.inner.worker.cancel();
            }
            self.inner.settled.cancel();
        }
        *outcome
    }

    pub(crate) async fn settled(&self) {
        self.inner.settled.cancelled().await;
    }
    pub(crate) async fn watchdog(&self, timer: impl std::future::Future<Output = ()>) {
        tokio::select! {
            biased;
            () = self.settled() => {},
            () = timer => { self.claim(ExecutionOutcome::TimedOut); }
        }
    }
    pub(crate) fn worker(&self) -> CancellationToken {
        self.inner.worker.clone()
    }
    pub(crate) fn timeout_flag(&self) -> Arc<AtomicBool> {
        self.inner.is_timeout.clone()
    }
    pub(crate) fn completion_guard(&self) -> CompletionGuard {
        CompletionGuard(self.clone())
    }
}

pub(crate) struct CompletionGuard(ExecutionCancellation);

pub(crate) fn complete(completion: Option<&ExecutionCancellation>) -> Result<(), RuntimeError> {
    if let Some(completion) = completion {
        match completion.claim(ExecutionOutcome::Completed) {
            ExecutionOutcome::Cancelled
            | ExecutionOutcome::Dropped
            | ExecutionOutcome::TimedOut => {
                return Err(super::super::control::cancellation_error(
                    &completion.inner.is_timeout,
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

impl CompletionGuard {
    pub(crate) fn cancellation(&self) -> &ExecutionCancellation {
        &self.0
    }

    pub(crate) fn finish<T>(&self, result: Result<T, RuntimeError>) -> Result<T, RuntimeError> {
        match self.0.claim(ExecutionOutcome::Completed) {
            ExecutionOutcome::Cancelled
            | ExecutionOutcome::Dropped
            | ExecutionOutcome::TimedOut => Err(super::super::control::cancellation_error(
                &self.0.inner.is_timeout,
            )),
            _ => result,
        }
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.0.claim(ExecutionOutcome::Completed);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod watchdog_tests {
    use super::*;

    #[test]
    fn only_terminal_publishers_freeze_the_invocation() {
        let cancellation = ExecutionCancellation::default();
        complete(None).unwrap();
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Running);
        complete(Some(&cancellation)).unwrap();
        cancellation.clone().cancel();
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Completed);
        let cancellation = ExecutionCancellation::default();
        cancellation.cancel();
        assert_eq!(
            complete(Some(&cancellation)).unwrap_err().code(),
            "RUNTIME_EXECUTION_CANCELLED"
        );
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Cancelled);
    }

    fn handle(cancellation: ExecutionCancellation) -> ExecutionHandle {
        let (_, events) = mpsc::channel(1);
        let (_, result) = oneshot::channel();
        ExecutionHandle::new(events, result, cancellation)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ready_timer_cannot_override_cancel_clone_drop_or_completion() {
        for winner in [
            ExecutionOutcome::Cancelled,
            ExecutionOutcome::Dropped,
            ExecutionOutcome::Completed,
        ] {
            for success in [false, true] {
                let cancellation = ExecutionCancellation::default();
                let owned = handle(cancellation.clone());
                let guard = cancellation.completion_guard();
                let (entered_tx, entered_rx) = std::sync::mpsc::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let timer = std::future::poll_fn(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    std::task::Poll::Ready(())
                });
                let watcher = cancellation.clone();
                let watchdog = tokio::spawn(async move { watcher.watchdog(timer).await });
                entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let result: Result<(), RuntimeError> = if success {
                    Ok(())
                } else {
                    Err(RuntimeError::new(
                        RuntimeErrorKind::InternalError,
                        "original error",
                    ))
                };
                let result = match winner {
                    ExecutionOutcome::Cancelled => {
                        owned.cancel_token().clone().cancel();
                        guard.finish(result)
                    }
                    ExecutionOutcome::Dropped => {
                        drop(owned);
                        guard.finish(result)
                    }
                    _ => guard.finish(result),
                };
                assert_eq!(cancellation.outcome(), winner);
                release_tx.send(()).unwrap();
                watchdog.await.unwrap();
                assert_eq!(cancellation.outcome(), winner);
                assert!(!cancellation.inner.is_timeout.load(Ordering::Acquire));
                if winner == ExecutionOutcome::Completed {
                    assert_eq!(result.is_ok(), success);
                    if let Err(error) = result {
                        assert!(error.to_string().contains("original error"));
                    }
                } else {
                    assert_eq!(result.unwrap_err().code(), "RUNTIME_EXECUTION_CANCELLED");
                }
            }
        }
    }

    #[tokio::test]
    async fn timer_winner_classifies_worker_before_signal_and_freezes_result() {
        let cancellation = ExecutionCancellation::default();
        let worker = cancellation.worker();
        let flag = cancellation.timeout_flag();
        let observed = tokio::spawn(async move {
            worker.cancelled().await;
            super::super::super::control::cancellation_error(&flag).code()
        });
        cancellation.watchdog(std::future::ready(())).await;
        cancellation.clone().cancel();
        drop(handle(cancellation.clone()));
        let guard = cancellation.completion_guard();
        let result = guard.finish(Ok::<_, RuntimeError>(()));
        assert_eq!(result.unwrap_err().code(), "RUNTIME_EXECUTION_TIMEOUT");
        assert_eq!(observed.await.unwrap(), "RUNTIME_EXECUTION_TIMEOUT");
        assert_eq!(cancellation.outcome(), ExecutionOutcome::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn completion_before_publication_stops_owned_watchdog_for_success_and_error() {
        for success in [false, true] {
            let cancellation = ExecutionCancellation::default();
            let (events_tx, events_rx) = mpsc::channel(1);
            let (result_tx, result_rx) = oneshot::channel();
            let mut owned = ExecutionHandle::new(events_rx, result_rx, cancellation.clone());
            owned.start_watchdog(Duration::from_secs(5));
            let result = if success {
                Ok(WorkflowOutputs::default())
            } else {
                Err(RuntimeError::new(
                    RuntimeErrorKind::InternalError,
                    "original error",
                ))
            };
            let guard = cancellation.completion_guard();
            let result = guard.finish(result);
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(10)).await;
            assert!(owned.watchdog.as_ref().unwrap().is_finished());
            assert_eq!(cancellation.outcome(), ExecutionOutcome::Completed);
            cancellation.clone().cancel();
            assert!(!cancellation.is_cancelled());
            drop(events_tx);
            result_tx.send(result).unwrap();
            assert_eq!(owned.collect().await.outputs.is_ok(), success);
            assert_eq!(cancellation.outcome(), ExecutionOutcome::Completed);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unwind_finishes_and_drop_aborts_watchdog() {
        let cancellation = ExecutionCancellation::default();
        let guard = cancellation.completion_guard();
        let task = tokio::spawn(async move {
            let _guard = guard;
            panic!("test unwind")
        });
        assert!(task.await.unwrap_err().is_panic());
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Completed);
        let cancellation = ExecutionCancellation::default();
        let mut owned = handle(cancellation.clone());
        owned.start_watchdog(Duration::from_secs(5));
        let watchdog = owned.watchdog.as_ref().unwrap().abort_handle();
        drop(cancellation.clone());
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Running);
        drop(owned);
        tokio::task::yield_now().await;
        assert!(watchdog.is_finished());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(cancellation.outcome(), ExecutionOutcome::Dropped);
        assert!(!cancellation.inner.is_timeout.load(Ordering::Acquire));
    }
}
