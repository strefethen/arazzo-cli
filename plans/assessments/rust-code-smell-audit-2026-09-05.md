# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-09-05
**Baseline:** `f82b2ff` (clean working tree, `main`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied.
**Build state at audit time:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0.
**Prior audit:** [`rust-code-smell-audit-2026-08-22.md`](rust-code-smell-audit-2026-08-22.md) (F1–F22).
Findings below are numbered `G#` to keep the two documents distinct. Where a `G`
finding restates a prior one, the heading says so and the body carries new
evidence rather than repeating the old text.

## Summary

- **Total findings:** 18 (13 new, 5 carried over with new evidence)
- **Highest-risk areas:**
  - `crates/arazzo-runtime/src/runtime_core/url.rs` + `client.rs` (URL assembly and redirect policy)
  - `crates/arazzo-cli/src/main.rs` + `crates/arazzo-mcp/src/main.rs` (`.env` handling)
  - `crates/arazzo-runtime/src/runtime_core/criteria.rs` → `engine_http.rs` (dropped diagnostics)
  - `crates/arazzo-expr/src/lib.rs` (`{$…}` interpolation diagnostics)
  - `crates/arazzo-runtime/src/debug/controller.rs` (uncancellable condvar gate)

**Top 5 most severe**

1. G1 — A path-parameter value of `..` climbs the URL path and retargets the request
2. G2 — 301/302 downgrade PUT/PATCH/DELETE to GET, diverging from the reqwest baseline the code claims to reproduce
3. G3 — `.env` in the working directory overrides the proxy variables reqwest reads
4. G4 — Every criterion `error` diagnostic is computed and then dropped outside the debugger
5. G5 — `{$…}` string interpolation still discards every expression diagnostic

---

## Findings

### G1 — A path-parameter value of `..` climbs the URL path and retargets the request

- **Category:** Security (input validation) / Correctness
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/url.rs:3-21` (`PATH_SEGMENT_ENCODE_SET`),
  `:42-68` (`replace_path_params`); consumed at
  `crates/arazzo-runtime/src/runtime_core/engine_http.rs:823-827`, `:925-927`
- **Evidence:**

```rust
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ').add(b'"').add(b'#').add(b'%').add(b'/')   // url.rs:6-11 — '.' is not in the set
    ...
let encoded = utf8_percent_encode(value, PATH_SEGMENT_ENCODE_SET).to_string(); // url.rs:59
```

- **Why it matters:** The encode set deliberately covers `/` and `%`, so a value cannot
  inject a separator or smuggle one in encoded form. It does not cover `.`, so `..`
  survives verbatim, and RFC 3986 §5.2.4 dot-segment removal then happens inside
  `url::Url::parse` at `client.rs:477`, before the request is sent. Verified against
  `url` 2.x:

  ```
  built  = https://api.example.com/v1/pets/../details
  parsed = https://api.example.com/v1/details
  ```

  Path parameter values are resolved from the ordinary expression context
  (`engine_http.rs:824` → `resolve_value_source`), which includes
  `$steps.<id>.outputs.*` and `$response.body`. So a prior step's response — server
  data, not document data — decides how many segments the next request climbs. A
  path with two placeholders (`/v1/{tenant}/pets/{petId}`) gives two climbs, enough
  to leave the versioned prefix entirely. The step's own `successCriteria` then
  evaluate against whatever the retargeted endpoint returned.
- **Notes / what to verify:** No test covers a dot-segment path-parameter value; the
  encoding tests exercise spaces, slashes and non-ASCII. Whether `.` and `..` should
  be refused, encoded, or normalized before joining is a design decision, not an
  encode-set tweak — a value of `.` collapses differently from `..`.

### G2 — 301/302 downgrade PUT/PATCH/DELETE to GET, diverging from the reqwest baseline the code claims to reproduce

- **Category:** Correctness (HTTP semantics)
- **Severity:** High
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/client.rs:599-616`
- **Evidence:**

```rust
// 30x semantics pinned by the characterization tests:
// 301/302/303 become GET and drop the body (and its headers); ...
if matches!(status, 301..=303) {
    if current_method != reqwest::Method::GET && current_method != reqwest::Method::HEAD {
        current_method = reqwest::Method::GET;   // client.rs:605
    }
    current_body = None;
```

- **Why it matters:** reqwest 0.12 follows redirects through
  `tower_http::follow_redirect`, whose policy (tower-http 0.6, `follow_redirect/mod.rs:273-291`)
  is *not* what this reproduces:

  ```rust
  StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => {
      if *this.method == Method::POST { *this.method = Method::GET; ... }   // POST only
  }
  StatusCode::SEE_OTHER => { if *this.method != Method::HEAD { *this.method = Method::GET; } ... }
  ```

  Under the inherited baseline a `PUT`/`PATCH`/`DELETE` that receives a 301 or 302 is
  re-sent as the same method with its body. Here it is silently re-sent as `GET` with
  the body and `Content-Type` dropped. The write never happens, the step's
  `successCriteria` are evaluated against the `GET` response, and the workflow reports
  success. 303 is the only status for which the blanket conversion is correct.
- **Notes / what to verify:** `crates/arazzo-runtime/tests/transport_characterization.rs:97`
  (`charac_301_302_303_convert_post_to_get_and_drop_body`) drives only `POST`, and
  `:137` covers 307/308, so no test exercises a non-POST method against 301/302 — the
  comment's claim that the behavior is "pinned by the characterization tests" holds
  for POST only.

### G3 — `.env` in the working directory overrides the proxy variables reqwest reads (prior F4, new evidence)

- **Category:** Security (transport / secrets)
- **Severity:** High (prior audit rated Medium; the proxy path is the reason to raise it)
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/main.rs:30` and `:262-295`; duplicated at
  `crates/arazzo-mcp/src/main.rs:28` and `:66-95`
- **Evidence:**

```rust
load_env_file(".env");          // main.rs:30 — before the runtime, before any client
...
std::env::set_var(key, &value); // main.rs:293 — unconditional, .env wins over real env
```

- **Why it matters:** F3's removal of the `$env` namespace closed the *expression*
  exfiltration path, but the loader is unchanged and now has a sharper consequence
  than the one the prior audit named. `HttpClient::new` builds its `reqwest::Client`
  after `load_env_file` has run, and `reqwest::ClientBuilder` defaults to
  `auto_sys_proxy: true` (`reqwest-0.12.28/src/async_impl/client.rs:309`), which
  resolves through `hyper_util`'s matcher reading `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`
  (`hyper-util-0.1.20/src/client/proxy/matcher.rs:231-233`). Proxy support is not
  feature-gated, so `default-features = false` in
  `crates/arazzo-runtime/Cargo.toml` does not disable it. A `.env` dropped beside a
  spec therefore redirects every outbound request in the run through a chosen proxy —
  cleartext `http://` targets in full, and `https://` targets' hostnames and timing —
  with no diagnostic. The same file also overrides `SSL_CERT_FILE`, `PATH`, and `HOME`.
  Malformed lines and I/O errors are still discarded (`Err(_) => continue`).
- **Notes / what to verify:** Whether `.env` precedence over the real environment is
  deliberate; most loaders default to the opposite. The only in-product consumer of
  the process environment is `parse_input_value` (G10) — nothing else reads it, so
  the loader's remaining reach is entirely into dependencies' env-driven behavior.

### G4 — Every criterion `error` diagnostic is computed and then dropped outside the debugger

- **Category:** Error handling (dropped diagnostics) / Observability
- **Severity:** Medium
- **Confidence:** High
- **Where:** produced at `crates/arazzo-runtime/src/runtime_core/criteria.rs:75`, `:87`,
  `:123`; consumed only at `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:596`;
  dropped at `crates/arazzo-runtime/src/runtime_core/engine_http.rs:637-647` and
  `engine_actions.rs:278`
- **Evidence:**

```rust
// criteria.rs:74-77 — the diagnostic is produced
Err(err) => { error = Some(format!("invalid regex: {err}")); false }
// engine_http.rs:640-647 — the trace record has no field to carry it
criteria.push(TraceCriterionResult {
    index, type_: ..., condition: ..., context: ..., result: evaluation.matched,
    warnings: evaluation.warnings.iter().map(|w| w.to_string()).collect(),
});
```

- **Why it matters:** `CriterionEvaluation::error` is the only signal distinguishing
  "the criterion evaluated and did not match" from "the criterion could not be
  evaluated at all" — an uncompilable `type: regex` pattern, a JSONPath outside the
  supported subset, or an XPath `version` the runtime rejects. `TraceCriterionResult`
  (`events.rs:283-293`) has no `error` field, the observer event carries only
  `passed`, and `engine_http.rs` copies `warnings` but never `error`. The single
  consumer is `insert_criterion_locals`, which populates debugger scopes. So in
  `arazzo run`, `arazzo run --json`, and the trace file, all three failures present
  as an ordinary criterion miss. Remediation item 5 of
  `plans/current/code-audit-remediation-plan.md` ("JSONPath silently returns `false`
  on unsupported syntax") is recorded as done because the diagnostic is now produced;
  it does not reach any non-debugger surface.
- **Notes / what to verify:** `criteria.rs` unit tests assert on `evaluation.error`
  directly, which is why the gap is invisible from the test suite — no test asserts
  that the error reaches a trace record or `--json` output.

### G5 — `{$…}` string interpolation still discards every expression diagnostic (prior F6, unchanged)

- **Category:** Error handling (dropped diagnostics)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-expr/src/lib.rs:151-159`, `:539-561`
- **Evidence:**

```rust
} else if value.contains("{$") {
    (Value::String(self.interpolate_string(value)), Vec::new())   // lib.rs:155
}
...
out.push_str(&self.evaluate_string(expr));                        // lib.rs:555
```

- **Why it matters:** `Vec::new()` is the whole diagnostic channel for the
  interpolated form. `interpolate_string` calls `evaluate_string`, which discards the
  warning list, so an unresolvable expression inside braces contributes empty text and
  no warning anywhere — not on stderr, not in `--json`, not under
  `--expr-diagnostics`. The braced form is the idiomatic one for parameter values and
  payload strings, so this is the path most documents actually take. Carried forward
  verbatim from F6; re-verified unchanged at `f82b2ff`.

### G6 — Cookie parameters and a `Cookie` header parameter silently overwrite each other

- **Category:** Correctness (request assembly)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_http.rs:415-439`;
  resolved against `crates/arazzo-runtime/src/runtime_core/client.rs:493-508`
- **Evidence:**

```rust
for param in &step.parameters {
    if param.in_ == Some(ParamLocation::Header) {
        headers.insert(param.name.clone(), resolved);        // engine_http.rs:424
    } else if param.in_ == Some(ParamLocation::Cookie) {
        cookie_parts.push(format!("{}={}", param.name, encoded));
    }
}
if !cookie_parts.is_empty() {
    headers.insert("Cookie".to_string(), cookie_parts.join("; "));  // engine_http.rs:438
}
```

- **Why it matters:** `headers` is a `BTreeMap<String, String>` keyed case-sensitively,
  while the field it becomes is case-insensitive. Two outcomes, both silent:
  a header parameter named `Cookie` is overwritten at `:438` by the assembled cookie
  parameters; a header parameter named `cookie` survives into the map as a second key
  and then wins at the client, because `BTreeMap` yields `"Cookie"` before `"cookie"`
  (`'C'` 0x43 < `'c'` 0x63) and `HeaderMap::insert` replaces (`client.rs:507`) — so
  every `in: cookie` parameter is dropped. Which side loses depends only on the
  casing the document author happened to use, and no warning is emitted either way.
  The same collision exists for `Content-Type` (set at `:409`) against a header
  parameter spelled `content-type`.
- **Notes / what to verify:** The client's case-insensitive collapse is deliberate and
  documented (`client.rs:484-492`); the gap is that request assembly builds the map
  case-sensitively before handing it over, so the collapse silently discards a value
  the assembler intended to keep.

### G7 — The debug gate blocks forever inside `spawn_blocking`, out of reach of cancellation (prior F10, new angle)

- **Category:** Concurrency / Resource lifecycle
- **Severity:** Medium
- **Confidence:** Medium (the hang is reasoned from the code path; not reproduced)
- **Where:** `crates/arazzo-runtime/src/debug/controller.rs:295-304`; bridged at
  `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:323-341`
- **Evidence:**

```rust
// controller.rs:295-300 — no timeout, no cancellation input
while !guard.continue_permit {
    guard = self.condvar.wait(guard).map_err(|_| "debug controller lock poisoned".to_string())?;
}
// engine_trace.rs:324 — moved onto the blocking pool, where the future's cancellation cannot reach it
tokio::task::spawn_blocking(move || { controller.gate_step(...) })
```

- **Why it matters:** `spawn_blocking` correctly keeps the condvar off the async
  worker threads, but it also severs the gate from cancellation. Dropping the
  `ExecutionHandle` cancels the `CancellationToken` and `execute_with_timeout`'s
  watchdog fires, yet a task parked in `Condvar::wait` observes neither: it holds a
  blocking-pool thread until some external caller sets `continue_permit`. Because
  `Runtime::drop` waits for blocking tasks, a paused debug session that hits its
  execution timeout leaves the process unable to finish shutting down. The related
  F10 sub-points are unchanged: `continue_permit` is a single global boolean rather
  than a per-stop permit (`controller.rs:61`, `:301`), and `stop_events` grows on
  every gate with draining left to the caller (`:281`, `:136`).
- **Notes / what to verify:** Whether any caller reliably invokes `force_resume`
  (`controller.rs:328`) on cancellation; nothing in `runtime_core` does.

### G8 — `Duration::from_secs_f64` in the rate limiter panics on a caller-supplied config value

- **Category:** Error handling / API design
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/client.rs:258-270`; public config at
  `:6-19`
- **Evidence:**

```rust
let missing = 1.0 - self.tokens;
let wait = missing / self.requests_per_second;
Some(Duration::from_secs_f64(wait.max(0.0)))     // client.rs:269
```

- **Why it matters:** `requests_per_second` is a `pub f64` on the public
  `RateLimitConfig`, unvalidated. Any positive value small enough that
  `missing / rps` overflows `Duration` (or reaches infinity) makes
  `Duration::from_secs_f64` panic — the fallible `try_from_secs_f64` is used
  correctly for the analogous `retryAfter` conversion at `engine_actions.rs:844` but
  not here. `panic = "abort"` in the release profile (`Cargo.toml:37`) turns that into
  an immediate process abort with no unwinding, in a library crate that
  `arazzo-mcp` embeds.
- **Notes / what to verify:** No CLI flag reaches `requests_per_second` today, so the
  reachable surface is library consumers and future flags.

### G9 — A server-supplied `Retry-After` becomes an unbounded sleep

- **Category:** Resource management / Security (hostile-server input)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_actions.rs:816-828`
- **Evidence:**

```rust
if let Ok(secs) = raw.parse::<u64>() {
    return Ok(Duration::from_secs(secs));    // engine_actions.rs:827 — no ceiling
}
```

- **Why it matters:** The configured `retryAfter` is validated for finiteness and
  representability (`:833-850`), but the header form — which takes precedence — is
  accepted up to `u64::MAX` seconds straight from the response. The only bound is
  the cancellation token that `sleep_with_cancel` selects on, so `arazzo run` is
  saved by its 5-minute `--execution-timeout` default. `Engine::execute` and
  `execute_collect` carry no timeout, so a library consumer (including anything built
  on the public API) parks indefinitely on a single hostile `Retry-After: 999999999`.
- **Notes / what to verify:** Whether a ceiling belongs here or at the caller; the
  configured-value path already has a representability check that this path skips.

### G10 — `-i key=$VAR` silently expands the environment, and a missing variable is sent as literal text

- **Category:** Security (secrets handling) / API design
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-cli/src/handlers.rs:743-782`
- **Evidence:**

```rust
if value.starts_with('$') {
    let var_name = value.strip_prefix('$').unwrap_or(&value)
        .trim_matches(|c: char| c == '{' || c == '}');
    if let Ok(found) = std::env::var(var_name) {
        return Value::String(found);        // handlers.rs:758
    }
}
// falls through to coercion — no diagnostic
```

- **Why it matters:** Two distinct problems. First, this is an undocumented
  environment-expansion surface that survived F3's removal of `$env`; combined with
  G3 it reads whatever a `.env` beside the spec defined. Second, and more likely to
  bite: when the variable is unset or misspelled, the branch falls through in silence
  and the literal string `"$API_TOKEN"` becomes the input value and is sent as the
  credential. That is the same failure mode F3 called out for `$env` ("a typo is
  indistinguishable from an empty secret"), reproduced here with the literal
  `$NAME` text instead of an empty string.
- **Notes / what to verify:** Only CLI flags reach this (`handlers.rs:61`, `:834`) —
  `test_runner` does not read inputs from spec files — so the operator always types
  the `$`. The surprise is the silence, not the capability.

### G11 — Two JSONPath implementations, one hand-rolled, decide different things

- **Category:** API design & maintainability
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/jsonpath.rs:23-170` versus
  `arazzo_expr::select_json_path` (used at `jsonpath.rs:44` and `payload.rs:97`)
- **Evidence:**

```rust
// jsonpath.rs:33-42 — a substring-heuristic gate, then a hand-written predicate parser
if let Some(reason) = detect_unsupported_jsonpath(trimmed) { return ...Unsupported(reason); }
if let Some(predicate) = parse_jsonpath_filter_predicate(trimmed) {
    return JsonPathOutcome::Matched(evaluate_jsonpath_filter_predicate(context_value, predicate));
}
match arazzo_expr::select_json_path(context_value, trimmed) { ... }
```

- **Why it matters:** A `type: jsonpath` criterion takes one of three paths depending
  on textual shape: a `..`/slice heuristic over quote-masked text (`:54-81`), a
  bespoke recursive predicate evaluator with its own precedence and truthiness
  (`:132-170`), or the shared evaluator. Selector objects and payload replacements
  take only the third. `AGENTS.md` makes exactly this argument for `operation_path.rs`
  — one classifier, because two independently written parsers for one field is how
  that surface drifted — and the same shape exists here for a spec-surface grammar
  (§5.8.11.4.3). The hand-rolled subset is also where G4's dropped `Unsupported`
  diagnostic originates, so the divergence is currently unobservable.
- **Notes / what to verify:** Whether the predicate evaluator exists because
  `select_json_path` cannot express filter predicates, or because it predates it.

### G12 — `local_source_paths` is a second, independent resolution of the same field, and it gates MCP filesystem access

- **Category:** API design & maintainability (with security consequence)
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/document_set.rs:289-307` versus
  the authoritative path at `:138-213`; consumed at
  `crates/arazzo-mcp/src/handlers.rs:260-266`
- **Evidence:**

```rust
// document_set.rs:298-303 — resolution written a second time, without the identity phase
let resolved = match Url::parse(&sd.url) {
    Ok(absolute) => absolute,
    Err(ParseError::RelativeUrlWithoutBase) => base.as_ref()?.join(&sd.url).ok()?,
    Err(_) => return None,
};
let path = resolved.to_file_path().ok()?;
```

- **Why it matters:** The two agree today, and the doc comment argues correctly that
  identity binding only ever *removes* a read. But the MCP server's entire filesystem
  gate is "whatever this second function returns"
  (`handlers.rs:263`), so the two must stay in lockstep for the gate to hold, and
  nothing enforces that: `bind` errors on cases `local_source_paths` returns `None`
  for, and a future read added to `bind` would be ungated by construction. That is
  the same one-field-two-parsers shape `AGENTS.md` prohibits for `operationPath`.
  The gate is also still check-then-use — paths are vetted at `handlers.rs:263` and
  read later inside `EngineBuilder::build` (prior F21's TOCTOU, unchanged).
- **Notes / what to verify:** `check_path_allowed` receives
  `resolved.to_string_lossy()` (`handlers.rs:263`), so a path with non-UTF-8 bytes is
  vetted under a different name than the one that will be opened.

### G13 — `panic!` and `unreachable!` bypass the workspace's own `unwrap`/`expect` deny, under `panic = "abort"`

- **Category:** Build/tooling & dependency hygiene
- **Severity:** Medium
- **Confidence:** High
- **Where:** `Cargo.toml:33-49`; 59 occurrences of the evasion idiom across
  `crates/*/src`, e.g. `crates/arazzo-runtime/src/runtime_core/redaction.rs:127`,
  `:134`, `crates/arazzo-runtime/src/runtime_core/deps.rs:5`,
  `crates/arazzo-runtime/src/runtime_core/events.rs:68`, `:72`, `:94`
- **Evidence:**

```toml
[profile.release]
panic = "abort"          # Cargo.toml:37
[workspace.lints.clippy]
unwrap_used = "deny"     # Cargo.toml:46-49 — panic!/unreachable! are not listed
```

```rust
Regex::new(r#"..."#).unwrap_or_else(|err| panic!("failed to compile bearer regex: {err}"))
```

- **Why it matters:** The lint expresses an intent — no panicking on the production
  path — that the codebase then routes around 59 times with
  `unwrap_or_else(|…| panic!(…))`, which is `unwrap` with extra steps and passes
  clippy. `clippy::panic` and `clippy::unreachable` exist and are not enabled. Most
  sites are inside `#[cfg(test)]` modules or `*_tests.rs` files where the idiom is
  fine, which is precisely what makes the handful of production ones easy to miss:
  `events.rs:68/72/94` are `#[allow(clippy::missing_panics_doc)]`-annotated panics in
  a public API (dead in practice — `collect` takes `self` by value, so the `Option`
  can never be `None`), and `input_validation.rs:195` is an `unreachable!` that holds
  only while serde_json's `arbitrary_precision` feature stays off. With
  `panic = "abort"` any of them terminates the process without unwinding, including
  inside the embedded MCP server.
- **Notes / what to verify:** Whether the test-file idiom should be exempted by
  `#[allow]` at the module level so the lint can be turned on for `src/`.

### G14 — `serde_json::to_vec(...).unwrap_or_default()` sends an empty body instead of failing

- **Category:** Error handling
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_http.rs:398`; same shape at
  `:844`
- **Evidence:**

```rust
serde_json::to_vec(value).unwrap_or_default()          // engine_http.rs:398
serde_json::to_string(&value).unwrap_or_default()      // engine_http.rs:844 (query object)
```

- **Why it matters:** A serialization failure produces an empty `Vec` and the request
  is sent anyway — with a `Content-Type` header claiming a body that is not there
  (`:402-410`) — rather than failing the step. `:844` does the same for an object-valued
  query parameter, sending `?name=` . The failure is hard to trigger for a
  `serde_json::Value` today, which is the argument for `expect`-style loudness rather
  than a silent default; the current form is indistinguishable from an intentional
  empty body in every downstream surface.

### G15 — A timing-comparison test asserts on wall-clock speedup

- **Category:** Testing & determinism
- **Severity:** Medium
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/tests/engine_parallel.rs:99-127`
- **Evidence:**

```rust
assert!(
    par_elapsed + Duration::from_millis(150) < seq_elapsed,
    "expected true concurrency, got sequential={seq_elapsed:?}, parallel={par_elapsed:?}"
);
```

- **Why it matters:** The assertion is a race against scheduler noise: it needs the
  parallel run to beat the sequential one by a fixed 150 ms margin on whatever machine
  CI happens to allocate. Under a loaded runner, a cold connection pool, or a
  single-core container the margin evaporates and the test fails without any behavior
  having changed. Related timing assertions sit at `engine_execution.rs:1939` and
  `:1985`.
- **Notes / what to verify:** Whether concurrency can be asserted structurally instead
  (the mock server already records per-request timestamps at
  `engine_parallel.rs:73`).

### G16 — Temp-file names in the DAP transcript tests rely on nanosecond uniqueness alone

- **Category:** Testing & determinism
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-debug-adapter/tests/dap_transcript_checkpoints.rs:246-250`,
  `dap_transcript_launch.rs:214-220`, `dap_transcript_engine_failure.rs:136`,
  `crates/arazzo-validate/src/tests/cases/public_api.rs:161`
- **Evidence:**

```rust
let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
let path = std::env::temp_dir().join(format!("arazzo-debug-checkpoint-{nanos}.yaml"));
```

- **Why it matters:** These share one process-wide `/tmp` with no process id and no
  sequence counter, so two concurrent `cargo test` runs (a CI matrix on a shared
  runner, or a local run alongside CI) can land on the same path and clobber each
  other's fixture. The codebase already knows the pattern: `document_identity.rs:44-56`
  composes nanos with `std::process::id()` *and* an `AtomicUsize`, and commit
  `17fcdb6` ("stop two input-validation fixtures claiming one temp directory") fixed
  exactly this class once already.

### G17 — `ExecutionHandle::collect()` buffers the whole event stream in memory

- **Category:** Resource management
- **Severity:** Low
- **Confidence:** High
- **Where:** `crates/arazzo-runtime/src/runtime_core/events.rs:64-85`, `:104-108`
- **Evidence:**

```rust
let mut events = Vec::new();
while let Some(event) = events_rx.recv().await { events.push(event); }   // events.rs:73-76
```

- **Why it matters:** `collect()` is the path the CLI and MCP server both take, and it
  retains every event for the run's lifetime. With `--trace`, each `TraceStep` carries
  a `TraceRequest` and `TraceResponse` including body text, so peak memory scales with
  (steps × retained body bytes) rather than with the channel's `channel_capacity`
  bound. `--trace-max-body-bytes` caps each body but nothing caps the total.
  `result_only()` (`:89`) avoids it by dropping the receiver, which is the shape a
  streaming consumer wants.

### G18 — Carried-forward findings re-verified as still live

- **Category:** (various — see the prior audit for each)
- **Severity:** as recorded there
- **Confidence:** High
- **Where / status at `f82b2ff`:**
  - **F11** (poisoned locks fabricate values) — still live, and the pattern has spread
    to new code: `crates/arazzo-runtime/src/runtime_core/state.rs:31-42`
    (`completed_workflows`) uses `.map(...).unwrap_or(false)` and
    `if let Ok(mut ...)`, so a poisoned lock silently reports "workflow not
    completed" and silently fails to record a completion. `client.rs:282-289`
    (`lock_recovering`) is the deliberate, documented counter-example.
  - **F12 / F13** (god modules) — `arazzo-validate/src/lib.rs` is 3,784 lines rather
    than 10,881, but that is test externalization into `src/tests/cases/` (commit
    `3afc4be`); production code is unchanged and `collect_diagnostics` is still a
    single 654-line function. `arazzo-expr/src/lib.rs` is 3,060 lines with the
    dispatcher intact.
  - **F14** (`run_tests` parameter count) — now 16 parameters at
    `crates/arazzo-cli/src/handlers.rs:786`; six `#[allow(clippy::too_many_arguments)]`
    suppressions remain workspace-wide.
  - **F16** (`Result<_, String>` at crate boundaries) — unchanged; every handler in
    `arazzo-mcp` and `arazzo-cli` still returns `Result<_, String>`.
  - **F18** (unbounded `RegexCache`) — unchanged at
    `crates/arazzo-runtime/src/runtime_core/criteria.rs:3-28`. Note the contrast with
    `arazzo-expr/src/matches_operator.rs:74-93`, which bounds its own table by count
    and key size and documents why; the two caches are now visibly inconsistent.
  - **F20** (`load_env_file` duplicated) and **F21** (MCP TOCTOU path check) —
    unchanged; see G3 and G12.
  - **F22** (multi-valued `Set-Cookie` collapsed to the last value) — unchanged at
    `client.rs:636-654`.

---

## Patterns / meta-smells

- **Diagnostics are computed and then dropped at the boundary.** G4 (criterion
  errors), G5 (interpolation warnings), and G10 (missing env var) are three instances
  of the same shape: the code knows something went wrong, constructs the message, and
  then hands back a value that cannot carry it. In each case a unit test asserts on
  the diagnostic at the point of production, so the drop is invisible from the test
  suite.
- **One field, two parsers.** `AGENTS.md` names this for `operationPath` and the
  codebase enforces it there (`operation_path.rs`, `operation_id.rs`,
  `source_reference.rs` are all single-owner). G11 (JSONPath) and G12
  (`sourceDescriptions` url resolution) are the same shape in surfaces the rule has
  not reached yet.
- **Lint intent routed around rather than satisfied.** G13: `unwrap_used`/`expect_used`
  are denied, and 59 call sites use `unwrap_or_else(|…| panic!(…))` to get the same
  behavior past the lint.
- **Case-sensitive maps standing in for case-insensitive fields.** G6 in request
  assembly; F22's `Set-Cookie` collapse and `client.rs:695-698`
  (`get("content-type").or_else(|| get("Content-Type"))`) are the same map choice
  showing through elsewhere.
- **Hostile-server input reaches unbounded resources.** G1 (response data steers the
  next request's path), G9 (response header sets the sleep). Both are bounded today
  only by `--execution-timeout`, which the library API does not apply.

## Non-findings (checked, no issue)

- `crates/arazzo-spec/src/source_reference.rs` and `operation_id.rs` — the
  `source-reference` ABNF is implemented once, split at the first dot per the vendored
  grammar, with a case-sensitivity negative test. Table-driven, no drift surface.
- `crates/arazzo-runtime/src/runtime_core/document_set.rs:397-411`
  (`remove_dot_segments`) — lexical, documented as deliberately diverging from symlink
  resolution, with the RFC 3986 §5.2.4 root floor handled.
- Header-name and header-value injection — `HeaderName::from_bytes` /
  `HeaderValue::from_str` at `client.rs:495-506` reject control characters, so CRLF in
  a cookie or header parameter is refused with an error rather than smuggled, despite
  `encode_cookie_value` (`url.rs:76-89`) not covering CR/LF itself.
- Redirect credential stripping — `is_cross_host_hop` (`client.rs:174-177`) matches
  reqwest's effective-authority predicate and is pinned by
  `cross_host_hop_predicate_matches_reqwest_baseline`; `Authorization`, `Cookie`,
  `Proxy-Authorization` and `cookie2` are all removed (`:617-627`).
- Response size limiting — both the `Content-Length` fast path (`client.rs:657-667`)
  and the streaming check (`:684-691`) are present and correct.
- `$ref` cycle guards in `arazzo-generate` — every `resolve_schema_ref` /
  `resolve_response_ref` / `resolve_request_body_ref` call site threads a `visited`
  set (`crud.rs:630-634`, `:898`, `examples.rs:21`, `openapi_describe.rs:74`, `:149`).
- `openapi_describe.rs:127`'s `unreachable!()` — genuinely unreachable; `Type::Object`
  is matched at `:89` before the arm containing it. Flagged only as part of G13's
  pattern, not as a live panic.
- `compute_max_iterations` (`engine_impl.rs:1116-1147`) — `u128` with saturating
  arithmetic throughout; a `retryLimit: u64::MAX` document produces a huge but
  representable bound, and the real ceiling is the 5-minute default
  `--execution-timeout`.
- `arazzo-mcp` directory walk (`state.rs:159-176`) — `DirEntry::file_type` does not
  follow symlinks, so a symlink loop under `--dir` cannot cause infinite recursion.
