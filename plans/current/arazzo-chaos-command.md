# Plan: `arazzo chaos` Command

## Goal

Add an `arazzo chaos` command that performs fault injection testing on Arazzo workflows. It runs workflows against a live API but intercepts specific steps to inject failures (error status codes, timeouts, malformed responses), then reports whether the workflow's declared error handling (`onFailure`, `retry`, `goto`) actually recovers.

## Side-Effect Warning (READ FIRST)

Chaos runs `1 + N × M` full workflow executions against a **live** API (N = executed steps, M = fault types). Non-targeted steps pass through to the real server on every run. If the workflow performs mutating operations (POST / PUT / DELETE, payments, emails, database writes), chaos mode will repeat every side effect up to `1 + N × M` times per workflow. **Do not point chaos at production.** The CLI must print a prominent warning banner on startup, and the docs must recommend running against a sandbox / stubbed endpoint. This is an operator concern, not a runtime one — the CLI cannot detect idempotency from the spec.

Additionally, auto mode has a scenario budget cap (default 50, configurable via `--max-scenarios`). If a workflow would exceed the cap, the CLI errors out rather than silently truncating — forcing the user to narrow `--faults` or use `--inject`.

## Design Principles

- **Leverage Seam A** — inject faults at the `HttpClientMode` level in `HttpClient::request`, the same integration point used by replay. Zero changes to execution logic above the client.
- **Auto by default** — when no `--inject` is passed, enumerate the steps executed in the baseline × the fault set. Targeted injection via `--inject` for specific scenarios.
- **Reuse `test` infrastructure** — chaos results are a distinct JSON contract (`ChaosOutput`) but share the TAP formatter shape and exit code convention with `test`. (They are not the same JSON envelope — `ChaosOutput` has its own schema because the semantics differ.)
- **Two-pass execution** — first run the workflow normally (baseline). Only then enumerate fault scenarios for steps the baseline actually executed. A workflow that can't pass cleanly is marked `skipped` — we cannot distinguish fault-induced failure from pre-existing brokenness.
- **Scoped to executed steps only** — auto mode enumerates only the steps the baseline reached. Conditional branches (alternate `onSuccess: goto`, divergent paths) that don't fire in the baseline are **not** chaos-tested. Document this as a known limitation; a future enhancement could chaos-test alternate branches by seeding `$inputs`.

---

## 1. CLI Interface

```
arazzo chaos <SPEC> [OPTIONS]
```

### Arguments

| Argument | Type | Description |
|----------|------|-------------|
| `spec` | `String` (positional, required) | Arazzo spec file to chaos-test |

### Options — Fault Configuration

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--workflow` | `Option<String>` | `None` (all workflows) | Specific workflow ID to test |
| `--inject` | `Vec<String>` | `[]` | Targeted fault (repeatable): `step=<id>,status=<code>` or `step=<id>,timeout` or `step=<id>,malformed` |
| `--faults` | `Vec<String>` | `["500", "502", "503", "504", "timeout", "malformed"]` | Fault tokens for auto mode (parsed into `FaultKind`) |

`--auto` is **not** a flag. Auto mode is the default behavior when no `--inject` is passed. Passing both `--inject` and any `--faults` override together is legal (overrides the auto set); the only invalid combination is caught at parse time (see edge cases).

Fault tokens and how each flag consumes them:

| Token (bare, for `--faults`) | Equivalent inside `--inject` (after `step=<id>,`) | `FaultKind` |
|------------------------------|---------------------------------------------------|-------------|
| `timeout` | `timeout` | `FaultKind::Timeout` |
| `malformed` | `malformed` | `FaultKind::MalformedResponse` |
| `<code>` where `code` parses as integer in `400..=599` (e.g. `500`) | `status=<code>` (e.g. `status=503`) | `FaultKind::StatusCode(code)` |

So `--faults 500,timeout,malformed` and `--inject step=pay,status=503` share the same underlying `FaultKind` parser but use flag-specific syntax. Reject anything else at CLI-parse time with a clear error. `FaultKind` itself is **not** a clap `ValueEnum` because it carries data.

### Options — Engine Flags (same as `test`)

| Flag | Type | Default |
|------|------|---------|
| `-i, --input` | `Vec<String>` | `[]` |
| `--input-json` | `Vec<String>` | `[]` |
| `-t, --http-timeout` | `Duration` | `30s` |
| `--execution-timeout` | `Duration` | `5m` |
| `-H, --header` | `Vec<String>` | `[]` |
| `--openapi` | `Vec<String>` | `[]` |
| `--parallel` | `bool` | `false` |
| `--strict-inputs` | `bool` | `false` |
| `--max-response-size` | `Option<usize>` | `None` |
| `--format` | `ChaosFormat {json, tap, table}` | `table` |

### Clap Definition (in `cli.rs`)

```rust
/// Fault injection testing for API workflows
Chaos {
    /// Arazzo spec file
    spec: String,

    /// Specific workflow to test (default: all)
    #[arg(long)]
    workflow: Option<String>,

    /// Targeted fault injection (repeatable).
    /// Format: step=<id>,<fault>  where <fault> is `timeout`, `malformed`,
    /// or `status=<400..=599>`.
    #[arg(long = "inject")]
    inject: Vec<String>,

    /// Fault tokens enumerated in auto mode (ignored when --inject is set).
    /// Accepts: timeout, malformed, or any HTTP status in 400..=599.
    #[arg(long = "faults", value_delimiter = ',')]
    faults: Vec<String>,

    /// Cap on the number of auto-generated scenarios per workflow.
    #[arg(long = "max-scenarios", default_value_t = 50)]
    max_scenarios: usize,

    /// Output format
    #[arg(long, value_enum, default_value_t = ChaosFormat::Table)]
    format: ChaosFormat,

    // ... all engine flags same as test (input, http-timeout, etc.)
},
```

`--faults` is parsed as `Vec<String>` with comma delimiters so both `--faults 500,timeout` and `--faults 500 --faults timeout` work. Tokens are validated and converted to `FaultKind` inside the handler, producing a precise error on bad input (CLI exits 1 with `ChaosOutput::Error` in JSON mode).

---

## 2. Fault Injection Architecture

### Integration Point: `HttpClientMode::Chaos`

File: `crates/arazzo-runtime/src/runtime_core.rs`

Add a third variant to the existing `HttpClientMode` enum (alongside `Live(reqwest::Client)` and `Replay(Arc<Mutex<ReplayState>>)`):

```rust
enum HttpClientMode {
    Live(reqwest::Client),
    Replay(Arc<tokio::sync::Mutex<ReplayState>>),
    Chaos(ChaosState),      // NEW — carries the live client + fault rule
}
```

`ChaosState` pairs the live reqwest client with the active fault rule. It must carry the client because non-targeted steps fall through to real HTTP:

```rust
pub struct ChaosState {
    /// Live client used for non-targeted steps.
    pub client: reqwest::Client,
    /// Which step to fault and how. `None` disables injection (equivalent to Live).
    pub active_fault: Option<ChaosRule>,
}

pub struct ChaosRule {
    pub step_id: String,
    pub fault: FaultKind,
}

pub enum FaultKind {
    StatusCode(i64),        // Return this status with empty JSON body
    Timeout,                // Return Err(RuntimeError { kind: HttpRequest, message: "chaos: connection timeout" })
    MalformedResponse,      // Return status 200 with body "<<<INVALID JSON>>>"
}
```

### How It Works in `HttpClient::request`

File: `crates/arazzo-runtime/src/runtime_core.rs`, around line 536 — the mode branch in `request()`.

**Prerequisite refactor**: today the Live path is inlined directly in `request()` after the `HttpClientMode::Replay` early-return. Introduce a helper `async fn live_request(&self, inner: &reqwest::Client, cfg: RequestConfig, cancel: &CancellationToken, is_timeout: &AtomicBool) -> Result<Response, RuntimeError>` containing the existing inline logic. Both `HttpClientMode::Live` and the fallthrough branch of `HttpClientMode::Chaos` call it. This is a pure extract-method refactor with no behavior change; verify by running the existing runtime tests before adding chaos.

Chaos branch:

```rust
HttpClientMode::Chaos(ref state) => {
    if let Some(rule) = &state.active_fault {
        if cfg.step_id == rule.step_id {
            return match &rule.fault {
                FaultKind::StatusCode(code) => {
                    let mut headers = BTreeMap::new();
                    headers.insert("content-type".to_string(), "application/json".to_string());
                    Ok(Response {
                        status_code: *code,
                        headers,
                        body: br#"{"error":"chaos: injected fault"}"#.to_vec(),
                        body_json: Some(json!({"error": "chaos: injected fault"})),
                        content_type: ContentType::Json,
                    })
                }
                FaultKind::Timeout => Err(RuntimeError::new(
                    RuntimeErrorKind::HttpRequest,
                    "chaos: simulated connection timeout",
                )),
                FaultKind::MalformedResponse => Ok(Response {
                    status_code: 200,
                    headers: BTreeMap::new(),
                    body: b"<<<INVALID JSON>>>".to_vec(),
                    body_json: None,
                    content_type: ContentType::Json,
                }),
            };
        }
    }
    // Not the target step — forward to live client
    self.live_request(&state.client, cfg, cancel, is_timeout).await
}
```

Key: non-targeted steps pass through to the real API. Only the faulted step gets a synthetic response. This means the workflow runs normally up to the fault point, then we observe the error handling.

Note: `RuntimeError::new` takes `impl Into<String>`, so `"literal"` works without `.to_string()`.

### EngineBuilder Addition

File: `crates/arazzo-runtime/src/runtime_core.rs`

```rust
impl EngineBuilder {
    pub fn chaos(mut self, rule: ChaosRule) -> Self {
        self.chaos_rule = Some(rule);
        self
    }
}
```

`HttpClient::new` currently takes `replay_trace_steps: Option<Vec<TraceStepRecord>>`. Extend its signature to also accept `chaos_rule: Option<ChaosRule>` and choose the mode by precedence: Replay > Chaos > Live. In `build()` (around line 1426), pass `self.chaos_rule` through. The Chaos branch constructs the live reqwest client identically to the Live branch and stuffs it into `ChaosState` alongside the rule. Chaos and Replay are mutually exclusive — if both are set, return a `RuntimeError` from `build()`.

### Public API

File: `crates/arazzo-runtime/src/lib.rs`

Export `ChaosRule`, `ChaosState`, `FaultKind` from the runtime crate so the CLI can configure them.

---

## 3. Execution Model

### Two-Pass Strategy

For each workflow × fault combination:

**Pass 1 — Baseline**: Run the workflow normally (no faults). Record:
- Did it pass? (required — can't test fault recovery on a workflow that fails normally)
- Which steps executed and in what order

**Pass 2 — Fault injection**: For each step in the baseline:
- Build a new engine with `ChaosRule { step_id, fault }`
- Run the workflow
- Record the outcome:
  - **Recovered**: workflow still passed (onFailure/retry handled it)
  - **Failed gracefully**: workflow failed but via onFailure `end` action (controlled shutdown)
  - **Crashed**: workflow errored with no handler (unprotected)

### Auto Mode Enumeration

When no `--inject` is passed:
1. Run baseline → collect the ordered set of unique `step_id`s that actually executed (dedup: a step that retried 3 times counts once).
2. Let `S` = executed steps, `F` = fault tokens in `--faults` (default `[500, 502, 503, 504, timeout, malformed]`).
3. Let `total = |S| × |F|`. If `total > --max-scenarios`, error out with a message listing `|S|`, `|F|`, the product, and the cap.
4. Otherwise, for each (step, fault) pair:
   - Build a fresh engine with `.chaos(ChaosRule { step_id, fault })`.
   - Run the workflow.
   - Classify the outcome (see §4).

For a workflow with 3 executed steps and the default 6-fault set, auto mode performs `1 + (3 × 6) = 19` full workflow executions against the live API. Each execution issues every non-faulted step's real HTTP call, so per-scenario request count equals the workflow's step count.

---

## 4. Result Types

### JSON Contract

```rust
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChaosOutput {
    Results {
        summary: ChaosSummary,
        workflows: Vec<ChaosWorkflowResult>,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChaosSummary {
    pub total_scenarios: usize,    // sum of scenarios across all workflows
    pub recovered: usize,          // workflow passed despite fault
    pub graceful: usize,           // workflow failed via onFailure `end` on the faulted step
    pub crashed: usize,            // fault propagated; no onFailure handler or unhandled error
    pub skipped: usize,            // scenario never reached the faulted step
    pub baselines_failed: usize,   // workflows whose baseline itself failed (no scenarios run)
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChaosWorkflowResult {
    pub workflow_id: String,
    pub baseline: ChaosBaseline,
    pub scenarios: Vec<ChaosScenario>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChaosBaseline {
    pub passed: bool,
    pub steps: Vec<String>,     // step IDs in execution order
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChaosScenario {
    pub step_id: String,
    pub fault: String,          // canonical: "status:503" | "timeout" | "malformed"
    pub outcome: ChaosOutcome,
    pub duration_ms: u64,
    pub error: Option<String>,  // runtime error message when outcome == Crashed
    pub decision_path: Option<String>,  // TraceDecisionPath enum value for the faulted step (for debugging)
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChaosOutcome {
    Recovered,    // workflow passed
    Graceful,     // workflow failed via controlled onFailure
    Crashed,      // unhandled error
    Skipped,      // baseline didn't reach this step
}
```

### Outcome Classification

`SuccessCriteriaFailed` by itself does **not** imply graceful shutdown — it just means criteria weren't met. To distinguish Graceful from Crashed we must inspect the trace events. The faulted step's `TraceDecision.path` tells us which onFailure branch fired (`End`, `GotoStep`, `GotoWorkflow`, `Retry`, or unhandled).

```
Classification algorithm:
  let final = ExecutionResult.outputs
  let faulted = last TraceStepRecord where step_id == injected step
  let decision = faulted.decision.path (or None if the step never ran)

  if faulted is None                                          → Skipped
     (baseline executes this step, but this run didn't reach it —
      e.g. an earlier step failed or diverted first)

  if final == Ok(_)                                            → Recovered
     (workflow completed; retry/onFailure branch produced success)

  if final == Err(_) AND decision == End                       → Graceful
     (explicit onFailure action with `end` type fired — controlled shutdown)

  otherwise                                                    → Crashed
     (unhandled fault: SuccessCriteriaFailed with no onFailure,
      network error, etc.)
```

This requires building engines with `.trace(true)` so `TraceStepRecord` events are emitted. The chaos runner consumes `ExecutionResult.trace_steps()` to find the faulted step and read its decision path.

---

## 5. Output Formats

### Table (default, human-readable)

```
⚠ Chaos mode runs this workflow 1 + 7 = 8 times against the configured API.
  Ensure the target is a sandbox, not production.

Workflow: checkout-flow
Baseline: PASS (3 steps, 1.2s)

  Step              Fault       Outcome     Detail
  ──────────────────────────────────────────────────────────────
  check-inventory   status:503  RECOVERED   onFailure → goto confirm-order
  check-inventory   status:500  GRACEFUL    onFailure → end
  check-inventory   timeout     CRASHED     no onFailure handler
  process-payment   status:503  RECOVERED   onFailure → goto retry-with-backup
  process-payment   timeout     CRASHED     no onFailure handler
  confirm-order     status:503  GRACEFUL    onFailure → end
  confirm-order     malformed   CRASHED     success criteria failed, no handler

Resilience: 2/7 recovered, 2/7 graceful, 3/7 crashed
Unprotected steps: check-inventory (timeout), process-payment (timeout), confirm-order (malformed)
```

Note: `RECOVERED` here comes from `onFailure → goto` redirecting past the failure, or from an entirely different step succeeding. It does **not** come from retrying the faulted step — chaos injects deterministically on every attempt of the targeted step, so an isolated retry loop on the faulted step will always exhaust its retry limit.

The fault label serialization is canonical: `status:<code>`, `timeout`, `malformed`. The JSON `fault` field uses the same form, giving table and JSON outputs one shared vocabulary.

### JSON (`--json` or `--format json`)

Emit `ChaosOutput` via `output_json()`.

### TAP (`--format tap`)

Each scenario is one TAP test case. TAP pass/fail maps to outcome:

| Outcome | TAP line |
|---------|----------|
| Recovered | `ok` |
| Graceful | `ok` (workflow's error handling worked as declared) |
| Crashed | `not ok` |
| Skipped | `ok ... # SKIP <reason>` |

Matching the table example above:

```
TAP version 13
1..7
ok 1 - checkout-flow::check-inventory [status:503] recovered
ok 2 - checkout-flow::check-inventory [status:500] graceful
not ok 3 - checkout-flow::check-inventory [timeout] crashed
ok 4 - checkout-flow::process-payment [status:503] recovered
not ok 5 - checkout-flow::process-payment [timeout] crashed
ok 6 - checkout-flow::confirm-order [status:503] graceful
not ok 7 - checkout-flow::confirm-order [malformed] crashed
```

Workflows whose baseline fails emit a single SKIP line that accounts for all enumerated-but-not-run scenarios, e.g.:

```
ok 8 - other-workflow::* [baseline] # SKIP baseline failed, 12 scenarios not run
```

---

## 6. Exit Codes

| Condition | Exit Code |
|-----------|-----------|
| All scenarios recovered or graceful (or all skipped due to baseline failure) | 0 |
| Any scenario crashed | 1 |
| Pre-execution error (parse, bad spec, invalid `--inject`/`--faults` token, `--max-scenarios` exceeded) | 1 |

Note on "all skipped": a workflow whose baseline fails yields zero crashed scenarios, so exit 0 is *technically* clean. The human table output prints a conspicuous `BASELINE FAILED` line for that workflow, and the JSON payload surfaces `baseline.passed = false` — but CI authors who care must gate on `summary.skipped > 0` themselves. This is a deliberate choice (we only fail on evidence of broken error handling, not on a broken baseline).

### `--json` vs `--format`

The global `--json` flag (e.g. `arazzo --json chaos ...`) overrides `--format` to JSON — same semantics as `test`. If a user passes `--json --format table`, JSON wins. Human-readable formatters (`table`, `tap`) run only when `--json` is false AND `--format` is not `json`.

---

## 7. Files to Create / Modify

### Runtime Crate (`arazzo-runtime`)

| File | Change | Lines (est) |
|------|--------|-------------|
| `runtime_core.rs` ~line 446 (`enum HttpClientMode`) | Add `Chaos(ChaosState)` variant | +1 |
| `runtime_core.rs` ~line 498 (`HttpClient::new`) | Accept `chaos_rule: Option<ChaosRule>`; construct Chaos mode when set | +15 |
| `runtime_core.rs` ~line 530 (`HttpClient::request`) | Extract existing Live path into `live_request` helper, add Chaos branch | +45 (including refactor) |
| `runtime_core.rs` ~line 1308 (`EngineBuilder` struct) | Add `chaos_rule: Option<ChaosRule>` field | +1 |
| `runtime_core.rs` ~line 1417 (builder methods block) | Add `.chaos(rule)` builder method | +5 |
| `runtime_core.rs` ~line 1426 (`build()`) | Forward `self.chaos_rule` to `HttpClient::new`; error if both replay and chaos set | +6 |
| `runtime_core.rs` (new types, near `Response`) | `ChaosState`, `ChaosRule`, `FaultKind` public types | +35 |
| `lib.rs` | Export `ChaosRule`, `ChaosState`, `FaultKind` | +1 |

**Subtotal: ~110 lines changed in runtime crate** (refactor adds ~30 lines over the initial estimate).

### CLI Crate (`arazzo-cli`)

| File | Change | Lines (est) |
|------|--------|-------------|
| `cli.rs` | Add `Chaos` variant + `ChaosFormat` enum | +70 |
| `main.rs` | Add `Commands::Chaos` match arm | +30 |
| `output.rs` | Add `ChaosOutput`, `ChaosSummary`, `ChaosWorkflowResult`, `ChaosScenario`, `ChaosOutcome`, `ChaosBaseline` types | +80 |
| `handlers.rs` | Add `run_chaos()` handler + schema registration | +50 |
| `chaos_runner.rs` (new) | Core logic: baseline run, fault enumeration, scenario execution, table/TAP formatters | +400 |
| `tests/schema_drift.rs` | Add schema drift test for `chaos` | +5 |

**Subtotal: ~635 lines in CLI crate**

### Documentation & Schema

| File | Change |
|------|--------|
| `docs/schemas/chaos.schema.json` | Generated JSON schema |
| `CLAUDE.md` | Add `chaos` to commands list |

**Total: ~750 lines** (runtime refactor adds to original estimate).

---

## 8. Implementation Sequence

### Step 1a: Runtime — Extract `live_request` helper (pure refactor)

1. In `runtime_core.rs`, extract the existing Live branch body from `HttpClient::request` into `async fn live_request(&self, inner: &reqwest::Client, cfg: RequestConfig, cancel: &CancellationToken, is_timeout: &AtomicBool) -> Result<Response, RuntimeError>`.
2. Replace the inline code with a call to `live_request(inner, cfg, cancel, is_timeout).await`.
3. **Verify**: `cargo test --workspace` — zero behavior change, all runtime tests must still pass.

### Step 1b: Runtime — Chaos mode types + HttpClient branch

1. Add public `ChaosRule`, `ChaosState`, `FaultKind` types to `runtime_core.rs`.
2. Add `HttpClientMode::Chaos(ChaosState)` variant.
3. Extend `HttpClient::new` signature to accept `chaos_rule: Option<ChaosRule>`; Replay takes precedence over Chaos (error if both set).
4. Add chaos branch in `HttpClient::request` — match on `cfg.step_id`, return synthetic response on match, forward to `live_request(&state.client, …)` otherwise.
5. Add `chaos_rule` field to `EngineBuilder`, `.chaos()` method, and thread into `build()`.
6. Export `ChaosRule`, `ChaosState`, `FaultKind` from `lib.rs`.
7. **Verify**: `cargo check -p arazzo-runtime`. Add unit tests:
   - Chaos with `FaultKind::StatusCode(503)` on a mock step returns status 503 without a network call.
   - Chaos with `FaultKind::Timeout` returns `RuntimeErrorKind::HttpRequest`.
   - Chaos on step A passes through step B to the live path (use a `wiremock` or local `tiny_http` server).
   - Building with both `.replay_trace_steps(...)` and `.chaos(...)` returns an error.

### Step 2: CLI — Types + skeleton

1. Add `ChaosFormat` enum and `Chaos` variant to `cli.rs`.
2. Add `ChaosOutput` and all result types to `output.rs`.
3. Add `Commands::Chaos` match arm in `main.rs`.
4. Add stub `run_chaos()` handler in `handlers.rs`.
5. Add `Some("chaos")` to `handlers::schema()`.
6. **Verify**: `cargo check`, `arazzo --json chaos spec.yaml` returns stub error.

### Step 3: Chaos runner — Baseline + single-fault execution

1. Create `chaos_runner.rs` with `run_chaos_suite()`:
   - Parse spec, build inputs/headers/config (reuse `test` patterns).
   - Run baseline for each workflow.
   - For targeted `--inject`: parse fault spec, run single scenario.
   - For auto: enumerate steps from baseline, run each step × fault type.
   - Classify outcomes (Recovered/Graceful/Crashed).
   - Build `ChaosOutput`.
2. Wire into handler.
3. **Verify**: `arazzo --json chaos examples/httpbin-error-handling.arazzo.yaml` produces real results.

### Step 4: Table formatter

1. Implement `format_chaos_table()` — the human-readable matrix.
2. Wire as default format.
3. **Verify**: colored table output on a real spec.

### Step 5: TAP + JSON formatters

1. TAP: one test line per scenario.
2. JSON: `output_json(&result)`.
3. **Verify**: both formats valid.

### Step 6: Schema, tests, documentation

1. Generate `docs/schemas/chaos.schema.json`.
2. Add schema drift test.
3. Update CLAUDE.md commands list.
4. **Verify**: `cargo test --workspace` all pass.

---

## 9. Edge Cases

| Scenario | Behavior |
|----------|----------|
| Baseline fails | `ChaosWorkflowResult.baseline.passed = false`. Scenarios list is empty; workflow contributes `skipped += executed_steps * faults` to the summary, clearly flagged in table output. |
| Workflow has 0 HTTP steps (only sub-workflows) | No scenarios to run. Empty `scenarios`. |
| `--inject` targets a step that doesn't exist in the spec | `ChaosOutput::Error` at CLI-parse time (validated against workflow before execution). |
| `--inject` targets a step the baseline reached, but fault causes an earlier step to run before it | If the chaos run never reaches the faulted step, outcome is `Skipped`. |
| Fault injection causes a downstream step to fail (ripple) | Classified based on workflow-level outcome + the faulted step's `TraceDecision.path` (see §4 Outcome Classification). |
| Workflow with retry recovers from fault | `Recovered` — retry semantics mean the faulted call is re-issued; since the fault is deterministic per step_id, **every** attempt gets the injected response. True recovery requires either the retry to give up via `onFailure: end`/`goto`, or a different step's retry. Document this clearly. |
| Workflow's onFailure `end` action fires on the faulted step | `Graceful`. |
| Workflow's onFailure `goto` redirects past the fault and workflow completes | `Recovered`. |
| Spec parse error | `ChaosOutput::Error`. |
| `--workflow` specified | Baseline + chaos are limited to that one workflow; other workflows aren't executed at all. |
| `--inject` + `--faults` both passed | Legal: `--inject` defines explicit scenarios, `--faults` is ignored (only auto mode consults it). |
| Fault token unparseable (e.g. `--faults 99` or `--faults foo`) | CLI-parse error. |

### Retry interaction — important subtlety

The plan's chaos fault is keyed on `step_id`, not on attempt number. A workflow step with `retryLimit: 3` that fails once normally would retry and succeed; under chaos, every attempt for that step_id gets the injected fault. This is intentional (tests whether `onFailure`/retry ultimately gives up cleanly), but means "retry succeeded" is only possible when chaos is injected on a **different** step than the one retrying. Make this explicit in docs.

---

## 10. Example Usage

```bash
# Auto mode: executed-steps × default fault set (runs against live API!)
arazzo chaos examples/httpbin-error-handling.arazzo.yaml -i code=200

# Specific workflow
arazzo chaos spec.yaml --workflow checkout-flow

# Targeted injection (repeatable; pins exactly these scenarios)
arazzo chaos spec.yaml \
  --inject step=process-payment,status=503 \
  --inject step=check-inventory,timeout

# JSON output for CI
arazzo --json chaos spec.yaml

# TAP output
arazzo chaos spec.yaml --format tap

# Custom fault set in auto mode (only timeouts and 503s)
arazzo chaos spec.yaml --faults timeout,503

# Raise auto budget for a 10-step workflow
arazzo chaos spec.yaml --max-scenarios 200
```

---

## 11. Non-Goals

| Exclusion | Rationale |
|-----------|-----------|
| Concurrent scenario execution | Sequential runs keep the output deterministic (stable order in TAP/table), avoid amplifying rate limits on the target API, and simplify side-effect reasoning for the operator. Parallelism is an easy future add once those concerns are addressed. |
| Probabilistic/random faults | Deterministic enumeration is reproducible and CI-friendly. Random faults would require a seed flag and reproducibility story. |
| Network-level faults (packet loss, jitter, DNS) | Requires OS-level hooks (e.g. `tc` / `pfctl`). Out of scope for an in-process fault injector. |
| Persistent fault injection (proxy mode) | Would require a long-running daemon. One-shot CLI is simpler and composable. |
| Chaos on sub-workflow (`workflowId`) steps | Sub-workflows don't go through `HttpClient`. A future enhancement could faultinject at the engine step boundary. |
| Chaos-testing branches the baseline didn't execute | Auto mode only enumerates executed steps. Alternate `onSuccess`/`onFailure` branches must be reached via different `$inputs` in a separate run. |
| Sharing `TestOutput` envelope with `test` | Chaos has different semantic axes (step × fault × outcome) than test (case pass/fail). A dedicated `ChaosOutput` keeps the schema honest. We share only the TAP line *format* and exit-code convention. |
