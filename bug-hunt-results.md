# Bug Hunt Results — arazzo-cli

**Date:** 2026-04-20
**Phase:** Bug Finder + Skeptic (combined — no prior results existed)
**Codebase:** arazzo-cli (Rust, `main` branch @ `09a05b7`)

---

## Methodology

Three parallel agents searched the codebase for bugs:
1. **Agent 1** — General bug scan (HTTP/headers, expression eval, parsing)
2. **Agent 2** — Error handling review (panics, swallowed errors, missing context)
3. **Agent 3** — Runtime logic deep-dive (execution flow, retries, criteria, parallelism)

Total bugs reported: **16**
After skeptic review: **1 accepted** (since fixed), **15 disproved**

---

## Skeptic Review

### DISPROVED — Header Case Sensitivity (Agent 1, Bugs 1–3)

| Field | Value |
|---|---|
| Claim | `HeaderName::to_string()` preserves wire casing, causing `key == "set-cookie"` and `headers.get("content-type")` to fail on Title-Case headers |
| Score at risk | P1 + P2 + P2 |
| Confidence | 99% |
| Decision | **DISPROVE** |

**Counter-argument:** The `http` crate's `HeaderName::as_str()` is documented as: *"The returned string will always be lower case."* Verified in source at `~/.cargo/registry/src/*/http-1.*/src/header/name.rs`. The `Display` impl calls `as_str()`, so `to_string()` always returns lowercase. The comparison `key == "set-cookie"` and `headers.get("content-type")` are both correct. The fallback `headers.get("Content-Type")` at line 635 is dead code (harmless).

---

### DISPROVED — Silently Ignored Event Sends (Agent 2, Bug 1)

| Field | Value |
|---|---|
| Claim | `let _ = ...` patterns on channel sends silently discard errors |
| Score at risk | P2 |
| Confidence | 95% |
| Decision | **DISPROVE** |

**Counter-argument:** Agent 2 self-acknowledged: "No actual bug here." These are `mpsc::Sender::send()` calls for optional event broadcasting. If the receiver dropped, the workflow is completing anyway. Ignoring the send error is correct.

---

### DISPROVED — Static Regex Panics (Agent 2, Bugs 2–3)

| Field | Value |
|---|---|
| Claim | `Regex::new(...).unwrap()` in `LazyLock` static initialization could panic in production |
| Score at risk | P3 + P2 |
| Confidence | 98% |
| Decision | **DISPROVE** |

**Counter-argument:** These are compile-time constant regex patterns. A panic here indicates a code bug (malformed literal), not a runtime data issue. This is idiomatic Rust — `lazy_static!` / `LazyLock` + `unwrap()` for known-valid patterns is the standard pattern used across the ecosystem. The patterns are validated by every test run.

---

### DISPROVED — ExecutionHandle Double-Consume Panics (Agent 2, Bug 4)

| Field | Value |
|---|---|
| Claim | `collect()` and `result_only()` can panic if called twice |
| Score at risk | P2 |
| Confidence | 99% |
| Decision | **DISPROVE** |

**Counter-argument:** Both methods take `mut self` **by value** (ownership transfer). Rust's ownership system makes double-calling impossible — the first call consumes the handle, and any second call is a compile error. The `#[allow(clippy::missing_panics_doc)]` annotation acknowledges the theoretical panic path is unreachable.

---

### DISPROVED — OnceLock::set() Ignored (Agent 2, Bug 5)

| Field | Value |
|---|---|
| Claim | `let _ = index.op_index.set(idx)` silently ignores failure |
| Score at risk | P3 |
| Confidence | 95% |
| Decision | **DISPROVE** |

**Counter-argument:** `OnceLock::set()` returns `Err` only when already set. This is intentional first-write-wins semantics. The surrounding code comment documents this intent.

---

### DISPROVED — JSONPath Null Context (Agent 3, Bug 1)

| Field | Value |
|---|---|
| Claim | `if context_value.is_null() { false }` should evaluate the condition instead of short-circuiting |
| Score at risk | P2 |
| Confidence | 90% |
| Decision | **DISPROVE** |

**Counter-argument:** A null context means the JSON path expression resolved to nothing. JSONPath cannot evaluate predicates against non-existent data. Returning `false` (criterion not satisfied) is the correct semantic: "this data doesn't exist, so any assertion about it fails." This is consistent with how `evaluate_jsonpath_condition` itself handles empty paths (line 265: `return !context_value.is_null()`).

---

### DISPROVED — Parallel VarStore Atomicity (Agent 3, Bug 2)

| Field | Value |
|---|---|
| Claim | Parallel steps may access stale data due to non-atomic VarStore updates |
| Score at risk | P2 |
| Confidence | 95% |
| Decision | **DISPROVE** |

**Counter-argument:** Parallel execution is level-based (`build_levels` constructs a DAG). All steps in a level complete before the next level starts (line 51: `join_set.join_next().await` drains the level). Steps within a level are independent by definition (same DAG level = no inter-dependencies). Outputs are written back sequentially after the level completes (line 216-218). The design is correct.

---

### DISPROVED — Step Attempt Counter (Agent 3, Bug 4)

| Field | Value |
|---|---|
| Claim | Step attempt counter is not properly isolated per (workflow, step) pair |
| Score at risk | P2 |
| Confidence | 90% |
| Decision | **DISPROVE** |

**Counter-argument:** The `next_attempt` function is only called when tracing is enabled (`self.inner.trace_enabled`, line 222-226) and is purely for trace output annotation. It has zero impact on execution correctness. Even if the counter drifts, the only effect is a cosmetic label in trace files.

---

### DISPROVED — Response Body Off-by-One (Agent 3, Bug 5)

| Field | Value |
|---|---|
| Claim | `body.len() + chunk.len() > max` allows `max + 1` bytes |
| Score at risk | P2 |
| Confidence | 99% |
| Decision | **DISPROVE** |

**Counter-argument:** Math: if `body.len() = 99`, `chunk.len() = 1`, `max = 100`, then `100 > 100 = false` → chunk accepted → body is 100 bytes. If `body.len() = 100`, `chunk.len() = 1`, then `101 > 100 = true` → rejected. The limit allows **exactly** `max` bytes, which is the standard interpretation of a byte limit. The check runs BEFORE extending, preventing any overshoot.

---

### DISPROVED — Retry Count Off-by-One (Agent 3, Bug 6)

| Field | Value |
|---|---|
| Claim | `retryLimit: 3` allows 4 total attempts instead of 3 |
| Score at risk | P2 |
| Confidence | 95% |
| Decision | **DISPROVE** |

**Counter-argument:** The Arazzo 1.0.0 spec (§4.6.6) defines `retryLimit` as *"the number of retry attempts"* — distinct from total executions. With `retryLimit=3`: 1 initial attempt + 3 retries = 4 total executions. The implementation is correct:
1. Step fails → `current=0`, `0 < 3` → Retry → increment to 1
2. Step fails → `current=1`, `1 < 3` → Retry → increment to 2
3. Step fails → `current=2`, `2 < 3` → Retry → increment to 3
4. Step fails → `current=3`, `3 >= 3` → RetryLimitExceeded

---

### DISPROVED — JSON Fallback to String (Agent 3, Bug 7)

| Field | Value |
|---|---|
| Claim | Invalid JSON bodies silently fall back to string without warning |
| Score at risk | P2 |
| Confidence | 95% |
| Decision | **DISPROVE** |

**Counter-argument:** The code has an explicit comment (lines 641-643): *"Intentional: response body may not be valid JSON (e.g. HTML, plain text). We attempt parsing and store None if it fails."* This is a deliberate design decision. Real-world APIs commonly return non-JSON error pages, HTML redirects, or malformed responses. Crashing on parse failure would make the tool less useful.

---

### DISPROVED — VarStore Arc Performance (Agent 3, Bug 8)

| Field | Value |
|---|---|
| Claim | `Arc::make_mut()` causes O(n²) copying in parallel mode |
| Score at risk | P3 |
| Confidence | 90% |
| Decision | **DISPROVE** |

**Counter-argument:** This is a performance observation, not a correctness bug. `Arc::make_mut` is the standard Rust CoW pattern. In practice, after parallel tasks complete and results are written back (single thread, single Arc reference), no cloning occurs. The "O(n²)" claim requires both many parallel steps AND large output maps, which is rare in Arazzo workflows. The design correctly prioritizes correctness (isolation) over micro-optimization.

---

## Final Scorecard

| Metric | Count |
|---|---|
| Total bugs reported | 16 |
| Disproved (false positives) | 15 |
| Accepted (verified bugs) | 1 |
| False positive rate | 94% |

### Verified Bug List

| ID | Severity | File | Description |
|---|---|---|---|
| GOTO-SKIP-1 | P2 | `engine_impl.rs:289-303` | Goto fallback in filtered execution can jump to wrong step — fixed: filtered goto now seeks the first in-scope step at or after the target |

---

## Key Insight

The highest-confidence false positive pattern was the **header case-sensitivity cluster** (Bugs 1–3). The `http` crate's `HeaderName` type normalizes all names to lowercase — a well-documented invariant that the agents failed to check before reporting. This is a common mistake when reviewing Rust HTTP code without understanding the `http` crate's guarantees.
