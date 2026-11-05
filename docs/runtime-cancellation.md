# Runtime cancellation API migration

`ExecutionHandle::cancel_token()` now returns `&ExecutionCancellation`.
Clone that handle to request cancellation from another task or thread:

```rust
let handle = engine.execute("workflow", inputs);
let cancellation = handle.cancel_token().clone();
cancellation.cancel();
let result = handle.collect().await;
```

Replace explicitly typed `CancellationToken` fields, channels and collections
that receive execution cancellation with `arazzo_runtime::ExecutionCancellation`.
The handle deliberately offers no raw worker token, dereference conversion,
child token or externally writable timeout flag. `outcome()` returns the
read-only `ExecutionOutcome`: Running, Completed, Cancelled, Dropped or TimedOut.
`is_cancelled()` observes the internal cancellation signal.

Completion, explicit cancellation (including any clone), execution-handle drop
and deadline expiry arbitrate under one mutex. The first terminal transition
wins permanently. Timeout classification is published before worker cancellation
is signalled. A losing timer cannot reclassify cancellation or completion.
Clones share authority; dropping a cancellation clone does not cancel execution.
Dropping the execution handle requests Dropped and aborts its owned watchdog.

Completion means terminal result production before its WorkflowCompleted
notification or result publication. Sequential and parallel terminal publishers
claim the same arbiter before reporting that result; tail workflow transfers
retain this authority, while ordinary nested workflows do not complete their
parent invocation. The outer RAII finalizer covers early returns and unwind. Both successful and error results claim Completed. A
prior cancellation, drop or timeout replaces the produced result with the
corresponding existing runtime cancellation error. A completion winner preserves
the original result even if cancellation occurs before collection. Task unwind
also settles completion; a missing result retains the existing internal error.
Cooperative cancellation does not promise preemption or cleanup of detached
transport work.

For single-step execution, use `handle.start_watchdog(duration)` instead of
spawning a timer that writes a timeout flag and cancels a raw token. The runtime
owns and stops this watchdog on settlement or handle drop. Starting another
watchdog replaces the previous timer; it cannot reset a terminal outcome.
Ordinary runtime HTTP timeout remains 30 seconds. An unrepresentable HTTP
deadline returns the existing HTTP request error instead of panicking.
