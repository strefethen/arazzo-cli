# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-09-19
**Baseline:** `38b6088` (`refactor(expr): extract simple-condition evaluation into its own module`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied, no source touched.
Scheduled run; seven fresh-context reviewers (JSONPath layer, expression evaluator, runtime
re-verification, front-end re-verification, generate/spec/DAP sweep, engine/build sweep, test
suite) plus a lead pass over `arazzo-mcp`. Every High-and-above finding from a reviewer was either
reproduced by that reviewer or re-reproduced by the lead; spec claims were re-checked against
[`spec/`](../../spec/README.md).
**Build state at audit time:** `cargo clippy --workspace --all-targets --all-features -- -D warnings`
exits 0; `cargo test --workspace` exits 0 (68 result blocks — 10 unit binaries, 51 integration
binaries, 7 doc-test crates — 1104 passed, 0 failed, 1 ignored). Every finding below is beyond
the reach of both the lint wall and the suite, and several are pinned as intended behaviour by a
passing test (I45).

**Prior audits:**
[`2026-08-22`](rust-code-smell-audit-2026-08-22.md) (F1–F22),
[`2026-09-05`](rust-code-smell-audit-2026-09-05.md) (G1–G18),
[`2026-09-12`](rust-code-smell-audit-2026-09-12.md) (H1–H34). Findings here are numbered `I#`.
Since the 09-12 baseline (`78b05fe`) the RFC 9535 JSONPath migration (epic ac-edd23, v0.7.0)
landed, so the new JSONPath layer and the extracted `simple_condition.rs` got dedicated review.

**Reading the confidence column.** **REPRODUCED** = demonstrated end-to-end against
`target/debug/arazzo-cli`, `arazzo-mcp`, `arazzo-debug-adapter`, a release build, or a scratch
library probe; the input is given. **Code-verified** = confirmed by reading the source and, where
behaviour depends on a dependency, that dependency's source in `~/.cargo/registry`. Nothing is
reported from inference alone. Repro fixtures live in the session scratchpad, not the repository.

---

## Summary

- **Total new findings:** 84 — **1 Critical, 10 High, 36 Medium, 37 Low**
- **Prior findings re-verified:** 65 — 6 closed, 13 changed or corrected, 46 still live
  (H23 and H11 are counted as changed: both are still live, with materially wider reachability)

**Highest-risk areas**

- `runtime_core/xpath.rs` → `uppsala` — any XPath criterion/output parses a hostile response with
  no depth or entity limit (I1)
- `arazzo-expr/src/simple_condition.rs` and `arazzo-expr/src/jsonpath.rs` — document-supplied
  structure drives unbounded recursion (I2) and 2^depth parsing (I3) that `--execution-timeout`
  cannot interrupt
- `arazzo-mcp/src/handlers.rs` + `protocol.rs` — tool arguments are not validated against their own
  `inputSchema`, so a stringified `dry_run` goes live (I4) and a sentinel timeout aborts the server (I5)
- `runtime_core/engine_impl.rs` / `engine_http.rs` — Step `timeout`, sub-workflow literals,
  form bodies and `{$expr}` serialization all diverge from the spec without a diagnostic (I7–I10)
- `arazzo-debug-adapter/src/dap/` — breakpoint mapping is keyed by ids, not by file (I33–I35)
- The conformance evidence system itself — it records an unenforced field as conformant (I7), and
  "covered" claims run tests that pin violations (I45) through a scanner that cannot see most tests (I46)

**Top 5 most severe**

1. **I1** — A hostile XML response aborts the process (≥200 nested elements in a debug build,
   ≥2500 in release) or eats ~1 GB from 505 bytes; the execution timeout cannot stop it (Critical, reproduced)
2. **I3** — A 103-byte JSONPath criterion that passes `--strict validate` hangs `run` indefinitely
   (High, reproduced)
3. **I4** — MCP `run_workflow` with `"dry_run": "true"` sends the live DELETE (High, reproduced)
4. **I2** — ~3000 nested parentheses in a `successCriteria` condition overflow the stack; `validate`
   calls the document valid (High, reproduced)
5. **I7** — Step `timeout` is never enforced, and the conformance audit lists it as "verified present
   and conformant" (High, reproduced)

---

## Critical

### I1 — XML responses are parsed with no depth or entity-expansion limit: a small hostile body aborts the process or exhausts memory

- **Category:** Security / resources (DoS from a remote server) · **Severity:** Critical · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/xpath.rs:66-74` (`xpath_backend`) → `uppsala 0.3.0`:
  `parser.rs:2230`/`:2425` (`parse_element` ↔ `parse_content`, one frame pair per nesting level),
  `dom.rs:912-920` (`collect_text`, recursive), `parser.rs:790-858` (`expand_entity_value`, cycle
  check only, no size cap)
- **Evidence:**

```rust
let text = std::str::from_utf8(body).map_err(|err| format!("XML is not UTF-8: {err}"))?;
let mut doc = uppsala::parse(text).map_err(|err| format!("invalid XML: {err}"))?;   // xpath.rs:73-74
```

- **Reproduction:**
  - **Depth:** a local server returns `<a>`×N `x` `</a>`×N as `application/xml`; the step has an
    `xpath` criterion `/a`. Debug CLI: N=50 passes, **N=200 aborts** (exit 134,
    `thread 'tokio-rt-worker' has overflowed its stack`). Shipped v0.7.0 release binary: N=1000 passes,
    N=2500 (≈17.5 KB, far under the 10 MiB response cap) aborts.
  - **Entities:** a DTD with 10 references per level. 7 levels (450 B) → 3.5 s / 160 MB;
    **8 levels (505 B) → 37.6 s / 959 MB**; 9 levels extrapolates to ~6 min / ~10 GB.
    With `--execution-timeout 2s` the 8-level run still took 38.5 s before reporting
    `RUNTIME_EXECUTION_TIMEOUT`.
- **Why it matters:** the attacker is any server the workflow calls, not the document author. A stack
  overflow aborts in every profile (`panic = "abort"` is irrelevant). The same path is reached by XPath
  outputs, bare `/…` outputs, and the debugger's watch evaluation (`debug/controller.rs:368`), and it
  kills the long-lived MCP server. Entity expansion is synchronous CPU work on a tokio worker, and
  cancellation is only observed between awaits, so the timeout that is supposed to bound a run is blind
  to it (the same blind spot as I3).

---

## High

### I2 — Deeply nested `(` or `!` in a condition overflows the stack; `validate` accepts the document

- **Category:** Resources (DoS) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-expr/src/simple_condition.rs:40-48` (one recursion per paren),
  `:77-82` (one per `!`), `:170-187` (`is_balanced_outer_parens` rescans the whole string at every level)
- **Evidence:**

```rust
if condition.starts_with('(') && condition.ends_with(')') && is_balanced_outer_parens(condition) {
    let inner = &condition[1..condition.len() - 1];
    let (result, w) = self.evaluate_condition_with_diagnostics(inner);
...
if condition.starts_with('!') && !condition.starts_with("!=") {
    let inner = condition[1..].trim();
    let (result, w) = self.evaluate_condition_with_diagnostics(inner);
```

- **Reproduction:** `condition: '((…($statusCode == 200)…))'` with N parens. `--strict validate` exits 0
  at every N. Debug `run`: N=1000 passes, **N=3000 aborts** (`has overflowed its stack`, exit −6).
  Release 0.7.0: N=8000 passes, N=12000 (24 KB) aborts; mixed `!(` overflows at ~6k (release) / ~1k
  (debug). The work is also quadratic: 10,000 `!` cost 264 ms before the stack runs out.
- **Why it matters:** the evaluator runs on 2 MiB tokio worker stacks in the CLI, the MCP server and the
  DAP backend. The fuzz property (`".{0,96}"`) cannot reach these depths. Ticket ac-67bf5's design calls
  for "bounded iterative parsing or an explicit nesting limit", but it is paused and no audit recorded
  the crash.

### I3 — Nested JSONPath filters parse in 2^depth time; a 103-byte query hangs `run` past its execution timeout

- **Category:** Performance / security (DoS) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** admission at `crates/arazzo-expr/src/jsonpath.rs:206-224`; cause in
  `serde_json_path-0.7.2/src/parser/selector/filter.rs:113-121` (`parse_basic_expr` tries
  `parse_comp_expr` — which parses `@[?…]` in full, then fails the singular-query check — and then
  `parse_exist_expr`, which parses it again) and `:165`
- **Evidence:**

```rust
let structural = expression
    .bytes()
    .filter(|byte| matches!(byte, b'.' | b'[' | b'(' | b'!' | b'&' | b'|'))
    .count();
if structural > QUERY_STRUCTURAL_LIMIT {          // 128 — counts characters, not nesting cost
```

- **Reproduction:** criterion `$[?@[?@[?…@.a…]]]` at depth 24 (103 bytes, 26 structural characters).
  `--strict validate` exits 0; `run … --json --execution-timeout 5s` was still at 99% CPU after 45 s.
  Lead re-timing of dry-run payload selectors (debug): depth 12 → 0.06 s, 14 → 0.22 s, 16 → 0.86 s —
  doubling per level. Release library timings: depth 20 (87 B) 7.1 s, depth 22 (95 B) 30.7 s. Nested
  `count(@[?…])` doubles the same way.
- **Why it matters:** the budget meant to bound query cost admits the pathological case and rejects a
  harmless flat 22-way `||` query (132 structural characters). Parsing happens per criterion
  evaluation and per selector/replacement, before the context is resolved, as synchronous CPU work that
  cancellation cannot interrupt. No test covers parse time for nested filters within the budget.

### I4 — MCP `run_workflow` coerces a non-boolean `dry_run` to `false` and sends live requests

- **Category:** Security / API contract (fail-open safety flag) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-mcp/src/handlers.rs:225-251`; declared schema at `crates/arazzo-mcp/src/tools.rs`
  (`"dry_run": {"type": "boolean"}`)
- **Evidence:**

```rust
let dry_run = args
    .get("dry_run")
    .and_then(Value::as_bool)
    .unwrap_or(false);
```

- **Reproduction:** a workflow whose only step is `DELETE /items/{id}` against a local server.
  `tools/call run_workflow {"workflow_id":"del","dry_run":"true"}` → `{"kind":"success"}` and the
  server logs `DELETE /items/42`. The same call with `"dry_run": true` → `{"kind":"dryRun", …}` and no
  request.
- **Why it matters:** the caller asked for "resolve requests without sending them" and got a live,
  possibly destructive call, with a success result that does not reveal the difference. LLM clients
  routinely stringify booleans. The same pattern silently drops a non-object `inputs` (`:225-229`),
  `parallel`, and non-integer timeouts; none of the handlers validates arguments against the
  `inputSchema` it advertises.

### I5 — An MCP tool argument aborts the release MCP server (H23, now client-reachable)

- **Category:** Panic reachable from a remote caller · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-mcp/src/handlers.rs:241-245` → `crates/arazzo-runtime/src/runtime_core/client.rs:515`
- **Evidence:**

```rust
let http_timeout = Duration::from_secs(
    args.get("http_timeout_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS),
);
// client.rs:515
let deadline = Instant::now() + self.timeout;
```

- **Reproduction:** `run_workflow {"workflow_id":"del","http_timeout_seconds":9223372036854775807}`
  (i64::MAX, the common "no limit" sentinel). Debug build: `panicked at client.rs:515:24: overflow when
  adding duration to instant`, tool result `RUNTIME_INTERNAL_ERROR`. **Release build** (built from HEAD
  in scratch): the process exits −6 (SIGABRT); a following `ping` is never answered. 2^62 and smaller
  do not overflow on macOS.
- **Why it matters:** H23 was recorded as reachable from CLI configuration. Through MCP it is one
  tool argument away, and the release profile turns it into loss of the whole server session.

### I6 — An empty key or NUL byte in `.env` panics both binaries before argument parsing (H3 sibling)

- **Category:** Error handling (panic on ambient input) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-cli/src/main.rs:278-293`; byte-identical copy at `crates/arazzo-mcp/src/main.rs:82-97`
- **Evidence:**

```rust
let Some((key, value)) = line.split_once('=') else { continue; };
let key = key.trim();
...
std::env::set_var(key, &value);
```

- **Reproduction:** `.env` containing `=value` in the working directory → `arazzo-cli --version` exits
  101: `failed to set environment variable "" to "value": Invalid argument (os error 22)`.
  `KEY=a<NUL>b` panics with `unexpected NUL byte`. Both binaries, on `--help` and `--version`.
- **Why it matters:** same blast radius as H3 (the tool is unusable from that directory), but a
  different mechanism — a fix to H3's slice leaves this panic in place, and F20's duplication means
  every fix must land twice. No `.env` ticket (ac-a2fbd, ac-51491) mentions either panic.

### I7 — Step `timeout` is never enforced, and the conformance audit records it as conformant

- **Category:** Spec conformance / evidence integrity · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-spec/src/lib.rs:350` (`pub timeout: Option<u64>`); no read anywhere in
  `crates/arazzo-runtime/src` (only displayed at `arazzo-cli/src/output.rs:795` and
  `arazzo-mcp/src/handlers.rs:87`); claim at `plans/assessments/arazzo-spec-conformance-audit.md:241-245`
- **Spec (Step Object, `spec/arazzo/v1.1.0.html`):** *"The maximum number of milli-seconds to wait for
  the step to complete before aborting and failing the step."*
- **Reproduction:** a step with `timeout: 200` against an endpoint that answers after 2 s succeeds after
  2.01 s.
- **Why it matters:** a slow response that must fail the step (and so, absent `onFailure`, the workflow)
  passes. The conformance audit lists "Step fields `timeout`, `correlationId`, `action`, `dependsOn`"
  under "Verified present and conformant", so the document AGENTS.md sends every spec-surface change to
  first currently asserts the opposite of the runtime's behaviour.

### I8 — Literal parameter values on a workflow-target step reach the child as strings

- **Category:** Correctness / spec conformance · **Severity:** High · **Confidence:** High — **REPRODUCED** (two reviewers independently)
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_impl.rs:936-949`, via
  `crates/arazzo-spec/src/lib.rs:535-561` (`Parameter::value_as_str`)
- **Evidence:**

```rust
ValueSource::Literal(_) => {
    let value_str = param.value_as_str();   // 5 -> "5", true -> "true", {a:1} -> "{\"a\":1}"
    ...
    } else { eval.evaluate(&value_str) }    // non-$ text comes back as Value::String
```

- **Reproduction:** `value: 5`, `true`, `{a: 1, b: [1, 2]}`, `[x, y]` arrive as `"5"`, `"true"`,
  `"{\"a\":1,\"b\":[1,2]}"`, `"[\"x\",\"y\"]"`; the child's request body goes out as `{"count":"5"}`.
  With `--strict-inputs` the run fails `RUNTIME_SUB_WORKFLOW_FAILED` ("expected type integer, got
  string").
- **Why it matters:** the spec says a Parameter value *"can be a constant"* and that for workflow
  targets parameters map to workflow inputs. MCP always builds with `.strict_inputs(true)`
  (`arazzo-mcp/src/handlers.rs:280`), so there a spec-valid document fails outright. The goto-workflow
  path types correctly (`engine_actions.rs:618-624` uses `resolve_value_source`) — two resolutions of
  the same field. ac-681d4 reroutes this code but does not name the type loss.

### I9 — A form-urlencoded mapping payload is sent as JSON; the content-type test is case-sensitive

- **Category:** Spec conformance / correctness · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_http.rs:387-399`
- **Evidence:**

```rust
if !ct.contains("json") {
    if let Value::String(s) = value {
        return s.as_bytes().to_vec();
    }
}
serde_json::to_vec(value).unwrap_or_default()
```

- **Reproduction:** the spec's own "Form Data example" (`contentType: application/x-www-form-urlencoded`
  with a mapping payload) is sent with that content type and the body
  `{"client_id":"abc","grant_type":"client_credentials"}`; `--strict validate` is silent. An XML
  content type with a mapping payload goes out the same way. A string payload is sent as `"hello"`
  under `application/json` but as raw `hello` under `Application/JSON` (media types are
  case-insensitive).
- **Why it matters:** OAuth token endpoints — the canonical form-body use — receive a JSON document
  labelled as a form. Not tracked in the conformance audit.

### I10 — `{$expr}` renders arrays, objects and `null` as an empty string in outbound requests

- **Category:** Correctness / data loss (spec MUST) · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-expr/src/lib.rs:153-154`, `:487`, `:651-657` (`to_string_value`)
- **Evidence:**

```rust
} else if value.contains("{$") {
    (Value::String(self.interpolate_string(value)), Vec::new())
...
    _ => Cow::Borrowed(""),   // Null, Array, Object
```

- **Spec (`spec/arazzo/v1.1.0.html`):** *"If the value is a parsed structure (e.g., a JSON object or
  array from a parsed response, or workflow input), it MUST be serialized as JSON per RFC 8259."*
- **Reproduction:** dry-run with `--input-json 'ids=[1,2]' --input-json 'filter={"a":1}'` and payload
  `'ids={$inputs.ids} filter={$inputs.filter}'` → body `"ids= filter= "`; header `X-Ids: "ids="`; no
  warning even with `--expr-diagnostics warn`.
- **Notes:** owned by ac-28ba3 (blocked). Not in the conformance audit; earlier smell audits recorded
  only the lost diagnostics (G5/F6) and `$USD` deletion.

### I11 — `generate` expands reused required-property schemas exponentially

- **Category:** Performance / DoS · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-generate/src/examples.rs:17`, `:205-214`
- **Evidence:**

```rust
if depth > 5 { return Value::Null; }
for name in required {            // uncapped; only optional props are capped at 5
    if let Some(prop_ref) = properties.get(name) {
        obj.insert(name.clone(), generate_example(&prop_ref, name, components, depth + 1));
```

- **Reproduction:** six component schemas, each with W required properties that `$ref` the next level.
  W=6: 2.5 KB in → 1.4 MB YAML, 51 MB, 0.28 s. W=8: 7.8 MB, 266 MB, 1.6 s. **W=10: 3.7 KB in → 29 MB
  YAML, 751 MB, 5.8 s.** W=16 (~5.7 KB) extrapolates to ~12 GB. Runs twice per resource (create and
  update bodies).
- **Why it matters:** reachable from `arazzo-cli generate` and MCP `generate_workflow`; a separate trigger
  from H11 (`minLength`). Each `$ref` is expanded with a fresh `visited` set, so depth is the only bound.

---

## Medium

### Front ends (MCP, CLI)

- **I12 — The MCP reader thread exits silently on a framing error, and malformed requests get no response.**
  Error handling · **REPRODUCED**. `crates/arazzo-mcp/src/protocol.rs:34-38`, `:183-204`, `:215-218`:
  `Err(_) => { let _ = tx.send(ReaderMsg::ReadError(())); break; }` — the error text is discarded by
  construction (`ReadError(())`), and the main loop `break`s to `Ok(())`. One invalid-UTF-8 line, or
  `Content-Length: abc`, ends the server with exit 0 and an empty stderr; queued requests are never
  answered. A request that carries an `id` but fails to deserialize (`{"id":2,"methd":"ping"}`) and a
  JSON parse error are logged and skipped with no JSON-RPC `-32600`/`-32700` response, so the client
  waits on `id: 2` forever; `"id": null` deserializes as `None` and is treated as a notification. The
  comment at `:190-191` ("A single malformed message should not terminate the server") holds only for
  the JSON layer.
- **I13 — H11's quadratic `minLength` padding is reachable from MCP `generate_example` and blocks the
  server.** Performance · **REPRODUCED**. `crates/arazzo-generate/src/examples.rs:325-329`
  (`while s.chars().count() < min { s.push('a'); }` — O(n) count per push) via
  `standalone_example.rs:72-75`. Over MCP: `minLength` 40,000 → 0.04 s; **1,000,000 → 23.7 s**, during
  which a queued `ping` is not answered (single serial dispatch thread). The client supplies the schema
  directly, so no document is needed.
- **I14 — Several commands ignore the `--json` structured-error contract.** API contract · **REPRODUCED**.
  `crates/arazzo-cli/src/handlers.rs:493`, `:554` (`arazzo_validate::parse(path).map_err(|err| err.to_string())?`)
  → `main.rs:42`. With `--json`, `list`, `steps`, `catalog`, `show` and `generate` exit 1 with empty
  stdout and plain text on stderr; inside `test --json`, input/header/`--openapi` parse errors bypass
  `emit_error`. None of those schemas in `docs/schemas/` defines an error shape. Contradicts AGENTS.md
  ("Every command keeps a `--json` contract with structured errors").

### JSONPath layer

- **I15 — Criteria and selectors always build located paths and JSON Pointer strings they discard;
  chained `..` exhausts memory.** Performance / resources · **REPRODUCED**.
  `crates/arazzo-expr/src/jsonpath.rs:148-159` (`query` always runs `query_located` +
  `to_json_pointer()`), consumed by `runtime_core/jsonpath.rs:31` (`!nodes.is_empty()`) and `select()`
  (`:166-175`). A 100-deep object (~700 B, under the depth limit) with `$..*..*..*..*` (13 B): 34 s and
  11.2 GB RSS for 3.9M nodes; six segments reached 37 GB. The upstream unlocated `JsonPath::query` on
  the same input: 0.65 s, 35 MB.
- **I16 — `match()`/`search()` recompile their regex for every node, and keep compiling after the first
  recorded failure.** Performance · **REPRODUCED**. `crates/arazzo-expr/src/jsonpath/iregexp_adapter.rs:109-124`
  (`match IRegexp::compile(pattern, mode)` per call). Release: an email pattern costs ~34 µs/node;
  `.{1,300}` ~1 ms/node; an over-limit `.{1,5000}` ~4.3 ms/node, failing identically on every node.
  This is a third regex owner with a third caching policy, beside `RegexCache` (`criteria.rs:3-28`) and
  `matches_operator.rs`.
- **I17 — A JSONPath criterion or selector over a non-JSON body evaluates against the raw text.**
  Spec conformance · **REPRODUCED**. `crates/arazzo-runtime/src/runtime_core/criteria.rs:77-86` checks
  only `is_null()`; `state.rs:253-256` stores non-JSON bodies as `Value::String`. Spec
  (§5.8.11.4.5 Evaluation Errors): "Invalid context (e.g., applying JSONPath to non-JSON data)" means the
  condition "MUST evaluate to fail". Against an empty `text/plain` body, `condition: $` passes and a
  selector output `$` returns `""`, with no warning. Not in the conformance audit.
- **I18 — The "context did not resolve" hard failure depends on whether the evaluator happened to warn.**
  Error handling · **REPRODUCED**. `crates/arazzo-runtime/src/runtime_core/payload.rs:167-177`
  (`if !warnings.is_empty() { return Resolution { value: Value::Null, … jsonpath_failed: … } }`), against
  the promise at `:30-38`. `$inputs.doc.missing` (resolved via `unwrap_or(Null)` at
  `arazzo-expr/src/lib.rs:202`) and `$response.body` before any response return Null with no warning,
  `$` over Null matches once, and the replacement writes `"b": null` silently. Only `$inputs.absent`
  is tested (`payload.rs:1200`).
- **I19 — Non-string YAML payload keys collapse to `""`; tagged and NaN values become null.**
  Error handling · **REPRODUCED**. `crates/arazzo-runtime/src/runtime_core/payload.rs:118-119`
  (`let key = k.as_str().unwrap_or_default().to_string();`), `:79-88`, `:131`. Payload keys `200: ok`,
  `404: not-found`, `true: yes-flag`, `ratio: .nan` pass `--strict validate` and serialize as
  `{"": "yes-flag", "name": "kept", "ratio": null}` — two keys overwritten, no warning.

### Expression evaluator

- **I20 — Numeric equality goes through f64 in both condition engines, so distinct 64-bit IDs compare equal.**
  Correctness · **REPRODUCED**. Simple conditions: `crates/arazzo-expr/src/lib.rs:608-609`, `:618-637`
  (`diff <= f64::EPSILON * a.abs().max(b.abs()).max(1.0)`) — with body `bigid = 10000000000000000`,
  `$response.body.bigid == 10000000000000002` **passes**; 2^53+1 == 2^53. JSONPath filters: vendored
  `vendor/serde_json_path_core/src/spec/selector/filter.rs:273-283` (`l.as_f64() == r.as_f64()`) —
  `$[?@.id == 1234567890123456789].name` selects ids …788, …789 and …800; `number_less_than`
  (`:361-371`) likewise. Snowflake-style IDs cannot be told apart. Owned for simple conditions by
  ac-60b0b / ac-ec3a1; the JSONPath half is unowned.
- **I21 — Outer-paren stripping ignores quotes, so grouped comparisons with `(`/`)` in a string literal
  evaluate wrong.** Correctness · **REPRODUCED**. `crates/arazzo-expr/src/simple_condition.rs:170-187`
  has no quote state (unlike `split_outside_quotes`, `:203-214`). With body `paren = ")"`, both
  `($response.body.paren == ')')` and `!($response.body.paren != ')')` fail the step.
- **I22 — Resolution failures under `$request`, `$response` and nested `$inputs` paths produce no warning.**
  Error handling · **REPRODUCED**. `crates/arazzo-expr/src/lib.rs:385-420` (`resolve_request` /
  `resolve_response` take no warnings sink), `:684`, `:688` (`.ok()`), `:202`, `:841`; `PathError`
  (`:44-60`) is `pub` but no public function returns it. `$response.body.stauts == 'ok'` fails the step
  with no `warnings` in `--json`, while `$inputs.missng == 'ok'` does warn. Silent Null for
  `$response.body#/stauts`, `$response.body.items[0` (unclosed), `$response.header.X-Missing`,
  `$response.bogus`, `$request.bogus`, `$inputs.obj.missing`; `$statusCode.anything` returns 200.
- **I23 — String literals: `''` escaping is unsupported (spec MUST), and a non-spec backslash escape can
  swallow the operator.** Spec conformance · **REPRODUCED**. `crates/arazzo-expr/src/lib.rs:584-591`;
  splitters at `simple_condition.rs:204`, `:210`, `:262`, `:268`, `lib.rs:548`, `:554`. Spec
  §5.8.11.1: *"escape the literal single quote using an additional single quote (’')"*.
  `$response.body.name == 'It''s'` fails; `'C:\' == 'x'` and `'a\' != 'a\'` are **true** (no operator
  found outside quotes → truthy bare word). Also accepts non-JSON numbers `+200`, `0200`, `2e2`. Owned by
  ac-67bf5; not in the conformance audit.
- **I24 — A standalone operand is judged by truthiness, so `null`, a bare `!`, `'false'` and any bare
  word pass.** Correctness · **REPRODUCED**. `crates/arazzo-expr/src/simple_condition.rs:93-96`
  (`if op.is_empty() { … return (is_truthy(&val), warnings); }`), `:77-81`, `lib.rs:595`. Conditions
  `null` and `!` both mark the step successful. Pinned at `simple_condition.rs:576`
  (`assert!(eval.evaluate_condition("just a string"))`). Owned by ac-60b0b.

### Engine

- **I25 — `run --step X` reports success without running X.** Correctness · **REPRODUCED**.
  `crates/arazzo-runtime/src/runtime_core/engine_impl.rs:344-361` (`Done` and a goto past the end both
  `break`), then `:426-427` returns `Ok(vars.step_outputs(step_id))`. If a dependency's `onSuccess` is
  `end`, the command returns `kind: success` with `{}` and the target's DELETE is never sent. The loop is
  a second copy of `execute_inner` that also skips the debug gate, before/after-step events and
  `WorkflowCompleted`.
- **I26 — A workflow step's declared `outputs` are ignored; every child output is copied in instead.**
  Spec conformance · **REPRODUCED**. `engine_impl.rs:771-795` never evaluates `step.outputs` for workflow
  steps; `:985-987` copies child outputs wholesale. `outputs: {renamed: $workflows.child.outputs.method}`
  → `null` with a warning, while the undeclared `$steps.call.outputs.method` → `"GET"`. Not in the
  conformance audit.
- **I27 — The root cause of a transport error is discarded, so a timeout and a refused connection are
  indistinguishable.** Error handling · **REPRODUCED**. `client.rs:538-547` attaches the reqwest error
  as `source`; `engine_impl.rs:525-535` (and `:292-302`, `engine_parallel.rs:84-94`) keep only
  `message`/`kind`, and `control.rs:19` rebuilds with `RuntimeError::new`. `--http-timeout 200ms`
  against a 1 s endpoint and a refused port both print "executing request: error sending request for
  url (…)" / `RUNTIME_HTTP_REQUEST`; `Error::source()` on any step error is `None`. The `with_source`
  wrapping for `SubWorkflowFailed`/`RetryReferenceFailed` (`engine_impl.rs:868`, `:903`, `:978`) is
  stripped one frame later. Probably in scope for ac-8a804.
- **I28 — Replay cannot reproduce a transport failure.** Correctness · **REPRODUCED**. A failed attempt
  is traced with `StepTraceData::default()` (`request: None`, `engine_impl.rs:533`); `replay.rs:27-29`
  skips records with no request. Live: `RUNTIME_HTTP_REQUEST`; replay of that trace:
  `RUNTIME_REPLAY_TRACE_EXHAUSTED`. In a retry chain, replay hands attempt 1 the second recorded
  response. The replay branch for a recorded error (`client.rs:776-784`) is unreachable from a live trace.
- **I29 — Replay decodes response bodies by a different rule than a live run.** Correctness ·
  **REPRODUCED**. Live tries JSON on any non-XML body (`client.rs:715-719`); replay only when the recorded
  type is `Json` (`:802-805`). A `text/plain` JSON body gives output `id: 7` live and `id: null` on
  replay; both exit 0.
- **I30 — A path template with no matching parameter is sent literally.** Correctness · **REPRODUCED**.
  `crates/arazzo-runtime/src/runtime_core/url.rs:178-179`
  (`out.push_str(&path[template.start..template.end])`). `DELETE /echo/{memberId}` with no `memberId`
  parameter is sent as `DELETE /echo/%7BmemberId%7D`, no warning, strict validate passes. Sibling of H24
  (declared but null) on a different code path; also applies to paths resolved from OpenAPI.
- **I31 — Parallel mode drops a failing step's own buffered events, including the cleartext-credential
  warning.** Observability / security posture · **REPRODUCED**.
  `crates/arazzo-runtime/src/runtime_core/engine_parallel.rs:273-281`:

  ```rust
  rx.close();
  while let Some(event) = rx.recv().await { events.push(event); }
  let execution = execution?;   // events dropped on Err
  ```

  One step sending `Authorization` to an unresolvable `http://` host: sequential run →
  `transportWarnings=1` in `--json` and the trace; `--parallel` → 0. The warning is latched once per host
  (`note_cleartext_warned`), so it never fires again for that host. Distinct from H12 (later siblings).

### Spec model

- **I32 — The model requires `sourceDescriptions[].type`, which the spec leaves optional.** Spec
  conformance · **REPRODUCED**. `crates/arazzo-spec/src/lib.rs:193-194` (`#[serde(rename = "type")] pub
  type_: SourceType,` with no default). In the Source Description Object's fixed fields, `name` and
  `url` are marked "REQUIRED" and `type` is not (*"The type of source description. Possible values are
  "openapi" or "asyncapi" or "arazzo""*). A document without `type` fails to parse:
  `sourceDescriptions[0]: missing field `type` at line 4 column 5`. Introduced by 0dea326; not in the
  conformance audit.

### Debug adapter

- **I33 — Breakpoints are not tied to a file, and each `setBreakpoints` replaces the session's only
  source index.** Correctness · **REPRODUCED**. `crates/arazzo-debug-adapter/src/dap/handlers.rs:122-128`,
  `dap/variables.rs:65-104`, `dap/source_index.rs:181-183`. Launch `simple.arazzo.yaml`, set a
  breakpoint in `other.arazzo.yaml` line 11: the session stops in `simple` and the stack trace reports
  `{"line": 11, "source": {"path": ".../simple.arazzo.yaml"}}` — line 11 there is `steps:`.
- **I34 — Breakpoints in called sub-workflows are moved onto the launched workflow, and the entry
  workflow is picked alphabetically.** Correctness · **REPRODUCED**. `dap/source_index.rs:235-237`,
  `dap/session.rs:188-201`, `:394-405`. With `workflowId: main`, a breakpoint on `sub::leaf` is
  "mapped line 22 to step on line 12" (`main::call-sub`), so sub-workflows cannot be broken into. With no
  `workflowId` (the extension's default), document order `zeta` → calls `alpha`: the adapter ran `alpha`
  directly as the entry; `zeta` never ran.
- **I35 — The source index assumes `workflowId`/`stepId` precede their sections and matches section keys
  at any depth.** Correctness · **REPRODUCED**. `dap/source_index.rs:537-558`, `:562-571`, `:673-680`,
  `:897-903`. `successCriteria` before `stepId` → breakpoint "verified" but never stops; `workflowId`
  after `steps` → `RUNTIME_WORKFLOW_NOT_FOUND: workflow "" not found`; a request payload containing an
  `outputs:` key yields "verified" breakpoints that never fire. Key-sorting formatters produce the
  failing orders.
- **I36 — Any handler error kills the adapter, and `continue` reports success before failing.** Error
  handling · **REPRODUCED**. `dap/handlers.rs:88`, `:252-264` (also next/stepIn/stepOut/pause),
  `dap.rs:101-111`, `transport.rs:51-62`:

  ```rust
  write_dap_message(writer, &response)?;   // success:true already sent
  runtime.controller.continue_execution().map_err(|err| format!("continuing runtime: {err}"))?;
  ```

  `launch {}` → no response, exit 1. `continue` after the run ended → `success: true`, then exit 1
  (`debug controller is not paused`). Any unparseable message ends the process.
- **I37 — Closing stdin while paused leaves the adapter running forever.** Resources · **REPRODUCED**.
  `crates/arazzo-debug-adapter/src/dap.rs:75-88`: on EOF the loop exits only once `runtime.terminated`
  is set, and nothing releases the paused engine. Alive at +12 s. H13 is the `disconnect` path; this is
  the EOF path.

### Generator

- **I38 — `generate` emits duplicate workflowIds when two paths share a last segment.** Correctness ·
  **REPRODUCED**. `crates/arazzo-generate/src/crud.rs:351-358`, `:687`. `/v1/items` and `/v2/items` both
  become `crud-items`; the generated document for the real `stripe.yaml` fixture fails validation with
  four duplicates.
- **I39 — Path templates are parsed as if each filled a whole segment.** Correctness · **REPRODUCED**.
  `crud.rs:326-339`, `:351-358` (`if last.starts_with('{') && last.ends_with('}')`). OAS 3.2 allows
  `path-segment = 1*( path-literal / template-expression )`. Twilio's
  `/Accounts/{AccountSid}{mediaTypeExtension}` produces an input named
  `AccountSid}{mediaTypeExtension`; the dry-run URL keeps the braces. `/…/querybuilder.json` yields step
  `create-querybuilder.json`, whose `$steps.…` reference then fails validation. `/items/{id}.json` and
  `/orders/prefix-{id}` are dropped with no warning.
- **I40 — Dotted names: a generated credential header comes out empty, and dotted input names are
  unreachable.** Security / spec conformance · **REPRODUCED**. `crud.rs:537`, `:540`
  (`param_value_expr: format!("$inputs.{scheme_name}")`); evaluator at `crates/arazzo-expr/src/lib.rs:188-202`.
  The ABNF allows `.` in `identifier` and OAS component keys allow `.`, so a scheme named `api.key` is
  valid. The generated document passes `--strict validate`; `run -i api.key=SECRET --dry-run` sends
  `"X-Key": ""` and warns `input "api" not found`. By contrast `$steps.s1.outputs.user.name` resolves
  the literal key `user.name` — the evaluator treats inputs and outputs differently. Owned (evaluator
  side) by ac-9eaf1.

### Build and test suite

- **I41 — The CI "MSRV (1.88)" job has never compiled with its MSRV toolchain.** Build · **Code-verified,
  CI logs read.** `.github/workflows/ci.yml:132-153` installs 1.88 via `dtolnay/rust-toolchain@1.88`, but
  `rust-toolchain.toml` (`channel = "1.98.1"`) overrides it for every `cargo` call in the checkout. The
  job log for run 35375553540 says the 1.98.1 toolchain "is currently in use (overridden by
  …/rust-toolchain.toml)"; the earlier "MSRV (1.82)" runs show the same with `stable`. This is how the
  1.82 claim drifted (6ee8cc7). The 1.88 claim does hold today (`cargo +1.88.0 check --workspace
  --all-targets --locked` exits 0), but only clippy's `incompatible_msrv` guards it, and that lint does
  not see dependency MSRV bumps.
- **I42 — Two test files hang instead of failing.** Testing · **REPRODUCED**.
  (a) `crates/arazzo-runtime/tests/debug_checkpoints.rs:127-145` (and 22 `wait_for_stop` sites across the
  four `debug_*.rs` files): an assertion that fails while the engine is paused drops the tokio runtime,
  whose shutdown waits forever on the paused `spawn_blocking` gate (`engine_trace.rs:323-324`,
  `controller.rs:298`). (b) `crates/arazzo-runtime/tests/header_field_lines.rs:38` (`server.recv()` with
  no timeout), `:67`, `:117` (run result discarded). Under a dead proxy, both ran >60 s until killed. CI
  would show only "has been running for over 60 seconds" until the 20-minute job timeout (`ci.yml:55`).
- **I43 — Suite results depend on the developer's proxy settings and on an untracked `.env`.**
  Hermeticity · **REPRODUCED** (proxy), code-verified (`.env`). No test clears `HTTP_PROXY`, and the
  runtime client applies the system proxy to `127.0.0.1`. `jsonpath_semantics`: 13/0 normally; 1 passed /
  12 failed with `HTTP_PROXY=http://127.0.0.1:9`; 13/0 again with `NO_PROXY=127.0.0.1,localhost`. Across
  all 51 integration binaries: 204 failures in 25 binaries, 2 hangs (I42). CLI children run with
  `current_dir(repo_root())` (`cli_jsonpath_gjson.rs:88`, `cli_jsonpath_migration.rs:107`,
  `golden_spec_baseline.rs:138`, `arazzo_1_1_conformance_followups.rs:60`, `cli_contract_snapshots.rs:39`)
  and the CLI loads `.env` from its cwd; `.env` is not in `.gitignore`. CI's network-namespace guard
  (`ci.yml:80-84`) cannot catch either.
- **I44 — Failure-path tests pass even when no response ever arrives.** Testing · **REPRODUCED**.
  `jsonpath_semantics.rs:206-216`, `xpath_semantics.rs:82-104` (whose comment claims it proves "the
  failure comes from version rejection, not from evaluation"), `engine_soap.rs:422-455`,
  `mcp_integration.rs:466`: all assert only `is_err()`, and all pass under the dead proxy where no HTTP
  response exists. Same pattern, code-verified: the failure half of
  `default_and_explicit_rfc9535_execute_identically` (`jsonpath_semantics.rs:328-336`, manifest
  evidence), `:271-285`, `:305-309`.
- **I45 — "Covered" conformance claims are evidenced by tests that assert known spec violations.**
  Testing / evidence integrity · **Code-verified.** `crates/arazzo-cli/tests/conformance/expressions.json:46-66`
  (claim `expr.decimal-retry-after-and-case-insensitive-simple-comparison`, `covered`) → adapter
  `arazzo-expr/src/lib.rs:14-21` → `simple_condition.rs:458-463`, which asserts
  `"10" < "2"` (H21) and `!($inputs.none == null)` (H4) — against the same §5.8.11.4.1 paragraph the
  claim cites. The adapter also runs the non-spec `contains`/`matches`/`in` tests (conformance F12).
  `expressions.json:30` lists `runtime_expression_gjson_dot_path_extension_is_unchanged` (asserts the F13
  extension works) as positive evidence for the RFC 9535 claim. Fixing H4, H21 or F13 as the spec
  requires turns a "covered" claim red.
- **I46 — The manifest's evidence scanner reads a Rust lifetime as a character literal and stops seeing
  tests.** Testing (drift guard) · **REPRODUCED** (verbatim scanner copy in a scratch binary).
  `crates/arazzo-cli/tests/conformance_manifest.rs:401-405` (`if input[cursor] == b'\'' { quote =
  Some(b'\''); … }`). Recognized/actual tests: `cli_integration.rs` 47/120, `jsonpath_backend.rs` 0/17,
  `mcp_integration.rs` 0/16, `validation.rs` 0/7, `simple_condition.rs` 0/21, `arazzo-expr/src/lib.rs`
  2/~53. It fails closed (no phantom tests), but authors already work around it
  (`cli_jsonpath_gjson.rs:141-145`), and the crate-root adapters that bundle I45's deviation-pinning
  tests exist because of it (F15). Its fixture test (`:1486`) has no lifetime case.
- **I47 — MCP tests use fixed temp-file names that race across concurrent runs.** Hermeticity ·
  **REPRODUCED**. `crates/arazzo-mcp/tests/mcp_integration.rs:319-320`, `:419-420`
  (`temp_dir.join("arazzo_mcp_test_validate.arazzo.yaml")`), removed only on success. Three concurrent
  loops × 150 runs of `test_validate_spec`: 8/450 failures (`No such file or directory` at `:447`,
  `valid == true` at `:358`). Two agents running `cargo test` at once is enough.

---

## Low

- **I48 — MCP `initialized` is written, never read.** `crates/arazzo-mcp/src/protocol.rs:213`, `:245-246`.
  `tools/call` is served before `initialize`; the flag implies lifecycle enforcement that does not exist
  (likely in scope for ac-23ae7).
- **I49 — `--dir` discovery silently skips symlinked spec files.** `crates/arazzo-mcp/src/state.rs:159-176`
  (`entry.file_type()` does not follow links; `is_file()` is false for a symlink). A symlinked
  `linked.arazzo.yaml` → `list_workflows` returns `[]` with no message (REPRODUCED). Non-UTF-8 paths are
  dropped the same way (`to_str()`).
- **I50 — `--allowed-dir` help understates what it gates.** `cli.rs:353`, `arazzo-mcp/src/main.rs:22` say
  "validate_spec"; the gate also covers `generate_workflow`, `describe_openapi` and `run_workflow`'s
  relative sources (`mcp/handlers.rs:263`, `:423`, `:475`).
- **I51 — `validate --strict` does not compile `type: regex` conditions.** `condition: "(20"` validates;
  at run time it silently evaluates false (the G4 path). REPRODUCED.
- **I52 — `RUNTIME_*` codes are applied inconsistently (a JSON contract).** Unrepresentable `retryAfter`
  (`1.0e300`) → `RUNTIME_INPUT_VALIDATION` in a workflow with no inputs (`engine_actions.rs:843-849`);
  illegal header value → `RUNTIME_HTTP_REQUEST` (`client.rs:501-505`) while comparable cases use
  `RUNTIME_INVALID_PARAMETER_VALUE`; `RUNTIME_MAX_CALL_DEPTH_EXCEEDED` is unreachable through
  workflow-step recursion (rewrapped as `RUNTIME_SUB_WORKFLOW_FAILED`, `engine_impl.rs:973-979`); the
  unused `From<reqwest::Error>` maps timeouts to `RUNTIME_EXECUTION_TIMEOUT` (`error.rs:170-178`);
  `STEP_MISSING_DEPENDENCY` lacks the prefix (`error.rs:107`); `RUNTIME_RATE_LIMITER_LOCK_POISONED` and
  `RUNTIME_UNSPECIFIED` are never constructed. REPRODUCED where noted.
- **I53 — `--dry-run` accepts requests the live run refuses.** Dry-run exits at `engine_http.rs:487-500`,
  before the client's header/method/URL checks (`client.rs:470-507`); a header value containing `\n` is a
  valid dry-run request and a live failure. REPRODUCED.
- **I54 — Non-ASCII response header values silently become `""`.** `client.rs:646`
  (`v.to_str().unwrap_or_default()`); `X-Name: café` → `$response.header.X-Name == ""`. REPRODUCED.
- **I55 — Response state is deep-copied several times per step, and more under a debugger.**
  `state.rs:253-256` clones `body_json` (or the whole body) per `make_eval_context`, 2–3× per step with
  actions (`engine_http.rs:622`, `engine_actions.rs:228`, `:337`/`:400`); XPath reparses per
  criterion/output; `execute_inner` clones the `Workflow` (`:453`). With a debugger attached, every
  checkpoint copies body, steps map and outputs even with no breakpoints (`engine_trace.rs:299-316`,
  `:658-659` clones `raw` and never uses it), and hover/evaluate results accumulate in the variable store
  until the next stop (`dap/variables.rs:203-218`).
- **I56 — Timeout watchdog tasks outlive the execution.** `engine_impl.rs:119-123` spawns a sleep for the
  full timeout with no link to completion; copy in `arazzo-cli/src/handlers.rs:150-157`. In the MCP
  server each run leaves a timer alive for up to 300 s.
- **I57 — Integration tests read `CARGO_BIN_EXE_*` at runtime and fall back to a hard-coded
  `target/debug` path.** 8 files use `std::env::var` (e.g. `schema_drift.rs:7`,
  `golden_spec_baseline.rs:42`), 2 use `env!`. Cargo 1.88 does not set the runtime variable (scratch
  crate, REPRODUCED); the fallback ignores `CARGO_TARGET_DIR` and can test a stale binary. `ci.yml:152`
  works around it.
- **I58 — Unused dependencies.** `arazzo-cli/Cargo.toml:22` (`indexmap`), `:29` (`url`); reqwest's `json`
  feature (`arazzo-runtime/Cargo.toml:18`) is never used. REPRODUCED with `-W unused-crate-dependencies`.
- **I59 — CI supply-chain hygiene.** `ci.yml:166` runs `cargo install cargo-audit` unpinned and without
  `--locked`; actions are pinned by tag, not SHA; `release.yml:14-15` grants `contents: write` at the top
  level, so the build matrix holds it too.
- **I60 — The JSON Pointer reader and the replacement writer disagree on array indexes.**
  `payload.rs:507-514` (`token.parse::<usize>()` accepts `01`, `+2`) vs `payload.rs:221-224`
  (`serde_json::Value::pointer` rejects both). Targets `/items/01` overwrite index 1 while the selector
  `/items/01` matches nothing. REPRODUCED.
- **I61 — Output strings not starting with `$` are silently read as `$response.body.<path>`, and
  `to_json_path`'s branches are dead.** `criteria.rs:230-235`, `payload.rs:531-539`. Outputs
  `items.0.name`, `items.#`, `hello` → `"x"`, `1`, `null`, no warnings, strict validate passes. Not in the
  README extensions table or conformance F10/F13. REPRODUCED.
- **I62 — JSONPath errors are reduced to "success criteria not met" by default.** The
  `invalid JSONPath syntax` detail appears only with `--expr-diagnostics` or `--trace`, contradicting the
  "actionable diagnostic instead of a silent false" promise at `runtime_core/jsonpath.rs:22-26`.
  REPRODUCED. Related to G4 / ac-1946c.
- **I63 — Case-insensitive comparison is lowercasing only, and only when both sides are strings.**
  `simple_condition.rs:361-375`: `STRASSE == 'straße'` false; `true == 'TRUE'` false while
  `'true' == 'TRUE'` true. REPRODUCED.
- **I64 — Six quote-aware scanners, six escape policies, one that ignores quotes.**
  `index_outside_quotes` (`lib.rs:543-565`) and `find_operator` (`simple_condition.rs:251-297`) toggle
  backslash state; `split_outside_quotes` (`:241`) does not; `split_list_elements` (`:330-338`) escapes
  outside quotes; `split_path_segments`/`find_matching_bracket` (`lib.rs:868-916`, `:1083-1122`) escape
  only inside quotes; `is_balanced_outer_parens` has none. I21, I23 and H20 all come from this.
- **I65 — DAP `scopes` ignores `frameId`.** `dap/handlers.rs:210-219`, `dap/variables.rs:125-133`: parent
  frames show the innermost frame's inputs, and each call resets the variable store so earlier
  references point at different data. REPRODUCED.
- **I66 — Payload conversion re-copies every subtree at every nesting level.** `arazzo-spec/src/lib.rs:770-777`
  (`serde_yaml_ng::from_value::<Self>(value.clone())` per level). The same 469 KB payload: validate
  0.51 s at depth 1 → 9.47 s at depth 110; dry-run 0.89 s → 18.80 s. Bounded by the YAML depth cap (128).
  REPRODUCED.
- **I67 — CRUD id guessing is fragile.** `crud.rs:630-631` (`resp.content.get("application/json")?`
  returns from the whole function): a bodiless 200 before a JSON 201 yields `itemId:
  $response.body.itemId`; on petstore, user steps send `create-user.outputs.id` as `{username}`.
- **I68 — Integer example generation overflows.** `examples.rs:121-141`, also via MCP
  `standalone_example.rs:94-108`: `minimum: i64::MAX, exclusiveMinimum: true` panics `attempt to add with
  overflow` in debug and wraps to `i64::MIN` in release. REPRODUCED (debug).
- **I69 — Composite request schemas (`allOf`/`oneOf`/`anyOf`) generate `payload: null` silently.**
  `examples.rs:55` (`_ => Value::Null`); the dry-run POST has no body. REPRODUCED.
- **I70 — Serialization adds empty-string fields to generated documents.** `arazzo-spec/src/lib.rs:294-297`,
  `:521-522`, `:581-582`, `:632-635` (`#[serde(default)]` without `skip_serializing_if`): `context: ''`,
  `reference: ''` on Parameter and Request Body objects (`reference` belongs to the Reusable Object),
  `format: ''`, `description: ''`. REPRODUCED.
- **I71 — Re-serializing `SchemaObject`/`PropertyDef` drops captured JSON Schema keywords.**
  `arazzo-spec/src/lib.rs:62-74`, `:32-35` keep only `x-*` on output. Latent: nothing re-serializes a
  parsed document today, and no test covers the round trip.
- **I72 — Two contradictory OpenAPI version checks.** `arazzo-generate/src/lib.rs:31` only guards string
  versions, so unquoted `openapi: 3.1` passes and `crud.rs:100-114` then warns about "best-effort 3.0
  compatibility"; a `swagger: "2.0"` document fails `missing field openapi` before the Swagger 2 message
  can appear. REPRODUCED.
- **I73 — `relative_document_url` climbs to the filesystem root when paths share only `/`.**
  `crud.rs:209-229`: generating from `~/…` into a temp directory emitted
  `url: ../../../../../../../../Users/<user>/…` — effectively absolute, embeds the username, breaks if
  either file moves. REPRODUCED.
- **I74 — Generated source names violate the identifier pattern.** `crud.rs:653-677`
  (`c.is_alphanumeric()` accepts any Unicode letter; the file-name fallback is not cleaned): "Café
  Überservice" → `café-überservice`; `my api v2.yaml` → `my api v2`. Both fail `--strict validate`.
  REPRODUCED.
- **I75 — Dead and mislabelled format handling.** `examples.rs:253`, `:256` (`password`/`byte` branches
  never match — `openapiv3` parses them as known variants, `schema.rs:772-790`);
  `openapi_describe.rs:160`, `:170` (`format!("{f:?}").to_lowercase()` reports `datetime`, not `date-time`).
- **I76 — The DAP's panic-reporting path cannot run in the shipped build.** `dap/session.rs:262-266`,
  `:330-341` expect a join error on panic; the VSIX bundles a release build with `panic = "abort"`, so an
  engine panic kills the adapter with no DAP message. No test covers `Panicked`.
- **I77 — "Deferred mapping" breakpoints are reported verified but never mapped.**
  `dap/source_index.rs:151-163` returns `verified: true` with no runtime breakpoints; nothing resolves
  them later, and `dap_transcript_breakpoints.rs` pins the behaviour.
- **I78 — Two tests send packets to 192.0.2.1.** `transport_tls.rs:386-449`, `cli_integration.rs:5390-5417`.
  Outbound connection attempts on every run; with a proxy set, the fake `Authorization` goes to the proxy.
- **I79 — Process environment mutated inside multi-threaded test binaries.** `arazzo-expr/src/lib.rs:1278-1281`,
  `cli_integration.rs:2268` (`std::env::set_var("ARAZZO_TEST_CODE", "204")`, never removed, leaks into
  later children; the file already has per-child `run_env` at `:4523-4535`). Both sites stop compiling as
  written under edition 2024.
- **I80 — Reserve-then-release port race.** `http_cancellation.rs:682-690` binds `127.0.0.1:0`, reads the
  address, drops the listener, and expects connection-refused; a concurrent bind can take the port.
- **I81 — Copied test helpers have drifted.** 14 temp-dir helpers with 4 naming schemes (I47, G16 are
  instances); 10 copies of `cli_bin` (I57); 15 `tiny_http` servers across 12 files, some blocking forever
  (I42); the rustls test server twice (`tests/common/mod.rs:305`, `cli_integration.rs:~4930`).
- **I82 — A test named for failure asserts success and pins non-spec syntax.**
  `simple_condition.rs:607-618` `split_outside_quotes_fails_on_escaped_quotes` asserts `true` for a
  double-quoted, backslash-escaped string (spec §5.8.11.1: *"Strings MUST use single quotes"*).
- **I83 — The stderr guard checks less than its name.** `arazzo-validate/tests/no_stderr_printing.rs:41-58`
  (`crate_sources_never_write_to_stderr`) greps only for `eprint!`/`eprintln!`; `writeln!(stderr(), …)`
  or `dbg!` pass.
- **I84 — The one ignored test cannot compile.** The ignored doctest at `arazzo-runtime/src/lib.rs:12`
  calls `arazzo_validate::parse`, which is not a dependency of `arazzo-runtime`.

---

## Re-verification of prior findings

Ticket links are to the owning ticket where one exists.

**Closed since the 2026-09-12 audit**

| ID | Evidence |
|---|---|
| H1 (`&&&` panic) | `split_predicate` is gone; `$[?(@.a &&& @.b)]` fails cleanly with `RUNTIME_SUCCESS_CRITERIA_FAILED`; pinned at `criteria.rs:347-365` ([ac-cda5c](https://sonos.scapedeck.com/docs/ac-tickets/ac-cda5c)). The shrunk `&&&` proptest seed still replays and still panics a reconstruction of the pre-fix splitter. |
| H5 (truthiness vs nodelist) | `runtime_core/jsonpath.rs:31` decides on `!nodes.is_empty()`; `$.flag` over `false` succeeds. Closed as conformance F24. |
| G11 (two JSONPath engines) | Closed for typed JSONPath: all three typed surfaces call `JsonPathQuery::parse`. A second filter engine remains in the F13 dot-path extension (`arazzo-expr/src/lib.rs:693-1122`): `$response.body.items[?(@.a == '1')].name` → `"x"` while the RFC selector `$.items[?@.a == '1'].name` → `null`. |
| F7 (parallel event channel) | `768750d`; `engine_parallel.rs:266-272` drains with a biased `select!`. |
| F8 (`matches` regex) | `291f2ac`; bounded `Arc<Regex>` cache plus `invalid regex:` warning (`matches_operator.rs:100-116`). |
| F19 (cancel/timeout mapping ×3) | `30987dc`; `control::cancellation_error` is the single owner. |

**Changed or corrected**

- **H13** (DAP teardown hang) — **worse than recorded.** Not a one-round-trip race: with a second
  breakpoint later in the same step, `disconnect` hangs the adapter 3/3 — `force_resume` releases the
  current gate and the next gate has no `check_cancelled()` before blocking. I37 is the EOF counterpart.
- **H23** — reachable from an MCP tool argument; aborts the release server (I5).
- **H11** — quadratic, not just unbounded (40k → 0.04 s, 1M → 23.7 s), and reachable from MCP
  `generate_example` (I13). I11 is a separate exponential trigger in the same generator.
- **H31** — mechanism corrected: a multi-line validation error does close its YAML scalar; the real break
  is a non-JSON response body flowing raw through `control.rs:30` into the TAP YAML block (a server can
  inject a column-0 `ok 2 - …` line), and JUnit output fails `xmllint` on control bytes. Owned by
  [ac-e5cd7](https://sonos.scapedeck.com/docs/ac-tickets/ac-e5cd7).
- **H26** — two directory predicates, not three (`test` now uses `discover_specs`). Core still live:
  `catalog` lists `specs/foo.yaml` only while `test` and MCP see only `specs/sub/bar.arazzo.yaml`.
- **H22** — the loop is throttled by the default 10 req/s limiter, not busy; still ends only on
  `RUNTIME_EXECUTION_TIMEOUT`, and the library `execute` has no timeout at all.
- **G4** — partly closed: `974bacb` turns JSONPath errors into `ExpressionWarning`s; regex (`criteria.rs:108-111`)
  and XPath (`:139-141`) errors are still dropped — `type: regex`, `condition: "(20"` leaves no trace
  anywhere. [ac-1946c](https://sonos.scapedeck.com/docs/ac-tickets/ac-1946c).
- **F12** — inline tests moved to `src/tests/`; `lib.rs` is 3,784 lines of production code and
  `collect_diagnostics` is still 653 lines. [epic:ac-6dee4](https://sonos.scapedeck.com/docs/ac-tickets/ac-6dee4).
- **F13** — improved: `arazzo-expr/src/lib.rs` 3,037 → 1,958 lines; the dispatcher 488 → 212 lines.
- **F17** — `output.rs` now has one test; every `emit_*`/`--json` shaping function is still untested.
- **F18** — overstated: regex keys are literal document text and MCP builds a fresh `Engine` per call, so
  the cross-invocation growth premise is false at HEAD.
- **G13** — 11 production `panic!`/`unreachable!` sites, not 9 (adds `arazzo-expr/src/lib.rs:79` and
  `arazzo-validate/src/lib.rs:3007`). **H27** re-exports 23 symbols, not 24.

**Still live** (re-checked at HEAD; reproduced where the original entry was High)

| ID | Note | Owner |
|---|---|---|
| H2 | Reproduced with credential forwarding: `-i env=example.com@127.0.0.1:18731 -H 'Authorization: Bearer …'` delivered the bearer token to the local "attacker" server. | [epic:ac-30ecb](https://sonos.scapedeck.com/docs/ac-tickets/ac-30ecb) reopened, no member ticket |
| H3 | `KEY="` still panics both binaries at `main.rs:286` / `:90`. | none (I6 is a sibling) |
| H4, H21 | `null` literal and ordering; pinned at `simple_condition.rs:463`, `:459` and now used as manifest evidence (I45). | [ac-60b0b](https://sonos.scapedeck.com/docs/ac-tickets/ac-60b0b) |
| H6 | `^{$inputs.want}$` fails where `^ok$` passes. | [ac-50846](https://sonos.scapedeck.com/docs/ac-tickets/ac-50846) |
| H7 | `?api_key=`, `X-Api-Key-V2`, body `api_key`/`private_key` in clear in dry-run and trace. | [ac-b4a51](https://sonos.scapedeck.com/docs/ac-tickets/ac-b4a51) |
| H8 (a, b, c) | Replay drift, transport-error URL (`?token=` twice in trace), DAP Request scope — all reproduced. | (b) [ac-8a804](https://sonos.scapedeck.com/docs/ac-tickets/ac-8a804) |
| H9 | JSON error body `{"api_key":"sk-live-…"}` unredacted. | none |
| H10 | `deps.rs:4` regex unchanged. | [ac-0686b](https://sonos.scapedeck.com/docs/ac-tickets/ac-0686b) |
| H12 | `--parallel`: server saw 3 requests, trace holds 1. | none |
| H14 | `Content-Length: 4611686018427387904` aborts both `serve` and the DAP adapter (exit 134). | MCP: [ac-fc556](https://sonos.scapedeck.com/docs/ac-tickets/ac-fc556); DAP: none |
| H15 | Replay of a just-written authenticated trace → `RUNTIME_REPLAY_REQUEST_MISMATCH`. | [ac-72ba1](https://sonos.scapedeck.com/docs/ac-tickets/ac-72ba1) |
| H16 | `validate_spec` read and echoed a file outside `--dir`. | [ac-950a8](https://sonos.scapedeck.com/docs/ac-tickets/ac-950a8), [ac-452ce](https://sonos.scapedeck.com/docs/ac-tickets/ac-452ce) |
| H17 | Typo'd source-qualified `operationId` and unknown `workflowId` validate, also `--strict`. | [ac-da7a0](https://sonos.scapedeck.com/docs/ac-tickets/ac-da7a0), [ac-fd667](https://sonos.scapedeck.com/docs/ac-tickets/ac-fd667) |
| H18 | One bad component reference hides two other errors. | none ([ac-bfae1](https://sonos.scapedeck.com/docs/ac-tickets/ac-bfae1) keeps the short-circuit) |
| H19 | More forms than recorded: `$statusCode >`, `<> 200`, `!== 200`, `200 = 200`, `== 500 \|\|\| true` all true. | [ac-60b0b](https://sonos.scapedeck.com/docs/ac-tickets/ac-60b0b) |
| H20 | Moved to `simple_condition.rs:251-296`; `items[?(@.a>0)].a == 1` false. | ac-67bf5 |
| H24 | `DELETE /accounts/7/members/` sent, exit 0, warning only in dry-run. | none |
| H25, H28, H29, H30, H32, H33, H34 | Unchanged; H32 and H33 reproduced, H34 via library probe (`mpsc bounded channel requires buffer > 0`). | H28: [ac-e81bc](https://sonos.scapedeck.com/docs/ac-tickets/ac-e81bc) |
| F10, F11, F14 (16 params, 6 `too_many_arguments`), F15, F16 (grown: 122 `Result<_, String>` in 23 files, from 104 in 10), F20, F21, F22 | Unchanged. | F15: [ac-4a7ab](https://sonos.scapedeck.com/docs/ac-tickets/ac-4a7ab); F20: [ac-a2fbd](https://sonos.scapedeck.com/docs/ac-tickets/ac-a2fbd) |
| G3 | `.env` `HTTP_PROXY` routed a live request through a local proxy, and `.env` beat the real environment. | [ac-51491](https://sonos.scapedeck.com/docs/ac-tickets/ac-51491) |
| G5 / F6 | `$USD` still deleted, no warning. | [ac-28ba3](https://sonos.scapedeck.com/docs/ac-tickets/ac-28ba3) |
| G6, G7, G8, G9, G10, G12, G14 (latent), G15, G16 (grown to 6 sites), G17 | Unchanged; G6, G8, G9, G10 reproduced. | G12: [ac-94dc8](https://sonos.scapedeck.com/docs/ac-tickets/ac-94dc8) |

---

## Patterns / meta-smells

- **Untrusted *structure* is treated as a budget.** The 09-12 audit noted document-supplied *numbers*
  used as budgets (H11, H22, H23). This run finds the structural version everywhere: XML nesting and
  entities from the server (I1), condition nesting (I2), JSONPath filter nesting (I3) and descendant
  chains (I15), schema fan-out in `generate` (I11), payload depth (I66). Two of these hold the stack, the
  rest hold the CPU — and every one of them is synchronous work on a tokio worker, so
  `--execution-timeout` cannot interrupt any of them. The JSONPath admission budget exists but measures
  character counts rather than cost.
- **Silent coercion at typed boundaries.** A value changes type or shape with no diagnostic: MCP
  `dry_run` (I4), sub-workflow literals (I8), form payloads (I9), `{$expr}` structures (I10), YAML keys
  (I19), non-ASCII headers (I54), f64 equality (I20), `.env` values (G10). The 09-12 pattern "criterion
  evaluation degrades to false/Null instead of failing" (I17, I18, I22, I24) is the read side of the same
  habit.
- **The conformance evidence system overclaims.** An unenforced field is recorded as "verified present and
  conformant" (I7); a "covered" claim runs tests that assert the violation (I45); the scanner that proves
  evidence exists cannot see most tests in several files (I46); and new spec deviations keep appearing
  outside the conformance audit (I10, I17, I23, I26, I32). The audit AGENTS.md sends every spec change to
  first is itself a place where bugs hide.
- **Still one field, two owners.** Live vs replay body decoding (I29), `--step` vs `execute_inner` (I25),
  goto vs sub-workflow parameter resolution (I8), three regex caches (I16), six quote scanners (I64),
  dry-run vs live request validation (I53), two `.env` loaders (F20 → I6 must be fixed twice). AGENTS.md's
  one-owner rule for `operationPath` has still not generalised.
- **Protocol servers treat bad input as process exit.** MCP (I12) and DAP (I36) both end the process on a
  framing or handler error, and both still allocate straight from `Content-Length` (H14). Neither
  validates arguments against the schema it advertises (I4).
- **The suite hangs rather than fails, and depends on the machine.** I42, I43, I44, I47 — the green
  1104/0 above is conditional on no proxy, no concurrent `cargo test`, and every assertion passing.

---

## Non-findings (checked, no issue)

- **No UTF-8 slicing panic in the evaluator.** Every slice lands on an ASCII delimiter; a grammar-aware
  fuzz of 3.3M cases across 6 entry points (300k debug with `catch_unwind`, 3M release) found 0 panics.
- **Proptest seeds survived the `38b6088` move.** Proptest 1.11 maps `src/simple_condition.rs` →
  `proptest-regressions/simple_condition.txt`; both fuzz seeds replay byte-identical inputs at HEAD and the
  `lib.txt` seed stays with its interpolation test. Only incidental cross-test pairings were lost.
- **The extraction left no dead code and changed no behaviour** (clippy with `-W dead_code
  -W unreachable_pub`; `resolve_body_value` identical to `78b05fe`).
- **JSONPath:** paren nesting at the structural limit and context depth at the admission limit evaluate on
  2 MiB threads; vendored slice/index arithmetic is checked; pointer escaping order is right; the RAII error
  frame restores on unwind and never spans an `.await`; adapter `match`/`search` shadow upstream built-ins;
  Selector zero/one/many normalisation is correct.
- **`catch_unwind` exists only in `iregexp_adapter` tests**, so `panic = "abort"` defeats no production
  catch site.
- **Workflow recursion is bounded** (`MAX_CALL_DEPTH = 10` on all three paths, `Box::pin`-ed);
  retry/backoff arithmetic saturates; `Retry-After: u64::MAX` reaches `tokio::time::sleep`, which clamps.
- **Cancellation** is checked at every loop and decision boundary, with cancellation first in every
  `select!`; reqwest's per-request timeout bounds body streaming.
- **Deep JSON responses** hit serde_json's 128 recursion limit and fall back to raw text — no stack risk.
- **`unsafe`** is forbidden workspace-wide and every crate inherits it; the vendored `serde_json_path_core`
  forbids it too.
- **Duplicate crate versions** are limited to `hashbrown`/`indexmap` v1 via iregexp-rs's build dependency
  and dev-only `getrandom`; the one git dependency is revision-pinned as decided.
- **Cyclic `$ref` in `generate`** is stopped by the visited sets plus the depth limit; output is
  deterministic across runs (stripe, quayio); empty `paths` is a clean error.
- **DAP stdout** is never written on any adapter-reachable path; there is a single controller lock, so
  there is no lock-ordering risk; the source-index parser is iterative.
- **Test ports** are all `127.0.0.1:0`; no test changes the process cwd; no `#[ignore]`d tests beyond I84;
  proptest runs the default 256 cases; all 45 manifest evidence references name tests that ran and passed;
  golden/snapshot guards only rewrite under an explicit variable that CI never sets; G15 did not flake in
  40 runs, 20 of them under CPU load.

---

## Verification commands used

```
cargo clippy --workspace --all-targets --all-features -- -D warnings   # exit 0
cargo test --workspace                                                 # exit 0, 1104 passed, 1 ignored
cargo build -p arazzo-cli --bin arazzo-cli                             # exit 0
cargo build --release -p arazzo-mcp        # scratch CARGO_TARGET_DIR, for I5's abort
arazzo-cli [--strict] validate <doc>
arazzo-cli run <doc> <workflow> [-i k=v] [--dry-run] [--json] [--parallel]
arazzo-mcp <spec> < ndjson-requests
```

Repro fixtures, hostile HTTP servers, the DAP driver, and library probes were written to the session
scratchpad, not to the repository; the working tree is unchanged apart from this file.
