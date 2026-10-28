# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-10-17
**Baseline:** `42fe188` (`docs: record standard operation target completion`). Working tree differs only in
`Cargo.toml` (`[profile.dev] debug = false`), which does not matter here.
**Method:** `rust-code-smell` skill. Inventory only: no fixes were applied and no source was touched.
Scheduled **full sweep**. Five fresh-context Opus reviewers each read one slice:
- runtime core;
- validate + spec;
- expr + generate;
- CLI + MCP;
- debugger (runtime `debug/` + DAP adapter).

"Max" was given as each reviewer's effort setting. Reviewers were read-only and ran no cargo
commands, so this run reproduced nothing new. The lead re-read the cited code for every High
finding, quoted the spec where the finding touches spec surface, and checked each High against
`tkt query --include-closed --grep`.

**Prior audits:** [`2026-08-22`](rust-code-smell-audit-2026-08-22.md) (F#) …
[`2026-10-03`](rust-code-smell-audit-2026-10-03.md) (K#). The 2026-10-10 run went straight to tkt
(epic ac-0eed7). Findings here are numbered `L#`.

**Confidence.**
- **Code-verified (lead):** the lead re-read the code at HEAD.
- **Traced:** a reviewer followed the code path, including upstream crates, but nothing was run.
- **Prior repro:** an earlier audit reproduced the finding and the code is unchanged since.

---

## Summary

- **Findings:** 107 reviewer findings, merged into the list below once duplicates across slices
  were removed. **0 Critical, 8 High** after dedup. Severities were re-rated across slices: the debugger
  deadlock was rated Critical by its reviewer and is held at High here, because VS Code most
  likely kills the adapter after disconnect anyway.
- **Highs by tracking status:**
  - **Untracked:** 4 (L1–L4).
  - **Tracked:** 4 (L5–L8), but every owning ticket is `ready` or `blocked`, not in progress.
- **Repeat offenders:** L3 (form body) was I9 on 2026-09-19. L4 (empty path segment) was H24 on
  2026-09-12. Both are still unticketed.
- **Highest-risk areas:**
  - `runtime_core/engine_http.rs` (`prepare_http_request`, `build_url_from_path`)
  - `runtime_core/redaction.rs`
  - DAP `session.rs::cleanup_runtime` together with `controller.rs::force_resume`
  - MCP `handlers.rs` / `protocol.rs`
  - `arazzo-generate/src/examples.rs`

## High findings

| # | Finding | Tracked? |
|---|---|---|
| L1 | DAP disconnect deadlocks if the engine reaches another breakpoint in the same step | **No** |
| L2 | stdin EOF while paused leaves the DAP adapter running forever | **No** |
| L3 | `application/x-www-form-urlencoded` mapping payload is sent as JSON | **No** (was I9) |
| L4 | Unresolved path/query/template values silently produce a different request, which is then sent | **No** (was H24) |
| L5 | Redaction vocabulary misses common credential names | Partly: [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51) |
| L6 | MCP file tools return the contents of any readable file inside parse-error text | Yes: [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8) → ac-452ce / ac-79d9f / ac-62191 |
| L7 | `generate` / MCP `generate_example`: quadratic `minLength` padding and exponential `$ref` fan-out | Yes: [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f) |
| L8 | Templated mapping/sequence sub-workflow parameters reach the child as a JSON string | Yes: [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4) (blocked) |

### L1 — DAP disconnect deadlocks when the engine stops again after `force_resume`
- **Category:** concurrency (shutdown deadlock).
- **Confidence:** code-verified (lead).
- **Where:**
  - `crates/arazzo-debug-adapter/src/dap/session.rs:391-409` (`cleanup_runtime`)
  - `crates/arazzo-runtime/src/debug/controller.rs:379-392` (`force_resume`), `:346-351` (gate wait)
  - `runtime_core/engine_http.rs:568-569, :609` (criterion and output gates)
- **Evidence:**
  ```rust
  if let Some(token) = &runtime.cancel_token { token.cancel(); }
  if runtime.controller.force_resume().is_err() { … }
  if let Some(monitor) = runtime.monitor_handle.take() {
      if monitor.join().is_err() { … }
  ```
  `force_resume` grants a single `continue_permit`. `gate_step` never checks the cancel token, and
  neither does `evaluate_step_response`, between its criterion and output gates.
- **Failure scenario:**
  1. Set breakpoints on a `successCriteria` line and an `outputs` line of the same step.
  2. Stop at the criterion, then send disconnect.
  3. The engine resumes and stops again at the output gate, waiting on the condvar.
  4. The monitor sees the cancelled token and joins the engine thread, and the main thread joins
     the monitor. Nothing can set `continue_permit` again, so `run_dap_stdio` never returns.
- **Scope:** action and retry gates have cancel checks on both sides; only the gates inside
  `evaluate_step_response` lack them.
- **Severity:** reviewer rated Critical; held at High. VS Code most likely SIGTERMs the adapter
  after the disconnect response (not verified offline), which would hide the hang. Library callers
  and transcript tests would hang. No test covers this request sequence.

### L2 — stdin EOF while paused orphans the adapter and never cancels the engine
- **Category:** lifecycle.
- **Confidence:** code-verified (lead).
- **Where:** `crates/arazzo-debug-adapter/src/dap.rs:75-99`. `cleanup_runtime` runs only from
  `handle_disconnect`.
- **Evidence:**
  ```rust
  if stdin_closed {
      let engine_done = state.runtime.as_ref().is_none_or(|runtime| runtime.terminated);
      if engine_done { break; }
  }
  thread::sleep(Duration::from_millis(5));
  continue;
  ```
- **Failure scenario:**
  1. The client crashes, or the extension host is killed, while stopped at a breakpoint.
  2. `terminated` never becomes true and nothing resumes or cancels the engine.
  3. The process spins on a 5 ms sleep forever, holding the tokio runtime and the blocked gate
     thread.
- **Related:** `ReadError` returns without cleanup (`dap.rs:101-103`).

### L3 — Form-urlencoded mapping payload is sent as JSON (re-found I9)
- **Category:** correctness / spec surface.
- **Confidence:** code-verified (lead). I9 reproduced it on 2026-09-19; the code is unchanged.
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_http.rs:286-297`.
- **Evidence:**
  ```rust
  if !ct.contains("json") {
      if let Value::String(s) = value { return s.as_bytes().to_vec(); }
  }
  serde_json::to_vec(value).unwrap_or_default()
  ```
- **Spec:** `spec/arazzo/v1.1.0.html` §5.8.14.2 gives *"A Form Data example"* with
  `contentType: application/x-www-form-urlencoded` and a mapping `payload` (`client_id`,
  `client_secret`, `code`, …).
- **Failure scenario:**
  - That exact example goes out as `{"client_id":…}` labelled as a form, so OAuth token endpoints
    return 400.
  - The content-type test is case-sensitive: `Application/JSON` sends a string payload raw.
  - The only test (`tests/engine_workflow.rs:137-167`) asserts the header, never the body.
  - `tkt` has no ticket for "urlencoded", "form data" or "I9", and the conformance audit has no
    entry either.

### L4 — Unresolved values silently change the request that is sent (re-found H24)
- **Category:** silent fallback.
- **Confidence:** code-verified (lead).
- **Where:**
  - `engine_http.rs:672-676` (path) and `:680` (a null query value is dropped)
  - `payload.rs:5-12` (`Value::Null => String::new()`)
  - `arazzo-expr` `resolution.rs:26-32` (`Missing` is erased to `Null`; `$response.*` misses
    produce no warning)
- **Failure scenario:**
  - `DELETE /pets/{petId}` run without `petId` sends `DELETE /pets/`, the collection URL.
  - An unresolved query filter disappears, so the request becomes unfiltered.
  - `--strict-inputs` and `--expr-diagnostics` are off by default, and the `error` mode is
    checked after requests have gone out (`arazzo-cli/src/run.rs:355-369`).
  - The expr reviewer traced a generate-produced chain: a wrong id guess becomes a null output,
    then `/items/`.
- **Tracking:** ac-fda34 covers only composed dry runs; no ticket owns the live-request path.

### L5 — Redaction vocabulary misses common credential names
- **Category:** security (secrets in persisted artifacts).
- **Confidence:** code-verified (lead).
- **Where:** `runtime_core/redaction.rs:12-34`. It surfaces in CLI `trace.rs:188-199`, `run.rs:116-126`,
  MCP `handlers.rs:302` and `run_export.rs`.
- **Evidence:**
  `SENSITIVE_EXACT = ["proxy-authorization","set-cookie","x-api-key","api-key"]`, plus `contains`
  checks against 10 stems.
- **What ac-b4a51 covers:** its separator normalization would catch `api_key`, `X-Goog-Api-Key` and
  `DD-API-KEY`.
- **What stays uncovered:** ac-b4a51 says *"Do not add new stems"*, so these stay in clear:
  - `?key=` (Google)
  - `private_key`
  - `Ocp-Apim-Subscription-Key`
  - `X-Auth-Key`
- **Related, Medium:** reqwest transport errors embed the raw URL, and `trace.rs` never redacts
  `error` fields (`client.rs:539-547`). That is owned by
  [ac-8a804](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8a804) and
  [ac-d95a9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d95a9).

### L6 — MCP file tools give read access to file contents through error text
- **Category:** security.
- **Confidence:** code-verified (lead) for the mechanism. The reviewer traced the serde message
  format in `serde_core` and `serde_yaml_ng`.
- **Where:**
  - `crates/arazzo-mcp/src/state.rs:62-65` (with no `--allowed-dir`, every path is allowed)
  - `arazzo-generate/src/lib.rs:15-16` (`format!("parsing OpenAPI spec \"{path}\": {err}")`)
  - `arazzo-validate/src/lib.rs:247`
- **Failure scenario:**
  1. `describe_openapi {"file_path":"~/.ssh/id_ed25519"}` is called.
  2. The PEM parses as one YAML scalar.
  3. The error reads `invalid type: string "-----BEGIN …"`, which returns the whole key to the
     agent.
  - `/dev/zero` or a FIFO hangs the server.
  - The `--allowed-dir` help text says only "validate_spec".
- **Tracking:** the policy decision ac-950a8 is `ready`; its implementations are blocked behind it.

### L7 — `generate` example walker: quadratic `minLength`, exponential `$ref` fan-out (I11, H11)
- **Category:** DoS.
- **Confidence:** prior repro (`minLength` 1,000,000 took 21–24 s; a 1 KB spec reached 2.1 GB). The
  code is unchanged.
- **Where:** `arazzo-generate/src/examples.rs:325-329` and `:17-19`; `standalone_example.rs:72-75`;
  MCP `handlers.rs:492-504`.
- **Failure scenario:** a single MCP call such as `{"type":"string","minLength":100000000}` stalls
  the serial MCP dispatcher for hours.
- **Tracking:** ac-2763f (`ready`) names both blowups in its acceptance criteria.
- **Note:** the 1,000-unit limit in project memory is decided, not shipped.

### L8 — Templated mapping sub-workflow parameters reach the child as a string (I8 residue)
- **Category:** correctness.
- **Confidence:** code-verified (lead).
- **Where:** `arazzo-spec/src/lib.rs:550-557` (`value_as_str` JSON-stringifies mappings) and
  `runtime_core/engine_impl.rs:826-853`.
- **Failure scenario:**
  - `value: {id: "{$inputs.id}"}` arrives as the string `"{\"id\":\"42\"}"`.
  - Under MCP's `strict_inputs(true)`, a child input declared `type: object` fails the run.
  - Warnings are discarded, and the legacy regex `interpolate_string` deletes bare `$USD`.
- **Tracking:** ac-681d4 (`blocked` on ac-d3fbd) routes every sub-workflow ValueSource through the
  central resolver, which would close this.

---

## Medium findings (condensed)

| # | Finding | Where |
|---|---|---|
| L9 | Stdout `print!`/`println!` panic on EPIPE; release `panic = "abort"` turns that into SIGABRT with no JSON error | `arazzo-cli/src/output.rs` (49 sites), `handlers.rs:194`, `Cargo.toml:42` |
| L10 | `Instant + Duration` overflow: `--http-timeout`/MCP `http_timeout_seconds` near `u64::MAX` panics | `runtime client.rs:515`; `cli.rs:379-381`; MCP `handlers.rs:241-245` |
| L11 | Malformed `-H` is only a warning that prints the raw value (token to stderr); `Name=value` mis-splits on `:` | `run.rs:134-146`, duplicated `handlers.rs:603-616` |
| L12 | `--trace` and `generate -o` can overwrite their own input files; only `--export-run` checks aliasing | `run.rs:34`, `trace.rs:173`, `handlers.rs:186` |
| L13 | The trace cuts the replay `body` (not just the preview) to 2 KiB, so replay diverges silently | `trace.rs:232-242` |
| L14 | Replay compares against redacted data, so bearer-auth workflows can't replay | `replay.rs:65-83` (ac-72ba1 owns it) |
| L15 | MCP framing errors swallow requests and exit 0; malformed JSON with an `id` gets no response | MCP `protocol.rs:189-228` (ac-fc556) |
| L16 | MCP `run_workflow` coerces wrong-typed args to defaults; `"dry_run": "true"` runs live | MCP `handlers.rs:225-251` |
| L17 | MCP dispatch is serial and blocking, and ignores `notifications/cancelled` | MCP `protocol.rs:219-249` |
| L18 | Dependency regex rejects leading-digit step ids the evaluator accepts, so `--parallel`/`--step` misorder | `runtime deps.rs:3-6` |
| L19 | Step `timeout` is never read; step `dependsOn` is ignored by sequential execution (audit doc says "correctly implemented") | `engine_impl.rs:460-471`; `conformance-audit.md:252` |
| L20 | `run --step` reports success after the iteration budget is exhausted; the full run errors | `engine_impl.rs:282-396` vs `:594-602` |
| L21 | Global iteration budget conflates `retryLimit` with goto-cycle protection | `engine_impl.rs:1008-1030` |
| L22 | Server `Retry-After` honored without an upper bound | `engine_actions.rs:832-837` |
| L23 | Cross-host redirects keep `X-API-Key` and resend 307/308 bodies (ac-f5018 decides) | `client.rs:626-636` |
| L24 | Parallel retries keep every attempt (body + trace) until the level ends | `engine_parallel.rs:289-291` |
| L25 | Response body deep-copied ~6× per attempt; trace copies the full body even when off (ac-9534f) | `criteria.rs:68-70`, `state.rs:322-325`, `engine_trace.rs:547-555` |
| L26 | Reusable-object wrapper fields silently dropped; component action warnings repeated N+1× | `validate lib.rs:2818, 2901, 2260` |
| L27 | Chained `components.inputs` `$ref` is left unresolved; the runtime then skips all input checks | `validate lib.rs:2554-2573`; `input_validation.rs:117` |
| L28 | `parse()` drops warnings, so global `--strict` affects only `validate` (typo `sucessCriteria` → 500 counts as success) | `validate lib.rs:218-226`; `cli.rs:24-27` |
| L29 | Wide YAML alias fan-out stays under serde_yaml_ng's jump budget (~27M nodes from 5 KB); two copies are held | `validate lib.rs:246-247` (traced) |
| L30 | `$`-prefixed value that fails to parse is sent as literal text with no diagnostic (`$input.token`) | `expr expression_string.rs:75-77, 109-110` |
| L31 | Legacy regex `interpolate_string` is a second template engine in production paths | `expr lib.rs:81-84, 187-215`; `engine_actions.rs:332-400` |
| L32 | JSONPath nesting budget assumes 2×/level; `match(value(@[?…]))` costs 4×/level | `expr jsonpath.rs:37-41` (traced) |
| L33 | Every JSONPath query builds located paths; `select()` clones overlapping matches (I15) | `expr jsonpath.rs:159-186` |
| L34 | I-Regexp compiled per node, per evaluation; the pattern can come from the response (I16) | `jsonpath/iregexp_adapter.rs:109-124` |
| L35 | DAP `scopes` resets variable refs mid-stop, so earlier `evaluate` refs point at different data | `dap/variables.rs:19-29, 130` |
| L36 | DAP default launch runs the workflow whose id sorts first among *all* files' breakpoints | `dap/session.rs:415-426` |
| L37 | DAP has one shared `source_index` slot; setBreakpoints on file B breaks A's frames | `dap/handlers.rs:128-134` |
| L38 | Explicit `workflowId` limits breakpoint resolution, so sub-workflow breakpoints move into the caller | `dap/source_index.rs:208-221` |
| L39 | Every gate deep-copies `EvalContext` under the controller mutex; Locals ships the full raw body | `controller.rs:298-300`; `engine_trace.rs:662-663` |
| L40 | DAP transcript tests depend on a 100 ms `INLINE_EVENT_TIMEOUT`; a slow stop cascades into the L2 hang | `session.rs:22, 385-387` |
| L41 | God functions: `run_workflow` 368 lines, `run_test_suite` 299, `client.rs::request` ~292, `execute_action` ~254, `build_url_from_path` ~254, `collect_diagnostics` 656, `build_workflow` 194 | various |
| L42 | CLI ↔ MCP duplicated logic has drifted (`StepSummary` fields, `""` vs `"unknown"`, missing `kind`) | MCP `handlers.rs:36-141` vs CLI `output.rs` |

## Low findings (one line each)

- **Runtime:**
  - RegexCache compiles and matches under one global mutex.
  - YAML non-string mapping keys become `""`.
  - Non-ASCII response header values become `""`.
  - The execution-timeout watchdog is never aborted.
  - Missing `WorkflowCompleted` events on several exits.
  - Library `eprintln!` bypasses `--json`.
  - `channel_capacity(0)` and tiny `requests_per_second` panic.
  - Lost `source` chains; dead `RateLimiterLockPoisoned` and panic-recovery codes under `panic = "abort"`.
  - Two dependency extractors disagree.
  - Unbounded `fs::read` of sources.
  - A missing server-variable default stays as literal `{var}`.
  - `body_free_error` string-parses a message format.
  - goto/retry ids are interpolated, so a response can steer control flow.
  - One wall-clock test.
- **Validate/spec:**
  - The workflow cycle check runs once per workflow (quadratic) and blames non-members.
  - Reference-error suppression matches on rendered path strings.
  - Five independent raw walkers render paths differently.
  - `validate()` skips the action-`name` check.
  - Inconsistent "valid but refused" severities.
  - Selector-type union accepts any mapping.
  - Step `dependsOn` and parameter merge are hand-copied.
  - Dead public API: `presented_method_and_target`, `is_value_empty`, `as_runtime_expression`.
- **Expr/generate:**
  - `$components.*` reported as an "unknown namespace".
  - `select_pointer` deep-clones its target.
  - `!=` does not complement `==` on coercion error.
  - Dead `PathError` and `is_truthy`.
  - Integer example `i64` overflow (I68).
  - OpenAPI names become unparsable expressions that are sent literally (K23 → silent).
  - Derived source names abort generation, untested (I74).
  - Silent fallbacks for `$ref`, media type and auth.
  - `readOnly` properties included in request bodies.
  - The two example walkers diverge (I75).
  - Deep schema clones per property.
  - Version guard misses unquoted `openapi: 3.1` (I72).
- **CLI/MCP:**
  - Spec-discovery errors swallowed; discovery rules differ by command.
  - `generate` argv rewrite fires on any literal `generate` token.
  - An unset `$VAR` input is sent literally; `.env` `export KEY=v` is mis-parsed.
  - `-v` redacts only top-level input keys.
  - The trace writer is weaker than the export writer (0644, leaves temp files behind).
  - TAP/JUnit escaping incomplete (newline, `#`, control characters).
  - Dead `TestOutput::Error`, so setup errors under tap/junit print nothing.
  - `--json` edge gaps (`test --format json` usage errors, `serve --json`).
  - `--step` re-implements the timeout wiring.
  - Unused `indexmap`/`url` dependencies; tokio `time` feature only reached transitively.
- **DAP:**
  - `scopes`/`evaluate` ignore `frameId`.
  - `pause` acknowledged but deferred to the next gate.
  - A poisoned controller kills the adapter on `pause`/`setBreakpoints`.
  - Unbounded `Content-Length` allocation.
  - 5 ms / 25 ms polling loops; stop history never cleared.
  - Every engine event retained, then discarded.
  - `wait_for_stop_count(Duration::MAX)` panics.
  - The stdout lock is held for the whole session (latent).
  - evaluate errors render as `"null"`; unreadable sources report "verified" breakpoints.
  - Duplicate `terminated`, a non-1:1 setBreakpoints response, stepOut at depth 0, entry reason `"pause"`.

## Patterns / meta-smells

- **Silent degradation by default.**
  - Null becomes `""` or `"null"`, and parse failures become literals.
  - `Missing` is erased to `Null`, and `parse()` drops warnings.
  - Diagnostics are opt-in and are evaluated after requests are sent (L4, L28, L30).
  - This is the pattern the agent contract's "no fallback" rule exists to prevent.
- **Two of everything:**
  - template engines (L31)
  - example walkers (L7)
  - dependency scanners (L18)
  - scheduler loops (L20)
  - credential lists: `redaction.rs`, the redirect strip list and the cleartext warning (L5, L23)
  - JSON-parse rules (live vs replay)
  - CLI/MCP handlers (L42)
- **Teardown is cooperative and partial.** The debug gate ignores cancellation, teardown is one
  `force_resume` plus unbounded joins, and EOF does no teardown at all (L1, L2).
- **`panic = "abort"`** turns every panic in L9, L10 and the DAP Lows into SIGABRT with no
  `{"kind":"error"}` object, and makes the panic-recovery error codes dead code.
- **Findings re-found but never ticketed.** I9 → L3 and H24 → L4 have each now been reported in
  two scheduled runs with no tkt entry.

## Non-findings (checked, clean)

- No `unsafe` anywhere; `forbid(unsafe_code)` at every crate root plus the workspace lint.
- No `unwrap`/`expect` on untrusted input in production code; slicing is on checked boundaries.
- Lock poisoning is never unwrapped. The debug controller has a single mutex with correct condvar
  loops and no channel operations under the lock.
- No std lock is held across `.await`, and recursion is boxed and capped (`MAX_CALL_DEPTH=10`,
  XML depth 64, condition nesting 32).
- `operationPath` has exactly one classifier, shared by validate and runtime.
- `--json` stdout discipline holds apart from L9 and the L-low edge gaps. MCP and DAP stdout
  carry only protocol frames.
- Response-size cap, redirect downgrade refusal and header CR/LF rejection are in place; event
  channels are bounded.
- Diagnostic order is deterministic, and duplicate YAML keys are rejected.

---

`★ Prompting gap ─────────────────────────────────`
- Observation: the lead's reviewer briefs said "no cargo build/test" and claimed `spec/*.html` was
  missing. The spec is fetched under `spec/arazzo/` and `spec/oas/`; the lead's root-level glob was wrong.
- Cost: two reviewers flagged the false premise. No High was reproduced this run, so L1–L4
  confidence rests on code reading.
- Suggest prompt improvements: the scheduled prompt could say "reproduce untracked Highs with the
  scratch-repro harness (scratchpad crate, own CARGO_TARGET_DIR)".
`─────────────────────────────────────────────────`
