# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-10-24
**Baseline:** `41d85a4` (`docs: assess AsyncAPI send payload authoring contract`). The working tree
differs in `CHANGELOG.md`, `Cargo.toml` (0.9.0 bump, `[profile.dev] debug = false`), `Cargo.lock`,
`README.md` and `SECURITY.md`. No uncommitted change touches `crates/`.
**Method:** `rust-code-smell` skill, run by a scheduled task. Inventory only: no fixes were applied
and no source was touched.

This is a **delta sweep** over `42fe188..41d85a4` (14 commits, about 6k lines in `crates/`). The
2026-10-17 full sweep is a week old. Three fresh-context Opus reviewers, each at max effort, read one
slice each:
- runtime AsyncAPI/NATS/controlled-exchange plus the engine changes;
- the debugger delta, with an L1/L2 re-check;
- the CLI/MCP/expr delta, with an L3–L8 re-check.

The reviewers were read-only and ran no cargo, so **nothing in this report was reproduced**. For
every High, the lead re-read the cited code at HEAD and checked it with
`tkt query --include-closed --grep`.

**Prior audit:** [`2026-10-17`](rust-code-smell-audit-2026-10-17.md) (L#). Findings here are
numbered `M#`. Reviewer-local IDs are given in brackets.

---

## Summary

- **New Highs: 2, both untracked.** M1 is a debugger regression class. M2 is a spec field that is
  silently ignored.
- **Carried Highs L1–L8: all 8 still present at HEAD, and all are now tracked.** None of the owning
  tickets is in progress.
- **New Mediums:** 9. **New Lows:** about 25, listed briefly.
- **Highest-risk areas:**
  - the debugger gate path: `engine_trace.rs::debug_gate_checkpoint`, `controller.rs::gate_step`,
    `step_attempt.rs::settle`;
  - `Step.timeout`, which is never read;
  - spec discovery, which has two owners (`handlers.rs` and `arazzo-mcp/src/state.rs`).

## High findings

| # | Finding | Tracked? |
|---|---|---|
| M1 | A breakpoint-condition error at a criterion or output checkpoint becomes a step failure, so onFailure retries or gotos | **No** |
| M2 | `Step.timeout` is parsed but never enforced; the conformance audit lists it as conformant | **No** |
| L1 | DAP disconnect deadlocks on a second breakpoint in the same step | Yes: [ac-5eb26](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5eb26) (ready) |
| L2 | stdin EOF while paused leaves the adapter running forever | Yes: [ac-5eb26](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5eb26) (ready) |
| L3 | form-urlencoded mapping payload sent as JSON | Yes: [ac-9394b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9394b) (ready, decision) |
| L4 | Unresolved path/query values silently change the request (`/pets/{id}` → `/pets/`) | Yes: [ac-704a6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-704a6) (ready, decision) |
| L5 | Redaction vocabulary misses `api_key`, `X-Goog-Api-Key`, `private_key`, … | Partly: [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51) (ready) |
| L6 | MCP/CLI parse-error text echoes file contents | Yes: [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8) (ready); wider surface, see M10 |
| L7 | generate: quadratic `minLength` padding, exponential fan-out | Yes: [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f) (ready) |
| L8 | Templated mapping sub-workflow parameter reaches the child as a JSON string | Yes: [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4) (blocked by ac-cd36b) |

The ticket that owned L3 and L4 changed after 10-17: both were untracked then and now have decision
tickets. Neither ac-5eb26 nor ac-9534f covers M1: ac-5eb26 says "Preserve genuine controller
errors", and those errors still travel through `settle`.

### M1 — Breakpoint-condition error inside a step body is routed as a step failure  [D1]

- **Category:** error handling (a swallowed error). **Severity:** High. **Confidence:** High.
  Code-verified by the lead; not run.
- **Where:**
  - `crates/arazzo-runtime/src/debug/breakpoints.rs:167-174`
  - `debug/controller.rs:414-420` (`gate_step`, `first_matching_breakpoint(...)?`)
  - `runtime_core/engine_trace.rs:323-331`
  - `runtime_core/engine_http.rs:583-584` (also the output gate at :625 and dry-run at :488)
  - `runtime_core/step_attempt.rs:136, :304-311`
- **Evidence:**
  ```rust
  // engine_http.rs:583 — inside execute_step_with_result
  self.debug_gate_success_criterion(&gate, index, &evaluation)
      .await?;
  // step_attempt.rs:136 / :305 — finish_attempt → settle
  let execution = settle(run);
  run.unwrap_or_else(|err| StepExecution { result: StepResult { success: false, .. } .. })
  ```
- **Why it matters:**
  - The step-line gate in `engine_impl.rs:490` uses `?` outside the attempt, so a bad condition
    there aborts the run, as tested.
  - Criterion and output gates run inside the attempt, so their `RuntimeErrorKind::DebugController`
    error becomes an ordinary failed attempt. `route_attempt` does not special-case that kind.
  - Scenario: a POST step with `onFailure: [{type: retry, retryLimit: 3}]` and a conditional
    breakpoint `$response.body#/status == 200` on its successCriteria line, against a body
    `{"status":"created"}`.
    - The numeric parse fails, so `InvalidEvaluation` → `gate_step` Err → failed attempt → retry.
    - Result: the POST is sent 4 times instead of 1, and the run ends with `RETRY_LIMIT_EXCEEDED`.
      The condition error never reaches the DAP client.
    - With a goto failure action, the workflow silently takes the failure branch.
  - No test covers conditional breakpoints at criterion or output checkpoints.
- **Verify:** a hermetic tiny_http DAP transcript of the scenario. Count the POSTs.

### M2 — `Step.timeout` is never enforced  [lead, from a runtime reviewer note]

- **Category:** spec conformance / correctness. **Severity:** High. **Confidence:** High.
  Code-verified by the lead; not run.
- **Where:**
  - `crates/arazzo-spec/src/lib.rs:349` (`pub timeout: Option<u64>`)
  - Readers are only display or describe code: `arazzo-cli/src/output.rs:843, :859` and
    `arazzo-mcp/src/handlers.rs:87`.
  - `git grep` finds no reader of `step.timeout` anywhere in `crates/arazzo-runtime/src`, and no
    validator or runtime rule that refuses it.
  - `plans/assessments/arazzo-spec-conformance-audit.md:248-252` lists Step `timeout` as "Verified
    present and conformant".
- **Spec:** `spec/arazzo/v1.1.0.html`, Step Object, `timeout`: *"The maximum number of
  milli-seconds to wait for the step to complete before aborting and failing the step. Consequently
  this will fail the workflow unless onFailure actions are defined."*
- **Why it matters:**
  - A step with `timeout: 500` against a server that takes 5 s succeeds after 5 s. Under the spec it
    fails at 500 ms and routes through onFailure.
  - Only the CLI-wide `--timeout-seconds` and the execution watchdog apply, and they cancel the
    whole run rather than failing the one step.
  - The conformance audit's claim is false, so the deviation is not tracked as a debt.
  - ac-ee786 and ac-fe844 explicitly kept AsyncAPI receive deadlines separate from `Step.timeout`.
    ac-eb31c describes the field as only "modeled and applicability-checked".
- **Verify:** a tiny_http server that sleeps 2 s, a step with `timeout: 100`, `arazzo run --json`.
  Expect `success` instead of a step failure.

## Medium findings

| # | Finding | Where | Notes |
|---|---|---|---|
| M3 [D2, R2] | Debug-gate `EvalContext` has no url, method or request data, so outputs like `$request.header.*`, `$url` and `$method` raise false `unresolvedOutput` diagnostics on every DAP run. `evaluate $request` at an output checkpoint regressed. In dryRun every `$response.body#/…` output is flagged. | `engine_trace.rs:298` vs `engine_http.rs:359-372`; `controller.rs:396-406`; `dap/variables.rs:288-296` | New in aa58c2d. Both reviewers found it independently. |
| M4 [R3] | Debug-gate locals, including two full body copies, are built for every criterion, output and action gate **without a debugger attached**. The `debug_controller` check comes after construction. | `engine_trace.rs:89-126, :294-296, :660-662` | Hot path. Pre-existing, and copied into dry-run. Adjacent to ac-9534f. |
| M5 [D4, R18] | With the debugger attached, each output gate makes 3 `EvalContext` clones plus a `DebugScopes` clone (was 1). `stackTrace` and `variables` clone the whole frame under the controller mutex. | `controller.rs:396-411, :104-127`; `breakpoints.rs:167-168` | Compounds L39. |
| M6 [D3] | `VariableStore` grows without bound within one stop: every hover, `scopes` or `variables` call deep-copies the body. | `dap/variables.rs:18-37, :224-226` | Trade-off from the L35 fix. |
| M7 [C1] | Spec discovery has two production owners, and they disagree. `catalog`/`show` read one level and accept any `*.json`. `test`/`serve`/`arazzo-mcp` recurse and accept only `*.arazzo.json`. `orders.json` is listed by catalog but missing from serve, and vice versa for nested files. | `arazzo-cli/src/handlers.rs:250-416`; `arazzo-mcp/src/state.rs:130-178` | Was H26 (2026-09-12). Widened by c124f79/73c2c40. README.md:98 at HEAD says "across a directory tree". |
| M8 [C2] | The recursive walker has no pruning (it walks `target/`, which has 112k entries, `node_modules`, `.git` and nested worktrees). One unreadable subdirectory aborts with a doubled, unnamed prefix (`scanning .: scanning .: Permission denied`). | `arazzo-mcp/src/state.rs:159-178` | A nested worktree makes `test .` run every workflow twice. |
| M9 [C3] | Valid JSON with surrogate-pair escapes (`🚀`, Python's `ensure_ascii` default) is rejected, because every parse goes through libyaml. | `arazzo-validate/src/lib.rs:246`; `arazzo-spec/src/lib.rs:960`; `arazzo-generate/src/lib.rs:15` | Spec §5.2: documents "may be represented either in JSON or YAML". Not reproduced. |
| M10 [R4] | Latent: `bind_local_source` reads `type: asyncapi` files at step time, outside `relative_openapi_source_paths`. That is the list MCP vets against `--allowed-dir`. The read is also unbounded and runs on the async worker. | `document_set.rs:217-249, :305-330`; `builder.rs:257-269` | Code at HEAD has no production caller. Matters once AsyncAPI is wired in. |
| M11 [R1] | NATS trial pump: while it waits on `send` into the full 1-slot channel, it cannot see `pump_stop`. Two unconsumed replies turn `close()` into a 2 s stall followed by `NATS_TRIAL_PUMP_JOIN_EXPIRED`. | `nats_transport_trial.rs:197-220, :348-365` | `cfg(test)` only. Contradicts 411317f's "cleanup on all exits". |

## Low findings (brief)

- **Debugger:**
  - [D5] `stackTrace` is all-or-nothing, so one YAML-alias step without an index fails the whole
    request.
  - [D6] `unwrap_or(Value::Null)` on output-expression serialization feeds a hard gate error
    (`engine_trace.rs:108`).
  - [D7] New DAP tests depend on the 100 ms `INLINE_EVENT_TIMEOUT` window. `controller.rs:683`
    joins with no bound.
  - [D8] The `inspect_frame` snapshot is a full controller with no-op mutators. `evaluate_watches`
    fails the whole batch on the first unresolved watch. `output_diagnostics` is unbounded for
    consumers that never drain it.
- **CLI/MCP:**
  - [C4] `serve x.arazzo.yaml --dir .` loads the same file twice, then refuses each of its
    workflows as ambiguous (no dedup, unlike `test_runner.rs:65`).
  - [C5] `arazzo-mcp --dir <no specs>` starts silently with zero workflows, while CLI `serve`
    errors.
  - [C6] `test` now reads any explicitly named file and echoes its scalar contents into
    TAP/JUnit, so a `.pem` lands in CI artifacts. `cli_json_errors.rs:225` pins the echo.
  - [C7] The golden drift guard skips `.arazzo.json` (`golden_spec_baseline.rs:111`).
  - [C8] Help and tool text still says YAML only, and the `--allowed-dir` help understates its
    scope.
  - [C9] Four consecutive unpushed commits (56b3234 … 6f04e27) fail `cargo test`, which breaks
    bisect. be99969 repairs them.
- **Runtime (mostly `cfg(test)` fixtures):**
  - [R5] Prepared NATS receive is not tied to its `workflow_id`.
  - [R6] Case-variant NATS headers collapse in HashMap order.
  - [R7] Send-side subject checks are weaker than async-nats subscribe; `{var}` addresses are
    published literally.
  - [R8] The two fixture executors duplicate each other and turn criterion warnings into
    RuntimeErrors, so failure-shaped messages never reach onFailure.
  - [R9] Send success depends on post-flush connection state, against §5.8.5.3 "completes
    immediately after the message is sent".
- **Runtime, other:**
  - [R10] A channelPath step without `action` is classed `InvalidTarget`, but the spec does not
    mark `action` REQUIRED. The validator rule (validate lib.rs:1381) is not in the conformance
    audit.
  - [R11] Pointer-decode block copy-pasted between `asyncapi_target.rs:130-148` and
    `operation_target.rs:146-161`. Three source binders.
  - [R12] About 416 lines of AsyncAPI seam ship under a module-wide `allow(dead_code)`.
  - [R13] `engine_impl.rs` (god-file watch list) grew 1,094 → 1,146 lines with `cfg(test)`
    dispatch and copy-pasted mode guards.
  - [R14] A panicking `Drop` on fixture dirs can double-panic and abort the lib test binary.
  - [R15] Live NATS fixture: port race, weak readiness probe, blocking sleeps in async tests,
    unbounded publish/flush.
  - [R16] Weak assertions: `buffered()` is monotonic so `:1103` proves nothing; `assert_ne!`
    accepts any wrong target.
  - [R17] `async-nats` adds 16 lockfile packages, including `syn` 3.x, to every
    `arazzo-runtime --lib` test build, for tests that are `#[ignore]`.

## Patterns / meta-smells

- **Debugger gates diverge from the engine.** M1, M3, M4 and M5 all come from the gate building its
  own context and locals and sitting inside the attempt boundary. Also, attaching the debugger can
  change a run's outcome: M1, and the deliberate step-line abort.
- **One-owner violations keep recurring:** spec discovery (M7, a repeat of H26), pointer decoding
  and source binding (R11), and the fixture executors (R8).
- **Conformance audit drift:** M2 (`timeout`) and R10 (`action`) are deviations that the audit
  calls conformant or does not list.
- **Unreproduced Highs:** every reviewer flagged that the no-cargo brief leaves Highs as traced
  rather than reproduced. That is now true for two consecutive runs (L1 and M1).

## Non-findings

- **No new panic paths:** no new `unwrap`, `expect` or indexing panic on production paths.
  `inspect_frame` slicing is in bounds.
- **Debugger locking:** a single mutex with no nested locks. Condvar loops re-check their
  predicates. No lost wakeups apart from L1.
- **NATS trial IO:** every IO step has a timeout, and receive is cancellation-safe.
  - Live tests are `#[ignore]` and gated on a pinned `NATS_SERVER_BIN`.
  - Fixture `finish` runs after a panic.
- **AsyncAPI resolver against AsyncAPI 3.0.0:** the version check, the Channel field allowlist and
  RFC 6901 unescaping are correct.
- **`$message` handling:** `$message.header` is case-insensitive, and `$message` is kept out of
  goto/retry target interpolation.
- **`--json` discipline** holds on the changed CLI paths. Removing `TEST_PATH_NOT_ARAZZO` breaks no
  released contract.
- **Recent fixes:** L35, L37 and L38 and several DAP Lows from 10-17 are fixed at HEAD.
