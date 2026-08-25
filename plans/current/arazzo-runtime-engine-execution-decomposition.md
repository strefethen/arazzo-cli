---
status: draft
epic:
created: 2026-08-22
revive-when:
---

# Arazzo Runtime Engine Execution Test Decomposition

**Seed:** "I want you to find the 5,018-line runtime integration test file which is a god module and I want you to create a plan on how to refactor it in an isomorphic way such that it is logically split up into tests that you can hold up as a model of proper god module refactoring. The plan should be such that it supports decomposition into tier 2 tickets so review the ticket writer skill to understand those requirements"

## Outcome and non-goals

`crates/arazzo-runtime/tests/engine_execution.rs` becomes a thin, deliberately
bounded integration-test facade over concern-owned modules. The existing
`engine_execution` Cargo target still runs the same accepted-baseline tests (96
in this drafting snapshot) in one process, against the same public runtime
surface, with the same fixtures, assertions, side effects, errors, timing
boundaries, and evidence entrypoints. Every accepted root-qualified identity and
exact-filter command remains valid.

The result is intended to be a reference refactor: every moved test has exactly
one named owner; cross-module support is narrow; migration is incremental and
reversible; equivalence is proved for every slice; and a bounded structural
guard prevents the facade or a child module from becoming the next God file.

This initiative does **not**:

- change runtime, validator, expression, CLI, MCP, DAP, or Arazzo semantics;
- add, delete, rename, rewrite, deduplicate, strengthen, or weaken behavioral
  tests;
- change dependencies, features, the existing `engine_execution` Cargo target,
  test runtime flavors, timeout values, mock-server behavior, or fixture
  contents; the only additive target is the bounded structural guard described
  below;
- split `tests/common/mod.rs`, `engine_parallel.rs`, production runtime modules,
  or any other God-file/watch-list entry;
- resolve behavior tickets whose tests currently forecast writes to this file;
- edit conformance claim meaning or treat existing code/tests as specification
  authority.

## Developer-visible workflows

1. **Run the established target.** `cargo test -p arazzo-runtime --test
   engine_execution` remains valid and executes accepted-baseline count N in
   one integration-test process.
2. **Find a test by behavior.** A maintainer follows the facade's declarative
   root adapter to the one concern module that owns its body.
3. **Add regression coverage.** The body lands in its concern owner; one guarded
   root-adapter/ledger entry is the only facade change. No behavior or generic
   support accretes there.
4. **Run focused coverage.** Existing leaf-name and root-qualified `--exact`
   filters stay valid; a ticket names every moved root case it must execute.
5. **Resolve conformance evidence.** Existing `path#test_name` references in the
   conformance manifest continue to resolve to non-ignored top-level tests in
   `engine_execution.rs` throughout and after migration.
6. **Recover from a bad slice.** Revert an independent head directly; after successors land, revert the contiguous suffix newest-first or repair forward.

## Decision-relevant facts

- At `04ee287`, `crates/arazzo-runtime/tests/engine_execution.rs` is exactly
  5,018 physical lines and blob `5552a97d1d6f5b27ddc01e5236c48fbc675813a1`.
  It contains 96 test entrypoints (92 Tokio, four synchronous), 11 helper/method
  functions, two observer structs, and no ignored tests.
- `cargo test -p arazzo-runtime --test engine_execution -- --list` reports 96
  tests and zero benchmarks; the target baseline passes 96/96. These commands,
  not a source-text count alone, establish Cargo reachability.
- The file began at 1,667 lines and 42 tests in commit `ae62d38`; 26 commits have
  touched it. A one-time physical split without an ownership guard would repeat
  the same accretion pattern.
- `crates/arazzo-runtime/Cargo.toml` declares no explicit `[[test]]` targets.
  Moving concern files directly under `tests/` would create separate integration
  binaries and change process isolation, scheduling, target names, and focused
  commands. The retry/cancellation cases use real clocks, `Notify`, atomics,
  cancellation, and mock-server threads, so that topology change is outside an
  isomorphic move.
- `tests/common/mod.rs` already owns the public-test HTTP server, core spec
  builders, engine builder, success criterion, YAML conversion, and observer
  fixtures (`start_server`, `make_spec`, `make_spec_with_base`,
  `new_test_engine`, `success_200`, `to_yaml`, and `TestObserver`). It is 652
  lines and must not absorb target-local helpers or a new wildcard prelude.
- Six executable evidence references point to top-level retry tests in this
  file: claim `runtime.retry-limit-and-failure-fallthrough` in
  `crates/arazzo-cli/tests/conformance/runtime.json` and claim
  `expr.decimal-retry-after-and-case-insensitive-simple-comparison` in
  `crates/arazzo-cli/tests/conformance/expressions.json`.
  `recognized_non_ignored_test_items` and `validate_evidence_reference` in
  `crates/arazzo-cli/tests/conformance_manifest.rs` scan the referenced physical
  Rust file rather than expanding nested modules.
- Repository search found no process-global environment/current-directory
  mutation or global mutable test state in this target. Within-test ordering and
  resource lifetime still matter; textual test order does not.
- Tracker revision and forecast membership changed during this planning pass.
  That volatility is itself decision-relevant: at least one ready behavior
  ticket, [ac-a983f](https://sonos.scapedeck.com/docs/ac-tickets/ac-a983f),
  forecast this file during the survey. Do not copy the drafting snapshot into
  implementation tickets. Immediately before acceptance, sandwich `tkt ready`
  and `tkt ls` plus `tkt show` for every active ticket between matching `tkt
  rev` hashes; derive overlaps from each ticket's `writes`, then inspect
  reservations separately. Retry the survey if the hashes differ.
- The accepted ledger defines N tests and E physically scanned evidence tests;
  today N=96/E=6. Refresh every count, cohort, adapter mode, and hard-reference
  row and repeat fresh review; a case without a settled owner returns to Draft.

## Architecture

### Preserve one target; add real ownership inside it

Keep `tests/engine_execution.rs` as the Cargo target root. It may contain only:

- `mod common;`;
- explicit `#[path = "engine_execution/<owner>.rs"] mod <owner>;`
  declarations;
- N-E logical invocations of the allowlisted async/sync adapter macros (rustfmt
  may wrap them); and
- E explicit one-expression adapters required by the physical-source
  conformance scanner.

Concern files live under `tests/engine_execution/`. They are real Rust modules,
not `include!` fragments, and each imports only the production types and
`crate::common` fixtures it consumes. Domain modules never import sibling domain
modules. Two target-local support leaves keep unrelated representations apart:
`selector_fixture.rs` owns only `selector`, while `captured_response.rs` owns
only `parse_json_body`. Neither exposes an engine, server, observer, wildcard
prelude, or generic fixture abstraction. A third leaf,
`compatibility_macros.rs`, owns only the two root-adapter macros and enters root
scope through its exact `#[macro_use] #[path = "engine_execution/compatibility_macros.rs"] mod compatibility_macros;` declaration.

The dependency graph is acyclic:

```text
engine_execution.rs
  +-- common
  +-- compatibility_macros
  +-- selector_fixture / captured_response
  +-- concern modules
        +-- common
        +-- named support leaves (only where inventoried)
```

No production visibility changes. No concern module depends on another concern
module. `selectors` consumes both value leaves; `subworkflows` and
`retry_references` consume only `selector_fixture`; `replacements` consumes only
`captured_response`. `tests/common/mod.rs` is reused without growth.

### Baseline-to-owner map

The current 96-test and 11-helper inventory is owned by
`plans/assessments/arazzo-runtime-engine-execution-inventory.md`. The plan adopts
this drafting map; acceptance refreshes its names/counts before ticketing:

| Owner module | Concern | Tests |
|---|---|---:|
| `workflow_dependencies.rs` | Dependency admission across execution entrypoints | 6 |
| `workflow_execution.rs` | Ordinary workflow entry and lifecycle behavior | 4 |
| `request_parameters.rs` | Effective workflow and path parameter handling | 2 |
| `action_routing.rs` | Success/failure actions and goto termination | 10 |
| `retry_limits.rs` | Omitted, zero, and custom retry-count policy | 7 |
| `retry_fallthrough.rs` | Exhaustion and later-action traversal | 5 |
| `retry_decisions.rs` | Effective retry decisions in traces and observers | 5 |
| `retry_timing.rs` | Retry waits, deadlines, and cancellation | 4 |
| `retry_references.rs` | Call-and-return retry targets | 5 |
| `subworkflows.rs` | Subworkflow/goto-workflow invocation and typed inputs | 10 |
| `operation_resolution.rs` | OpenAPI source loading and operation resolution | 11 |
| `selectors.rs` | Structured selector normalization and diagnostics | 2 |
| `replacements.rs` | JSON request-body replacement semantics | 6 |
| `dry_run.rs` | Request planning without transport side effects | 3 |
| `execute_step.rs` | Filtered and single-step execution | 10 |
| `runtime_contract.rs` | Public error and internal API-version contracts | 4 |
| `response_limits.rs` | Response-size boundary contracts | 2 |

All 96 bodies stay mechanically unchanged except for imports, `pub(super)` case
visibility, and movement of their test attribute to a root adapter; nothing
becomes `pub(crate)`. Attached docs/non-test attributes stay on their owner item.
Domain-local helpers remain private. The assessment fixes
all helper ownership: retry observers remain with their retry concerns;
operation/testdata and replacement helpers remain local; only `selector` and
`parse_json_body` enter their named support leaves.

### Preserve every accepted test identity with a bounded facade

Each owner exposes a same-named `pub(super)` case without a test attribute. For 86 ordinary
Tokio cases and four synchronous cases, logical `root_async_case!(owner::name)`
or `root_sync_case!(owner::name)` invocations generate root test functions with
the original attribute flavor and name. The six cases referenced by conformance
JSON remain physically written top-level `#[tokio::test]` functions because its
scanner does not expand macros. Every adapter delegates once; no case is itself
test-annotated, so each body executes exactly once in the same harness/runtime
topology as before.

Those numbers describe the current N=96/E=6 snapshot. At acceptance the same
rule becomes N-E macro adapters partitioned by the frozen attribute ledger plus
E physical adapters. The current physical identities are:

- `arazzo_11_retry_limit_counts_omitted_zero_one_and_two`
- `arazzo_11_retry_sites_are_independent_in_serial_and_execute_step_observers`
- `arazzo_11_exhausted_retry_nonmatching_later_actions_returns_stable_limit`
- `retry_after_trace_and_observer_keep_configured_and_effective_delays_distinct`
- `invalid_retry_after_is_rejected_before_exhausted_retry_fallthrough`
- `exhausted_retry_does_not_convert_or_schedule_an_unused_finite_overflow_delay`

The complete current root-name-to-owner ledger lives in the assessment. The guard
verifies every macro/direct adapter and rejects assertions or fixtures in the
facade. This retains byte-identical `--list` names, root-qualified `--exact`
commands, and conformance paths without 96 manually repeated wrapper bodies.

### Isomorphism contract

For every migration slice:

- **Inputs:** the same Arazzo specs, workflow inputs, OpenAPI/testdata files,
  builder options, headers, cancellation handles, clocks, and mock responses.
- **Outputs:** the same results, error kinds/codes/messages, trace records,
  observer events, request captures, counts, values, and authored assertions.
- **Side effects:** the same loopback HTTP calls, background server lifetime,
  fixture reads, waits, notifications, and cancellations in the same
  within-test order.
- **Errors:** no expected error, failure route, zero-request assertion, timeout,
  or retry-exhaustion behavior changes.
- **Ordering/concurrency:** the target remains one process; every Tokio test
  keeps its current macro/runtime flavor; no test task, sleep, timeout, or
  execution mode changes. Source/list order is not treated as execution order.
- **Reachability:** all N root-qualified names and `--exact` commands remain;
  none becomes ignored; the six evidence functions remain physically visible;
  moved modules use only public production surfaces.
- **Allowed structural changes:** explicit local imports, exact `pub(super)`
  case/support visibility, and one adapter call frame per test.
- **Explicitly not preserved:** physical source line numbers/backtraces and
  nondeterministic inter-test start/completion order. Root names, target/process
  topology, within-test sequencing, and test attributes are preserved.
- **Excluded from proof:** any behavior improvement, fixture rewrite, helper
  abstraction beyond the two named support leaves, production change, or split
  of the behavioral target.

If a slice cannot make this card credible, it does not move. A discovered
behavior defect becomes separate work and the original test remains unchanged.

### Required order and Tier 2 decomposition shape

Before acceptance or ticket creation, land or otherwise resolve every ready or
in-progress behavior writer for the file. Under one stable `tkt rev` sandwich,
refresh this assessment and freeze one commit, blob, exact name/item/attribute/
doc-content ledger, and passing baseline. Blocked/paused/deferred forecasts depend on
final ticket 23 and retarget to landed owners afterward; if that cycles or cannot
wait, resolve them before freezing. Never rebase; an inventoried change stops.

All implementation is serialized because slices share target roots. Each
movement slice owns one behavior; each guard slice owns one structural policy;
all are independently green and reviewed before the next. Non-standalone
support lands with its first consumer. The Tier 2 chain is:

1. Extract `response_limits`; establish the async adapter macro and harness.
2. Extract `runtime_contract`; add the now-used synchronous adapter macro.
3. Extract `workflow_dependencies`.
4. Extract `workflow_execution`.
5. Extract `request_parameters`.
6. Extract `action_routing`.
7. Establish `selector_fixture` and extract its first consumer, `subworkflows`.
8. Extract `operation_resolution` with its local testdata builders.
9. Establish `captured_response` and extract its first consumer, `selectors`.
10. Extract `replacements` with its local helpers.
11. Extract `dry_run`.
12. Extract `execute_step`.
13. Extract `retry_limits` and its physical evidence adapter.
14. Extract `retry_fallthrough` and its physical evidence adapter.
15. Extract `retry_decisions`, `RetryDelayObserver`, and four evidence adapters.
16. Extract `retry_timing` with `RetryCancellationObserver`.
17. Extract `retry_references`.
18. Add the layout-target root and `rust_lex` with adversarial scanner tests.
19. Add `rust_items` over the landed lexical contract.
20. Add `facade_policy` and its negative fixtures.
21. Add `ownership_policy` and its negative fixtures.
22. Add `ledger_policy` and its negative fixtures.
23. Reconcile the frozen ledger, update `god-files.md`, and run cumulative
    system verification without adding another code owner.

Each ticket cites the accepted plan/anchors, concrete `reads:`/advisory `writes:`,
pass/fail outcomes, and focused/cumulative proof. In `### File Impact`, movement
tickets Modify the facade only to replace inventoried bodies with adapters;
guard tickets treat it read-only. `common`, production, and unrelated owners are
always must-not-grow. Test files are
exempt from the Create count, not from the independently-shippable-outcome or
lower-cost-context sizing gates. Before each edit, fail closed on unattributed
dirty overlap, run `tkt reservations check --ticket <id> <each declared path>`,
start the ticket, and run `tkt reservations acquire --reason <reason> <id>`;
reservations are inspected separately from `tkt rev`.

Create a dedicated epic and run the mandatory fresh-context plan-to-ticket and
ticket-to-plan transfer audit before implementation because the chain exceeds
five tickets. Preserve unrelated dirty work throughout.

### Anti-recurrence guard

The five-ticket structural tail builds the additive `engine_execution_layout`
Cargo target as a thin `tests/engine_execution_layout.rs` root over five modules:
`rust_lex.rs` (<=300 lines), `rust_items.rs` (<=350), `facade_policy.rs` (<=250),
`ownership_policy.rs` (<=300), and `ledger_policy.rs` (<=250). This is one
structural outcome; its scanner and policies cannot accrete into one file.

`rust_lex`/`rust_items` self-containedly adapt the proven comment/literal
scrubbing, attribute, visibility, delimiter, and item-walk approach in
`conformance_manifest.rs::{scrub_non_code,scan_rust_item_list}`, but emit outer
`///` docs before scrubbing other comments. Negative fixtures cover decoys/malformed input;
the accepted grammar is direct path-module declarations, both adapter-macro
definitions/invocations, direct function/items and outer attributes, visibility, `use`
paths, and balanced bodies. Unknown or malformed forms fail closed. No parser
dependency or cross-crate test-support surface is added.

The policies inspect only both target roots, their exact one-level directories,
and themselves. They enforce:

- an exact behavioral file/declaration allowlist with no orphan, duplicate,
  `include!`, wildcard prelude, nested support module, or facade behavior;
- exactly N-E macro adapters and E physical adapters, each delegating once to
  its ledger owner; every owner case is same-named and `pub(super)`;
- exactly N names/attribute flavors and no ignored/duplicate executable test;
  owner cases have no test/ignore/cfg/should-panic attribute, while every frozen
  non-test attribute/comment content (currently four `allow`, two `derive`, and
  35 doc lines on 11 cases) stays attached to its exact owner item;
- no `pub(super)` item except the N owner cases and two value helpers;
- no concern-to-concern import/reference and exact contents/exports from the
  three support leaves;
- facade <=320 lines, each concern <=800, each support leaf <=80, the guard root
  <=100, and each guard-policy cap above.

The guard does not scan production source, `tests/common`, unrelated targets,
manifests, or general module graphs. Widening it is a new design decision.

### Rejected shapes

- **Separate behavioral integration targets:** change commands, process isolation, scheduling, and evidence paths; not isomorphic.
- **`include!` fragments:** give physical files but not Rust ownership boundaries.
- **Move helpers into `tests/common`:** makes a broad 652-line module the next gravity well.
- **One support/prelude bucket:** recreates hidden cross-domain coupling.
- **One giant move or cleanup-while-moving:** defeats slice-level equivalence proof and Tier 2 sizing.
- **Change conformance JSON paths:** needlessly changes a second test unit and evidence identities.

## Coverage

- **Security engineering:** N/A because this changes test source topology only;
  it adds no runtime input, network destination, credential path, dependency,
  production visibility, persistence, or trust boundary. Verification rejects
  production/config/dependency diffs and preserves loopback-only fixtures.
- **Testing strategy:** Each slice compares the exact qualified-name/attribute/
  owner ledger, runs every moved root name explicitly, then lists and runs the
  full `engine_execution` target and all three repository build gates before
  commit. Evidence-wrapper slices also run `cargo test -p arazzo-cli --test
  conformance_manifest`. Final proof repeats the gates plus serial behavioral
  execution, the separately counted layout target, and frozen-base-to-head review.
- **Concurrent writers:** One implementation owner at a time. Repeat the stable
  revision-sandwich forecast survey, inspect reservations, and acquire the
  slice's declared paths before edits; never stack on an unreviewed candidate.
  New behavior tickets target the landed owner rather than the facade.
- **Rollout / rollback:** Land one mechanically moved slice per reviewed atomic
  commit. Never keep duplicate old/new tests or a fallback harness. On any
  related failure or uncertain proof, revert an independent head slice; if
  later slices depend on it, revert the contiguous suffix newest-first or fix
  forward. The thin facade remains for compatibility after completion.

## Material risks

- A test can be dropped, duplicated, ignored, or separated from an attribute; exact-ledger proof is mandatory.
- Adapter macros can hide a no-op, wrong owner, wrong runtime flavor, or
  duplicate test if treated as boilerplate. The immutable N-entry map, source
  policies, exact `--list` comparison, and explicit moved-name runs cover each
  layer independently.
- Real-time retry/timeout cases can be sensitive to changed process topology or
  scheduling. Retaining one target removes the largest topology risk; default
  and serial final runs provide complementary evidence.
- Resolve feature writers before freezing; afterward, stop on any changed inventoried item or overlap.
- A generic helper or broad layout scanner could become a replacement God
  module. Support exports and each guard-policy module are explicitly bounded.

## System success condition

From one final reviewed candidate:

- `engine_execution.rs` is at most 320 lines and contains only declarations,
  N-E guarded macro invocations, and E physical adapters;
- all N baseline tests, every frozen helper/method item, struct, impl block,
  and every attached attribute are reconciled exactly once to the owner map,
  with no ignored or behaviorally changed case;
- every concern module is reachable in the unchanged `engine_execution` target
  and is below 800 lines;
- the unchanged behavioral target reports the exact same N root names and
  passes them under default parallelism and serial execution; the additive
  structural target is counted separately; and all E frozen conformance
  evidence references validate and execute;
- `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and
  `cargo test --workspace` pass;
- production source, dependencies, features, fixtures, and semantic contracts
  are unchanged; and
- a fresh cumulative review finds no lost reachability, helper gravity,
  duplicated execution, or behavior drift, after which `god-files.md` moves the
  entry from the active table to a dated resolved record.

---

**Disposition:** Draft — architecture is decision-ready. Acceptance and Tier 2 decomposition require a fresh tracker-fenced writer survey and a post-writer baseline freeze; no epic or tickets created.
