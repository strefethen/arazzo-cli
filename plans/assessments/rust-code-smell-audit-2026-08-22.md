# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-08-22
**Baseline:** `04ee287` (working tree, `main`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied.
**Build state at audit time:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0.

## Summary

- **Total findings:** 22
- **Highest-risk areas:**
  - `crates/arazzo-runtime/src/runtime_core/xpath.rs` + `criteria.rs` (XPath criterion evaluation)
  - `crates/arazzo-expr/src/lib.rs` (`$env`, interpolation diagnostics, `matches` operator)
  - `crates/arazzo-validate/src/lib.rs` (god module)
  - `crates/arazzo-cli/src/output.rs` + `crates/arazzo-mcp/src/handlers.rs` (duplicated `operationPath` parser)
  - `crates/arazzo-runtime/src/debug/controller.rs` (condvar gate)

**Top 5 most severe**

1. F1 — XPath boolean/number success criteria can never fail
2. F2 — XML namespaces stripped with regexes over the whole document, including text content
3. F3 — `$env.*` resolves arbitrary process environment into outbound requests
4. F5 — Two uncovered copies of the `operationPath` parser that disagree with the canonical classifier
5. F6 — Every expression diagnostic inside a `{$…}` interpolated string is discarded

---

## Findings

### F1 — XPath boolean and numeric success criteria always evaluate true

- **Category:** Correctness (7 — API/maintainability consequences; behaves as a silent logic bug)
- **Severity:** Critical
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/xpath.rs:60-74`, consumed at `crates/arazzo-runtime/src/runtime_core/criteria.rs:103-104`, decided by `crates/arazzo-expr/src/lib.rs:1025-1033`
- **Evidence:**

```rust
uppsala::XPathValue::Boolean(b) => XPathSelection {
    value: Value::String(b.to_string()),   // xpath.rs:71
    match_count: 1,
},
// criteria.rs:103
Ok(selection) => is_truthy(&selection.value),
// lib.rs:1030
Value::String(v) => !v.is_empty(),
```

- **Why it matters:** An XPath `successCriterion` whose expression yields a boolean or a
  number is converted to a `Value::String` before truthiness is decided.
  `is_truthy(Value::String("false"))` is `true` because the string is non-empty; likewise
  `Value::String("0")`. So `type: xpath` criteria such as `not(//item)`,
  `//status = 'error'`, or `count(//error) > 0` pass unconditionally, and a step that
  should fail its criteria is reported successful. Node-set criteria are unaffected,
  which is why this has gone unnoticed.
- **Notes / what to verify:** `crates/arazzo-runtime/tests/engine_soap.rs:413`
  (`soap_xpath_criterion_failure`) is the only negative XPath-criterion test and it uses a
  non-matching node-set path, so it exercises the `NodeSet`→`Null` route and never reaches
  the `Boolean`/`Number` arms. There is no negative test for either arm.
- **Remediated 2026-08-23 (working tree):** `XPathSelection` now carries a `truthy`
  field computed from the typed XPath result via XPath 1.0 boolean coercion
  (`uppsala::XPathValue::to_boolean`) before stringification, and the criterion
  decision uses it. This also fixes the adjacent §5.8.11.4.4 arm the original
  finding did not call out: a matched node-set whose text content is empty now
  passes (non-empty node-set) instead of failing through `Null`. Negative tests
  cover every arm at unit, criterion, and engine level
  (`xpath.rs`, `criteria.rs`, `engine_soap.rs::soap_xpath_boolean_criterion_decides_step_outcome`).
  Output/selector/debugger `value` normalization is intentionally unchanged.

### F2 — XML namespaces are stripped with regexes applied to the whole document

- **Category:** Correctness / Security (input parsing)
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/xpath.rs:3-22`
- **Evidence:**

```rust
static XMLNS_RE = Regex::new(r#"xmlns(?::\w+)?="[^"]*""#);
static NS_PREFIX_RE = Regex::new(r"<(/?)[\w-]+:");
let text = XMLNS_RE.replace_all(text, "");
let text = NS_PREFIX_RE.replace_all(&text, "<$1");
let mut doc = uppsala::parse(&text)...
```

- **Why it matters:** Three distinct failure modes.
  (a) The regexes run over the *entire* serialized document, so any occurrence of
  `xmlns="…"` or `<prefix:` inside element text, an attribute value, a comment, or a
  CDATA section is silently rewritten before parsing — the response body a workflow reads
  is not the body the server sent.
  (b) `XMLNS_RE` only matches double-quoted attributes; a server emitting
  `xmlns='urn:x'` keeps its namespaces and every unprefixed XPath in the document then
  fails to match, so behavior depends on the remote server's quoting style.
  (c) Two full-document `replace_all` allocations per XPath evaluation, repeated for each
  criterion and each output expression on the same response.
- **Notes / what to verify:** Whether `uppsala` 0.3 offers namespace-aware evaluation that
  would make the pre-pass unnecessary; the crate is a single external dependency on a
  0.x version sitting on the response-parsing path.

### F3 — `$env.*` resolves arbitrary process environment into outbound requests

- **Category:** Security (secrets handling)
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-expr/src/lib.rs:186-189`
- **Evidence:**

```rust
"env" => {
    let name = remainder.unwrap_or("");
    Value::String(env::var(name).unwrap_or_default())
}
```

- **Why it matters:** `$env` is not in the Arazzo 1.1.0 expression surface. An Arazzo
  document is untrusted input in every serving mode this repo ships (`arazzo run` on a
  downloaded spec, `arazzo serve` / `arazzo-mcp` exposing `run_workflow` to an agent).
  A document can place `$env.AWS_SECRET_ACCESS_KEY` in a parameter, header, or body and
  direct the request at any host, so the expression source is a general credential
  exfiltration primitive. Missing variables resolve to `""` with no warning, so a typo is
  indistinguishable from an empty secret.
- **Notes / what to verify:** Already tracked as F9 in
  `plans/assessments/arazzo-spec-conformance-audit.md` and as R3 in
  `plans/current/code-audit-remediation-plan.md` (§8.3 proposes an allowlist).
  Listed here because it is live in shipped code, not to re-open the decision.

### F4 — `.env` in the working directory silently overwrites real process environment

- **Category:** Security / Resource lifecycle
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/main.rs:30` and `:262-291`; duplicated at `crates/arazzo-mcp/src/main.rs:66`
- **Evidence:**

```rust
load_env_file(".env");          // main.rs:30, before the runtime is built
...
std::env::set_var(key, &value); // main.rs:290 — unconditional
```

- **Why it matters:** `set_var` is called unconditionally, so a `.env` file in whatever
  directory the CLI happens to run from takes precedence over the operator's real
  environment — including `PATH`, `HOME`, proxy variables, and cloud credentials. Combined
  with F3, dropping a `.env` beside a spec is enough to inject values into any workflow
  request. The behavior is undocumented in the crate-level doc comment listing the
  commands. Parse handling also silently discards malformed lines and I/O errors
  (`Err(_) => continue`), so a typo'd `.env` produces no signal.
- **Notes / what to verify:** Whether precedence (`.env` over real env) is deliberate;
  most `.env` loaders default to the opposite.

### F5 — Two uncovered copies of the `operationPath` parser disagree with the canonical classifier

- **Category:** API design & maintainability (7) with correctness consequences
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/output.rs:758-790` and `crates/arazzo-mcp/src/handlers.rs:112-150`,
  versus `crates/arazzo-spec/src/operation_path.rs:99-120`
- **Evidence:**

```rust
// output.rs:784 / handlers.rs:138 (near-verbatim duplicates of each other)
if let Some((first, rest)) = path.split_once(' ') {
    let upper = first.to_uppercase();
    if known_methods.contains(&upper.as_str()) { return (upper, rest.to_string()); }
}
let method = if has_body { "POST" } else { "GET" };
(method.to_string(), path.to_string())
```

- **Why it matters:** `AGENTS.md` states `operation_path.rs` is the one `operationPath`
  classifier. These are a third and fourth copy, and they differ from
  `split_operation_method` in two observable ways:
  (a) they upper-case the candidate, so `get /pets` is displayed as method `GET` and path
  `/pets`, while the runtime — which uses the case-sensitive canonical classifier — treats
  the whole string `get /pets` as the path and builds a different URL;
  (b) when no method prefix is present they invent one from `request_body.is_some()`, so
  `arazzo list`/`steps` and the MCP `describe_workflow`/`list_workflows` responses report a
  method the runtime never derived. The `operation_path_agreement` drift guard
  (`crates/arazzo-cli/tests/operation_path_agreement.rs`) exists precisely to stop this and
  covers only the runtime and validate surfaces — neither copy is inside its scope.
- **Notes / what to verify:** The MCP copy is the one an agent reads to decide what a
  workflow does, so the divergence is user-visible in the highest-trust surface.

### F6 — Expression diagnostics inside `{$…}` interpolated strings are discarded, and bare `$Ident` text is deleted

- **Category:** Error handling (3)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-expr/src/lib.rs:150-158`, `:543-565`, `:78-80`, `:471-473`
- **Evidence:**

```rust
} else if value.contains("{$") {
    (Value::String(self.interpolate_string(value)), Vec::new())   // lib.rs:153-154
}
// interpolate_string → evaluate_string → evaluate → discards the warning vec
static INTERPOLATE_RE = r"\{(\$[^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)";
```

- **Why it matters:** Two consequences.
  (a) The interpolation branch hard-codes `Vec::new()` for warnings, so `--expr-diagnostics`
  reports nothing for an unresolved expression inside an interpolated string — the one place
  a typo is easiest to make. The warning is produced inside `evaluate_with_diagnostics` and
  then thrown away by `evaluate_string`.
  (b) The regex's second alternative matches a *brace-less* `$Ident`. In a string that
  already contains `{$`, an unrelated literal such as `"pay in $USD"` is matched, resolved
  to an unknown namespace, coerced by `to_string_value(Value::Null)` to `""`
  (`lib.rs:1021`), and deleted from the output — silent data loss in a request body or
  header with no diagnostic.
- **Notes / what to verify:** `AGENTS.md` lists `{$expr}` as the interpolation form;
  whether the brace-less alternative is intended at all is a spec-surface question.

### F7 — Parallel-step event channel is bounded at 64 and drained only after the step finishes

- **Category:** Async & runtime hazards (4)
- **Severity:** Medium
- **Confidence:** Medium
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_parallel.rs:254-276`
- **Evidence:**

```rust
let (tx, mut rx) = mpsc::channel(64);
let minimal_ctx = ExecutionContext { event_tx: tx, ... };
let execution = self.execute_http_step(&minimal_ctx, ...).await?;   // sends happen here
let mut events = Vec::new();
rx.close();
while let Some(event) = rx.recv().await { events.push(event); }      // drained only now
```

- **Why it matters:** The receiver is owned by the same task that awaits
  `execute_http_step`, and nothing polls it until that call returns. Every emit inside the
  step is `event_tx.send(..).await` on a bounded channel; once 64 events are buffered the
  65th send parks forever and the step deadlocks until the execution timeout or
  cancellation fires. The magic `64` has no comment tying it to a bound on per-step event
  count.
- **Notes / what to verify:** Count the maximum events one `execute_http_step` can emit
  (before/after-step, step-completed, trace-step, observer, dry-run, per-criterion). If it
  can be driven by document content — many `successCriteria` on one step, for example —
  the bound is reachable from input rather than merely theoretical.

### F8 — The ` matches ` operator recompiles its regex on every evaluation and swallows compile errors

- **Category:** Performance (6) + Error handling (3)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-expr/src/lib.rs:619-626`
- **Evidence:**

```rust
" matches " => {
    let pattern = to_string_value(&rv);
    match Regex::new(&pattern) {
        Ok(re) => re.is_match(&to_string_value(&left)),
        Err(_) => false,
    }
}
```

- **Why it matters:** Two problems in four lines. The `RegexCache`
  (`runtime_core/criteria.rs:3`) only serves `type: regex` criteria; the `simple`-condition
  `matches` operator compiles from scratch on every criterion evaluation, every step, every
  retry attempt. And `Err(_) => false` maps "your pattern does not compile" onto "the
  condition is false", so a malformed pattern is indistinguishable from a legitimate
  non-match and produces no warning, unlike the `type: regex` path which surfaces
  `invalid regex: {err}`.

### F9 — Failure errors embed 500 bytes of unredacted response body

- **Category:** Security (secrets in logs/errors)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/control.rs:10-27`
- **Evidence:**

```rust
let mut body_preview = String::from_utf8_lossy(&resp.body).to_string();
...
format!("step {step_id}: success criteria not met (status={}, body={})",
        resp.status_code, body_preview)
```

- **Why it matters:** The crate has a redaction module (`redaction.rs`) with
  `redact_text_patterns` for exactly this shape of data, and it is not applied here. The
  resulting `RuntimeError` message is printed to stderr, embedded in `--json` output, and
  written into trace files, so an auth-endpoint failure body containing a token or a
  session cookie is persisted verbatim.
- **Notes / what to verify:** Whether the trace-writing path
  (`crates/arazzo-cli/src/trace.rs:128`) redacts the `error` string as well as the
  request/response bodies.

### F10 — Debug controller gate: single global permit, unbounded condvar wait, unbounded stop-event log

- **Category:** Concurrency & thread-safety (5) + Resource management (9)
- **Severity:** Medium
- **Confidence:** Medium
- **Where:** `crates/arazzo-runtime/src/debug/controller.rs:53-66`, `:232-303`; bridged at `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:323-341`
- **Evidence:**

```rust
struct ControllerState { ..., continue_permit: bool, stop_events: Vec<DebugStopEvent>, ... }
...
while !guard.continue_permit {
    guard = self.condvar.wait(guard).map_err(|_| "debug controller lock poisoned".to_string())?;
}
guard.continue_permit = false;
```

- **Why it matters:** Three distinct issues.
  (a) `continue_permit` is one shared boolean, not per-waiter: if two gates ever wait
  concurrently, one `continue_execution()` releases whichever thread wakes first and the
  other keeps waiting — a non-deterministic debug session.
  (b) The wait has no timeout and runs inside `tokio::task::spawn_blocking`
  (`engine_trace.rs:324`). Blocking-pool tasks are not cancellable and runtime shutdown
  joins them, so a DAP client that disconnects without reaching `force_resume` leaves a
  parked thread and can hang runtime teardown.
  (c) `stop_events` is append-only for the life of the controller; a long stepping session
  grows it without bound, and `arm_run_mode_from_stop` only ever reads `.last()`.
- **Notes / what to verify:** Whether the engine can reach a debug gate from more than one
  task at a time (i.e. whether `can_execute_parallel` is guaranteed false under debug).
  If it can, (a) is a live race rather than a latent one.

### F11 — Poisoned locks silently fabricate values instead of surfacing

- **Category:** Error handling (3) / Concurrency (5)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:439-448`, `crates/arazzo-runtime/src/runtime_core/state.rs:38-49`
- **Evidence:**

```rust
// engine_trace.rs:440
match ctx.step_attempts.lock() {
    Ok(mut guard) => { ... next }
    Err(_) => 0,
}
// state.rs:45
pub(super) fn mark_workflow_completed(&self, workflow_id: &str) {
    if let Ok(mut completed) = self.completed_workflows.lock() { completed.insert(...); }
}
```

- **Why it matters:** Three different poison policies coexist in one crate:
  `client.rs:282` recovers with a documented rationale (`lock_recovering`),
  `criteria.rs:19` recovers inline (`unwrap_or_else(|e| e.into_inner())`),
  and these two fabricate a result. `next_attempt` returning `0` writes a wrong `attempt`
  number into `TraceStepRecord`, which is a serialized contract consumed by
  `golden_spec_baseline`. `mark_workflow_completed` dropping the insert means a later
  `dependsOn` check sees the workflow as never completed and the run fails for a reason
  unrelated to the document. Neither emits a diagnostic.

### F12 — God module: `arazzo-validate/src/lib.rs` (10,881 lines) with a 650-line function

- **Category:** API design & maintainability (7)
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-validate/src/lib.rs` — the crate's only source file
- **Evidence:** 10,881 lines; ~3,712 production, ~7,169 inline test; 305 `fn` items;
  `collect_diagnostics` spans `:1154-1804` (650 lines); four functions take 8 parameters
  (`:1054`, `:2766`, `:3056`, `:3385`).
- **Why it matters:** Directly violates the `AGENTS.md` "No God modules. Ever." rule. Every
  validation domain — diagnostics, the parse pipeline, raw-YAML wire checks, component
  resolution, provenance, and each typed rule family — shares one namespace and one
  compilation unit, so any change recompiles and re-reviews all of it, and there is no
  file-level ownership boundary to place a new rule against.
- **Notes / what to verify:** Already scoped in
  `plans/current/arazzo-validate-decomposition.md` (accepted 2026-08-16, implementation not
  started). This finding is the current state, not a new proposal. Logged to `god-files.md`
  per the `AGENTS.md` obligation.

### F13 — Second god file: `arazzo-expr/src/lib.rs` (3,037 lines) with a 488-line dispatcher

- **Category:** API design & maintainability (7)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-expr/src/lib.rs` — the crate's only source file
- **Evidence:** 3,037 lines (~1,740 production); `evaluate_with_diagnostics` spans
  `:172-660`, a single `match namespace { … }` covering every `$…` namespace plus inline
  sub-parsing.
- **Why it matters:** `AGENTS.md` names this match as "the" namespace dispatch, which makes
  it the single point every expression change touches. At 488 lines each arm is a distinct
  grammar with its own edge cases (`$inputs` alone branches on `#`, `.`, and neither), and
  F3, F6, and F8 all live inside this one file with no module boundary separating
  evaluation, coercion, comparison, and interpolation.

### F14 — `run_tests` takes 17 parameters; six `too_many_arguments` suppressions

- **Category:** API design & maintainability (7)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/handlers.rs:786` (17 params);
  `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:452` (10), `:407` (10), `:344` (9);
  `crates/arazzo-runtime/src/runtime_core/engine_http.rs:397` (9);
  `crates/arazzo-generate/src/crud.rs:877` (9)
- **Evidence:** `#[allow(clippy::too_many_arguments)]` appears at six sites in `src/`
  (three of them in `engine_trace.rs` alone).
- **Why it matters:** `run_tests` immediately repacks 12 of its 17 arguments into a
  `TestRunOptions` struct (`handlers.rs:876-889`) — the parameter list is transport for a
  struct that already exists. Long positional lists of same-typed values (`bool`, `usize`,
  `Option<String>`) are the classic setting for a silent argument-order defect that the
  type checker cannot catch, and the lint that would flag it is suppressed rather than
  answered.

### F15 — Test-harness adapters live at the crate root of production source

- **Category:** Testing (10) / maintainability
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-validate/src/lib.rs:5-58`
- **Evidence:**

```rust
// The conformance manifest's hermetic source scanner does not recognize the
// nested tests in this large library module. Keep these minimal executable
// adapters before the library items and delegate all assertions to the direct,
// comprehensive YAML/JSON validation tests below.
#[cfg(test)] #[test]
fn conformance_required_any_values_positive_evidence() { tests::raw_required_values_...(); }
```

- **Why it matters:** Eight no-op wrapper tests sit above the `use` statements of the
  crate's production source purely because a scanner in
  `crates/arazzo-cli/tests/conformance_manifest.rs` cannot see nested test modules. The
  tooling limitation is encoded in the shape of the library rather than fixed in the
  scanner, and the comment admits it. It also produces a second failure mode: renaming a
  nested test silently breaks the manifest's evidence reference rather than the test.

### F16 — `Result<_, String>` is the de facto error type at crate boundaries

- **Category:** Error handling (3) / API design (7)
- **Severity:** Medium
- **Confidence:** High
- **Where:** 104 occurrences across 10 files; densest in
  `crates/arazzo-debug-adapter/src/dap/handlers.rs` (19),
  `crates/arazzo-runtime/src/debug/controller.rs` (17),
  `crates/arazzo-cli/src/output.rs` (13), `crates/arazzo-cli/src/handlers.rs` (13),
  `crates/arazzo-mcp/src/handlers.rs` (9)
- **Evidence:**

```rust
.map_err(|_| "debug controller lock poisoned".to_string())?   // ×20 in controller.rs
```

- **Why it matters:** The workspace defines exactly three real error types
  (`RuntimeError`, `arazzo_validate::Error`, `PathError`); everything else stringifies.
  Callers cannot match on a cause, the original `std::error::Error` source chain is dropped
  at each boundary, and the twenty identical poison strings in `controller.rs` mean the DAP
  layer cannot distinguish "lock poisoned" from any other failure. `RuntimeError` already
  demonstrates the pattern the rest of the workspace declines to follow.

### F17 — `output.rs` is 869 lines with zero unit tests and owns a `--json` contract surface

- **Category:** Testing (10)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/output.rs`
- **Evidence:** `grep -c 'cfg(test)' → 0`. The file shapes `StepInfo`/`StepSummary` (the
  `list`/`steps` JSON payloads) and contains the duplicated parser from F5.
- **Why it matters:** `AGENTS.md` makes `--json` a per-command contract whose changes
  require refreshing `docs/schemas/`. The module that builds those payloads has no
  unit-level coverage at all; it is exercised only indirectly through
  `cli_contract_snapshots`, so a shaping bug that does not move a snapshot line is
  invisible. Its sibling `crates/arazzo-mcp/src/handlers.rs` does have a test module —
  and the shared duplicated parser is only covered on that side.

### F18 — Unbounded `RegexCache`

- **Category:** Resource management (9)
- **Severity:** Low
- **Confidence:** Medium
- **Where:** `crates/arazzo-runtime/src/runtime_core/criteria.rs:3-27`
- **Evidence:**

```rust
pub(crate) struct RegexCache { cache: Mutex<HashMap<String, Regex>> }
...
cache.insert(pattern.to_string(), re);   // never evicted
```

- **Why it matters:** Keys are criterion condition strings and there is no eviction or size
  cap. For a static document the key set is bounded by document size, but the same `Engine`
  is reused across workflow invocations in the MCP server, and any path that reaches
  `is_match` with a pattern derived from response data would grow the map for the process
  lifetime. Compiled `Regex` values are not small.
- **Notes / what to verify:** Whether a `criterion.condition` can ever be interpolated
  before it reaches `is_match`. If conditions are always literal document text, this stays
  Low; if not, it is an unbounded-memory path driven by remote input.

### F19 — Cancel/timeout error mapping is written three times

- **Category:** Maintainability (7)
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/client.rs:292-301`,
  `crates/arazzo-runtime/src/runtime_core/state.rs:27-36`,
  `crates/arazzo-runtime/src/runtime_core/control.rs:45-56`
- **Evidence:** Three independent `if is_timeout.load(Ordering::Acquire) { ExecutionTimeout }
  else { ExecutionCancelled }` blocks with identical message strings.
- **Why it matters:** `RUNTIME_*` codes and their messages are a documented contract
  (`AGENTS.md`: "changing one is a breaking contract change") that appears in `--json` and
  in golden baselines. Three copies means a contract change has three edit sites and any
  one of them can be missed, producing an inconsistent code for the same condition
  depending on which layer noticed the cancellation.

### F20 — `load_env_file` duplicated across two binaries

- **Category:** Maintainability (7)
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/main.rs:262` and `crates/arazzo-mcp/src/main.rs:66`
- **Why it matters:** A ~30-line hand-rolled dotenv parser with non-trivial quote and
  escape handling exists twice with no shared owner. Any fix to the precedence question in
  F4, or to the silent error swallowing, has to land in both or the two binaries diverge on
  what a `.env` file means.

### F21 — MCP allowed-directory check is TOCTOU and probes via the parent directory

- **Category:** Security (path traversal)
- **Severity:** Low
- **Confidence:** Medium
- **Where:** `crates/arazzo-mcp/src/state.rs:62-88`
- **Evidence:**

```rust
let canonical = std::fs::canonicalize(path).or_else(|_| {
    path.parent()...and_then(std::fs::canonicalize)
});
...
for dir in allowed { if canonical.starts_with(dir) { return Ok(()); } }
```

- **Why it matters:** `canonicalize` resolves symlinks at check time; the file is opened
  later. A symlink swapped between the two lets a read escape `--allowed-dir`. The
  component-wise `Path::starts_with` is correct (a sibling `/allowed-evil` will not match
  `/allowed`), so the prefix logic itself is sound — the gap is only the check/use window.
- **Notes / what to verify:** The threat model for `--allowed-dir`. If it is a guardrail
  against agent mistakes rather than a defense against a local attacker, this is
  acceptable and worth stating in the doc comment.

### F22 — Multi-valued `Set-Cookie` headers are collapsed to the last value

- **Category:** Correctness
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/client.rs:630-647`
- **Evidence:**

```rust
// For Set-Cookie, keep only the last value (no lossless
// representation in BTreeMap<String, String>).
if key == "set-cookie" { *existing = value.clone(); }
```

- **Why it matters:** The comment names the cause but not the consequence: a login response
  that sets a session cookie and a CSRF cookie in two `Set-Cookie` headers loses one of
  them, so `$response.header.set-cookie` returns a partial value and any workflow that
  chains authentication through cookie outputs silently carries the wrong state. The chosen
  representation (`BTreeMap<String, String>`) is the constraint, and it is a public type on
  `Response`.

---

## Patterns / meta-smells

- **Silent degradation is the house default.** `Err(_) => false` (F8), `Err(_) => 0` (F11),
  `if let Ok(..)` swallowing a write (F11), `Vec::new()` for warnings (F6),
  `unwrap_or_default()` on a missing env var (F3), `Err(_) => continue` on a malformed
  `.env` line (F4). Each is locally defensible and collectively they mean a
  misconfigured run looks identical to a correct one. This sits awkwardly beside the
  `AGENTS.md` rule against fallback or degraded-mode behavior without approval.
- **Type erasure at boundaries.** Value-shaped information is repeatedly flattened to
  `String` before a decision is made on it: XPath booleans (F1), errors (F16), null
  coercion in interpolation (F6). F1 is what happens when that habit meets a truthiness
  check.
- **Copy-paste across the CLI/MCP surface pair.** `parse_step_target` and
  `parse_operation_path` (F5) and `load_env_file` (F20) each exist twice, once per
  front-end. The two surfaces present the same data to different consumers and have no
  shared presentation layer, so `output.rs` and `arazzo-mcp/src/handlers.rs` drift by
  construction.
- **Hotspot concentration.** `crates/arazzo-expr/src/lib.rs` carries F3, F6, F8, and F13;
  `runtime_core/{xpath,criteria}.rs` carry F1, F2, and F18. Both are single-file modules,
  which is likely not a coincidence — there is no boundary at which a reviewer is forced to
  look at one concern in isolation.
- **Two god files, one already scoped.** F12 has an accepted decomposition plan; F13 does
  not. Nothing currently prevents a third from forming — `runtime_core/engine_impl.rs`
  (1,169 lines) and `payload.rs` (1,277) are the nearest candidates.
- **Inline tests inflate module size past the point of navigability.** 7,169 of
  `arazzo-validate/src/lib.rs`'s lines and 757 of `payload.rs`'s 1,277 are `#[cfg(test)]`.
  The tests are good; their placement is what makes the files unreadable, and it is what
  forced the scanner workaround in F15.

## Non-findings (checked, clean)

- **`unsafe`.** Zero `unsafe` blocks workspace-wide. `#![forbid(unsafe_code)]` on every
  library crate root and on `arazzo-cli/src/main.rs`. The only match for "unsafe" is a doc
  comment in `url.rs`.
- **`unwrap`/`expect` in production paths.** All 22 occurrences are inside `#[cfg(test)]`
  modules; verified by comparing each line number against the file's `#[cfg(test)]`
  boundary. The three `LazyLock` regex initializers use
  `unwrap_or_else(|err| panic!(...))` on compile-time-constant patterns, which is
  appropriate.
- **HTTP response size limits.** `client.rs:649-680` checks `Content-Length` first *and*
  enforces the cap while streaming chunks, so a lying or absent `Content-Length` does not
  bypass it.
- **Redirect policy.** `decide_redirect` (`client.rs:99-112`) is a pure function; hop limit
  pinned to a named constant, https→http downgrade refused by default, and referer
  credentials stripped (`client.rs:216-217`).
- **Timeout composition.** A whole-chain deadline is recomputed per hop
  (`client.rs:507-527`) rather than relying on reqwest's builder timeout, with a comment
  explaining why.
- **Workflow recursion.** `MAX_CALL_DEPTH` is enforced at `engine_impl.rs:434`, and async
  recursion is correctly `Box::pin`-ed.
- **Async lock discipline.** Genuinely contended async state uses `tokio::sync::Mutex`
  (`client.rs:730`, `:807`); `std::sync::Mutex` appears only around non-await critical
  sections. The one place a `Condvar` must meet async is bridged through `spawn_blocking`
  with a comment (`engine_trace.rs:323`) — see F10 for the residual issue, but the bridge
  itself is right.
- **Directory traversal in `collect_arazzo_files`.** `DirEntry::file_type` does not follow
  symlinks, so symlinked directories are skipped and a symlink loop cannot cause infinite
  recursion.
- **Redaction coverage.** `redaction.rs` handles headers, URL query strings, nested JSON,
  and free text, with `redact_url_query` deliberately avoiding URL reconstruction to
  prevent replay drift. F9 is a missing *call site*, not a gap in the module.
- **Build hygiene.** MSRV pinned at the workspace level and inherited by all eight crates;
  `[lints]` tables present; 47 dependency entries use `workspace = true`; `panic = "abort"`
  in release. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  exits 0 at this baseline.
