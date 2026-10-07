# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-09-26
**Baseline:** `0f83071` (`fix(runtime): announce and complete each run --step attempt as execute does`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied, no source touched.
Scheduled run, **delta audit**. Since the [2026-09-19](rust-code-smell-audit-2026-09-19.md) baseline
(`38b6088`), eight runtime commits landed (step-attempt single owner, parallel plain retries and
fallbacks, stream-ordered observer callbacks, cancellation-vs-outcome fixes, per-operation
servers). Four fresh-context Opus reviewers covered: (1) step-attempt recording / cancellation,
(2) parallel levels + dependency DAG, (3) server selection (6edabde), (4) re-verification of the
09-19 Critical/High findings plus touched engine Mediums. Crates the delta did not touch
(`arazzo-expr`, `arazzo-mcp`, `arazzo-generate`, `arazzo-validate`, DAP) were not re-swept.
**Build state at audit time:** `cargo clippy --workspace --all-targets --all-features -- -D warnings`
exits 0; `cargo test --workspace` exits 0 (71 result blocks, 1160 passed, 0 failed, 1 ignored).

**Prior audits:** [`2026-08-22`](rust-code-smell-audit-2026-08-22.md) (F#),
[`2026-09-05`](rust-code-smell-audit-2026-09-05.md) (G#),
[`2026-09-12`](rust-code-smell-audit-2026-09-12.md) (H#),
[`2026-09-19`](rust-code-smell-audit-2026-09-19.md) (I#). Findings here are numbered `J#`.

**Confidence.** **REPRODUCED** = demonstrated against a HEAD build or a scratch probe crate
(path deps on the runtime); the input is given. **Code-verified** = confirmed by reading source.
Spec quotes are from [`spec/`](../../spec/README.md). Fixtures lived in the session scratchpad.

---

## Summary

- **Total new findings:** 26 — **0 Critical, 4 High, 8 Medium, 14 Low/Info**
- **Prior findings re-verified:** 17 — **1 closed (I31)**, 16 still live (I1–I11 all live; H10,
  I25, I26, I27, I28 live)

**Highest-risk areas**

- `runtime_core/deps.rs` — the parallel-level DAG is built from a different view of the step than
  the one that executes (J1; carried H10), and `run --step`'s dependency set diverges from
  parallel's again (J6)
- `runtime_core/engine_impl.rs` — the sequential scheduler walks declaration order and ignores
  `dependsOn` (J4); `execute_step_inner` falls through to `Ok` on loop exhaustion (J3, sibling of I25)
- `runtime_core/engine_parallel.rs` + `step_attempt.rs` — the "one owner" attempt protocol is still
  split; parallel announces separately and records attempts cancelled before they ran (J2, J8)
- `runtime_core/builder.rs::read_servers` — server URLs with query/fragment, scheme-less
  `host:port`, or leftover `{var}` pass the new pre-send check (J9–J11)

**Top 5 most severe**

1. **J1** — Workflow-level `parameters` reading `$steps` add no DAG edge; `--parallel` sends the
   request without the value and reports success (High, reproduced)
2. **J4** — Sequential execution ignores step `dependsOn` order, a spec MUST (High, reproduced)
3. **J2** — A parallel level records failed attempts for steps cancelled before they ran (High, reproduced)
4. **J3** — `run --step` returns `Ok` when it hits the iteration limit; `run` returns
   `IterationLimitExceeded` (High, reproduced)
5. **J5** — Routing criteria that read a later step make `--parallel` succeed where sequential fails
   (Medium, reproduced)

**Systemic observation.** Three of the four Highs are sequential/parallel/`run --step` divergences.
The workspace has three schedulers with three independently derived notions of "what this step
depends on" (`execution_refs`/`routing_refs` for levels, `extract_step_refs` for `run --step`,
declaration order for sequential) and a parity test (`step_recording.rs:623`) that covers
recording but not ordering, cancellation, or exhaustion.

---

## Findings

### J1 — Workflow-level `parameters` that read `$steps` are not scanned when building parallel levels
- **Category:** Correctness (dependency DAG) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/deps.rs:249-251`; merge happens later at
  `engine_parallel.rs:63-68`
- **Evidence:**
  ```rust
  for p in &step.parameters {
      scan_value_source_refs(&p.value, &mut scan);
  }
  // engine_parallel.rs
  merge_workflow_params(&workflow.parameters, &mut s);
  ```
- **Repro:** workflow parameter `token: $steps.login.outputs.token`; step `use` consumes it.
  Sequential sends `/use?token=T0K3N`. `--parallel` sends `/login` and `/use` at the same instant,
  `/use` without the token, and the run reports `kind: success`.
- **Why it matters:** silent wrong request with a success outcome. The scheduler scans the step's
  own list, not the merged list that executes.
- **Verify:** `compute_transitive_deps` (`run --step`) uses `extract_step_refs`, which also reads
  only `step.parameters` — likely the same gap.

### J2 — A parallel level announces and records attempts that were cancelled before they ran
- **Category:** Correctness / cancellation parity · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `engine_parallel.rs:52-58` (no cancel check between announcement and spawn) vs
  `engine_impl.rs:292-293`, `:474-475`
- **Evidence:**
  ```rust
  for idx in level.iter().copied() {
      ...
      self.emit_before_step_event(exec_ctx, workflow_id, &step).await;
  }
  // Spawn parallel steps via JoinSet   (no check_cancelled here)
  ```
- **Repro:** the repo's `run_cancelled_during_announcement` setup (observer cancels inside
  `StepStarted`, current-thread runtime) plus `.parallel(true)`. Sequential: `["before:s"]`,
  0 requests. Parallel, one step: `before:s, after:s, trace:s#1:Error`, `StepCompleted` with
  "execution cancelled", 0 requests. Two steps: `t` is still announced after the cancel, and both
  get AfterStep, StepCompleted and an Error trace record.
- **Why it matters:** 0f83071's stated rule is that an attempt cancelled during its announcement is
  "neither run nor recorded". Traces and observers get failed-step records for requests that never
  went out. Existing cancel tests (`http_cancellation.rs:835`, `:865`) only cancel mid-flight.

### J3 — `run --step` reports `Ok` on iteration-limit exhaustion; `run` reports `IterationLimitExceeded`
- **Category:** Correctness (wrong outcome) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `engine_impl.rs:280`, `:393-394` vs `:592-600`
- **Evidence:**
  ```rust
  for _ in 0..max_iterations { ... }
  exec_ctx.check_cancelled()?;
  Ok(vars.step_outputs(step_id))          // execute_step_inner
  ```
- **Repro:** one step `a`, `onSuccess: goto a`. `execute` gives `Err(IterationLimitExceeded)` after
  10 attempts; `execute_step("w","a",..)` gives `Ok({})` after 10 attempts.
- **Why it matters:** a non-terminating workflow exits 0 under `run --step`. Pre-existing (since
  61c9d9b) and a sibling of I25 (same fall-through to `Ok`), but 0f83071 claims the two entry points
  now match.

### J4 — Sequential execution ignores step `dependsOn`; parallel honours it
- **Category:** Spec conformance / mode divergence · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `engine_impl.rs:452-500` walks `step_index` in declaration order and never reads
  `step.depends_on`; `deps.rs:236-240` feeds `dependsOn` into parallel levels
- **Spec** (`spec/arazzo/v1.1.0.html`, Step Object `dependsOn`): "Tools supporting only sequential
  execution MUST execute steps in an order that satisfies both explicit (dependsOn) and implicit
  (output reference) dependencies."
- **Repro:** step `use` declared first with `dependsOn: [login]`. Sequential sends `/use` then
  `/login`; `--parallel` sends `/login` then `/use`. `validate` accepts the document.
- **Why it matters:** the same document runs in different orders by mode, and the conformance audit
  lists step `dependsOn` as conformant (`arazzo-spec-conformance-audit.md:245`, the same line I7
  flagged for `timeout`).

### J5 — Routing criteria that read a later-declared step make parallel and sequential disagree on outcome
- **Category:** Correctness (mode divergence) · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `deps.rs:136-146` adds edges from `routing_refs` (9f9e508)
- **Repro:** steps `[s, later]`; workflow `failureActions` plain retry with criterion
  `$steps.later.outputs.ok == true`; `s` returns 503 once. Sequential:
  `Err(SuccessCriteriaFailed)`. Parallel: `later` runs first, `s` retries, run is `Ok`, no fallback
  reported.
- **Why it matters:** outcome depends on `--parallel`. Request-input refs already reordered steps;
  9f9e508 extends that to routing. Interacts with J4 (sequential has no reorder at all).

### J6 — `run --step` misses routing dependencies that parallel levels track; a unit test pins the gap
- **Category:** Duplicated / diverging logic · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `deps.rs:221-228` (`extract_step_refs`, own `condition` only) vs `routing_refs`
  (`:192-208`, `condition` + `context` + inherited workflow-level lists)
- **Evidence:**
  ```rust
  for action in step.on_success.iter().chain(&step.on_failure) {
      for c in &action.criteria {
          scan_step_refs(&c.condition, &mut refs);
  ```
- **Repro:** retry criterion `context: $steps.probe.outputs.token`, or a workflow-level
  `failureActions` condition reading `probe`. Full run: `/login`, `/use` ×2,
  `RUNTIME_RETRY_LIMIT_EXCEEDED`. `run --step consumer`: `probe` never runs, `/use` once,
  `RUNTIME_SUCCESS_CRITERIA_FAILED`.
- **Why it matters:** the unit test at `deps.rs:500` asserts the gap ("extract_step_refs does not
  read action criteria contexts; routing does"), so it is locked in, not tracked.

### J7 — A retry's `stepId` reference runs a step with no lifecycle records
- **Category:** Correctness / observability · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `engine_impl.rs:782-784` calls `execute_step_with_result` directly
- **Evidence:**
  ```rust
  let execution_result = self
      .execute_step_with_result(exec_ctx, workflow_id, &step, vars, depth)
      .await;
  ```
- **Repro:** `[a, b]`, `b` fails 503 once and retries with `stepId: a`. Observer sees
  `RequestPrepared, RequestSent:a, CriterionEvaluated` for the recovery run of `a`, but no
  StepStarted/StepCompleted, no stream BeforeStep/AfterStep, no trace record, no debug gate —
  while `a`'s outputs in `vars` are overwritten.
- **Why it matters:** contradicts `step_attempt.rs:1-8` ("every scheduler records its attempts in
  this module"). A real request and a `$steps.a` change leave no trace. Pre-existing.

### J8 — The attempt protocol is not in one owner: parallel levels announce separately from `begin_attempt`
- **Category:** Maintainability (invariant by convention) · **Severity:** Medium · **Confidence:** High — Code-verified
- **Where:** `engine_parallel.rs:56`, `:221` call `emit_before_step_event` directly; numbering at
  replay (`step_attempt.rs:184`); sequential announces and numbers together in `begin_attempt`
  (`step_attempt.rs:108-120`)
- **Why it matters:** anything added to `begin_attempt` silently skips parallel levels; J2 is exactly
  that drift. Only the recording parity test holds the modes together.

### J9 — A server URL with a query or fragment is accepted and the path is glued on after it
- **Category:** URL construction / spec input validation · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `builder.rs:364-367` accepts; `engine_http.rs:793` concatenates
- **Evidence:**
  ```rust
  ServersField::Base(url.trim_end_matches('/').to_string())
  // engine_http.rs:793
  format!("{}{}", resolved_base.trim_end_matches('/'), resolved_path)
  ```
- **Spec** (`spec/oas/v3.2.0.html`, Server Object `url`): "Query and fragment MUST NOT be part of
  this URL."
- **Repro:** `https://q.example.com/api?key=abc` + `/q` plans `https://q.example.com/api?key=abc/q`;
  `https://f.example.com/api#x` + `/frag` plans `…/api#x/frag` (the operation path becomes a
  fragment, so the request hits `/api`).
- **Why it matters:** 6edabde's stance is "refuse an unusable declared level, don't guess"; this
  one silently routes to the wrong endpoint. Document-level `derive_servers_base` shares the rule.

### J10 — Scheme-less `host:port` and non-HTTP schemes are not classified as relative, so they still fail only at send
- **Category:** Validation gap vs commit claim · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `document_set.rs:311-316`
- **Evidence:**
  ```rust
  pub(super) fn is_relative_reference(reference: &str) -> bool {
      matches!(Url::parse(reference), Err(ParseError::RelativeUrlWithoutBase))
  }
  ```
- **Repro:** operation `url: "localhost:8080/v1"` parses with scheme `localhost`; dry run plans
  `localhost:8080/v1/noscheme`, live run fails `RUNTIME_HTTP_REQUEST` ("builder error"). `file:///etc`
  behaves the same. Any other `Url::parse` error (e.g. `InvalidPort`, see J11) also passes.
- **Why it matters:** the commit (and conformance F25) claims scheme-less URLs are now refused before
  sending. `every_relative_document_server_fails_the_build` (`openapi_servers.rs:747`) tests only
  `"."`, `"./test"`, `"api.example.test/v1"`.

### J11 — Undeclared variables, or ones with no string `default`, leave `{var}` in the server URL
- **Category:** Server variable substitution · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `builder.rs:350-363`; no post-substitution brace check
- **Evidence:**
  ```rust
  let (Some(name), Some(default)) = (name.as_str(),
      var.get("default").and_then(serde_yaml_ng::Value::as_str)) else { continue; };
  url = url.replace(&format!("{{{name}}}"), default);
  ```
- **Repro:** `https://n.example.com:{port}/v1` with `default: 8443` (YAML int) dry-runs to
  `…:{port}/v1/numport`, live fails "invalid port number". `https://{tenant}.example.com/v1` with no
  `variables` plans `https://{tenant}.example.com/…`. `{scheme}://…` is misreported as "relative",
  blaming the runtime rather than the document.
- **Spec** (Server Variable `default`): "REQUIRED. The default value to use for substitution, which
  SHALL be sent if an alternate value is not supplied."
- **Notes:** the non-string default is recorded debt; the missing-variable and missing-default cases
  are not. The path-parameter-fills-host issue is already F25 debt and not re-counted.

### J12 — A failed sub-workflow step reports and stores outputs; a failed HTTP step does neither
- **Category:** Correctness (per-attempt outputs) · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `engine_impl.rs:875-889` sets outputs before criteria; `:681` reports them
- **Evidence:**
  ```rust
  for (name, value) in outputs {
      vars.set_step_output(&step.step_id, &name, value);
  }
  ...
  if !evaluate_criterion(criterion, &eval_post, ...) {
      return Ok(StepResult { success: false, ...
  ```
- **Why it matters:** HTTP failed attempts leave `vars` untouched (`engine_http.rs:677-693`). The
  `deps.rs:185-190` rationale ("a failed attempt records no outputs") holds only because
  sub-workflow steps never run in parallel.

### J13 — `#[must_use]` on `BegunAttempt` does not enforce what its doc claims
- **Category:** Maintainability · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `step_attempt.rs:67-70`; callers `engine_impl.rs:292-293`, `:474-475`
- **Why it matters:** `must_use` only catches a never-bound value; every caller drops the binding via
  an early `?`, which compiles silently. `finish_attempt` also takes `step`/`workflow_id` separately
  from `begun`, and both callers discard the `SettledAttempt`.

### J14 — `is_cancellation()` relies on an unenforced assumption about who produces `ExecutionTimeout`
- **Category:** Error handling · **Severity:** Low · **Confidence:** Medium — Code-verified
- **Where:** `error.rs:118-124` vs public `From<reqwest::Error>` at `error.rs:178-186` (maps HTTP
  timeout to `ExecutionTimeout`)
- **Why it matters:** holds today because execution paths map reqwest errors explicitly
  (`client.rs:539`, `:683`). A future `?` on a reqwest error would make a per-request timeout look
  like a cancellation, and `engine_impl.rs:200`, `:581` would skip `mark_workflow_completed`.

### J15 — `DryRunRequest` is emitted at a different stream position in parallel mode
- **Category:** Event ordering / duplication · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** sequential `engine_impl.rs:690-701` (before AfterStep); parallel
  `engine_parallel.rs:112-120` (after AfterStep/StepCompleted/trace)
- **Why it matters:** 6f235e4 claims stream-order parity; this breaks it, and the emission is
  duplicated at two sites.

### J16 — Parallel retries hold every attempt's response, trace data and events until the level ends
- **Category:** Resource use · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `engine_parallel.rs:249`, `:289-291` (`retried.push((attempt, retry))`), `:329-344`
  (unbounded `events` Vec)
- **Why it matters:** worst case ≈ retryLimit × `max_response_bytes` (10 MiB default) per step per
  level; `effective_retry_limit` keeps `u64::MAX`. The bounded `mpsc::channel(64)` no longer bounds
  memory. Sequential drops each attempt's response after routing.

### J17 — Parallel observer/stream events are held back until the whole level finishes
- **Category:** Latency / design · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `engine_parallel.rs:307-336` buffers; `:228-235` replays after `join_next` drains
- **Why it matters:** a step retrying with long `retryAfter` shows only StepStarted until its slowest
  sibling finishes; `-v` and MCP progress stall. A documented trade-off for stream order.

### J18 — The parallel panic path drops finished sibling results and emits no `WorkflowCompleted`
- **Category:** Panic handling / event contract · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `engine_parallel.rs:94-103`
- **Why it matters:** every step was already announced; results whose requests were really sent are
  discarded with no AfterStep or trace, yet `execute_inner:444` still marks the workflow completed.
  Reachable only through a task panic.

### J19 — `WorkflowCompleted` construction and the "unsupported flow" error are copy-pasted across schedulers
- **Category:** Duplication · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `WorkflowCompleted` at `engine_parallel.rs:152-161`, `:167-176`, `engine_impl.rs:529-538`,
  `:577-586`, `:604-613`; flow error at `engine_parallel.rs:137-140`, `:275-278`
- **Why it matters:** the paths already disagree on when `mark_workflow_completed` runs relative to
  the event; a4d6c97 and f408d77 each had to touch every copy.

### J20 — Server variable `enum` is never checked
- **Category:** Spec input validation · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `builder.rs:350-363`
- **Spec:** "If the enum is defined, the value MUST exist in the enum's values."
- **Repro:** `{env: {default: evil, enum: [prod, dev]}}` plans `https://evil.example.com/enum`.

### J21 — Server variable substitution depends on declaration order
- **Category:** Substitution correctness · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `builder.rs:361` (repeated `String::replace` in mapping order)
- **Repro:** `{a}` default `"{b}"`, `{b}` default `"x"` plans `https://x.example.com/x/chain`.

### J22 — Document-level malformed `servers` reports "declares no servers[0].url"
- **Category:** Diagnostics · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `builder.rs:299-311` folds `NotAnArray` and `MissingUrl` together; Path Item / Operation
  levels are precise (`engine_http.rs:1104-1111`)

### J23 — Server-URL substitution is duplicated in `generate` with a different "relative" rule
- **Category:** Duplication · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `arazzo-generate/src/crud.rs:129-145` (`starts_with('/')`) vs `builder.rs:350-367`
  (RFC 3986); acknowledged at `builder.rs:328-330`
- **Why it matters:** `generate` can emit a workflow the runtime then refuses at build (e.g.
  `servers: [{url: "."}]`).

### J24 — CLI `-H` headers now reach whichever host an operation's own `servers` names
- **Category:** Security (credential scope) · **Severity:** Low (informational) · **Confidence:** Medium — Code-verified
- **Where:** `engine_http.rs:210-213`, `:299-304` pick the per-operation base; `HttpClient`
  default headers apply to every request
- **Why it matters:** routing is spec-correct, but `-H Authorization` now goes to an operation-level
  third-party host, not only the document host. No host allowlist exists in `arazzo-mcp/src`.

### J25 — OAS 3.2 `query` method and `additionalOperations` are not indexed
- **Category:** Spec coverage · **Severity:** Low · **Confidence:** High — Code-verified (found independently by two reviewers)
- **Where:** `builder.rs:426-428` (`http_methods` list)
- **Why it matters:** their `servers` and `operationId`s are never seen, so an `operationId` target on
  such an operation cannot resolve. Pre-existing; verify whether the conformance audit tracks it.

### J26 — `cli_parallel_fallback.rs` has a dead binary fallback that could run a stale build
- **Category:** Tests · **Severity:** Info · **Confidence:** High — Code-verified
- **Where:** `crates/arazzo-cli/tests/cli_parallel_fallback.rs:106-113`
- **Why it matters:** `CARGO_BIN_EXE_arazzo-cli` is always set for this package's integration tests,
  so `../../target/debug/arazzo-cli` never runs; if it did it could be out of date.

### Test gaps (cross-cutting, new code)
- No cancellation case for parallel announcement (J2); parity test lacks ordering/exhaustion cases (J3–J5).
- No negative server-URL tests for query/fragment, `host:port`, non-http schemes, missing variable,
  missing/non-string default, or enum (J9–J11, J20). AGENTS.md requires a negative case for
  spec-surface tests; the variables test (`openapi_servers.rs:397-437`) is success-only.

---

## Prior findings re-verified at `0f83071`

| ID | Status | Evidence | Tracking |
|---|---|---|---|
| I1 XML depth/entities | Live | `<a>`×200 → exit 134 stack overflow; `xpath.rs:74` unchanged | general [ac-7a6ba](https://sonos.scapedeck.com/docs/ac-tickets/ac-7a6ba) only |
| I2 nested parens | Live | `--strict validate` OK at 3000; `run` exit 134 | [ac-67bf5](https://sonos.scapedeck.com/docs/ac-tickets/ac-67bf5) (paused) |
| I3 nested JSONPath | Live | d20 (97 B) ran 27.3 s under `--execution-timeout 2s` | general [ac-7a6ba](https://sonos.scapedeck.com/docs/ac-tickets/ac-7a6ba) only |
| I4 MCP `dry_run:"true"` | Live | live `DELETE /items/42` sent | none |
| I5 MCP timeout overflow | Live | panic at `client.rs:515:24`; release `panic = "abort"` | [ac-1aa44](https://sonos.scapedeck.com/docs/ac-tickets/ac-1aa44) (gated on [ac-950a8](https://sonos.scapedeck.com/docs/ac-tickets/ac-950a8)) |
| I6 `.env` empty key / NUL | Live | both binaries panic on `=value` | none |
| I7 step `timeout` ignored | Live | `timeout: 200` vs 2 s endpoint → success in 2.39 s | none |
| I8 literal params → strings | Live | child body `{"count":"5",...}`; now `engine_impl.rs:826-827` | none named |
| I9 form payload as JSON | Live | `engine_http.rs:395` | none |
| I10 `{$expr}` drops arrays/objects | Live | header `ids=`; `arazzo-expr/src/lib.rs:656` | [ac-28ba3](https://sonos.scapedeck.com/docs/ac-tickets/ac-28ba3) (blocked) |
| I11 `generate` blow-up | Live | 3.3 KB → 2.9 MB YAML | none |
| H10 `$steps` regex misses digit-leading ids | Live | re-reproduced: `1login` under `--parallel` sends `/use` without token, success; `deps.rs:3-6` | [ac-0686b](https://sonos.scapedeck.com/docs/ac-tickets/ac-0686b) |
| I25 `run --step` success without running target | Live | `engine_impl.rs:313-315` → `:394` fall-through (see J3) | adjacent [ac-a73b6](https://sonos.scapedeck.com/docs/ac-tickets/ac-a73b6) |
| I26 workflow-step `outputs` ignored | Live | `engine_impl.rs:875-876` | [ac-bb1c3](https://sonos.scapedeck.com/docs/ac-tickets/ac-bb1c3) |
| I27 transport root cause discarded | Live | timeout and refused port indistinguishable | [ac-8a804](https://sonos.scapedeck.com/docs/ac-tickets/ac-8a804) |
| I28 replay can't reproduce transport failure | Live | `RUNTIME_REPLAY_TRACE_EXHAUSTED` | general [ac-130aa](https://sonos.scapedeck.com/docs/ac-tickets/ac-130aa) |
| I31 `--parallel` drops failing step's events | **Closed** | `engine_parallel.rs:330-344` returns drained events (6f235e4/882119c) | — |

Eight of the eleven Critical/High findings from 09-19 have no ticket naming them: I1, I3, I4, I6,
I7, I8, I9 and I11 have none, or only the general budget assessment. No open ticket cites the 09-19 audit.

---

## Patterns / meta-smells

- **Three schedulers, three dependency models.** Sequential (declaration order), parallel levels
  (`execution_refs` + `routing_refs`), and `run --step` (`extract_step_refs`) each decide "what runs
  first" differently. J1, J4, J5, J6 and carried H10 are all instances.
- **"One owner" is partial.** `step_attempt.rs` owns recording, but announcement (J8), retry-reference
  execution (J7), dry-run emission (J15) and `WorkflowCompleted` (J19) remain per-scheduler.
- **Fall-through to `Ok` in `execute_step_inner`.** I25 and J3 are the same shape at different exits.
- **Pre-send validation stops at `RelativeUrlWithoutBase`.** J9–J11 all pass because the only check is
  one `Url::parse` error kind.
- **Tests pin gaps.** `deps.rs:500` asserts J6; the conformance audit still lists `dependsOn` and
  `timeout` as conformant (J4, I7).

## Non-findings (checked, clean)

- No `RUNTIME_*` code changed; `EngineEvent::SequentialFallback` is added to a `#[non_exhaustive]` enum.
- Parallel channels/JoinSet: sender outlives the attempt, `rx.close()` + drain collects everything,
  JoinSet drop aborts remaining tasks, no un-awaited handles, no lock held across `.await`.
- Retry counting: 1-based per `RetrySite` across all three paths; `RetryScheduled` ordering matches
  sequential; attempt numbering stable (results sorted by index before replay).
- Late-cancel fixes (a4d6c97, f408d77): every `WorkflowCompleted` emission is preceded by
  `check_cancelled`; GotoWorkflow and parallel defer to the callee's result.
- Observer delivery: `ContextRole::AttemptBuffer` prevents double delivery; sequence numbers come
  only from `exec_ctx`.
- Server precedence: Operation > Path Item > document, implemented once (`operation_server`,
  `builder.rs:381-399`) and used by both resolvers; `servers: []` inherits; `servers: null` refused;
  `//host` treated as relative; no double slashes; no unwrap on the path.
- New tests use sleeps only to perturb completion order, not in assertions; Condvar wait is bounded.
- `build_levels` is O(n²) and clones `VarStore` per step — negligible at realistic sizes.
