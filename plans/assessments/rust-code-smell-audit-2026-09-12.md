# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-09-12
**Baseline:** `78b05fe` (`chore(release): prepare 0.6.1 with path parameter safety fix`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied, no source touched.
**Build state at audit time:** `cargo clippy --workspace --all-targets --all-features -- -D warnings`
exits 0, and `cargo test --workspace` exits 0 (62 suites, 1051 passed, 0 failed, 1 ignored). Every
finding below is therefore beyond the reach of both the lint wall and the existing test suite —
several are actively pinned as intended behaviour by a passing test (H4, H5, H21, H22).

**Prior audits:**
[`rust-code-smell-audit-2026-08-22.md`](rust-code-smell-audit-2026-08-22.md) (F1–F22) and
[`rust-code-smell-audit-2026-09-05.md`](rust-code-smell-audit-2026-09-05.md) (G1–G18, still
untracked in git). Findings here are numbered `H#`.

**Reading the confidence column.** Findings marked **REPRODUCED** were demonstrated end-to-end
against `target/debug/arazzo-cli` during this audit; the exact input is given. Findings marked
**Code-verified** were confirmed by reading the source and, where behaviour depends on a
dependency, that dependency's source in `~/.cargo/registry`. Nothing is reported from inference
alone.

---

## Summary

- **Total findings:** 34 (9 reproduced end-to-end, 25 code-verified)
- **1 Critical, 11 High, 14 Medium, 8 Low**

**Highest-risk areas**

- `runtime_core/jsonpath.rs` — a reachable process abort (H1)
- `runtime_core/url.rs` + `builder.rs` — the 0.6.1 path-safety guard stops at the authority (H2)
- `arazzo-expr/src/lib.rs` + `runtime_core/criteria.rs` — three untracked spec MUST/SHOULD
  deviations that silently invert success criteria (H4, H5, H6)
- `runtime_core/redaction.rs` and its consumers — redaction is applied per-consumer and three
  consumers skip it (H7, H8)
- `arazzo-cli/src/main.rs` + `arazzo-mcp/src/main.rs` — `.env` parsing aborts before argv (H3)

**Top 5 most severe**

1. **H1** — A criterion containing `&&&` panics the process (Critical, reproduced)
2. **H2** — A path parameter substituted into the URL *authority* retargets the request to an
   attacker-chosen host (High, reproduced)
3. **H4** — The `null` literal is parsed as the string `"null"`, inverting every `!= null`
   success criterion (High, reproduced, spec MUST)
4. **H3** — A stray quote in `.env` aborts both binaries before argument parsing (High, reproduced)
5. **H6** — `{$expr}` is never substituted into `regex`/`jsonpath`/`xpath` conditions (High,
   reproduced, spec MUST)

---

## Critical

### H1 — A `successCriteria` condition containing `&&&` panics the process

- **Category:** Correctness / DoS (reachable panic) · **Severity:** Critical · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/jsonpath.rs:253-259` (`split_predicate`)
- **Evidence:**

```rust
if paren_depth == 0 && bracket_depth == 0 && input[idx..].starts_with(delimiter) {
    let part = input[start..idx].trim();          // jsonpath.rs:254
    if !part.is_empty() { parts.push(part); }
    start = idx + delimiter.len();
    found = true;
}
```

  The loop never skips the bytes it just consumed. For `@.a &&& @.b`: at `idx = 4` the delimiter
  matches and `start` becomes 6; at `idx = 5` it matches again and `input[6..5]` inverts.

  The sibling implementation in `arazzo-expr` has exactly the guard this copy lacks —
  `crates/arazzo-expr/src/lib.rs:721`: `if idx < start { prev_backslash = false; continue; }`.

- **Reproduction:** a step whose criterion is `condition: '$[?(@.a &&& @.b)]'`, `type: jsonpath`,
  against a live 200 response:

```
thread 'tokio-rt-worker' panicked at crates/arazzo-runtime/src/runtime_core/jsonpath.rs:254:29:
byte range starts at 6 but ends at 5
execution task completed without sending result
```

  The same document with `&&` completes normally. `|||` and a bare `$[?(&&&)]` behave identically.
- **Why it matters:** `[profile.release]` sets `panic = "abort"`, so in a release build this is a
  hard process kill rather than a failed step. `arazzo-mcp` executes caller-supplied specs for the
  life of a session on a single serial dispatch thread, so one malformed criterion from an MCP
  client terminates the server. Two or fewer consecutive delimiter characters are fine, which is
  why no ordinary document has hit it.

---

## High

### H2 — A path parameter substituted into the URL authority retargets the request; the 0.6.1 safety guard is path-scoped by construction

- **Category:** Security (SSRF / host retargeting) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/url.rs:6-22` (`PATH_SEGMENT_ENCODE_SET`),
  `url.rs:108-120` (`PathParameterValidation::validate`),
  `crates/arazzo-runtime/src/runtime_core/builder.rs:308-315`
- **Evidence:** `validate` filters every recorded substitution span to the path component:

```rust
let (path_start, path_end) = path_component_bounds(url);
for part in self.parts.iter().filter(|part| {
    part.end <= url.len() && ... part.start >= path_start && part.end <= path_end
```

  but `replace_path_params` is applied to the whole target string, and the encode set deliberately
  permits `:` and `@` — the two authority delimiters. The repo's own test asserts the consequence:

```rust
assert_eq!(replaced("https://{host/name}/items", &[("host/name", "..")]),
           "https://../items");            // url.rs:447-450
```

  Reachable without an author writing a host template: `builder.rs:308-315` `continue`s past any
  OpenAPI server variable whose `default` is not a *string*, leaving `{name}` literal in the
  derived base with no error and no warning.
- **Reproduction:** OpenAPI `servers: [{url: "https://api.{env}", variables: {env: {default: 123}}}]`
  (integer default), Arazzo step parameter `{name: env, in: path, value: $inputs.env}`:

```
$ arazzo-cli run authority.arazzo.yaml w1 -i env=example.com@evil.test --dry-run --json
  "url": "https://api.example.com@evil.test/pets"
$ python3 -c "from urllib.parse import urlsplit; print(urlsplit('https://api.example.com@evil.test/pets').hostname)"
  evil.test
```

  `api.example.com` is demoted to userinfo; the effective host is `evil.test`. `-i env=example.com:8443`
  injects a port the same way. `/` *is* blocked (`example.com/..` → `%2F`), so the encode set works
  as designed — the gap is the region the validator declines to inspect.
- **Why it matters:** an `$inputs`- or `$steps`-sourced value redirects the request, and any
  `Authorization` header on it, to an attacker-chosen host while the URL still reads as the
  legitimate one. `crates/arazzo-runtime/tests/path_parameter_safety.rs` has twelve tests, all
  targeting the path; none exercises the authority.
- **Notes:** this is not a re-report of G1 — G1's fix is complete and correct for what it scopes.
  Whether authority substitution should exist at all is a design decision, not an encode-set tweak.

### H3 — A stray quote in `.env` aborts both binaries before argument parsing

- **Category:** Panic reachable from input · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-cli/src/main.rs:283-292`, duplicated at `crates/arazzo-mcp/src/main.rs:87-96`
- **Evidence:**

```rust
let value = if (trimmed.starts_with('"') && trimmed.ends_with('"'))
    || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
{
    trimmed[1..trimmed.len() - 1]      // main.rs:286
```

  For a one-byte value `"`, both `starts_with` and `ends_with` are true and the slice is `[1..0]`.
- **Reproduction:** a `.env` containing `KEY="` in the working directory:

```
$ arazzo-cli --version
thread 'main' panicked at crates/arazzo-cli/src/main.rs:286:20:
byte range starts at 1 but ends at 0
exit 101
```

- **Why it matters:** `load_env_file(".env")` runs before clap, so *every* command in *both*
  binaries dies — including `--version` and `--help`. Under `panic = "abort"` there is no
  diagnostic tying the abort to the file. For `arazzo serve` / `arazzo-mcp` the server dies before
  it speaks protocol, which an MCP client sees as an immediate transport close. F20 already records
  that `load_env_file` is duplicated; the consequence is that this must be fixed in two places.

### H4 — The `null` literal is parsed as the string `"null"`, inverting every `!= null` success criterion

- **Category:** Spec conformance (§5.8.11.1, §5.8.11.4.1) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-expr/src/lib.rs:905-936` (`parse_value`)
- **Evidence:** there is no `"null"` arm:

```rust
match token {
    "true"  => Value::Bool(true),
    "false" => Value::Bool(false),
    _ => { /* quoted-string handling, else */ Value::String(token.to_string()) }
}
```

  The vendored spec (`spec/arazzo/v1.1.0.html`, §5.8.11.4.1) reads: *"null only equals itself
  (null == null is true). Comparing null with any other value evaluates to false."*
- **Reproduction:** against a body of `{"explicit_null": null}`:

| condition | spec | actual |
|---|---|---|
| `$response.body.explicit_null == null` | pass | **fail** |
| `$response.body.explicit_null != null` | fail | **pass** |
| `$response.body.missing != null` | fail | **pass** |

- **Why it matters:** `!= null` is the canonical "did I actually get data back?" guard, and it
  passes precisely when there is no data. Every such criterion in existence is inverted.
- **Notes:** `lib.rs:1998` pins the wrong behaviour —
  `assert!(!eval.evaluate_condition("$inputs.none == null"))` with `none` set to `Value::Null`.
  Not tracked in [`arazzo-spec-conformance-audit.md`](arazzo-spec-conformance-audit.md) (F1–F23).

### H5 — JSONPath criteria decide on value truthiness instead of nodelist emptiness

- **Category:** Spec conformance (§5.8.11.4.3) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/jsonpath.rs:44-47`, consumed at `criteria.rs:80-92`
- **Evidence:**

```rust
Ok(selection) => JsonPathOutcome::Matched(is_truthy(&selection.value)),
```

  `JsonPathSelection` already carries `match_count`, the value the spec asks for, and the code
  discards it. Vendored spec §5.8.11.4.3: *"A condition passes (truthy) when the JSONPath expression
  returns a non-empty nodelist (one or more nodes). A condition fails (falsy) when the JSONPath
  expression returns an empty nodelist (zero nodes)."*
- **Reproduction:** against `{"enabled": false, "count": 0, "empty": ""}`:

| condition | nodes | spec | actual |
|---|---|---|---|
| `$.enabled` | 1 | pass | **fail** |
| `$.count` | 1 | pass | **fail** |
| `$.empty` | 1 | pass | **fail** |
| `$.missing` | 0 | fail | fail |

- **Why it matters:** this is the exact bug F1 fixed for XPath — `xpath.rs:14` now carries a
  dedicated `truthy` EBV field and `criteria.rs:121` uses it. The JSONPath arm was never given the
  same treatment, so any `type: jsonpath` criterion over a boolean-`false`, zero, or empty-string
  field fails a step the spec says must pass. Textbook "asserted at the point of production
  (`lib.rs:2397-2436`), discarded by the caller."

### H6 — `{$expr}` is never substituted into `regex` / `jsonpath` / `xpath` conditions

- **Category:** Spec conformance (§5.8.11.3, MUST) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/criteria.rs:69-128`; signature at `jsonpath.rs:24`
- **Evidence:** all three typed arms pass `&criterion.condition` verbatim — `regex_cache.is_match(&criterion.condition, …)`
  (`:72`), `evaluate_jsonpath_condition(eval, …, &criterion.condition)` (`:84`),
  `select_xpath(…, &criterion.condition, …)` (`:114`). The evaluator is threaded in and deliberately
  unused:

```rust
pub(super) fn evaluate_jsonpath_condition(
    _eval: &ExpressionEvaluator,      // jsonpath.rs:24
```

  `interpolate_string` has no call site on any criterion condition. Vendored spec §5.8.11.3:
  *"For regex, jsonpath, and xpath conditions, runtime expressions MUST be embedded within the
  condition string using {} curly braces. The runtime expressions are evaluated first, then
  substituted into the condition string before the expression is evaluated."*
- **Reproduction:** with `body.price == "10"` and `-i want=10`:

```
regex  ^10$              (literal, control)  -> PASS
regex  ^{$inputs.want}$                      -> fail
jsonpath $[?(@.price == "{$inputs.want}")]   -> fail
```

- **Why it matters:** the spec's own worked examples are matched against the literal brace text, so
  a regex criterion silently never matches and a jsonpath criterion compares against the string
  `{$inputs.expectedStatus}`. `deps.rs:119/130/135` already scans criterion conditions for
  `$steps.` references and builds DAG edges for expressions that are then never substituted.
- **Notes:** not tracked in the conformance audit.

### H7 — `is_sensitive_key` misses `api_key`, and the two redaction paths in one file disagree about it

- **Category:** Security (secret exposure) · **Severity:** High · **Confidence:** High — verified by exhaustive replay of the predicate
- **Where:** `crates/arazzo-runtime/src/runtime_core/redaction.rs:12-33` vs `:130-135`
- **Evidence:** the stem list is matched with `contains` against a lowercased name:

```rust
const SENSITIVE_STEMS: [&str; 10] = ["password","passwd","secret","token","authorization",
                                     "apikey","cookie","session","credential","pwd"];
```

  `"api_key".contains("apikey")` is false — the underscore breaks the stem — and `api_key` is not in
  the four-entry `SENSITIVE_EXACT` list either. Meanwhile the text-pattern path *in the same file*
  names it explicitly:

```rust
r"(?i)(password|passwd|secret|token|authorization|apikey|api_key|credential|pwd)(\s*[:=]\s*)\S+"
```

  Replaying `is_sensitive_key` over realistic field names, these are **not** redacted:
  `api_key`, `api_key_id`, `X-Api-Key-V2`, `private_key`, `privateKey`, `signing_key`,
  `access-key`, `passphrase`, `jwt`, `bearer`, `otp`, `pin`, `auth`.
- **Why it matters:** `is_sensitive_key` is the sole gate for header redaction, URL-query
  redaction, JSON body/output redaction and `--verbose` input echo. A response or request body
  carrying `{"api_key": "sk-live-…"}` passes through `redact_json_value` untouched into traces,
  `--json` output and error messages. The file's own regex is proof the authors consider that name
  sensitive.

### H8 — Redaction is applied per-consumer, and three consumers skip it

- **Category:** Security (secret exposure) · **Severity:** High · **Confidence:** High — code-verified
- **Where:** three independent sites
- **Evidence:**

  **(a) Replay drift errors dump every header and the full URL.**
  `crates/arazzo-runtime/src/runtime_core/replay.rs:75-83`:

```rust
format!("replay request drift at seq {seq} attempt {attempt}: headers expected {:?} got {:?}",
        expected.headers, actual.headers)
```

  `Authorization`, `Cookie` and `X-API-Key` are printed verbatim; `replay.rs:69` does the same for
  the URL, though `redact_url_query` exists precisely for that.

  **(b) Every transport error message carries the full URL including its query string.**
  `runtime_core/error.rs:169` builds the message from `err.to_string()`, and reqwest's `Display`
  (`reqwest-0.12.28/src/error.rs:40-42`) appends `" for url ({url})"`. reqwest's own doc comment
  (`src/error.rs:12-15`) names the hazard — *"Errors may include the full URL … If the URL contains
  sensitive information (e.g. an API key as a query parameter), be sure to remove it
  (`without_url`)"* — and the repo calls `without_url` **zero** times.

  **(c) The DAP ships unredacted headers and full raw bodies to the debugger.**
  `runtime_core/engine_trace.rs:640` inserts `requestHeaders` verbatim, `:655` `responseHeaders`,
  `:658` `responseBodyRaw`. The CLI trace path redacts the identical data at
  `crates/arazzo-cli/src/trace.rs:185-197`; the DAP path calls neither `redact_headers` nor
  `redact_url_query`.
- **Why it matters:** a secret that reads `[REDACTED]` in `--trace` output is plaintext in the
  debugger's Variables pane, in any replay-drift error, and in every connection-failure message —
  all of which reach `--json`, logs and CI output. The failure-criteria path *was* fixed (see the
  F9 re-verification below), which makes the remaining gaps easy to miss.

### H9 — JSON response bodies never receive pattern-based redaction

- **Category:** Security (secret exposure) · **Severity:** High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-runtime/src/runtime_core/control.rs:22-31` (`step_result_error`)
- **Evidence:** the two redaction strategies are mutually exclusive:

```rust
let mut body_preview = if let Some(mut body_json) = resp.body_json.clone()
    .or_else(|| serde_json::from_slice(&resp.body).ok())
{
    redact_json_value(&mut body_json);      // key-name matching only
    body_json.to_string()
} else {
    redact_text_patterns(&String::from_utf8_lossy(&resp.body))   // pattern matching only
};
```

- **Why it matters:** a JSON body gets key-name redaction and no pattern scan, so a token in a
  *value* under an innocuous key survives: `{"data": "Bearer sk-live-abc"}` is emitted in full,
  while the identical bytes served as `text/plain` are caught by `RE_BEARER`. Combined with H7,
  `{"api_key": "sk-live-…"}` is redacted by neither path.

### H10 — The `$steps.<id>` dependency regex is narrower than the evaluator's grammar, so valid documents lose DAG edges

- **Category:** Correctness (DAG construction) · **Severity:** High · **Confidence:** High — validation behaviour reproduced
- **Where:** `crates/arazzo-runtime/src/runtime_core/deps.rs:3-6`, consumed by `build_levels`
  (`:42`) and `compute_transitive_deps` (`:205`)
- **Evidence:**

```rust
Regex::new(r"\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.")     // deps.rs:4 — first class excludes digits and '-'
```

  The evaluator uses no character class at all — `crates/arazzo-expr/src/lib.rs:219`:
  `after.split_once(".outputs.")`. And the validator's identifier class is
  `^[A-Za-z0-9_\-]+$` (`arazzo-validate/src/lib.rs:2057`), which permits a leading digit.
- **Reproduction:** a document with `stepId: 9getPet` and a second step whose parameter value is
  `$steps.9getPet.outputs.id` validates **clean — with no warning even under `--strict`**:

```
$ arazzo-cli --strict validate digitid.yaml
Valid Arazzo 1.0.1 spec: digit id
```

  yet `STEP_REF_RE` cannot match `9getPet`.
- **Why it matters:** under `--parallel` the dependent step gets indegree 0, joins the same level as
  its dependency, and resolves `$steps.9getPet.outputs.id` against a snapshot taken before that
  level ran — yielding `Null` with only a warning. Under `execute_step` the dependency is never
  scheduled at all. A fully valid document silently executes in the wrong order.

### H11 — `generate` hangs and exhausts memory on a document-supplied `minLength`

- **Category:** DoS from untrusted input · **Severity:** High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-generate/src/examples.rs:320-336` (`apply_length_constraints`)
- **Evidence:**

```rust
if let Some(min) = min_length {
    while s.chars().count() < min {
        s.push('a');
    }
}
```

- **Why it matters:** `min_length` comes straight from the OpenAPI document. `minLength: 4000000000`
  grows a 4 GB string one byte at a time while re-running an O(n) `chars().count()` on every
  iteration — quadratic, so it never completes. Reachable from `arazzo generate` and from the MCP
  `generate_example` tool, where the document is caller-supplied. No cap exists anywhere on the path.

### H12 — A failed step in a parallel level silently discards every later sibling's events, trace records and outputs

- **Category:** Observability / lost audit data · **Severity:** High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_parallel.rs:66-79` and `:166-179`
- **Evidence:** `level_results` is sorted by step index and each entry's buffered events are
  replayed only when the loop reaches it:

```rust
for event in std::mem::take(&mut par_exec.events) {
    let _ = exec_ctx.event_tx.send(event).await;      // :76
}
...
FlowDecision::Error(err) => { ...; return Err(err); }  // :180
```

  `FlowDecision::Error` is the ordinary outcome of a step failing its criteria with no matching
  failure action, so this is a live path, not a defensive one.
- **Why it matters:** siblings with a higher index ran to completion and issued real HTTP requests,
  but produce no `AfterStep`, no `StepCompleted`, no trace record and none of their buffered
  events. From the outside those requests never happened. The cleartext-credential warning is
  latched once per host (`client.rs:401-403`, `HashSet::insert`), so if it fires inside a discarded
  sibling the fact that credentials went over plaintext HTTP is never reported for that host again
  for the rest of the run — exactly the case `events.rs:231-233` says must not be silent.

---

## Medium

### H13 — DAP teardown can hang the adapter process: `force_resume` is not a latch

- **Category:** Deadlock · **Severity:** Medium-High · **Confidence:** High (interleaving spelled out) — code-verified
- **Where:** `crates/arazzo-debug-adapter/src/dap/session.rs:370-388`, `:245-269`;
  `crates/arazzo-runtime/src/debug/controller.rs:328-341`; gates at `engine_http.rs:669`, `:710`
- **Evidence:** `if !guard.waiting { return Ok(()); }` — no persistent flag is set. Interleaving:
  `disconnect` cancels the token and calls `force_resume` while the engine is awaiting an HTTP
  response (`waiting == false`, so nothing is armed); the response arrives; `debug_gate_success_criterion`
  runs with no `check_cancelled()` in that loop, matches a breakpoint, and blocks in `Condvar::wait`
  with no resumer left; the monitor then calls `h.join()` unconditionally and blocks forever.
- **Why it matters:** `handle_disconnect` never returns, so the adapter must be killed. New angle on
  G7: the missing piece is a *sticky* disarm, and the monitor's unconditional join escalates a stuck
  gate into a process hang. The window is one HTTP round trip.

### H14 — Client-controlled `Content-Length` drives an unbounded allocation, in both servers

- **Category:** Resource exhaustion · **Severity:** Medium-High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-mcp/src/protocol.rs:106-112`; same shape at
  `crates/arazzo-debug-adapter/src/dap/transport.rs:104-116`
- **Evidence:** `let mut buf = vec![0u8; content_length];` with no ceiling between parse and
  allocate. `read_line` above it is also uncapped, so a header line with no newline grows without
  bound.
- **Why it matters:** `Content-Length: 999999999999` allocates before a payload byte is read; Rust's
  allocation-error handler aborts, and `panic = "abort"` removes any graceful path. This is the
  first thing the MCP server does with untrusted bytes. `mcp_integration.rs` frames only
  well-formed payloads.

### H15 — Trace redaction makes `replay` structurally impossible for any authenticated workflow

- **Category:** Feature conflict · **Severity:** Medium-High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-cli/src/trace.rs:185-192` vs `runtime_core/replay.rs:63-80`
- **Evidence:** `--trace` writes `Authorization: [REDACTED]` (asserted by `cli_integration.rs:2532`),
  and `validate_replay_request` then compares recorded headers byte-for-byte against the freshly
  built real request, failing with `REPLAY_REQUEST_MISMATCH`.
- **Why it matters:** `cli.rs:37` advertises the two together — "use `--trace <trace.json>` to
  capture redacted replay evidence" — but any workflow with an `Authorization`/`Cookie`/`X-API-Key`
  header, a `token=` query parameter, or a matching body field can never round-trip. The single
  round-trip test (`cli_integration.rs:2653`) uses a fixture with no sensitive fields.
  `redact_url_query`'s own comment says it avoids URL reconstruction "to avoid … replay drift" — the
  author was tracking drift, but the value substitution *is* the drift.

### H16 — The MCP allowed-directory gate is off by default

- **Category:** Security posture · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-mcp/src/state.rs:62-66`; construction at `arazzo-mcp/src/main.rs:42-46`
  and `arazzo-cli/src/main.rs:244-248`
- **Evidence:**

```rust
let Some(allowed) = &self.allowed_dirs else { return Ok(()); };
```

  and both call sites build `allowed_dirs` only from a separate `--allowed-dir` flag; `--dir` is
  **not** implicitly added.
- **Why it matters:** the documented invocation `arazzo serve --dir ./specs` leaves the gate
  disabled, so `validate_spec`, `generate_workflow` and `describe_openapi` read any path the process
  can reach — a read/probe primitive over the filesystem for the MCP client, since parse errors
  quote file content. The safe configuration requires naming the same directory twice.
  Distinct from F21 (the check's TOCTOU mechanics) and G12; the gate's own mechanics are sound
  (`Path::starts_with` is component-wise, and the OpenAPI-only filter matches the loader's).

### H17 — Step `operationId` and `workflowId` targets are never validated; the canonical classifier has no call site in the validator

- **Category:** Validation completeness · **Severity:** Medium-High · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-validate/src/lib.rs:1553-1615`, import list at `:65-70`
- **Evidence:** the step-target match handles `OperationPath` and async `ChannelPath`/`WorkflowId`,
  and `OperationId` plus non-async `WorkflowId` fall into `_ => {}`. `classify_operation_id` is
  imported only by `arazzo-runtime/src/runtime_core/engine_http.rs:116-122`.
- **Why it matters:** `operationId: $sourceDescriptions.tpyo.getPet` and `workflowId: doesNotExist`
  both pass `arazzo validate` clean and fail only at run time, while the exactly parallel cases —
  `operationPath` naming an unknown source (`:1556`), an *action's* `workflowId` (`:3100`) — are hard
  errors. AGENTS.md names `operation_path.rs` as the one classifier so runtime and validate cannot
  each parse it; here validate simply does not parse it at all.

### H18 — A component-resolution failure aborts the whole diagnostic pass

- **Category:** Error handling · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `crates/arazzo-validate/src/lib.rs:292-293`, `:325-326`
- **Evidence:** `resolve_components(&mut spec, Some(&raw)).map_err(Error::ComponentResolution)?`
  returns before `collect_diagnostics` runs. Every underlying failure is a bare `String`.
- **Why it matters:** a single `$components.parameters.tpyo` suppresses *all* other diagnostics in
  the document, and `arazzo-cli/src/output.rs:419-424` renders it with `kind: None, path: None` —
  no `ValidationErrorKind`, no document path — while every other bad reference is a structured
  `invalidReference`.

### H19 — Malformed comparisons silently pass or silently fail, with no diagnostic

- **Category:** Correctness (§5.8.11.4.5) · **Severity:** Medium · **Confidence:** High — measured
- **Where:** `crates/arazzo-expr/src/lib.rs:563-628`, `:905-936`
- **Evidence:** the right-hand side is whatever text follows the operator, run through `parse_value`,
  which never fails. With `$statusCode` = 200: `$statusCode >` → **true** (right operand `""`);
  `$statusCode !=` → **true**; `$statusCode <> 200` → **true** (`<` wins, RHS becomes `"> 200"`);
  `$statusCode == 200 ||` → **true**. All emit zero warnings.
- **Why it matters:** a typo'd operator or truncated condition silently flips a step's pass/fail with
  nothing in `--json`, the trace, or stderr.

### H20 — `find_operator` ignores bracket/paren depth, so filter expressions make criteria false

- **Category:** Correctness · **Severity:** Medium · **Confidence:** High — measured
- **Where:** `crates/arazzo-expr/src/lib.rs:774-820`
- **Evidence:** only quote state is tracked — no depth counters, unlike the sibling
  `split_outside_quotes` (`:712-772`). The first operator character *inside* a filter wins. Against
  `items: [{"id":1,"qty":5},{"id":2,"qty":50}]`, `$response.body.items[?(@.qty>10)].id == 2`
  evaluates the left operand to `2` on its own but the whole condition to **false**.
- **Why it matters:** the mis-split left operand fails path tokenization, which `resolve_body_value`
  swallows via `.ok()` (`:1041`), yielding `Null`, which compares unequal to everything. A criterion
  copied from a working `outputs:` expression silently evaluates false.

### H21 — Ordered comparison against `null`, and no numeric-string coercion

- **Category:** Spec conformance (§5.8.11.4.1) · **Severity:** Medium · **Confidence:** High — measured
- **Where:** `crates/arazzo-expr/src/lib.rs:976-1006`
- **Evidence:** `compare_values` special-cases null (`:938-945`) but `compare_ordered` does not —
  `to_string_value(Null)` is `""`, so `$response.body.explicit_null < 5` is **true**. Separately
  `to_f64` returns `None` for every non-`Number`, so with `price` = the JSON string `"10"`,
  `price > 9` is **false** and `price < 9` is **true**; `lib.rs:1994` pins `"10" < "2"`.
  Spec: *"Numeric strings SHOULD be coerced to numbers when compared with numeric operators."*
- **Why it matters:** `==` accidentally works for integers (both sides stringify to `"200"`), so the
  inconsistency is easy to miss: `code == 200` is true while `code > 199` is false for the same value.

### H22 — The iteration-limit guard is defeated by a document-supplied `retryLimit`

- **Category:** Unbounded loop from input · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `engine_impl.rs:1116-1147`, loop headers at `:265` and `:489`
- **Evidence:** `total_budget.saturating_mul(2)` over a `u128`; a single step with
  `retryLimit: 18446744073709551615` yields `max_iterations ≈ 2^65`. The test at `:1187` asserts
  this as intended.
- **Why it matters:** `IterationLimitExceeded` exists to stop a "possible infinite retry/goto loop"
  (`:707`), and a document can set it beyond any wall-clock reach. With `retryAfter: 0` this is an
  unbounded busy loop driven purely by document input.

### H23 — `Instant::now() + self.timeout` panics on a large configured timeout

- **Category:** Panic reachable from configuration · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `runtime_core/client.rs:515`; input path `arazzo-cli/src/cli.rs:375-379`
- **Evidence:** `Instant::add` panics on overflow. Every other duration in the file is defensive —
  `deadline.checked_duration_since` on line 521, `Duration::try_from_secs_f64` in
  `engine_actions.rs:844` — this one addition is not. `ClientConfig.timeout` is `pub`, so library
  embedders reach it too.

### H24 — An unresolved path parameter becomes an empty path segment and the request is still sent

- **Category:** Correctness / security · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `engine_http.rs:823-827`, `payload.rs:8`
- **Evidence:** a missing input resolves to `Value::Null` with a warning, and `value_to_string` maps
  `Value::Null => String::new()`.
- **Why it matters:** `DELETE /accounts/{id}/members/{memberId}` with an unresolved `memberId` sends
  `DELETE /accounts/7/members/` — a different, possibly collection-level route. Only a warning is
  produced. The 0.6.1 safety policy refuses `{empty}.` but not the plain empty segment.

### H25 — Two YAML parsers with divergent alias semantics; the debugger's index silently omits aliased content

- **Category:** Duplicated parsing / correctness · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `serde_yaml_ng` in six crates; `yaml-rust2` only in
  `crates/arazzo-debug-adapter/src/dap/source_index.rs:5`, `:1021-1034`
- **Evidence:** the DAP consumes the event stream, and an alias records nothing:

```rust
YamlEvent::Alias(_) => self.handle_alias(),      // :1034
fn handle_alias(&mut self) { self.consume_value_in_parent(); }   // :1021
```

  The runtime, by contrast, *expands* aliases — verified: a document whose second step uses
  `successCriteria: *crit` validates with both steps populated.
- **Why it matters:** for any document using anchors and aliases — an ordinary YAML idiom for shared
  parameter or criteria blocks — the executed structure and the debugger's source index disagree, so
  breakpoints and line mappings for aliased regions are wrong or absent. There is **zero** anchor/alias
  test coverage in the debug adapter. Same shape as the `operationPath` rule AGENTS.md names, at
  whole-document scope.

### H26 — Three different "is this an Arazzo spec file?" predicates disagree inside one binary

- **Category:** Consistency · **Severity:** Medium · **Confidence:** High — code-verified
- **Where:** `arazzo-cli/src/handlers.rs:627-635`, `arazzo-cli/src/test_runner.rs:458-460`,
  `arazzo-mcp/src/state.rs:166-172`
- **Evidence:** any `.yaml`/`.yml`, case-insensitive, non-recursive; versus `.arazzo.yaml` only,
  case-sensitive, non-recursive; versus `.arazzo.yaml` only, case-sensitive, **recursive**.
- **Why it matters:** `catalog ./specs` and `serve --dir ./specs` can see disjoint sets — a
  `foo.yaml` appears in `catalog` and never in `serve`; a `sub/bar.arazzo.yaml` the reverse. The
  documented agent workflow (`cli.rs:13`) chains exactly those two commands.

---

## Low

- **H27 — `helpers.rs` is a dead migration shim.** `runtime_core/helpers.rs` re-exports 24 symbols
  for **one** consumer (`builder.rs:229`, `helpers::RegexCache::new()`), kept compiling by a blanket
  `#![allow(unused_imports)]` at `:6`. Its own doc comment calls it a shim "during the split"; the
  split landed in `3cb8a48`.
- **H28 — `RegexCache` holds its mutex across compile *and* match.** `criteria.rs:14-27`. The
  comment claims "matching takes nanoseconds", but `criteria.rs:71` matches against a whole
  response body (default cap 10 MiB), and `Regex::new` is inside the guard too. The sibling
  `matches_operator.rs:15-25` documents in detail why it deliberately compiles *outside* its guard —
  two caches, opposite policies, no shared owner.
- **H29 — Library code writes diagnostics with `eprintln!`.** `engine_impl.rs:954-959`, `:1033-1035`,
  `:1071-1073`, `builder.rs:415-418`, `engine_actions.rs:618-624`. Every other engine signal travels
  as an `EngineEvent`; an embedder (MCP, VS Code adapter) can neither capture nor suppress these.
- **H30 — `Err(String::new())` is a magic sentinel meaning "already reported".**
  `output.rs:373`, `:574`, `:702`; `handlers.rs:819`, `:941`; consumed at `main.rs:41-45`. Five call
  sites rely on the convention, one documents it. Any future `map_err` producing `""` silently
  becomes "exit 1 with no message".
- **H31 — Multi-line error strings corrupt TAP, the default `test` format.**
  `test_runner.rs:484`, `:512`: `yaml_escape` handles `\` and `"` only, and `ValidationReport`'s
  Display is multi-line, so a validation failure emits an unterminated quoted scalar. `xml_escape`
  (`:634-640`) has the analogous gap for XML-illegal control bytes in JUnit output.
- **H32 — `generate --json` without `-o` prints YAML.** `handlers.rs:446-452` consults `global.json`
  only on the `-o` branch, so the documented contract (`schema generate`) is not honoured. No test
  passes `--json` to `generate`.
- **H33 — `--verbose` writes unredacted request URLs to stdout.** `output.rs:609-641` uses
  `println!` for diagnostics (contradicting `cli.rs:13` and the comment at `output.rs:349-350`) and
  reads from the *unredacted* `trace_steps`, not the redacted clone made at `handlers.rs:230`.
- **H34 — `EngineBuilder::channel_capacity(0)` is accepted and aborts at `execute()`.**
  `builder.rs:100-104` validates nothing; `tokio::sync::mpsc::channel` asserts `buffer > 0`
  (`tokio-1.53.1/src/sync/mpsc/bounded.rs:160`). `build()` already returns `Result`, so the
  validation has a home.

---

## Re-verification of prior findings

**Closed since the 2026-09-05 audit**

- **G1** (path parameter `..`) — **fixed and sound within the path component.** The three-commit
  sequence `2c8802b` / `f53e45c` / `298b49a` validates against the *finalized* URL at
  `engine_http.rs:1019` (after every query mutation), models all four WHATWG double-dot spellings
  (`url.rs:252-275`), and covers every emission site through `build_url_from_path`. No bypass found
  inside the path. The authority is a separate, unguarded region — H2.
- **G2** (301/302 method downgrade) — **fixed and exact.** `da8ff64` matches
  `tower-http-0.6.8/src/follow_redirect/mod.rs:272-291` on method rewrite, payload drop and the four
  payload headers. `remove_sensitive_headers` and `make_referer` also match reqwest's own.
- **F9** (500 bytes of unredacted response body in failure errors) — **fixed.** `control.rs:22-31`
  now redacts before truncating. The redaction it applies is incomplete for JSON bodies — H9.

**Overstated by the prior audit**

- **G13** ("59 call sites use `unwrap_or_else(|…| panic!(…))` to get past the lint") — the
  production exposure is **9 sites**, not 59: `events.rs` ×3, `redaction.rs` ×2,
  `input_validation.rs:195`, `deps.rs:5`, `openapi_describe.rs:127`, `arazzo-cli/src/main.rs:34`.
  The rest are in `#[cfg(test)]` modules or `tests/`. All five `#[allow(clippy::unwrap_used)]`
  suppressions are on test modules. The remaining production sites are lazily-compiled regexes,
  runtime construction, and already-consumed handles. The one worth watching is
  `input_validation.rs:195`, whose invariant silently depends on `serde_json`'s
  `arbitrary_precision` feature staying off (it is off today; nothing enforces it).

**Still live**

F11 (poisoned locks fabricate values — new site at `engine_trace.rs:439-449`, feeding
`TraceStepRecord.attempt`, which the `golden_spec_baseline` guard compares), F12/F13 (god modules),
F14 (`run_tests` parameter count), F16, F18, F20 (and H3 is its consequence), F21, F22, G3–G12,
G14–G17.

---

## Patterns / meta-smells

- **The same parser written twice, with the second copy missing a guard.** H1 is the sharpest
  instance: `jsonpath.rs`'s `split_predicate` is `arazzo-expr`'s `split_outside_quotes` minus the
  `idx < start` check, and that single missing line is a process abort. H10 (step-ref regex vs
  evaluator), H17 (`classify_operation_id` unused by validate), H25 (two YAML parsers), H26 (three
  spec-file predicates) are the same shape. AGENTS.md already names this rule for `operationPath`;
  it has not reached the other fields.
- **Redaction is a per-consumer opt-in rather than a property of the data.** H7, H8, H9, H33 are
  four instances. Each consumer independently decides to redact, and the ones added most recently
  (replay drift, DAP variables) did not. The key list itself is hand-maintained and already
  disagrees with the regex in the same file.
- **Criterion evaluation degrades to `false`/`Null` instead of failing.** H4, H5, H19, H20, H21 all
  produce a wrong pass/fail with no diagnostic. G4 explains the structural reason the diagnostics
  cannot escape: `TraceCriterionResult` (`engine_http.rs:640-647`) has a `warnings` field but no
  `error` field, so `evaluation.error` is read at exactly one place in the tree — inside the debug
  gate.
- **Untracked spec deviations.** H4, H5, H6 and the `''` escaping gap are MUST/SHOULD-level and
  appear nowhere in [`arazzo-spec-conformance-audit.md`](arazzo-spec-conformance-audit.md) (F1–F23),
  which is the document AGENTS.md points at before touching spec surface.
- **Document-supplied numbers are trusted as budgets.** H11 (`minLength`), H22 (`retryLimit`),
  H23 (`--timeout-seconds`), H14 (`Content-Length`), H34 (`channel_capacity`).

---

## Non-findings (checked, no issue)

- **YAML alias bomb (billion laughs) is bounded.** libyaml's repetition limit rejects a 10^8-node
  alias ladder in 0.01 s at 8 MB RSS: `validation failed: parsing arazzo yaml: repetition limit
  exceeded`. Nesting depth is separately capped at 128 (`serde_yaml_ng-0.10.0/src/de.rs:112`).
- **YAML merge keys are refused, not misread.** `<<: *anchor` is reported as
  `unrecognized field "<<"` under `--strict`.
- **MCP stdout framing is clean.** Every function reachable from `protocol::serve` → `dispatch` was
  traced, plus a `println!`/`print!` grep across all six library crates: zero stdout writes on any
  MCP-reachable path. The engine's cleartext warning is `eprintln!`.
- **No lock guard is held across an `.await`** anywhere in the runtime or debug slices; every
  `std::sync::Mutex` use is inside a synchronous function. The rate-limiter's `tokio::sync::Mutex`
  guard is scoped and dropped before `sleep_with_cancel`.
- **Header injection.** `HeaderName::from_bytes` / `HeaderValue::from_str` (`client.rs:495-506`)
  reject CR/LF/NUL and surface a real `RuntimeError`.
- **Response size bounds** — both the `Content-Length` fast path and the streaming accumulator
  enforce `max_response_bytes`.
- **`sleep_with_cancel`, `await_transport`** — `biased` select with cancellation first; no lost
  wakeup, and a test pins the already-cancelled case.
- **`build_levels` cycle detection and arithmetic** — no underflow; unknown step references are
  correctly edgeless.
- **Atomic orderings** — `is_timeout` and `engine_done` both use `Release`/`Acquire` pairs;
  sequence counters are `Relaxed` and single-writer.
- **`MetadataReceiver`** (`source_index.rs:487-1010`) is SAX-style with an explicit stack, not
  recursion — no stack-depth risk from a hostile document.
- **Regex catastrophic backtracking** is not applicable — the `regex` crate is a finite automaton.
- **`insecure_hosts` matching, redirect hop accounting, JSON Pointer bounds checks, numeric
  equality in enum validation, `remove_dot_segments`, `refs.rs` cycle guards,
  `source_reference.rs` / `operation_id.rs` ABNF** — all re-read, all sound.

---

## Verification commands used

```
cargo clippy --workspace --all-targets --all-features -- -D warnings   # exit 0
cargo test --workspace                                                 # exit 0, 1051 passed
cargo build -p arazzo-cli --bin arazzo-cli
arazzo-cli --strict validate <doc>
arazzo-cli run <doc> w1 [-i k=v] [--dry-run] --json
```

Repro fixtures were written to a session scratchpad, not to the repository; the working tree is
unchanged apart from this file.
