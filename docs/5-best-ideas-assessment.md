# Assessment: 5 Best Ideas vs. Current Codebase

**Date**: 2026-03-05
**Context**: Post async refactor (Phases 1–6) + 5 post-review fixes (RetryScheduled emission, execute_step timeout, DAP CancellationToken, parallel is_timeout sharing, doc example update).

---

## 1. Deterministic Trace + Replay + Redaction

**Readiness: 85% (trace+redaction shipped; replay is the gap)**

What's already done:
- **Trace capture**: Complete. `TraceStepRecord` captures seq, request, response, headers, body preview, criteria evaluations, decision path, outputs, timing, and error — all per step.
- **Redaction**: Complete. 18 sensitive keys (auth, tokens, cookies, etc.) auto-redacted in headers, query params, and JSON bodies. Configurable body truncation (default 2KB).
- **Schema**: Frozen `trace.v1` with atomic file writes, versioning policy, and changelog.
- **Observer events**: The `ExecutionObserver` trait + `RetryScheduled` emission means all major lifecycle events are now observable.
- **Async streaming**: `ExecutionHandle` streams `EngineEvent::TraceStep` records in deterministic seq order.

What's missing for **replay**:
- Trace loader/parser (~100 LOC)
- Response injector trait / mock HTTP client adapter (~200 LOC)
- Request validator (compare live vs. recorded) (~100 LOC)
- `arazzo replay <trace.json>` CLI command (~100 LOC)

**Verdict**: Trace + redaction is ship-ready today. Replay is ~650 LOC of additive work with no architectural changes needed. **Highest ROI feature to complete next.**

---

## 2. Contract-Enforced Execution (Input Validation + Strict Mode + Assertions)

**Readiness: 25% (types exist; zero enforcement)**

What exists:
- `Workflow.inputs: Option<SchemaObject>` is parsed from specs with `properties`, `required`, and `JsonSchemaType` — but **never validated at runtime**.
- Expression diagnostics (`--expr-diagnostics warn|error`) catch unresolved `$inputs.X` references at evaluation time — but this is reactive, not proactive.
- Success criteria (`SuccessCriterion`) exist per-step but there are no workflow-level assertions.

What's glaringly missing:
- **No input validation**: `VarStore::set_input()` blindly accepts all inputs. Missing required fields silently become `Null`. Extra inputs silently ignored.
- **No strict mode**: No flag, no concept, no code path.
- **No workflow-level assertions**: Only step-level success criteria exist.
- The `SchemaObject` model is parsed and then completely ignored by the engine.

**Verdict**: Low-hanging fruit for Phase 1 (required field + type validation ~200 LOC). Strict mode and assertions need spec-level additions but the engine architecture supports them cleanly. **Best bang-for-buck small feature.**

---

## 3. Reliability Guardrails (Timeouts, Bounded Bodies, Retries, Circuit Breaker)

**Readiness: 75% (timeouts and retries are strong; body limits and circuit breaker are gaps)**

What's strong:
- **Timeouts**: Two-tier system — per-request HTTP timeout (`--http-timeout`, default 30s) and workflow-wide execution timeout (`--execution-timeout`, default 5m). `execute_step()` now also respects timeout. Parallel steps share the parent timeout flag.
- **Retries**: Spec-driven via `onFailure` actions with `retry_limit` and `retry_after`. Observer visibility into retries via `RetryScheduled`. Comprehensive test coverage (8+ test cases).
- **Rate limiting**: Token bucket algorithm (10 RPS, 20 burst) with cancellation-aware wait. Applied before every HTTP request.
- **Cancellation**: `CancellationToken` + `AtomicBool` timeout flag propagated through all execution paths (single-step, full-workflow, parallel, and DAP).

What's missing:
- **Response body limits**: `resp.bytes().await` reads entire responses with **no size bound**. A 1GB response = OOM. This is the biggest production risk.
- **Circuit breaker**: Zero implementation. No per-host failure tracking, no fast-fail on degraded services.
- **Exponential backoff**: Retry delay is fixed (`retry_after` seconds), no jitter/exponential strategy.
- **Per-host rate limiting**: Single global limiter, not per-target.

**Verdict**: The async refactor + post-review fixes completed the timeout/cancellation story. **Body size limits are the critical missing guardrail** (~100 LOC in `HttpClient::request()`). Circuit breaker is a larger effort (~200-300 LOC) but important for production use.

---

## 4. First-Class Workflow Test Runner (`arazzo test`)

**Readiness: 60% (infrastructure exists; needs composition into a command)**

What exists:
- **Mock HTTP server**: Full `tiny_http`-based mock server in test `common/mod.rs` with custom response handlers, concurrent request support.
- **Dry-run mode**: `--dry-run` pre-validates request paths without HTTP calls.
- **Snapshot testing**: 6 snapshot files in `tests/snapshots/` with normalization (strip timestamps, versions). Home-rolled but functional.
- **Trace as assertion target**: `TraceFile` captures the complete execution audit trail — perfect for "expected vs. actual" comparison.
- **JSON output**: All commands support `--json` with schema-valid structured output.
- **Engine reusability**: `Arc<EngineInner>` means the same engine can run multiple workflows without rebuilding.

What's missing:
- **`arazzo test` CLI command**: No test discovery, no fixture loading, no result reporting.
- **Fixture file format**: No convention for declaring mock responses alongside specs.
- **Test discovery**: No scanning for `*.test.yaml` or similar patterns.
- **Result reporting**: No TAP/JUnit output, no per-test pass/fail summary.

**Verdict**: Mostly a composition task — the pieces exist. **A basic `arazzo test` with fixture files + expected outputs could ship in ~400 LOC.**

---

## 5. Compiled Execution Plan + Caching

**Readiness: 40% (good architecture; no caching exists)**

What's strong:
- **Engine immutability**: `Arc<EngineInner>` wraps all indexes. An `Engine` is cheaply cloneable and reusable across unlimited executions.
- **One-time indexing**: `WorkflowIndex` (workflow_index, step_indexes, op_index, source_descriptions_map) is built once at `EngineBuilder::build()` time.
- **Global regex cache**: `INTERPOLATE_RE` uses `LazyLock` for one-time compilation.

Where the pain is:
- **YAML parsing dominates cold-start** (~60-70% of 800-1200ms startup). `serde_yml::from_slice()` runs on every CLI invocation.
- **Expressions re-parsed every evaluation**: `ExpressionEvaluator::evaluate()` does prefix stripping, namespace dispatch, and path tokenization on every call — ~10-20 evaluations per step with no memoization.
- **OpenAPI parsed per invocation**: Each `--openapi` spec gets full YAML deserialization.

What's missing:
- **No spec caching**: No serialized plan format, no file-hash-keyed cache.
- **No expression compilation**: No `CompiledExpression` type.
- **No plan file**: No `.arazzo.plan` binary format, no `Engine::from_plan()` constructor.

**Verdict**: Expression pre-compilation is the highest-ROI performance improvement. Full plan caching is architecturally feasible but lower priority until the tool sees batch/CI workloads. **Medium confidence — the pain isn't universal yet.**

---

## Ranking After Post-Review Fixes

| # | Feature | Readiness | Effort to Ship v1 | Impact |
|---|---------|-----------|-------------------|--------|
| 1 | **Trace + Replay** | 85% | ~650 LOC (replay only) | Highest — reproducible debugging |
| 2 | **Input Validation** | 25% | ~200 LOC (required + type checks) | High — eliminates silent failures |
| 3 | **Reliability Guardrails** | 75% | ~200 LOC (body limits + backoff) | High — production safety |
| 4 | **Test Runner** | 60% | ~400 LOC (composition) | Medium-High — enables CI trust |
| 5 | **Compiled Plan + Cache** | 40% | ~500 LOC (expr compilation) | Medium — performance |

### Key files referenced

- `crates/arazzo-runtime/src/runtime_core.rs` — Engine, EngineBuilder, WorkflowIndex, ExecutionHandle, ObserverEvent
- `crates/arazzo-runtime/src/runtime_core/engine_impl.rs` — execute_inner, execute_with_timeout, execute_step
- `crates/arazzo-runtime/src/runtime_core/engine_actions.rs` — retry logic, FlowDecision
- `crates/arazzo-runtime/src/runtime_core/engine_parallel.rs` — parallel step execution
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs` — HttpClient, response body handling
- `crates/arazzo-runtime/src/runtime_core/engine_trace.rs` — trace event emission
- `crates/arazzo-cli/src/trace.rs` — trace file writer, redaction engine
- `crates/arazzo-cli/src/handlers.rs` — CLI run handler
- `crates/arazzo-expr/src/lib.rs` — expression evaluator (no caching)
- `crates/arazzo-spec/src/lib.rs` — SchemaObject, Workflow.inputs (unused at runtime)
- `crates/arazzo-debug-adapter/src/dap.rs` — DAP adapter with CancellationToken
