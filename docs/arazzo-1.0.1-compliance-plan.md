# Arazzo 1.0.1 Full Compliance Plan

## Status snapshot

Compiled from direct source analysis of all crates (March 2026).
The prior roadmap (`docs/roadmap-close-spec-coverage.md`) identified the right categories but was written before several features landed. This document supersedes it.

**Last updated:** 2026-04-26

**Progress:** Phases 1–3 implemented and shipped (`8e7f639`); Phase 6 vendor extension roundtrip shipped (`5846c73`). 3 gaps remaining (down from 7).
Also completed: `serde_yml` → `serde_yaml_ng` migration (`f4f2074`) to clear Dependabot advisories.

---

## Coverage matrix

The table below maps every Arazzo 1.0.1 feature to its current status and the authoritative source location.

| # | Feature | Status | Location |
|---|---------|--------|----------|
| 1 | `arazzo` version field required | Full | `arazzo-validate/src/lib.rs:119` |
| 2 | `info.title` required | Full | `arazzo-validate/src/lib.rs:133` |
| 3 | `info.version` required | Full | `arazzo-validate/src/lib.rs:140` |
| 4 | `info.summary` field | Full | `arazzo-spec/src/lib.rs:48` |
| 5 | `info.description` field | Full | `arazzo-spec/src/lib.rs:50` |
| 6 | `sourceDescriptions[].name` required | Full | `arazzo-validate/src/lib.rs:151` |
| 7 | `sourceDescriptions[].url` required | Full | `arazzo-validate/src/lib.rs:164` |
| 8 | `sourceDescriptions[].type` (openapi / arazzo) | Full | `arazzo-spec/src/lib.rs:56` |
| 9 | `workflow.workflowId` required | Full | `arazzo-validate/src/lib.rs:190` |
| 10 | `workflow.inputs` schema + `$ref` resolution | Full | `arazzo-validate/src/lib.rs:496` |
| 11 | `workflow.steps` — `stepId` required | Full | `arazzo-validate/src/lib.rs:237` |
| 12 | `step.operationId` target | Full | `engine_http.rs:42` |
| 13 | `step.operationPath` target | Full | `engine_http.rs:50` |
| 14 | `step.workflowId` (sub-workflow) target | Full | `engine_impl.rs:558` |
| 15 | One-of target constraint enforced | Full | `arazzo-spec/src/lib.rs:235` |
| 16 | `step.parameters[].in` = path | Full | `engine_http.rs:437` |
| 17 | `step.parameters[].in` = query | Full | `engine_http.rs:441` |
| 18 | `step.parameters[].in` = header | Full | `engine_http.rs:106` |
| 19 | `step.parameters[].in` = cookie | Full | `engine_http.rs:110` |
| 20 | `step.parameters` `$ref` to `components.parameters` | Full | `arazzo-validate/src/lib.rs:584` |
| 21 | `workflow.parameters` inherited by steps | Full | `engine_impl.rs:725` |
| 22 | `step.requestBody.payload` — literal | Full | `engine_http.rs:63` |
| 23 | `step.requestBody.payload` — expression resolution | Full | `helpers.rs:725` |
| 24 | `step.requestBody.payload` — `{$expr}` interpolation | Full | `helpers.rs:725` via `resolve_value` |
| 25 | `step.requestBody.contentType` | Full | `engine_http.rs:98` |
| 26 | **`step.requestBody.replacements`** | **Missing** | Not in spec model or runtime |
| 27 | `step.successCriteria` — `simple` type | Full | `helpers.rs:181` |
| 28 | `step.successCriteria` — `regex` type | Full | `helpers.rs:151` |
| 29 | `step.successCriteria` — `jsonpath` type | Full | `helpers.rs:162` |
| 30 | `step.successCriteria` — `xpath` type | Full | `helpers.rs:169` |
| 31 | `successCriteria.context` expression | Full | `helpers.rs:145` |
| 32 | `successCriteria.type` object form with version | Full | `arazzo-spec/src/lib.rs:356` |
| 33 | `step.onSuccess` / `step.onFailure` — step level | Full | `engine_actions.rs:8` |
| 34 | `workflow.successActions` / `workflow.failureActions` — workflow defaults | Full | `engine_actions.rs:8` |
| 35 | `onSuccess` / `onFailure` — criteria-guarded matching | Full | `engine_actions.rs:163` |
| 36 | Action type `end` | Full | `engine_actions.rs:201` |
| 37 | Action type `goto` (stepId) | Full | `engine_actions.rs:227` |
| 38 | Action type `goto` (workflowId) | Full | `engine_actions.rs:250` |
| 39 | Action type `retry` with `retryLimit` | Full | `engine_actions.rs:271` |
| 40 | Action type `retry` with `retryAfter` | Full | `engine_actions.rs:311` |
| 41 | `Retry-After` response header override | **Done** | `engine_actions.rs:compute_retry_after_delay` (Phase 2, `8e7f639`) |
| 42 | `$ref` to `components.successActions` | Full | `arazzo-validate/src/lib.rs:546` |
| 43 | `$ref` to `components.failureActions` | Full | `arazzo-validate/src/lib.rs:553` |
| 44 | `$ref` to `components.inputs` | Full | `arazzo-validate/src/lib.rs:497` |
| 45 | `step.outputs` expression map | Full | `engine_http.rs:360` |
| 46 | `workflow.outputs` expression map | Full | `engine_impl.rs:708` |
| 47 | `$inputs.name` expression | Full | `arazzo-expr/src/lib.rs:135` |
| 48 | `$inputs.name.sub.path` deep access | Full | `arazzo-expr/src/lib.rs:146` |
| 49 | `$steps.<id>.outputs.<name>` | Full | `arazzo-expr/src/lib.rs:159` |
| 50 | `$env.VAR_NAME` | Full | `arazzo-expr/src/lib.rs:130` |
| 51 | `$statusCode` | Full | `arazzo-expr/src/lib.rs:190` |
| 52 | `$method` | Full | `arazzo-expr/src/lib.rs:195` (EvalContext.method populated at `engine_http.rs:102`) |
| 53 | `$url` | Full | `arazzo-expr/src/lib.rs:203` (EvalContext.url populated at `engine_http.rs:145`) |
| 54 | `$response.header.Name` | Full | `arazzo-expr/src/lib.rs:287` |
| 55 | `$response.body.path` (dot path) | Full | `arazzo-expr/src/lib.rs:292` |
| 56 | `$response.body#/json/pointer` (RFC 6901) | Full | `arazzo-expr/src/lib.rs` — `resolve_body_access` handles `#` prefix |
| 57 | `$request.header.Name` | Full | `arazzo-expr/src/lib.rs:263` (populated at `engine_http.rs:147`) |
| 58 | `$request.query.Name` | Full | `arazzo-expr/src/lib.rs:267` (populated at `engine_http.rs:148`) |
| 59 | `$request.path.Name` | Full | `arazzo-expr/src/lib.rs:272` (populated at `engine_http.rs:149`) |
| 60 | `$request.body` / `$request.body.path` | Full | `arazzo-expr/src/lib.rs:279` (populated at `engine_http.rs:150`) |
| 61 | `$outputs.name` (workflow output accumulation) | Full | `arazzo-expr/src/lib.rs:210` (populated at `engine_impl.rs:719`) |
| 62 | `$outputs.name#/json/pointer` | Full | `arazzo-expr/src/lib.rs:213` |
| 63 | `$sourceDescriptions.{name}.url` | Full | `arazzo-expr/src/lib.rs:226` |
| 64 | `{$expr}` string interpolation | Full | `arazzo-expr/src/lib.rs:94` (via `resolve_value`) |
| 65 | `{sourceName}./path` operationPath routing | Full | `helpers.rs:637` + `engine_http.rs:405` |
| 66 | `//xpath` output expression | Full | `helpers.rs:215` |
| 67 | `||` / `&&` in simple conditions | Full | `arazzo-expr/src/lib.rs:322` |
| 68 | `==`, `!=`, `>`, `<`, `>=`, `<=` comparison | Full | `arazzo-expr/src/lib.rs:388` |
| 69 | `contains`, `matches`, `in` operators | Full | `arazzo-expr/src/lib.rs:419` |
| 70 | `!` NOT operator in simple conditions | **Done** | `arazzo-expr/src/lib.rs:evaluate_condition_with_diagnostics` (Phase 1, `8e7f639`) |
| 71 | `()` grouping in conditions | **Done** | `arazzo-expr/src/lib.rs:split_outside_quotes` + `is_balanced_outer_parens` (Phase 1, `8e7f639`) |
| 72 | `$workflows.<id>.inputs.<name>` | **Done** | `arazzo-expr/src/lib.rs` + `runtime_core.rs:WorkflowEvalState` (Phase 3, `8e7f639`) |
| 73 | `$workflows.<id>.outputs.<name>` | **Done** | `arazzo-expr/src/lib.rs` + `runtime_core.rs:VarStore.workflow_states` (Phase 3, `8e7f639`) |
| 74 | `step.dependsOn` (workflow ordering) | **Missing** | Not in spec model |
| 75 | Parallel step execution (DAG) | Full | `engine_parallel.rs`, `helpers.rs:839` |
| 76 | `--parallel` CLI flag | Full | `arazzo-cli/src/main.rs` |
| 77 | Dry-run mode | Full | `engine_http.rs:163` |
| 78 | Trace artifacts (`trace.v1`) | Full | `engine_trace.rs` |
| 79 | Deterministic trace replay | Full | `runtime_core.rs:472` |
| 80 | Rate limiting (token bucket) | Full | `runtime_core.rs:387` |
| 81 | Input validation (schema) | Full | `input_validation.rs` |
| 82 | `--strict-inputs` | Full | `engine_impl.rs:672` |
| 83 | Sub-workflow execution (up to depth 10) | Full | `engine_impl.rs:599` |
| 84 | Max call depth guard | Full | `engine_impl.rs:302` |
| 85 | Retry loop iteration limit guard | Full | `engine_impl.rs:518` |
| 86 | Cancellation via `CancellationToken` | Full | `runtime_core.rs:328` |
| 87 | Timeout watchdog | Full | `engine_impl.rs:78` |
| 88 | `$ref` cycle / unknown ref error | Full | `arazzo-validate/src/lib.rs:592` |
| 89 | **Vendor extensions (`x-*`) preservation** | Full | `arazzo-spec/src/lib.rs:10` + `serde_roundtrip.rs` (Phase 6, `5846c73`) |
| 90 | `sourceDescriptions[].type` = `arazzo` (Arazzo-as-source) | Partial | Parsed; not used as sub-workflow router at runtime |
| 91 | path percent-encoding (RFC 3986) | Full | `helpers.rs:8` |
| 92 | Response body size guard | Full | `runtime_core.rs:584` |
| 93 | `dependsOn` cycle detection | **Missing** | DAG not implemented for workflow ordering |

**Summary: 85 full, 4 partial, 4 missing.** (was 79/4/10 before phases 1–3)

---

## Gap analysis

### G1 — `requestBody.replacements` (Spec §4.7.5)

The spec defines a `replacements` array on `requestBody`. Each replacement carries:
- `target` — RFC 6901 JSON Pointer (for JSON payloads) or XPath (for XML payloads)
- `value` — expression evaluated at send time

This allows partial overlay of a template body without re-specifying the whole payload. Not modeled in `arazzo-spec` and not handled by `resolve_payload` in `helpers.rs`.

### ~~G2 — `Retry-After` response header override (Spec §4.6.6)~~ ✅ Done (Phase 2, `8e7f639`)

Implemented `compute_retry_after_delay()` in `engine_actions.rs`. Case-insensitive header lookup, integer-seconds parsing, header takes precedence per spec. 5 new tests.

### ~~G3 — `!` NOT operator in simple criterion conditions (Spec §4.8.3)~~ ✅ Done (Phase 1, `8e7f639`)

Implemented in `evaluate_condition_with_diagnostics`. Guards against consuming `!=`. 4 new tests.

### ~~G4 — `()` grouping in simple conditions (Spec §4.8.3)~~ ✅ Done (Phase 1, `8e7f639`)

Paren-aware splitting in `split_outside_quotes` + `is_balanced_outer_parens` helper. 5 new tests.

### ~~G5 — `$workflows.<id>.inputs.<name>` / `$workflows.<id>.outputs.<name>` (Spec §4.9)~~ ✅ Done (Phase 3, `8e7f639`)

`WorkflowEvalState` struct added to `EvalContext`. `VarStore.workflow_states` populated after sub-workflow completion. 5 new tests.

### G6 — `step.dependsOn` (Spec §4.5.4)

The spec defines an optional `dependsOn: [workflow-id, ...]` field on `Workflow` for explicit ordering when running multiple workflows from the same spec without a calling relationship. Not modeled in `arazzo-spec`, not handled by the CLI `run` command.

### ~~G7 — Vendor extensions (`x-*`) roundtrip (Spec §4.1)~~ ✅ Done (Phase 6, `5846c73`)

Implemented `VendorExtensions` with flattened `x-*` preservation across the spec model, including custom `Step` serde and component resolution coverage. Non-`x-*` unknown fields are still dropped.

### G8 — `sourceDescriptions[].type = arazzo` runtime routing

When a source description's type is `arazzo`, the spec allows referencing operations from that linked Arazzo spec using `workflowId` steps that delegate to the sub-spec. The current implementation models the type in the struct but treats all source descriptions as OpenAPI URL sources at runtime.

---

## Phased implementation plan

### Phase 1 — `!` NOT and `()` grouping in conditions ✅ SHIPPED (`8e7f639`)

**Justification:** These are correctness gaps in the expression evaluator. Any LLM-authored spec that writes `!$response.body.error` or groups boolean sub-expressions will silently produce wrong results. The fix is self-contained within `arazzo-expr/src/lib.rs` and has zero runtime coupling.

**Effort: 0.5–1 day**

**Files:**
- `crates/arazzo-expr/src/lib.rs`

**Changes:**

1a. Add `!` NOT prefix handling in `evaluate_condition_with_diagnostics`:

```rust
// At the top of evaluate_condition_with_diagnostics, before || / && split:
if let Some(inner) = condition.strip_prefix('!') {
    let inner = inner.trim_start_matches('(').trim_end_matches(')').trim();
    // but only if the whole suffix is a balanced paren group or a bare expression
    let (result, w) = self.evaluate_condition_with_diagnostics(inner);
    warnings.extend(w);
    return (!result, warnings);
}
```

The correct approach avoids consuming `!=` as a NOT — strip `!` only when the next char is not `=`:
```rust
if condition.starts_with('!') && !condition.starts_with("!=") {
    let inner = condition[1..].trim();
    let (result, w) = self.evaluate_condition_with_diagnostics(inner);
    warnings.extend(w);
    return (!result, warnings);
}
```

1b. Add paren-aware splitting in `split_outside_quotes`:

The function needs to track paren depth so `(a || b) && c` is not split on the `||` inside parens. Replace the current char-by-char loop to also track `depth: usize`, incrementing on `(` and decrementing on `)`, and only record a split when `paren_depth == 0`.

1c. Add grouping resolution in `evaluate_condition_with_diagnostics`:

After the NOT check and before `||`/`&&` splits, detect a fully-parenthesized expression (first char `(`, last char `)`, balanced depth reaches zero only at last char) and strip the outer parens before recursing.

**Test cases (add to `crates/arazzo-expr/src/lib.rs` test module):**

```
!true                           → false
!false                          → true
!$statusCode == 404             → true when status == 200
($statusCode == 200)            → true when status == 200
($statusCode == 200 || $statusCode == 201) && $response.body.id != ""
!($response.body.error)         → true when error is null/false
```

**Integration test:** Add a `testdata/` fixture with a step whose `successCriteria` uses `!($statusCode == 500)` and verify it passes on 200.

---

### Phase 2 — `Retry-After` response header override ✅ SHIPPED (`8e7f639`)

**Justification:** The spec mandates this. Any API that enforces rate limits via `Retry-After` (GitHub, Stripe, OpenAI) will be incorrectly retried with the configured delay instead of the server-dictated one. This is a correctness bug for real-world usage.

**Effort: 0.5 day**

**Files:**
- `crates/arazzo-runtime/src/runtime_core/engine_actions.rs`

**Changes:**

In `execute_action`, `ActionType::Retry` arm, after determining the retry will proceed (line ~311), inspect the response for `Retry-After`:

```rust
// Existing: if action.retry_after > 0 { sleep for action.retry_after }
// New: compute effective_delay first, then sleep

let effective_delay = compute_retry_after_delay(action, ctx.response_headers());
if effective_delay > Duration::ZERO {
    sleep_with_cancel(effective_delay, ctx.cancel, ctx.is_timeout).await?;
}
```

Add a free function `compute_retry_after_delay`:
```rust
fn compute_retry_after_delay(action: &OnAction, headers: Option<&BTreeMap<String, String>>) -> Duration {
    let configured = Duration::from_secs(action.retry_after);
    let Some(hdrs) = headers else { return configured; };
    let raw = hdrs.get("retry-after")
        .or_else(|| hdrs.get("Retry-After"))
        .map(|s| s.as_str())
        .unwrap_or("");
    if raw.is_empty() { return configured; }
    // Integer form: seconds
    if let Ok(secs) = raw.parse::<u64>() {
        return Duration::from_secs(secs).max(configured);
    }
    // HTTP-date form (RFC 7231 §7.1.3): parse and compute delta from now
    // Use `httpdate` crate (already in Cargo if available, otherwise add it)
    // or do a minimal strptime. On parse failure, fall back to configured.
    configured
}
```

The `ExecuteActionContext` needs the response reference. Add `response: Option<&Response>` to `ExecuteActionContext` and thread it from `handle_step_result` (which receives `ctx.result.response`).

**Note on `httpdate` dependency:** The `httpdate` crate parses RFC 7231 date strings. It is a tiny pure-Rust crate (no native deps). If the workspace `Cargo.toml` does not already include it, add `httpdate = "1"` to `arazzo-runtime/Cargo.toml`. Alternatively, for a zero-dep implementation, only the integer form of `Retry-After` needs to be supported initially (the common case for rate-limited APIs).

**Test cases:**
- Response has `Retry-After: 5` → sleep 5 seconds (mock `tokio::time`)
- Response has `Retry-After: 1`, configured `retryAfter: 10` → use max (10s) or header (1s)? — Spec says header takes precedence (use 1s)
- Response has no `Retry-After` → use `action.retry_after`
- Response has malformed `Retry-After` → fall back to `action.retry_after`

Use `tokio::time::pause()` / `tokio::time::advance()` in tests to avoid real sleeps.

---

### Phase 3 — `$workflows.<id>.inputs` and `$workflows.<id>.outputs` ✅ SHIPPED (`8e7f639`)

**Justification:** Complex multi-workflow specs authored by LLMs will use these to pass context between sibling workflows. Without them, all cross-workflow data must flow through sub-workflow parameters, which is more cumbersome and not how the spec intends it.

**Effort: 1–1.5 days**

**Files:**
- `crates/arazzo-expr/src/lib.rs`
- `crates/arazzo-runtime/src/runtime_core.rs`

**Changes:**

3a. Add `workflows` to `EvalContext`:

```rust
// In EvalContext:
pub workflows: BTreeMap<String, WorkflowEvalState>,

#[derive(Debug, Clone, Default)]
pub struct WorkflowEvalState {
    pub inputs: BTreeMap<String, Value>,
    pub outputs: BTreeMap<String, Value>,
}
```

3b. Add `$workflows` evaluation branch in `evaluate_with_diagnostics`:

```rust
"workflows" => {
    // $workflows.<id>.inputs.<name>
    // $workflows.<id>.outputs.<name>
    let after = remainder.unwrap_or("");
    let (wf_id, tail) = match after.split_once('.') {
        Some(pair) => pair,
        None => return (Value::Null, warnings),
    };
    let Some(state) = self.ctx.workflows.get(wf_id) else {
        warnings.push(...);
        return (Value::Null, warnings);
    };
    if let Some(rest) = tail.strip_prefix("inputs.") {
        return (state.inputs.get(rest).cloned().unwrap_or(Value::Null), warnings);
    }
    if let Some(rest) = tail.strip_prefix("outputs.") {
        return (state.outputs.get(rest).cloned().unwrap_or(Value::Null), warnings);
    }
    (Value::Null, warnings)
}
```

3c. Populate in `VarStore`:

Add `workflow_states: BTreeMap<String, WorkflowEvalState>` to `VarStore`. In `eval_context`, copy it to `ctx.workflows`.

3d. Update `execute_inner` to register each completed sub-workflow's state:

After `execute_inner` returns `outputs` for a child workflow, call `vars.register_workflow_state(child_id, child_inputs, outputs)`.

**Test cases:**
- Evaluate `$workflows.auth.outputs.token` after sub-workflow `auth` completes
- Evaluate `$workflows.setup.inputs.env` to read another workflow's declared inputs
- Missing workflow ID → `Value::Null` with warning

---

### Phase 4 — `requestBody.replacements`

**Justification:** This is required for PATCH-style partial updates where a canonical template body exists in components and individual steps overlay only changed fields. Without it, every step must inline its full body. This is the highest-complexity feature and the most niche — most workflows either inline the body or use expression-resolved payload fields.

**Effort: 2–3 days**

**Files:**
- `crates/arazzo-spec/src/lib.rs`
- `crates/arazzo-validate/src/lib.rs`
- `crates/arazzo-runtime/src/runtime_core/helpers.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`

**Changes:**

4a. Add `Replacement` type and `replacements` field to spec model:

```rust
/// A single payload replacement targeting a JSON Pointer or XPath location.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Replacement {
    #[serde(default)]
    pub target: String,   // RFC 6901 JSON Pointer or XPath expression
    #[serde(default)]
    pub value: serde_yaml_ng::Value,  // expression to evaluate
}

// In RequestBody:
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub replacements: Vec<Replacement>,
```

4b. Add validation: `target` must be non-empty, `value` must be non-null.

4c. Add `apply_replacements` in `helpers.rs`:

```rust
pub(super) fn apply_replacements(
    body: Value,
    replacements: &[Replacement],
    eval: &ExpressionEvaluator,
) -> Value {
    let mut out = body;
    for rep in replacements {
        let new_value = eval.resolve_value(&value_to_string_yaml(&rep.value));
        // Parse rep.target as RFC 6901 JSON Pointer
        if rep.target.starts_with('/') {
            apply_json_pointer_replacement(&mut out, &rep.target, new_value);
        }
        // XPath replacement (XML bodies) is out of scope for the first implementation.
        // Log a warning if target looks like XPath and content-type is XML.
    }
    out
}

fn apply_json_pointer_replacement(root: &mut Value, pointer: &str, value: Value) {
    // Walk the pointer segments, creating objects/arrays as needed.
    // Pointer format: /key1/key2/3  (3 is an array index)
    let tokens: Vec<&str> = pointer.trim_start_matches('/').split('/').collect();
    let mut current = root;
    for (i, token) in tokens.iter().enumerate() {
        let token = token.replace("~1", "/").replace("~0", "~");
        let is_last = i == tokens.len() - 1;
        current = match current {
            Value::Object(map) => {
                if is_last {
                    map.insert(token, value);
                    return;
                }
                map.entry(token).or_insert(Value::Object(Default::default()))
            }
            Value::Array(arr) => {
                if let Ok(idx) = token.parse::<usize>() {
                    if is_last && idx < arr.len() {
                        arr[idx] = value;
                        return;
                    }
                    if idx < arr.len() { &mut arr[idx] } else { return; }
                } else { return; }
            }
            _ => return,
        };
    }
}
```

4d. Call `apply_replacements` in `engine_http.rs:prepare_http_request` after `resolve_payload`:

```rust
let mut body_json = if let Some(req_body) = &step.request_body {
    if let Some(payload) = &req_body.payload {
        Some(resolve_payload(payload, &eval))
    } else { None }
} else { None };

if let (Some(body), Some(req_body)) = (&mut body_json, &step.request_body) {
    if !req_body.replacements.is_empty() {
        *body = apply_replacements(body.clone(), &req_body.replacements, &eval);
    }
}
```

**Test cases:**
- JSON pointer `/name` replaces top-level key
- JSON pointer `/address/city` replaces nested key
- JSON pointer `/items/0` replaces first array element
- `~0` and `~1` escape sequences in pointer tokens
- Expression value in replacement (`$inputs.userId`)
- Missing target path → body unchanged (no panic)
- Empty `replacements` array → no change

---

### Phase 5 — `workflow.dependsOn`

**Justification:** Allows the `run-all` use case where a spec defines multiple independent workflows with ordering constraints. Without `dependsOn`, the user must manually run workflows in order. This is the lowest priority because the single-workflow `run <spec> <id>` path already handles the common case, and sub-workflow steps handle the compositional case.

**Effort: 1.5–2 days**

**Files:**
- `crates/arazzo-spec/src/lib.rs`
- `crates/arazzo-validate/src/lib.rs`
- `crates/arazzo-cli/src/main.rs`
- `crates/arazzo-runtime/src/runtime_core.rs`

**Changes:**

5a. Add `depends_on: Vec<String>` to `Workflow` in spec model:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub depends_on: Vec<String>,
```

5b. Add validation: each `dependsOn` entry must reference a known `workflowId`.

5c. Add cycle detection in validation using Kahn's algorithm (same pattern as `build_levels` in `helpers.rs`).

5d. Add `run-all` sub-command (or a `--all` flag on `run`) to the CLI that:
1. Builds the workflow dependency DAG
2. Executes level-by-level in topological order using `build_workflow_levels`
3. Collects outputs from each workflow into a shared map
4. Prints per-workflow results

5e. Add `build_workflow_levels` in a new helper mirroring `build_levels` but operating on `Workflow` objects:

```rust
pub fn build_workflow_levels(spec: &ArazzoSpec) -> Result<Vec<Vec<usize>>, RuntimeError> {
    // Same Kahn's algorithm as build_levels but:
    // - nodes = workflow indices
    // - edges come from dependsOn: [] entries
    // Returns Vec<Vec<usize>> level groups for in-order execution
}
```

**Test cases:**
- Single workflow with no `dependsOn` → level `[[0]]`
- Linear chain: A → B → C → levels `[[0],[1],[2]]`
- Diamond: A → B, A → C, B → D, C → D → levels `[[0],[1,2],[3]]`
- Cycle: A → B → A → error `DependencyCycle`
- Unknown dependency → validation error
- `--all` CLI flag executes in topological order and passes outputs forward

---

### Phase 6 — Vendor extensions (`x-*`) roundtrip ✅ SHIPPED (`5846c73`)

**Justification:** Spec conformance requires preserving `x-*` fields. This is also a prerequisite for the planned `x-arazzo-cli` auth extension (documented in `docs/future/vendor-extensions.md`). The design is fully specified in that document — this phase is purely implementation.

**Effort: 1–2 days**

**Files:**
- `crates/arazzo-spec/src/lib.rs`
- `crates/arazzo-spec/tests/serde_roundtrip.rs`

**Changes shipped:**

1. Added `pub type VendorExtensions = BTreeMap<String, serde_yaml_ng::Value>;`
2. Added flattened, filtered extension maps to the in-scope spec model objects.
3. Updated `StepSerde` and `Step` serialization/deserialization to carry extensions through the custom target handling.
4. Preserved only `x-*` prefixed keys while continuing to drop non-extension unknown fields.
5. Added spec roundtrip, validation/component-resolution, generator, and CLI smoke coverage.

**Test cases:** See `docs/future/vendor-extensions.md` §Test Plan.

---

### Phase 7 — `sourceDescriptions[].type = arazzo` runtime routing (post-core)

**Status:** The type field is parsed correctly. At runtime, when a step uses `workflowId` from an Arazzo-typed source description, the engine should resolve the target workflow from the linked Arazzo spec rather than the current spec. This requires:
- Loading and parsing the linked Arazzo spec at engine init
- Maintaining a second `Engine` instance (or workflow index) for the linked spec
- Routing `StepTarget::WorkflowId` to the correct engine based on source prefix

This is architecturally significant (multi-engine dispatch, cross-spec state). Defer until phases 1–6 are complete.

---

## Dependency map

```
G3 (NOT)   ──────┐
G4 (paren) ───── Phase 1 (self-contained in arazzo-expr)
                  │
G2 (Retry-After) ─── Phase 2 (engine_actions — depends on nothing)
                      │
G5 ($workflows) ─────── Phase 3 (EvalContext + VarStore)
                          │
G1 (replacements) ──── Phase 4 (spec model → validate → helpers → engine_http)
                         │
G6 (dependsOn) ──────── Phase 5 (spec model → validate → CLI)
                          │
G7 (vendor ext) ──────── Phase 6 (spec model only — no runtime coupling)
```

Phases 1 and 2 are fully independent of each other and can be parallelized.
Phase 3 depends on nothing but its own EvalContext extension.
Phase 4 depends on Phase 3 only if replacement `value` expressions should access `$workflows.*` (otherwise independent).
Phase 5 is independent (spec model + CLI — no runtime expression coupling).
Phase 6 is independent of all others (spec model only).

---

## Test plan per phase

All tests must pass the pre-commit gate:
```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

### Phase 1 tests — arazzo-expr

File: `crates/arazzo-expr/src/lib.rs` (existing test module)

| Test name | Assertion |
|-----------|-----------|
| `not_operator_negates_true` | `eval.evaluate_condition("!true") == false` |
| `not_operator_negates_false` | `eval.evaluate_condition("!false") == true` |
| `not_operator_on_expression` | status=404: `!($statusCode == 200) == true` |
| `not_does_not_consume_ne_operator` | `$a != $b` with a!=b → true |
| `paren_grouping_simple` | `($statusCode == 200) == true` |
| `paren_grouping_or` | `($statusCode == 200 \|\| $statusCode == 201)` with 201 → true |
| `paren_grouping_with_and` | `($a == 1 \|\| $b == 2) && $c == 3` |
| `nested_parens` | `(($a == 1)) && $b == 2` |

Fixture test: `testdata/not-operator.arazzo.yaml` — step with `successCriteria: [{condition: "!($statusCode == 500)"}]`, run against mock returning 200, assert pass.

### Phase 2 tests — arazzo-runtime

File: `crates/arazzo-runtime/src/runtime_core/engine_actions.rs` (new test module or integration)

| Test name | Assertion |
|-----------|-----------|
| `retry_after_header_integer_overrides_config` | header `Retry-After: 2`, config `retry_after: 10` → sleep 2s |
| `retry_after_header_respected_when_no_config` | header `Retry-After: 3`, config `retry_after: 0` → sleep 3s |
| `retry_after_config_used_when_no_header` | no header, config `retry_after: 5` → sleep 5s |
| `retry_after_malformed_header_falls_back` | header `Retry-After: not-a-date`, config `retry_after: 4` → sleep 4s |

Use `tokio::time::pause()` to make tests instantaneous.

### Phase 3 tests — arazzo-expr + arazzo-runtime

| Test name | Assertion |
|-----------|-----------|
| `workflows_expression_inputs` | `$workflows.auth.inputs.env` resolves to registered value |
| `workflows_expression_outputs` | `$workflows.auth.outputs.token` resolves after `auth` completes |
| `workflows_unknown_id_is_null` | `$workflows.unknown.outputs.x` → `Value::Null` + warning |
| `workflows_state_available_in_next_wf` | Integration: run two workflows, second reads first's output via `$workflows.*` |

### Phase 4 tests — spec + runtime

| Test name | Assertion |
|-----------|-----------|
| `replacement_top_level_key` | `/name` replaces `{"name":"old"}` → `{"name":"new"}` |
| `replacement_nested_key` | `/a/b` replaces `{"a":{"b":1}}` → `{"a":{"b":99}}` |
| `replacement_array_element` | `/items/0` replaces first item |
| `replacement_pointer_escape_tilde0` | `~0` in token decodes to `~` |
| `replacement_pointer_escape_tilde1` | `~1` in token decodes to `/` |
| `replacement_missing_path_noop` | `/nonexistent/key` → body unchanged, no panic |
| `replacement_expression_value` | value `$inputs.userId` resolves correctly |
| `replacement_empty_list_noop` | no replacements → body unchanged |

### Phase 5 tests — spec + validate + CLI

| Test name | Assertion |
|-----------|-----------|
| `depends_on_single_dep` | workflow B depends on A → levels `[[A],[B]]` |
| `depends_on_chain` | A→B→C → levels `[[A],[B],[C]]` |
| `depends_on_diamond` | A→B, A→C, B→D, C→D → levels `[[A],[B,C],[D]]` |
| `depends_on_cycle_error` | A→B→A → `DependencyCycle` validation error |
| `depends_on_unknown_id_error` | depends on `"ghost"` → `InvalidReference` validation error |

### Phase 6 tests — arazzo-spec

Replace existing `parse_ignores_unknown_fields_and_drops_them_on_serialize` with:

| Test name | Assertion |
|-----------|-----------|
| `parse_preserves_vendor_extensions_on_root` | `x-arazzo-cli` on root survives parse→serialize→parse |
| `parse_preserves_vendor_extensions_nested` | Nested extension points survive roundtrip |
| `parse_preserves_all_extension_value_shapes` | `null`, scalar, array, and object extension values survive |
| `parse_drops_non_vendor_unknown_fields` | `foo: bar` (no `x-` prefix) is dropped |
| `step_custom_serde_preserves_vendor_extensions_and_target` | Step's custom serde path preserves `x-*` and target selection |

---

## Regression risk assessment

| Phase | Risk | Rationale |
|-------|------|-----------|
| 1 | **Low** | Isolated to `arazzo-expr/src/lib.rs`. Additive code paths. Existing proptest fuzzing catches regressions. `!` must guard against consuming `!=`. |
| 2 | **Low** | ~40 lines in one function in `engine_actions.rs`. Additive — only activates when `Retry-After` header present. |
| 3 | **Low** | Adds a new `$workflows` branch in the expression evaluator. The `_` fallback currently returns `Null`, so no existing behavior changes. New `WorkflowEvalState` map in `VarStore` is additive. |
| 4 | **Moderate** | Touches `resolve_payload` in `helpers.rs` — the payload construction hot path. JSON Pointer mutation after expression resolution could interact with existing interpolation. Needs careful ordering: resolve first, then replace. |
| 5 | **Moderate** | Adds a new scheduling layer above the existing workflow execution loop. Cross-workflow state management is new surface area. The parallel step scheduler (Kahn's algorithm) is well-tested and the workflow-level version mirrors it. |
| 6 | **Low** | `serde(flatten)` on structs. Deserialization of existing fields is unaffected. Custom `Step` serde needs careful handling. |

**Safety net:** 339 existing tests (all hermetic) catch regressions. Integration tests in `cli_integration.rs` exercise full end-to-end flows including traces, replays, parallel execution, and sub-workflows. Implementation strategy: ship phases 1-3 first (low risk, high value), run full suite, then proceed to 4-5.

---

## Effort estimates (single maintainer with AI agent)

| Phase | Feature | Complexity | Estimate |
|-------|---------|------------|----------|
| 1 | ~~NOT + grouping operators~~ | ~~Low~~ | ✅ Done |
| 2 | ~~`Retry-After` header~~ | ~~Low~~ | ✅ Done |
| 3 | ~~`$workflows.*` expressions~~ | ~~Medium~~ | ✅ Done |
| 4 | `requestBody.replacements` | High — spec model + runtime + JSON Pointer mutation | 2–3 days |
| 5 | `workflow.dependsOn` | Medium — spec model + DAG + CLI command | 1.5–2 days |
| 6 | ~~Vendor extensions (`x-*`)~~ | ~~Medium — serde flatten + custom Step serde~~ | ✅ Done (`5846c73`) |
| 7 | Arazzo-as-source runtime | Very high — multi-engine dispatch, deferred | — |

**Completed: phases 1–3 and 6.** Remaining for phases 4–5: **3.5–5 days** toward ~100% spec coverage.

---

## Ordering rationale

**Phase 1 first** — The NOT operator and grouping are pure expression correctness. Any spec that writes `!$statusCode == 500` is silently broken today. Zero architectural risk, zero coupling, completely self-contained.

**Phase 2 second** — A 40-line correctness fix for the most common production scenario (rate-limited APIs). `Retry-After` is not optional when targeting APIs like GitHub or OpenAI. The fix does not change any interfaces.

**Phase 3 third** — `$workflows.*` expressions require adding a map to `EvalContext` and threading it through `VarStore`, but it touches no HTTP pipeline code. A prerequisite for workflows that pass state between siblings.

**Phase 4 fourth** — `requestBody.replacements` is the only remaining runtime feature that affects the HTTP dispatch path. It depends on the spec model being complete (phases 1–3 do not change the spec model, so this can be done at any point, but is more impactful after expression correctness is solid).

**Phase 5 fifth** — `dependsOn` is a CLI-level concern, not a runtime-core concern. It requires a new command or flag and touches the spec model. Independent, but lower value than correctness features.

**Phase 6 last** — Vendor extension roundtrip is a correctness-of-spec-preservation concern, not a runtime correctness concern. Most users never observe the difference. The design doc is ready; this is a straightforward implementation task whenever there is capacity.

---

## What is already implemented (audit confirms complete)

These items were listed as missing in the original roadmap but are confirmed present in the current codebase:

- **Cookie parameter dispatch** — `engine_http.rs:110`, cookie params collected and joined as `Cookie` header
- **String interpolation (`{$expr}`)** — `resolve_value` in `arazzo-expr/src/lib.rs:94` dispatches to `interpolate_string` for strings containing `{$`
- **Workflow-level `successActions`/`failureActions`** — `engine_actions.rs:8`, step-empty fallback to workflow-level is fully wired
- **Workflow-level `parameters`** — `engine_impl.rs:725`, `merge_workflow_params` called before every step
- **`$outputs.name`** — `arazzo-expr/src/lib.rs:210`, evaluated from `EvalContext.outputs`
- **`$response.body#/json/pointer`** — `resolve_body_access` handles the `#` separator via `Value::pointer()`
- **`$url`** — `EvalContext.url` populated in `engine_http.rs:145` via `make_post_request_eval_context`
- **`$request.*`** — all four sub-namespaces (header, query, path, body) are in `EvalContext` and populated in `engine_http.rs:147-150`
- **`$sourceDescriptions.{name}.url`** — `arazzo-expr/src/lib.rs:226`, `source_descriptions_map` built at engine init and threaded into context
- **Multiple source descriptions routing** — `engine_http.rs:405`, `parse_source_prefix` routes to per-source base URL
- **`$method`** — `EvalContext.method` populated in `engine_http.rs:102`
