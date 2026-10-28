# Message exchange engine spike — 2026-10-18

## Decision context

The existing sequential engine can execute a controlled send/receive exchange
with a small test-only dispatch hook. Existing payload resolution, message
expressions, criteria evaluation, output publication, failure routing, and retry
machinery are usable. The concrete remaining context boundary is between step
evaluation and action evaluation: the latter reconstructs its context from an
HTTP response, so the experiment's received message is lost there.

This is experimental evidence, not an accepted implementation design or an
AsyncAPI support claim. No broker, transport framework, new crate, dependency,
or public API was introduced. Production sources in the working checkout were
not changed. The artifact is uncommitted.

## Experiment

`experiment.patch` applies to a disposable snapshot of the current working
source, identified by `baseline.json`. All hooks and the new module are behind
`cfg(test)`. Four existing files gain only module registration, optional test
state, initialization, and a sequential dispatch branch. The new
`runtime_core/message_spike.rs` owns the controlled exchange and its tests.

Tests invoke the real `Engine::execute` / `execute_collect` entrypoints. The
normal scheduler calls the injected exchange, publishes its outputs, settles
attempts, and runs actions. There is no replacement scheduler. The adapter
calls existing `resolve_payload_detailed`, `evaluate_criterion_detailed`, and
`evaluate_output_value_detailed`; it provides a small experimental evaluation
loop rather than extracting the HTTP loop into a proposed production API.

The fixture resolves input `order = 7` into a sent JSON payload, places a reply
in a one-slot inbox before send returns, receives it, checks
`$message.payload#/accepted`, extracts numeric `id = 42` and a message header,
and sends the extracted ID onward. Workflow outputs preserve both values.

## Observed results

| Test | Observation |
|---|---|
| Immediate reply and typed chaining | Reply survives until the receive step; criteria pass; message header and numeric payload output reach workflow outputs; later send receives numeric 42. |
| Failed message criterion | Step fails with the existing criteria-failure error; later send does not execute. |
| Missing message | Receive ends with the explicit experimental timeout marker; its active waiter and receive mutex are released. |
| Timeout with one failure retry | Existing routing makes exactly two receive attempts, emits two completion events carrying the timeout marker, and ends with retry exhaustion. No republish occurs. |
| Cancellation | Test waits until receive has registered, then cancels through the real execution handle. Execution reports cancellation; active waiter exits and the mutex is available. |
| Send failure | Existing failure handling stops execution before receive starts. |
| Paired action conditions | `$message.payload#/id == 42` does not select the end action; `$steps.receive.outputs.id == 42` does. The former proceeds to the later send, while the latter ends the workflow. |

The last test deliberately characterizes an unresolved integration gap. It is
not acceptance of message loss or a regression in shipped message support.

## Limits of the evidence

- Direct typed/serde Engine fixtures bypass CLI validation and AsyncAPI target
  resolution. The source URL is a fixture label; no AsyncAPI document is loaded.
- Reply retention belongs to the in-memory inbox, not to dependency ordering.
  A real transport still needs its own evidence for replies arriving before a
  receive step starts.
- The experimental timeout covers receive waiting only. The specification
  describes `timeout` as the maximum time “to wait for the step to complete
  before aborting and failing the step.” Whole-step deadline coverage remains
  unresolved. See [the vendored Step definition](../../../spec/arazzo/v1.1.0.html#stepTimeout).
- `SPIKE_STEP_TIMEOUT` uses `RuntimeErrorKind::Unspecified` solely to distinguish
  the experimental step failure from invocation cancellation. It proposes no
  public error code. The retry test shows why that distinction matters.
- Cleanup proof covers the active receive future and mutex, not broker
  subscriptions or connection shutdown. Test state is attached to the Engine
  for one invocation; reuse or concurrent use of an Engine is not proved.
- Only JSON payloads, simple message criteria, string headers, and direct
  expression outputs were exercised. Diagnostics deliberately fail the spike
  rather than establishing a production warning policy.
- No correlation interpretation was added. Arazzo makes correlation filtering
  conditional on `correlationId` being specified; this isolated exchange omits
  it. See [receive completion rules](../../../spec/arazzo/v1.1.0.html#defining-success-for-asynchronous-steps).
- Parallel execution has a separate HTTP call site and was not integrated.
  Dry-run, tracing, capture, and debugger modes are rejected by the spike path.
  The experiment emits normal attempt/completion events but does not implement
  message-aware criterion observer events or presentation contracts.

## Verification and review

- `cargo check --offline -p arazzo-runtime`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --offline -p arazzo-runtime --all-targets --all-features -- -D warnings`: passed.
- `cargo test --offline -p arazzo-runtime`: 572 passed, 0 failed, 1 ignored doc test.
- Saved patch reapplied to a fresh snapshot: all 7 focused tests passed,
  including the final cancellation mutex assertion.
- `cargo test --offline --release -p arazzo-runtime --lib message_spike`:
  all 7 focused tests passed, including timeout, cancellation, and the final
  cancellation mutex assertion.

The first broad run was stopped by the sandbox denying an existing test's
loopback HTTP listener. The permitted rerun passed. No remote services were
used. The full suite preceded only the final cancellation mutex assertion;
fresh-snapshot focused verification covers that final test-only addition.

A separate read-only reviewer checked the hooks, tests, and evidence limits;
no required corrections were found. Suggested timeout and mutex assertions
were incorporated. The reviewer did not run tests; execution evidence is in
the logs retained here.

## Smallest next decision

Decide whether to proceed with a bounded change that carries received message
context through step completion into action evaluation. The spike supports
that specific boundary as useful work; it does not justify selecting a broker
or introducing a transport framework. Target resolution, actual message I/O,
whole-step deadlines, and mode support remain separate unproved work.

## Reproduce

From the repository root:

```sh
python3 plans/assessments/2026-10-18-message-exchange-spike/reproduce.py --test
```

The script checks baseline fingerprints, copies sources to a fresh temporary
directory, checks/applies the patch there, and runs only the focused offline
tests. It never patches the working checkout. Omit `--test` to prepare the
snapshot without running it. An existing `CARGO_TARGET_DIR` can be used to reuse
build artifacts. If the source fingerprints change, review/rebase the experiment
instead of forcing the old patch onto new code.
