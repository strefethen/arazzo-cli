# Code Audit Remediation Plan

**Created:** 2026-06-14
**Source:** Full workspace code audit (runtime, expr/spec/validate/generate, CLI/DAP/MCP/extension, project health)
**Status:** Not started
**Audience:** Future agents and maintainers picking up remediation work

---

## How to use this document

Each work item below is self-contained and independently shippable. Items are
ordered by priority. For every item you will find:

- **Why** — the defect and its impact
- **Files** — exact paths and current line numbers (verified at `e5dfede`; re-grep
  before editing since line numbers drift)
- **Current code** — the relevant snippet as it exists today
- **Change** — concrete implementation direction
- **Tests** — what must be added or made to pass
- **Acceptance** — the bar for "done"

Update the **Status tracker** when you start/finish an item. Keep edits surgical
and consistent with existing conventions (no `unwrap`/`expect`/`todo`; these are
`deny` lints workspace-wide in `Cargo.toml:46-49`).

### Status tracker

| # | Item | Severity | Status | Owner |
|---|------|----------|--------|-------|
| 1 | Hermetic CLI tests (remove live httpbin dependency) | High (CI correctness) | Not started | — |
| 2 | CHANGELOG / release hygiene for 0.2.x | High (release blocker) | Not started | — |
| 3 | Component action merge loses explicit `type` override | Medium (correctness) | Not started | — |
| 4 | Goto target index skip in filtered execution | Medium (correctness) | Not started | — |
| 5 | JSONPath silently returns `false` on unsupported syntax | Medium (UX/foot-gun) | Not started | — |
| 6 | Missing `$ref` cycle guards in generator | Medium (crash/DoS) | Not started | — |
| 7 | Finish the VS Code extension | Feature completion | Not started | — |
| 8 | MCP security hardening | Security | Not started | — |

> Verification baseline: `cargo build --workspace --all-targets`, `cargo clippy
> --workspace --all-targets`, and `cargo fmt --check` are all currently clean.
> `cargo test --workspace` currently fails **only** on the three tests addressed
> by item 1.

---

## 1. Hermetic CLI tests — remove live httpbin.org dependency

**Severity:** High — these are the only failing tests in the workspace, and they
fail in any network-restricted environment (sandbox, offline CI, httpbin
outage). The project explicitly advertises "337 tests, all hermetic
(tiny_http test servers, no external API calls)" in `CHANGELOG.md`, so this is
also a truth-in-advertising fix.

### Why

Three `run --step` integration tests execute workflows against the live public
service `https://httpbin.org` and assert HTTP success:

- `run_step_json_single_step_no_deps` — `crates/arazzo-cli/tests/cli_integration.rs:1853`
- `run_step_json_with_dependency_resolution` — `crates/arazzo-cli/tests/cli_integration.rs:1881`
- `run_step_no_deps_succeeds_for_standalone_step` — `crates/arazzo-cli/tests/cli_integration.rs:1913`

They reach the network via the example specs:
- `fixture_spec()` → `examples/httpbin-get.arazzo.yaml` (`sourceDescriptions[0].url: https://httpbin.org`)
- `examples/httpbin-chained-posts.arazzo.yaml`

The CLI itself behaves correctly (it emits a structured `RUNTIME_HTTP_REQUEST`
error JSON on failure); the tests are simply non-hermetic.

### Constraint

The test harness runs the CLI as a **subprocess** (`run()` at
`cli_integration.rs:72`, `Command::new(cli_bin())`). A mock server therefore must
bind a real localhost port, and the spec handed to the subprocess must point its
`sourceDescriptions` URL at that port. No cli integration test currently does
this — there is no in-process server helper and `crates/arazzo-cli/Cargo.toml`
has **no `[dev-dependencies]` section** yet.

### Change

1. Add a dev-dependency to `crates/arazzo-cli/Cargo.toml`:
   ```toml
   [dev-dependencies]
   tiny_http = "0.12"
   ```
   (The runtime crate already uses `tiny_http` in its test suite, so it is an
   established workspace test dependency — confirm the exact version with
   `cargo tree -p arazzo-runtime` / `Cargo.lock` and match it.)

2. Add a test helper to `cli_integration.rs` that starts a background
   `tiny_http::Server`, returns its `http://127.0.0.1:<port>` base URL, and serves
   canned JSON for `/get` and the chained-post endpoints. Shut it down on drop.

3. Rewrite the three tests to **generate a temp spec** (use the existing `TempDir`
   helper defined at `cli_integration.rs:10`, e.g. the copy-into-tempdir pattern
   around `cli_integration.rs:1815`)
   whose `sourceDescriptions[0].url` is the local server URL, then run the CLI
   against that temp spec. Assert on the canned response outputs (`origin`, `url`,
   `enriched_action`).

   For the dependency-resolution test, the temp spec must reproduce the
   `post-initial` → `post-enriched` dependency from
   `examples/httpbin-chained-posts.arazzo.yaml` but target the local server.

### Acceptance

- `cargo test -p arazzo-cli --test cli_integration` passes with **no network
  access**.
- No test in the default `cargo test --workspace` run performs outbound DNS/HTTP.
- `examples/httpbin-*.arazzo.yaml` remain unchanged (they are user-facing
  examples and legitimately point at the real service).

---

## 2. CHANGELOG / release hygiene for 0.2.x

**Severity:** High — release blocker for cutting `0.2.2`.

### Why

- Workspace version is `0.2.2` (`Cargo.toml:15` via `workspace.package.version`).
- `CHANGELOG.md` contains a single `## [0.1.0] - 2026-03-13` section and **no
  0.2.x entries**, despite two point releases of accumulated fixes
  (Set-Cookie handling, goto-workflow input forwarding, retry self-loop count,
  `arazzo test` command, JSONPath count tokenization, vendor extension
  preservation, etc. — see `git log`).
- **No git tags exist at all** (`git tag` is empty), so 0.1.0 was never actually
  tagged either. The release tooling (`scripts/release/cut-tag.sh`) expects a
  `vMAJOR.MINOR.PATCH` tag matching `Cargo.toml`.

### Change

1. Add `## [0.2.0]`, `## [0.2.1]`, `## [0.2.2]` sections to `CHANGELOG.md`
   (newest first, above `0.1.0`), following the existing Keep-a-Changelog format.
   Reconstruct entries from `git log --oneline v-range` grouped by the existing
   headings (CLI / Expression Language / Workflow Engine / Security / Quality).
   Notable commits to mine:
   - `7a70a94 feat(cli): add arazzo test command for CI-native API contract testing`
   - `ffe22cb fix(runtime): special-case Set-Cookie to avoid comma-join corruption`
   - `d51c83f fix(runtime): goto-workflow forwards parent inputs`
   - `6384d78 fix(runtime): do not increment retry_count on goto self-loops`
   - `7550006 fix(expr): use f64_approx_eq in compare_ordered`
   - `8e8cdd6 fix(validate): unused retry field warnings to stderr`
   - `3fe61ec fix(cli): load .env before starting tokio runtime`
   - `c6cd734 Fix JSONPath count predicate tokenization`
   - `5846c73 Preserve Arazzo vendor extensions` (note `e1252fe` is the
     companion compliance-plan doc update, not a user-visible change)
   - `3cb8a48 Refactor runtime core into focused modules`
2. Move the `--step` / `--no-deps` bullets: they are currently listed under
   `0.1.0` (`CHANGELOG.md:24-25`) but the feature stabilized in the 0.2.x line.
   Verify against history and place them in the correct section.
3. Decide and document the tagging story in `CONTRIBUTING.md` or a short
   `RELEASING.md`: bump `workspace.package.version`, update `CHANGELOG.md`, then
   `scripts/release/cut-tag.sh`. Optionally add a CI guard that fails a release
   build if the top CHANGELOG version != `Cargo.toml` version.

### Acceptance

- `CHANGELOG.md` top entry version matches `Cargo.toml` (`0.2.2`).
- Every user-visible change since 0.1.0 is represented.
- Release process is written down once, in one place.

---

## 3. Component action merge loses an explicit `type` override

**Severity:** Medium — silent incorrect behavior. Confirmed bug; existing test
`parse_bytes_component_action_preserves_local_overrides`
(`crates/arazzo-validate/src/lib.rs:1984`) does **not** cover the failing case.

### Why

`OnAction.type_` is a plain enum with a serde default of `End`:

```rust
// crates/arazzo-spec/src/lib.rs:531-539
#[derive(... Default ...)]
pub enum ActionType {
    #[default]
    End,
    Goto,
    Retry,
}

// crates/arazzo-spec/src/lib.rs:557-558
#[serde(rename = "type", default)]
pub type_: ActionType,
```

When resolving a `$components.{success,failure}Actions.*` reference, the validator
overlays "locally set" fields onto the component, detecting "set" via inequality
with the default:

```rust
// crates/arazzo-validate/src/lib.rs:668-671
// Merge: start with resolved component, overlay non-default local fields
let mut merged = resolved.clone();
if action.type_ != ActionType::default() {   // <-- bug
    merged.type_ = action.type_;
}
```

Because `ActionType::default() == End`, a local action that **explicitly** writes
`type: end` to override a `retry`/`goto` component is indistinguishable from one
that omitted `type`, so the override is silently dropped.

This is the same class of problem `SuccessCriterion` already solved correctly by
using `Option<CriterionType>` plus `has_declared_type()`
(`crates/arazzo-spec/src/lib.rs:468, 526`). Adopt that house style.

### Change (`Option<ActionType>`, matches existing convention)

1. In `crates/arazzo-spec/src/lib.rs`, change the field to optional and add an
   accessor:
   ```rust
   #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
   pub type_: Option<ActionType>,

   impl OnAction {
       /// Effective action type (`End` when omitted, per Arazzo defaults).
       pub fn action_type(&self) -> ActionType {
           self.type_.unwrap_or_default()
       }
       /// Whether `type` was explicitly declared in the spec.
       pub fn has_declared_type(&self) -> bool {
           self.type_.is_some()
       }
   }
   ```
2. Fix the merge to use presence, not value
   (`crates/arazzo-validate/src/lib.rs:669`):
   ```rust
   if action.type_.is_some() {
       merged.type_ = action.type_;
   }
   ```
3. Update all readers of `action.type_` (it is now `Option`). Blast radius
   (verified — re-grep `\.type_\b` near `action`/`on_success`/`on_failure`):
   - `crates/arazzo-runtime/src/runtime_core/engine_actions.rs:206` `match action.type_` → `match action.action_type()`
   - `crates/arazzo-runtime/src/runtime_core/engine_actions.rs` `action.type_.to_string()` at lines 223, 231, 246, 258, 269, 281, 317, 344, 355 → `action.action_type().to_string()`
   - `crates/arazzo-runtime/src/runtime_core/engine_impl.rs:815` `.filter(|a| a.type_ == ActionType::Retry)` → `a.action_type() == ActionType::Retry`
   - `crates/arazzo-runtime/src/runtime_core/engine_trace.rs:145` `action.type_.to_string()` → `action.action_type().to_string()`
   - `crates/arazzo-validate/src/lib.rs:371` `action.type_ == ActionType::Goto` → `action.action_type() == ...`
   - `crates/arazzo-validate/src/lib.rs:417,421,427` (`!= ActionType::Retry`, display) → `action.action_type()`
   - Struct literals that set `type_: ActionType::X` must become `type_: Some(ActionType::X)`:
     - `crates/arazzo-generate/src/crud.rs:756`
     - test constructors in `crates/arazzo-runtime/src/lib.rs:204,214,234,243,262` and `crates/arazzo-validate/src/lib.rs:1463,1629,...`
   - Test assertions `assert_eq!(action.type_, ActionType::X)` must become
     `assert_eq!(action.action_type(), ActionType::X)` (numerous in
     `crates/arazzo-validate/src/lib.rs` ~lines 1060-1717 and `lib.rs:210` in runtime).
4. Check serde round-trip: `crates/arazzo-spec/tests/serde_roundtrip.rs` —
   omitted `type` should remain omitted on re-serialize (the
   `skip_serializing_if = "Option::is_none"` handles this; add/adjust a case).

This is the correct fix and matches the `SuccessCriterion` precedent already in
the codebase. The ~15-site blast radius is mechanical (swap reads for the
`action_type()` accessor, wrap literals in `Some(...)`); work through every call
site rather than narrowing the change. If the `Option<ActionType>` approach turns
out to be wrong for a reason discovered mid-implementation, pivot the design and
update this plan — do not fall back to a sentinel/`#[serde(skip)]` workaround.

### Tests

Add to `crates/arazzo-validate/src/lib.rs` tests (next to bug #26 test at line
1984): a component defines `type: retry`; the local reference overrides
`type: end`; assert the resolved action is `End`. This test must fail before the
fix and pass after.

### Acceptance

- Explicit `type: end` overriding a non-`end` component survives resolution.
- Omitted `type` still defaults to `End` and serializes without a `type` key.
- Full workspace build + tests green.

---

## 4. Goto target index skip in filtered execution

**Severity:** Medium — incorrect execution order. Accepted (not yet fixed) in
`bug-hunt-results.md` ("ACCEPTED — Goto Target Index Skip in Filtered Mode").

### Why

In filtered execution (`arazzo run <workflow> <step>`, or `--step`), `steps_to_run`
is a sparse subset of step indices (transitive deps of the target). When a goto
lands on an index **not** in the set, the fallback advances the cursor by one
slot instead of seeking to the next step at-or-after the target:

```rust
// crates/arazzo-runtime/src/runtime_core/engine_impl.rs:289-303
FlowDecision::Next(next_idx) => {
    // Find the position of next_idx in our filtered steps_to_run set.
    if let Some(pos) = steps_to_run.iter().position(|&i| i == next_idx) {
        run_cursor = pos;
    } else if next_idx > idx {
        // Goto target is past us but not in our filtered set — advance.
        run_cursor += 1;                 // <-- bug: may land BEFORE next_idx
    } else {
        return Err(RuntimeError::new(
            RuntimeErrorKind::GotoTargetNotFound,
            format!("goto target step index {next_idx} not in execute_step scope"),
        ));
    }
}
```

Example: `steps_to_run = [0, 2, 5]`, current `run_cursor = 0` (idx 0), goto target
`next_idx = 3`. `position(== 3)` fails; `3 > 0` so `run_cursor = 1` → next executed
step is index `2`, which is **before** the target. Correct behavior is to jump to
index `5` (first step ≥ 3).

### Change

Replace the `else if` branch with a seek to the first remaining step ≥ `next_idx`:

```rust
FlowDecision::Next(next_idx) => {
    if let Some(pos) = steps_to_run.iter().position(|&i| i == next_idx) {
        run_cursor = pos;
    } else if let Some(pos) = steps_to_run.iter().position(|&i| i >= next_idx) {
        // Target not in the filtered set: resume at the next step at-or-after it.
        run_cursor = pos;
    } else {
        // Target is past every step in scope: nothing left to run.
        break;
    }
}
```

This seek is correct because `steps_to_run` is sorted ascending —
`compute_transitive_deps` returns a `BTreeSet<usize>` and the target index is
inserted into it before collecting (`engine_impl.rs:194-196`), so
`position(|&i| i >= next_idx)` lands on the *first* in-scope step at-or-after the
target.

Note the terminal case changes from a `GotoTargetNotFound` error to `break`
(graceful end), matching the semantics of "goto a step beyond the filtered tail."
Be deliberate about one behavior change this introduces: a goto whose target is
**absent from the filtered set and below the current cursor** previously returned
`GotoTargetNotFound`; it now resumes at the first in-scope step ≥ target instead
of erroring. Confirm that is acceptable for filtered execution. The
`GotoTargetNotFound` kind is still used by the retry branch
(`engine_impl.rs:313`) and `engine_actions.rs:254`, so the error variant itself
is not orphaned.

### Tests

Add a runtime test (alongside `crates/arazzo-runtime/tests/engine_execution.rs`
goto cases) that builds a workflow with ≥6 steps, filters to a sparse
`steps_to_run` set with a gap, and asserts a goto into the gap resumes at the
correct next step (and never executes a step before the target). Also remove this
item from `bug-hunt-results.md`'s open list once fixed.

### Acceptance

- Goto into a filtered gap resumes at the first step ≥ target.
- Existing goto/retry tests still pass.

---

## 5. JSONPath criteria silently return `false` on unsupported syntax

**Severity:** Medium — silent failure / debugging foot-gun. The hand-rolled
JSONPath engine covers only a subset of the grammar; anything outside it
evaluates to `false` with no diagnostic, so a user writing `$..items[*]` sees a
failed success-criterion and no reason why.

### Why

`evaluate_jsonpath_condition` (`crates/arazzo-runtime/src/runtime_core/jsonpath.rs:3`)
handles: simple paths, `$[?(...)]` filter predicates (`&&`/`||`), `count(...)`
predicates, and literal comparisons. Unsupported constructs — recursive descent
`..`, wildcard `*`, array slices `[a:b]`, unions — fall through to a normalized
path lookup that resolves to `Null`/`false`:

```rust
// jsonpath.rs:21-27
let normalized = normalize_jsonpath_path(trimmed);
if normalized.is_empty() {
    return !context_value.is_null();
}
let value = scoped_eval.evaluate(&format!("$response.body.{normalized}"));
is_truthy(&value)
```

The caller (`criteria.rs:80-86`) already has an `error: Option<String>` channel on
`CriterionEvaluation` (used by the `regex` arm at `criteria.rs:74-77`) and a
`warnings` vector — but the `jsonpath` arm never populates either.

### Change

1. In `jsonpath.rs`, add a cheap detector for unsupported tokens and return a
   structured result rather than a bare `bool`. Minimal approach: introduce
   ```rust
   pub(super) enum JsonPathOutcome {
       Matched(bool),
       Unsupported(String), // human-readable reason, e.g. "recursive descent (`..`) not supported"
   }
   ```
   and have `evaluate_jsonpath_condition` return it. Detect at least: `..`
   (recursive descent), bare `*`/`[*]` (wildcard), and `[<int>:<int>]` (slice).
   Keep the existing supported paths returning `Matched(_)`.
2. In `criteria.rs`, the `"jsonpath"` arm (lines 80-86) maps `Unsupported(reason)`
   to `error = Some(format!("unsupported JSONPath: {reason}"))` and
   `condition_result = false`, mirroring the `regex` arm's error handling. This
   surfaces through the existing trace/warning machinery and the
   `--expr-diagnostics` flag rather than vanishing.
3. Document the supported JSONPath subset in `README.md` (expression-language
   section) and in a rustdoc comment on `evaluate_jsonpath_condition`.

### Tests

In `crates/arazzo-runtime` tests (criteria/helper tests), add cases asserting
that `$..foo`, `$[*]`, and `$.items[0:2]` produce a non-empty `error`/diagnostic
(not a silent `false`), and that currently-supported expressions are unchanged.

### Acceptance

- Unsupported JSONPath yields a visible diagnostic via `error`/warnings.
- No regression for supported filter/count/comparison predicates.
- Subset is documented.

---

## 6. Missing `$ref` cycle guards in the generator

**Severity:** Medium — stack overflow / DoS on a malformed or hostile OpenAPI
document.

### Why

`crates/arazzo-generate/src/refs.rs` guards cycles for schema refs via a
`visited: HashSet<String>` (`refs.rs:7-24`) but the request-body and response
resolvers recurse with **no** cycle tracking:

```rust
// refs.rs:26-39
pub fn resolve_request_body_ref<'a>(...) -> Option<&'a openapiv3::RequestBody> {
    match rb_ref {
        ReferenceOr::Item(rb) => Some(rb),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/requestBodies/")?;
            let comps = components.as_ref()?;
            let next_ref = comps.request_bodies.get(name)?;
            resolve_request_body_ref(next_ref, components)   // unbounded recursion
        }
    }
}
// refs.rs:41-54 resolve_response_ref — same shape, same gap
```

A document with `requestBodies: { A: { $ref: '#/components/requestBodies/A' } }`
(or a longer cycle) recurses until the stack overflows and aborts the process.

### Change

Thread a `visited: &mut HashSet<String>` through both functions exactly as
`resolve_schema_ref` does: insert the resolved name and return `None` on
reinsertion. Update the two call sites (find them with
`grep -rn 'resolve_request_body_ref\|resolve_response_ref' crates/arazzo-generate/src`)
to pass a fresh `HashSet` per top-level resolution.

### Tests

Extend `refs.rs` tests (the schema-cycle test is at `refs.rs:72-92`) with a
`requestBodies` self-cycle and a `responses` self-cycle, each asserting
`None` (no panic, no overflow).

### Acceptance

- Cyclic request-body/response refs return `None` instead of overflowing.
- Generation of valid specs is unchanged.

---

## 7. Finish the VS Code extension

**Goal:** Ship the Arazzo debugger as a first-class, marketplace-ready tool for
API-workflow development — drop `"preview": true` once the items below are done.
The debugger backend (DAP server) already works and is well-tested; the gaps are
in the client extension's tests, packaging robustness, and a redundant client-side
stub that should be removed, not implemented.

### 7a. Delete the dead client-side breakpoint-mapping stubs

**Key finding:** the DAP server already owns YAML→step line mapping. It parses the
spec with `yaml_rust2` (`crates/arazzo-debug-adapter/src/dap.rs:15-16`), builds a
line index, and resolves breakpoints server-side in `resolve_source_breakpoints`
(`dap.rs:1034`), returning `verified: true/false` per breakpoint
(`dap.rs:1056,1075`). VS Code sends raw source lines via `setBreakpoints`
(`dap.rs:317`) and the server maps them to step checkpoints.

Therefore the client-side TypeScript stubs duplicate (incompletely) work the
server already does and are currently dead:
- `vscode-arazzo-debug/src/yamlStepIndex.ts` — `buildWorkflowStepIndex` returns
  `{ steps: [] }` (placeholder).
- `vscode-arazzo-debug/src/breakpointMapper.ts` — calls the placeholder, discards
  the result, returns line-only records.

**Action:** delete both files (and any imports). Do **not** implement a second
YAML parser in TypeScript — the server is the single source of truth for
line→step resolution. If a future feature needs client-side step awareness (e.g.
inline decorations), reuse VS Code's existing YAML tooling rather than
hand-rolling, and open a separate proposal.

### 7b. Replace the fake smoke test with real integration tests

`vscode-arazzo-debug/src/test/smoke.test.ts` is `assert.equal(true, true)` —
zero coverage. Replace with tests that exercise the actual surface:

1. **DAP transcript test (highest value, no VS Code host needed):** spawn the
   built `arazzo-debug-adapter` binary, drive a scripted DAP session over
   stdio (initialize → launch with `stopOnEntry` → setBreakpoints → continue →
   inspect a scope → disconnect) against a tiny fixture spec and a local
   `tiny_http`/Node mock server, and assert on responses/events. This mirrors the
   Rust transcript tests in `crates/arazzo-debug-adapter/tests/` but proves the
   TypeScript launch/argument plumbing (`adapterClient.ts`) end-to-end.
2. **Unit tests for `debugConfigProvider.ts`:** assert defaulting logic
   (`resolveDebugConfiguration` fills `type`/`request`/`name`/`inputs`/`stopOnEntry`,
   and returns `undefined` + error when `spec` is missing — `debugConfigProvider.ts:30-35`).
3. **Unit tests for `adapterClient.ts` helpers:** `asString` / `asStringArray`
   coercion and the `runtimeExecutable` override path vs bundled-binary path.

Use `@vscode/test-electron` only if a full host is needed; otherwise prefer the
node:test runner already in use so CI stays fast. Wire the new tests into the
`vscode-extension` CI job (`.github/workflows/ci.yml:156`), which today only runs
`npm run lint` (typecheck) and build — add a `npm test` step.

### 7c. Harden binary bundling

`vscode-arazzo-debug/scripts/copy-binary.js` copies a single
`target/release/arazzo-debug-adapter` into `bin/` at `vscode:prepublish`
(`package.json` script) and the runtime resolves it in
`adapterClient.ts:getBundledBinaryPath` (`adapterClient.ts:6-14`). Gaps to close:

- **Per-platform VSIX:** the marketplace expects platform-specific packages
  (`win32-x64`, `linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`). The
  release workflow `.github/workflows/vscode-release.yml` already references
  platform packaging — verify it builds the matching Rust target per platform and
  that `copy-binary.js` picks the right artifact (currently it only switches on
  `.exe` extension, `copy-binary.js:8`, and assumes the host arch).
- **macOS universal or dual-arch:** decide between universal binary vs separate
  `darwin-x64`/`darwin-arm64` VSIX; document the choice.
- **Integrity:** record and verify a SHA-256 of the bundled binary at package time
  (the release pipeline already produces `SHA256SUMS.txt` for CLI binaries — reuse
  that convention).
- **Missing-binary UX:** `adapterClient.ts:38-43` already shows a helpful error;
  keep it, and ensure the dev-mode `runtimeExecutable` override path is documented
  in `vscode-arazzo-debug/README.md`.

### 7d. Documentation and preview flag

- Expand the stub docs: `docs/debugger-user-guide.md` (currently ~34 lines) and
  `docs/debugger-troubleshooting.md` (~40 lines) to cover launch config, scopes,
  conditional breakpoints, sub-workflow call stacks, and the
  `runtimeExecutable`/`runtimeArgs` dev override.
- The README screenshot reference (`docs/images/vscode-debugger.png`, embedded at
  `README.md:275`) resolves to a real 1414×1103 PNG, so no action is needed here
  beyond refreshing it if the UI changes — the earlier "missing screenshot"
  finding was incorrect.
- Once 7a–7c land and the extension is dogfooded against the example specs, drop
  `"preview": true` and bump `vscode-arazzo-debug/package.json` to `1.0.0`. Add a
  `CHANGELOG.md` entry in the extension folder.

### Acceptance

- `yamlStepIndex.ts` / `breakpointMapper.ts` removed; no dead imports.
- Real DAP + unit tests run in CI (`npm test` in the `vscode-extension` job).
- Per-platform VSIX builds with the correct binary and a verified checksum.
- Debugger user guide + troubleshooting are complete; `preview` flag removed;
  extension versioned `1.0.0`.

---

## 8. MCP security hardening (fleshed-out plan)

**Context:** `crates/arazzo-mcp` exposes a hand-rolled JSON-RPC MCP server (no
SDK; framing in `crates/arazzo-mcp/src/protocol.rs`) with seven tools:
`list_workflows`, `describe_workflow`, `run_workflow`, `validate_spec`,
`generate_workflow`, `describe_openapi`, `generate_example`. It runs locally over
stdio with user consent. The existing notes
(`docs/future/mcp-security.md`, 8 lines) acknowledge three risks but defer all
hardening. This section is the concrete plan; it supersedes that stub (replace the
stub's body with a pointer to this document when implementing).

### Threat model

MCP servers run locally, but the **client driving them is an AI agent** that can
choose tool arguments. The trust boundary is therefore: a user explicitly loads
specs and starts the server; the agent then decides *which* tools to call with
*which* arguments. The risks below all stem from the agent (or a malicious spec)
steering execution in ways the user did not intend.

| Risk | Surface | Today |
|------|---------|-------|
| **R1 — Path traversal / arbitrary file read** | `validate_spec`, `generate_workflow`, `describe_openapi`, `generate_example` accept file paths | `check_path_allowed` (`crates/arazzo-mcp/src/state.rs:62-88`) canonicalizes + checks an allowlist, but it is **not applied to every file-taking tool**, nor to OpenAPI `$ref`/source files pulled in *transitively* by a loaded spec |
| **R2 — SSRF / internal network access** | `run_workflow` executes real HTTP to URLs from specs | No URL policy. The runtime `ClientConfig` (`crates/arazzo-runtime/src/runtime_core/client.rs:23-27`) has timeout + rate limit but **no host allow/deny list**; a spec can target `http://169.254.169.254/…`, `localhost`, RFC-1918 ranges |
| **R3 — Env var / secret exfiltration** | `$env.VAR` expressions resolve host env; outputs flow back to the agent | No env allowlist; any `$env.SECRET` can surface in returned outputs |
| **R4 — Resource exhaustion / DoS** | `run_workflow` accepts arbitrary inputs | Hard-coded 30s HTTP / 300s execution timeouts exist, but no concurrency cap or per-session quota |
| **R5 — Information disclosure via errors** | parse/validation error messages | May reveal file existence / partial contents |

### Workstream 8.1 — Unified path sandbox (R1)

1. Add an `--allowed-dirs <dir>[,<dir>...]` server flag (and an env fallback,
   e.g. `ARAZZO_MCP_ALLOWED_DIRS`). Default: the directory of each spec loaded at
   startup, plus an explicit opt-in for anything broader.
2. Route **every** filesystem access through `check_path_allowed`
   (`state.rs:62-88`), including:
   - all four file-taking tools (audit `crates/arazzo-mcp/src/handlers.rs` for
     each path argument),
   - transitive reads: OpenAPI `$ref` to external files and
     `sourceDescriptions[*].url` that are `file://`/relative paths resolved during
     `validate_spec`/`generate_workflow`/`describe_openapi`.
3. Canonicalize before the check and reject symlink escapes (canonicalize already
   resolves symlinks; add a test that a symlink pointing outside the allowlist is
   denied).

### Workstream 8.2 — Outbound URL policy / SSRF guard (R2)

1. Extend `ClientConfig` (`crates/arazzo-runtime/src/runtime_core/client.rs:23`)
   with an optional URL policy, e.g.:
   ```rust
   pub struct UrlPolicy {
       pub allow_hosts: Option<Vec<String>>,   // None = allow all (current behavior)
       pub deny_private_ranges: bool,           // block RFC1918 / loopback / link-local / ULA
   }
   ```
   Default for the **library/CLI** stays permissive (no behavior change; the CLI
   runs user-authored specs by hand). Default for the **MCP server** is
   `deny_private_ranges: true` and an optional `allow_hosts`.
2. Enforce in `HttpClient::request` *before* sending
   (`client.rs:140`/`:166`): parse `cfg.url`, resolve the host, and reject when it
   is loopback/link-local/private (unless explicitly allow-listed). Be careful to
   check the **resolved IP**, not just the hostname, to avoid DNS-rebinding and
   `localhost`-alias bypasses; reject on resolution failure.
3. Surface a distinct `RuntimeErrorKind` (e.g. `BlockedByUrlPolicy`) so the denial
   is observable in traces and MCP responses.
4. Wire an `--allow-host`/`--allow-private-network` flag on the MCP server (and CLI
   `run`, for parity) so users can opt back in deliberately.

### Workstream 8.3 — Environment variable allowlist (R3)

1. Add `--allowed-env <NAME>[,<NAME>...]` (and `ARAZZO_MCP_ALLOWED_ENV`) to the
   MCP server. When set, `$env.X` resolves only allow-listed names; others resolve
   to `Null` and emit a diagnostic (not the secret).
2. Find the env resolution path in `arazzo-expr` (the `$env.` handling) and thread
   an optional allowlist from the runtime/engine builder down to the evaluator.
   Default for the MCP server: deny all `$env` unless explicitly allowed; default
   for the CLI: unchanged (loads `.env`).
3. Ensure redaction already covers any allow-listed secret that legitimately flows
   into a trace (cross-check `crates/arazzo-runtime/src/runtime_core/redaction.rs`).

### Workstream 8.4 — Quotas and concurrency caps (R4)

1. Cap concurrent `run_workflow` executions per server (a semaphore in the MCP
   session state, `crates/arazzo-mcp/src/state.rs`).
2. Keep the existing 30s/300s timeouts; make them configurable via flags and
   document the defaults.
3. Optionally add a per-session execution count / wall-clock budget.

### Workstream 8.5 — Error hygiene (R5)

Review `validate_spec`/`generate_*` error construction in
`crates/arazzo-mcp/src/handlers.rs` so messages returned to the agent do not echo
absolute paths or file contents beyond what is necessary; prefer the structured
`ValidateError {kind, path, message}` shape already used elsewhere.

### Documentation

Replace the body of `docs/future/mcp-security.md` with: the threat model table
above, the implemented controls, and the default posture per surface
(CLI = permissive/by-hand, MCP = sandboxed-by-default). Add an
`MCP Server` security subsection to `README.md` and stop documenting the MCP
server as unconditionally safe.

### Acceptance

- Every file-taking MCP tool enforces the path sandbox, including transitive refs;
  symlink-escape test passes.
- `run_workflow` blocks loopback/link-local/private targets by default on the MCP
  server (resolved-IP check), with an explicit opt-in flag; a test asserts a
  `169.254.169.254` / `127.0.0.1` spec is denied.
- `$env` resolution is deny-by-default on the MCP server with an allowlist flag.
- Concurrency cap and configurable timeouts in place.
- `docs/future/mcp-security.md` and `README.md` reflect the real posture.

---

## Appendix — verification commands

```bash
# Build, lint, format (all currently clean)
cargo build --workspace --all-targets
cargo clippy --workspace --all-targets
cargo fmt --check

# Tests (item 1 must make this fully green offline)
cargo test --workspace

# VS Code extension (item 7)
cd vscode-arazzo-debug && npm install && npm run lint && npm test

# Re-confirm line numbers before editing (they drift)
grep -n '<symbol>' <file>
```

### Cross-references

- Prior audit and accepted/disproved findings: `bug-hunt-results.md`
- Contributor/build rules: `AGENTS.md`
- Existing MCP security notes (to be superseded by item 8):
  `docs/future/mcp-security.md`
- Related future-work proposals: `docs/future/`, `plans/current/`
