# Draft: Retire the validator decomposition and contain `arazzo-validate` incrementally

**Status:** Approved and applied 2026-10-05. The new epic is
[ac-edfb8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-edfb8) and the guard ticket is
[ac-dc2d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-dc2d4). A fresh-context transfer
audit returned PASS WITH FOLLOW-UPS; it and its follow-ups are recorded on ac-edfb8 and
[ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48). The record below is the
approved draft as applied. The §2 rule text also lives in the repo-local `god-files.md`.
**Date:** 2026-10-05
**Approved direction (2026-10-05):**
1. Option A: lift the "stop validator feature work" freeze.
2. A standing placement rule for the God module.
3. Archive `plans/current/arazzo-validate-decomposition.md`.
4. A `lib.rs` growth guard.

## 1. Why the chain goes

- **The plan never got going.** `plans/current/arazzo-validate-decomposition.md` (accepted 2026-08-16)
  froze validator feature work behind 12 work items. That became 34 serial tickets ending at
  [ac-1a4de](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1a4de). Only work item 1 landed
  (`3afc4be`, 2026-08-25).
- **The plan is stale by its own rule.** Three validator commits have landed since its baseline
  `e252c979`, and the plan says tickets must be refreshed before kickoff when that happens.
- **The God module is smaller than recorded.** `crates/arazzo-validate/src/lib.rs` is 3,784 lines.
  Apart from about 50 lines of inline conformance adapter tests, all of it is production code.
  `god-files.md` still shows 10,881. The concrete hotspot is `collect_diagnostics`, about 650 lines,
  plus 305 functions in one namespace.
- **The incremental pattern already works.** The XPath advisory (`2517b49`) added validator
  behavior in a sibling module, `xpath_advisory.rs`, without growing `lib.rs`.
- **Global policy doesn't require a cleanup project first.**
  `/Users/stevetrefethen/github/agents/LEGACY-MODULES.md` says an unrelated legacy problem "does not
  require a cleanup project before the authorized task can continue".
- **The plan's stated gate was wrong.** It said the `arazzo-validate → arazzo-expr` edge is
  "acyclic after the validator decomposition", but `arazzo-expr` does not depend on
  `arazzo-validate`, so the edge is acyclic today.

## 2. The operating rule (owner: `god-files.md`)

The following replaces the `arazzo-validate` record's disposition in `god-files.md`:

> **Disposition (2026-10-05, Steve-approved): incremental containment.**
>
> Repairs may change code in place. New responsibilities go in a private sibling module under
> `crates/arazzo-validate/src/`. `lib.rs` may gain only module declarations, delegating calls, and
> variants of the existing public diagnostic types. This is a standing approval: an expansion that
> follows the rule needs no further sign-off.
>
> `tests/lib_rs_size_guard.rs` fails when `lib.rs` exceeds `LIB_RS_MAX_LINES` (3,784 at creation).
> The cap may be lowered and never raised without Steve's approval. Every validator ticket's File
> Impact states its expected `lib.rs` line delta.
>
> The decomposition plan is archived at `plans/archive/arazzo-validate-decomposition.md`. Known
> hotspot: `collect_diagnostics`.

## 3. Ticket dispositions

| Ticket(s) | Disposition | Change |
|---|---|---|
| 30 move tickets (list below) | Close as not planned | Reason: "no code change — arazzo-validate decomposition plan archived 2026-10-05; incremental containment per god-files.md replaces the serial re-layout" |
| [ac-da0ff](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da0ff) | Update in place (re-scope) | Standalone bounded extraction of expression validation into `src/expressions.rs` (§6.2). Drop its chain dependencies. Move it to the new epic |
| [ac-ded8d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ded8d) | Update in place (re-scope) | Standalone bounded extraction of Step dependency helpers into `src/step_dependencies.rs` (§6.3). Drop its chain dependencies. Move it to the new epic |
| New guard ticket | Create | §6.1, in the new epic |
| New epic | Create | "Contain the arazzo-validate God module incrementally", with members: guard, ac-da0ff, ac-ded8d |
| [epic:ac-6dee4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-6dee4) | Close as not planned (after moving its two survivors out) | Remove the edge ac-33d48 → ac-6dee4, so the conformance epic no longer waits on the cleanup epic |
| [ac-4cd56](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4cd56) | Update in place (rewrite Design/AC/Testing) | §6.4. Edges: −ac-1a4de, +ac-da0ff |
| [ac-a7fe9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a7fe9) | Update in place (rewrite Design/AC/Testing) | §6.5 |
| [ac-fae45](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fae45) | Update in place (rewrite Design/AC/Testing) | §6.6. Edge: +ac-ded8d |
| [ac-475c6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-475c6) | Update in place (targeted edits) | §6.7. Edge: −ac-1edd3. `plan:` → the conformance plan |
| [ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3) | Update in place (targeted edits) | §6.8. Edge: −ac-1a4de |
| [ac-76ab7](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-76ab7) | Update in place (targeted edits) | §6.9. It now solely owns Step `workflowId` target validation |
| [ac-da7a0](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da7a0) | Update in place (narrow) | §6.10. Hand its `workflowId`-target half to ac-76ab7. Edges: +ac-76ab7, +ac-ded8d |

The 30 move tickets to close:
[ac-d7ea9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d7ea9),
[ac-83e0b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-83e0b),
[ac-c77de](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c77de),
[ac-0f81b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0f81b),
[ac-d41c4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d41c4),
[ac-b2a0e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b2a0e),
[ac-63385](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-63385),
[ac-89d6b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-89d6b),
[ac-2f039](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2f039),
[ac-e9b76](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e9b76),
[ac-3ae90](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3ae90),
[ac-bfae1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bfae1),
[ac-e7c93](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e7c93),
[ac-e0406](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e0406),
[ac-1edd3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1edd3),
[ac-17e4e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-17e4e),
[ac-35c1c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-35c1c),
[ac-42af6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-42af6),
[ac-29a06](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-29a06),
[ac-942ce](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-942ce),
[ac-dfb52](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-dfb52),
[ac-0821e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0821e),
[ac-cd657](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-cd657),
[ac-6fe5a](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-6fe5a),
[ac-caa89](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-caa89),
[ac-327b0](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-327b0),
[ac-4a7ab](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4a7ab),
[ac-dce17](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-dce17),
[ac-8e35e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8e35e),
[ac-1a4de](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1a4de).

Before closing, I checked every member for dependents outside the chain and its epic. Only
ac-1a4de (→ ac-4cd56, ac-d15a3) and ac-1edd3 (→ ac-475c6) have any, and those edges are removed
above. [ac-0149e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0149e) is untouched; it is
real model work and ready now.

## 4. Plan changes

- **Archive** `plans/current/arazzo-validate-decomposition.md` → `plans/archive/` (a new directory,
  defined in SDLC §4), with this line under the title: "**Archived 2026-10-05** (Steve-approved
  Option A): replaced by incremental containment recorded in `god-files.md`. Not implementation
  authority."
- **Amend** `plans/current/arazzo-1.1-conformance-evidence.md`:
  - Line 165: "…accepted and acyclic after the validator decomposition." → "…accepted; it is
    acyclic because `arazzo-expr` does not depend on `arazzo-validate`."
  - Line 550 (slice 12): "through the decomposed validator" → "through the validator, placing
    field-mode enforcement in its sibling expression module per `god-files.md`".
  - Line 582 (ac-fd667 note): "…and follows the validator decomposition" → "…and follows
    [ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3)".
  - Line 589: "It does not wait for the validator decomposition, which gates only validator field
    enforcement." → "The validator decomposition plan was archived on 2026-10-05. Validator work
    follows the incremental containment rule in `god-files.md`."
- **Update** the `god-files.md` record: the row becomes 3,784 lines, about 3,730 production and about
  50 inline (the eight conformance adapters), with the §2 disposition. The section text reflects
  that the test body was externalized by `3afc4be`.

## 5. Queue after apply

- **Ready immediately:**
  - the guard ticket;
  - [ac-da0ff](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da0ff);
  - [ac-ded8d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ded8d);
  - [ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3);
  - [ac-76ab7](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-76ab7);
  - [ac-0149e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0149e), followed by
    [ac-475c6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-475c6) and then
    [ac-4074e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4074e).
- **These five all write `lib.rs`:** the guard, ac-da0ff, ac-ded8d, ac-d15a3 and ac-76ab7. Run them
  one at a time through live reservations; none semantically depends on another. Landing the guard
  first is recommended.
- **Still waiting on conformance work, unchanged:**
  - ac-4cd56 waits on ac-28ba3, ac-e2aaa, ac-5c231, and now ac-da0ff;
  - ac-a7fe9 waits on ac-4cd56 and ac-d15a3;
  - ac-fae45 waits on ac-70691, ac-2f84e, ac-4cd56, and now ac-ded8d;
  - ac-da7a0 waits on ac-f4ea8, and now ac-76ab7 and ac-ded8d.
- **Expected `lib.rs` deltas:**

  | Ticket | Delta |
  |---|---|
  | ac-da0ff | about −300 |
  | ac-ded8d | about −140 |
  | ac-76ab7 | about −30 |
  | ac-a7fe9 | about −40 |
  | ac-475c6 | ≤ 0 (its existing design already requires no net growth) |
  | ac-d15a3 | ≤ 0 |
  | ac-4cd56 | small positive (a provenance bit and delegating calls), absorbed by ac-da0ff's reduction |
  | ac-fae45 | ≤ 0 |

## 6. Ticket drafts

### 6.1 New: Cap arazzo-validate lib.rs at its current size

**Type:** task · **Priority:** 1 · **Plan:** `investigation` (Steve's 2026-10-05 decision, recorded in `god-files.md`) · **Epic:** the new containment epic
**Reads:** `god-files.md`, `crates/arazzo-validate/src/lib.rs`, `crates/arazzo-validate/tests/no_stderr_printing.rs`
**Writes:** `crates/arazzo-validate/tests/lib_rs_size_guard.rs`

#### Goal

Make `cargo test` fail whenever `crates/arazzo-validate/src/lib.rs` grows past its size on
2026-10-05. The recorded God module then cannot regrow while validator work resumes under the
incremental containment rule in `god-files.md`.

#### Design

- Create `tests/lib_rs_size_guard.rs` as a standalone integration target, shaped like
  `tests/no_stderr_printing.rs`, because it guards the source tree rather than behavior.
- `const LIB_RS_MAX_LINES: usize` is set to `include_str!("../src/lib.rs").lines().count()` measured
  at ticket start. That is 3,784 on 2026-10-05. If the measurement is higher, stop and report: the
  cap must not start above the approved size.
- A private helper `assert_within_cap(source: &str, cap: usize)` asserts the count. Its panic message
  states that `lib.rs` is a recorded God module (`god-files.md`), that new responsibilities belong in
  a sibling module, that `lib.rs` may gain only module declarations, delegating calls, and public
  diagnostic variants, and that the constant may be lowered but raising it needs Steve's approval.
- A second test calls the helper with a synthetic over-cap string under `#[should_panic]`, so the
  failure path is proven without editing source files.
- `include_str!` registers `lib.rs` as a build input, so Cargo re-runs the guard whenever `lib.rs`
  changes.
- Non-goals: no lexical scanning, no per-function or per-file limits beyond `lib.rs`, and no change
  to `lib.rs`.

##### File Impact

- Create `crates/arazzo-validate/tests/lib_rs_size_guard.rs`: owns the `lib.rs` line cap and its
  failure message.
- Modify none.
- Must not grow: `crates/arazzo-validate/src/lib.rs` (expected delta 0).

#### Acceptance Criteria

- The guard passes at the measured cap, and the `#[should_panic]` test proves an over-cap source
  fails.
- The failure message names `god-files.md`, the placement rule, and the lower-only cap policy.

#### Testing Obligations

- `cargo test -p arazzo-validate --test lib_rs_size_guard`
- `cargo fmt --all -- --check`, strict workspace Clippy, `cargo test --workspace`

### 6.2 ac-da0ff: re-scoped body

**Title:** Extract typed expression, selector, and criterion validation (unchanged)
**Plan:** `plans/current/arazzo-1.1-conformance-evidence.md` · **Epic:** the new containment epic · **Dependencies:** none
**Reads:** `god-files.md`, `crates/arazzo-validate/src/lib.rs`, `crates/arazzo-validate/src/tests/cases/expressions.rs`
**Writes:** `crates/arazzo-validate/src/lib.rs`, `crates/arazzo-validate/src/expressions.rs`, `crates/arazzo-validate/src/tests/**`

#### Goal

Move the validator's existing expression-bearing validation out of `lib.rs` into a private sibling
module with no behavior change. [ac-4cd56](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4cd56)
then adds field-mode enforcement there without growing the God module.

#### Design

- This is a bounded extraction per LEGACY-MODULES. Move to `crates/arazzo-validate/src/expressions.rs`
  as `pub(crate)` items: `validate_output_value`, `validate_output_step_reference`,
  `validate_value_source`, `validate_selector`, `ExpressionTypeRules`,
  `expression_type_version_supported`, `validate_expression_type`, `validate_replacements`,
  `validate_criterion`, and any private helper used only by them.
- Helpers with other callers stay in `lib.rs` and are imported through `crate::`.
- `lib.rs` declares `mod expressions;` and keeps its existing call sites in `collect_diagnostics`,
  `validate_parameter`, and `validate_actions`.
- Behavior is identical: diagnostic kind, severity, path, message, cardinality, and order are
  unchanged, and parsed/direct results match.
- Existing unit fragments under `src/tests/cases/` keep their assertions. Only import paths may
  change.
- No public API, Cargo, or diagnostic change. Expected `lib.rs` delta: about −300.

##### File Impact

- Create `crates/arazzo-validate/src/expressions.rs`: owns typed expression, selector, criterion,
  replacement, and output validation.
- Modify `crates/arazzo-validate/src/lib.rs`: remove the moved items and add the module
  declaration.
- Modify `crates/arazzo-validate/src/tests/**`: import paths only, if needed.
- Must not grow: `lib.rs`.

#### Acceptance Criteria

- The listed responsibilities exist only in `src/expressions.rs`, and `lib.rs` declares and calls
  that module.
- All existing tests pass with unchanged assertions, and every workspace drift guard passes without
  rebaselining.
- `lib.rs` ends shorter than it started, and no public API or diagnostic changes.

#### Testing Obligations

- `cargo test -p arazzo-validate` (including `lib_rs_size_guard` once it has landed)
- `cargo test -p arazzo-cli --test conformance_manifest`
- The four drift guards, `cargo fmt --all -- --check`, strict workspace Clippy, `cargo test --workspace`

### 6.3 ac-ded8d: re-scoped body

**Title:** Extract typed dependency classification and cycle helpers (unchanged)
**Plan:** `plans/current/arazzo-1.1-conformance-evidence.md` · **Epic:** the new containment epic · **Dependencies:** none
**Writes:** `crates/arazzo-validate/src/lib.rs`, `crates/arazzo-validate/src/step_dependencies.rs`, `crates/arazzo-validate/src/tests/**`

#### Goal

Move Step and Workflow dependency classification and cycle detection out of `lib.rs` into a private
sibling module with no behavior change.
[ac-fae45](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fae45) and
[ac-da7a0](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da7a0) then extend dependency validation
there.

#### Design

- Move `StepDependency`, `classify_step_dependency`, `local_step_depends_on_has_cycle`, and
  `local_workflow_depends_on_has_cycle` (including their inner `visit` helpers) to
  `crates/arazzo-validate/src/step_dependencies.rs` as `pub(crate)` items.
- The call sites in `collect_diagnostics` (`dependsOn` classification and both cycle checks) remain
  and call the module.
- Behavior, diagnostics, order, and tests are unchanged, and there is no public change. Expected
  `lib.rs` delta: about −140.

##### File Impact

- Create `src/step_dependencies.rs`: owns Step and Workflow dependency classification and cycle
  detection.
- Modify `src/lib.rs`: remove the moved items and add the module declaration.
- Modify `src/tests/**`: imports only.
- Must not grow: `lib.rs`.

#### Acceptance Criteria and Testing Obligations

The same as 6.2, applied to the dependency responsibilities.

### 6.4 ac-4cd56: Design edits (the rest of the body is unchanged)

- Replace the paragraph that begins "Implement only after the final validator topology…" with:

  > Implement after [ac-da0ff](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da0ff) has moved
  > expression validation into `crates/arazzo-validate/src/expressions.rs`, plus the corrected
  > classifier from [ac-28ba3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-28ba3) and the
  > simple-condition parser from [ac-67bf5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67bf5)
  > (closed). Add a direct normal dependency from `arazzo-validate` to `arazzo-expr`; it is acyclic.
  > `src/expressions.rs` consumes `parse_runtime_expression` and `classify_value_string`. It must
  > not create a lexer, regex, prefix scanner, or whole-document traversal.

- Change "Apply these exact modes at the existing decomposed append sites:" to "Apply these exact
  modes at the existing append sites in `lib.rs`'s traversal, which delegate to
  `src/expressions.rs`:".
- Change "Preserve the final pipeline's append order…Extend private provenance only where the final
  topology lacks that distinction." to "Preserve the current append order and parsed/direct parity.
  Extend the private `ResolutionProvenance` in `lib.rs` only by the origin bit needed to suppress
  inherited duplicates."
- Replace the File Impact with:
  - Modify `crates/arazzo-validate/Cargo.toml`: add the workspace `arazzo-expr` path dependency.
  - Modify `crates/arazzo-validate/src/expressions.rs`: the sole field-mode enforcement owner.
  - Modify `crates/arazzo-validate/src/lib.rs`: the `ResolutionProvenance` origin bit and any new
    delegating calls only. Expected delta small and positive, within the cap after ac-da0ff.
  - Create `crates/arazzo-validate/tests/cases/expressions.rs`, declared in `tests/cases/mod.rs`:
    focused public parsed/direct matrices.
  - Do not modify action validation, runtime, CLI, MCP, manifests, or logged God tests.
- Acceptance: replace "…and the layout guard accepts that dependency while continuing to reject
  unapproved dependency variants" with "…and no dev, build, or target-specific dependency is added".
- Testing: replace "`cargo test -p arazzo-validate --test source_layout`, validator parity/inventory
  guards" with "`cargo test -p arazzo-validate` (including `lib_rs_size_guard`)".
- Reads: drop the decomposition plan, `validation/expressions.rs`, `resolution/provenance.rs`, and
  `layout_guard/cargo_manifest.rs`. Add `god-files.md`, `src/expressions.rs`, and `src/lib.rs`.
  Writes: as in the File Impact.

### 6.5 ac-a7fe9: Design edits

- Replace "Implement against the final decomposed paths from ac-1a4de, not the God module." with:

  > Move `validate_action_target_references` into a private sibling module,
  > `crates/arazzo-validate/src/action_targets.rs`, in this ticket as a bounded extraction, then
  > correct it there. `action_allows_target` has three other callers and stays in `lib.rs`. Pass the
  > consuming Workflow's Step-ID set from the existing `validate_actions` traversal; do not add a
  > second traversal.

- Replace the File Impact with:
  - Create `src/action_targets.rs`: exact local and action-source target classification.
  - Modify `src/lib.rs`: remove the moved function and keep the delegating call. Expected delta
    about −40.
  - Modify `src/tests/cases/action_targets.rs`: invert `validate_goto_runtime_expression_step_id`,
    which pins the non-spec dynamic Step target.
  - Create `tests/cases/action_targets.rs`, declared in `tests/cases/mod.rs`: public
    parsed/direct, component/use-site, and exact-path matrices.
  - Must not grow `lib.rs`, expression/runtime source, or logged God tests.
- Acceptance: replace "…retain the final validator topology with no inherited duplicates" with
  "…retain current provenance behavior with no inherited duplicates".
- Testing: replace "validator parity/inventory/layout guards" with "`lib_rs_size_guard`".
- Reads/Writes: drop the decomposition plan, `validation/actions.rs`, and `resolution/actions.rs`.

### 6.6 ac-fae45: Design edits

- Replace the paragraph that begins "Implement after ac-4cd56, which itself follows the final
  validator topology…" with:

  > Implement after [ac-4cd56](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4cd56), which adds
  > the `arazzo-expr` dependency and the expression owner's path and source keys, and after
  > [ac-ded8d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ded8d), which moves the dependency
  > helpers into `crates/arazzo-validate/src/step_dependencies.rs`. Extend that module; do not
  > create another graph module.

- Change "At the decomposed Step append site" to "At the existing Step traversal in `lib.rs`, through
  one delegating call into `src/step_dependencies.rs`,".
- Replace the File Impact with:
  - Modify `src/step_dependencies.rs`: implicit edges, unknown-producer answers, deduplication keys,
    and one combined-cycle result.
  - Modify `src/lib.rs`: one delegating call at the Step traversal, plus removal of the old
    explicit-only cycle diagnostic emission. Expected delta ≤ 0.
  - Modify `tests/cases/mod.rs`: declare the case module only.
  - Create `tests/cases/implicit_dependencies.rs`.
  - Must not grow `lib.rs` beyond delegation, logged God tests, runtime/CLI/MCP/DAP, or manifests.
- Testing: replace "validator parity/inventory/layout guards" with "`lib_rs_size_guard`".
- Reads/Writes: drop `validation/dependencies.rs`, `validation/steps.rs`, and
  `validation/expressions.rs`. Add `src/step_dependencies.rs`, `src/expressions.rs`, and
  `src/lib.rs`.

### 6.7 ac-475c6: targeted edits

- `plan:` → `plans/current/arazzo-1.1-conformance-evidence.md`. Remove the dependency on ac-1edd3.
- Goal: delete ", then hand the corrected overlay and parity corpus to the validator decomposition
  instead of extracting the current violation".
- Design:
  - Delete "The accepted decomposition plan now makes this correction a precondition of work item 6."
  - Change "After the typed-leaf predecessor and canonical `arazzo-spec::effective_step_parameters`
    land" to "After the canonical `arazzo-spec::effective_step_parameters` from
    [ac-0149e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0149e) lands".
  - Replace the last paragraph with: "Do not change action Parameter rules, add expression
    validation, modify runtime behavior, or update conformance status in this ticket. This is a
    repair in place in `lib.rs`; its net `lib.rs` delta must be ≤ 0 (`lib_rs_size_guard`)."
- File Impact:
  - The Create entry becomes `crates/arazzo-validate/tests/cases/workflow_parameter_inheritance.rs`,
    declared in `tests/cases/mod.rs`, inside the existing integration harness rather than a new
    Cargo target.
  - "Must not create final decomposition modules" becomes "Must not create new production modules".
- Acceptance: drop "module extraction," from the last bullet.
- Testing: the target becomes `cargo test -p arazzo-validate --test validation
  workflow_parameter_inheritance`.

### 6.8 ac-d15a3: targeted edits

- Remove the dependency on ac-1a4de.
- Append to the Design:

  > Validator placement (`god-files.md` rule): move `arazzo` version validation into a private
  > sibling module, `crates/arazzo-validate/src/version.rs`. That covers the `spec.arazzo` checks at
  > the top of `collect_diagnostics` and `declares_pre_1_1`. The module owns version syntax and
  > feature-set classification, and `lib.rs` delegates to it. Add the `sourceDescriptions[].url`
  > URI-reference check at the existing `{path}.url` required-field site. The net `lib.rs` delta
  > must be ≤ 0. Runtime tests go in a focused target, not the logged `engine_execution.rs`.

- Writes: replace `crates/arazzo-runtime/tests/engine_execution.rs` with a focused runtime target, and
  add `crates/arazzo-validate/src/version.rs`. Normalize the legacy absolute paths with
  `tkt upgrade tickets --paths-only`.

### 6.9 ac-76ab7: targeted edits

- Append to the Design:

  > Validator placement (`god-files.md` rule): move the Step-target checks (the `match &step.target`
  > block in `collect_diagnostics`) into a private sibling module,
  > `crates/arazzo-validate/src/step_targets.rs`, and add the `workflowId` reference checks there.
  > `lib.rs` keeps one delegating call plus the new `ValidationErrorKind::UnsupportedWorkflowId`
  > variant and its string. Expected `lib.rs` delta: about −30. This ticket solely owns Step
  > `workflowId` target validation, including the case-sensitive local-target rule that
  > [ac-da7a0](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-da7a0) previously also claimed.

- Testing: "Table-driven cases in `crates/arazzo-validate/src/tests/cases/`" becomes "Public
  table-driven cases in `crates/arazzo-validate/tests/cases/step_workflow_targets.rs`". Add a
  case-mismatched local target row.
- File Impact: Create `src/step_targets.rs`. Writes: add it, and normalize the absolute paths.

### 6.10 ac-da7a0: narrowing edits

- Goal: "Validate Step Object workflow targets and cross-scope Step `dependsOn` references…" becomes
  "Validate cross-scope Step `dependsOn` references…". Step `workflowId` targets belong to
  [ac-76ab7](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-76ab7).
- Design:
  - Change "Add one structured classifier for Step `workflowId` targets and explicit `dependsOn`
    entries" to "Add one structured classifier for explicit Step `dependsOn` entries".
  - Delete the "For a valid local `workflowId` target…" sentence.
  - Append: "Validator changes extend `crates/arazzo-validate/src/step_dependencies.rs`
    ([ac-ded8d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ded8d)), and `lib.rs` gains only
    delegating calls. Reuse the cross-document warning kind and wording introduced by ac-76ab7.
    Runtime and CLI evidence go in focused targets, not the logged `engine_execution.rs` or
    `cli_integration.rs`."
- Acceptance: delete the first bullet (local `workflowId` targets), which now belongs to ac-76ab7.
- Dependencies: add ac-76ab7 and ac-ded8d. Normalize the absolute paths.

## 7. Apply sequence (after approval)

1. Docs commit: archive the plan, apply the four conformance-plan edits, and update `god-files.md`
   (showing you the diff before committing).
2. `tkt`:
   - create the epic and the guard ticket;
   - epic-move ac-da0ff and ac-ded8d into the new epic;
   - rewrite and edit the nine tickets in §6 with revision guards;
   - make the edge changes in §3;
   - close the 30 move tickets, then ac-6dee4;
   - normalize the legacy absolute paths.
3. Run `tkt lint --severity warn` on every touched ticket.
4. Fresh-context transfer audit of this restructure, recorded on the new epic and on
   [ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48).
5. Optional follow-up, not included: one ticket to split `collect_diagnostics` by domain, if wanted
   later.
