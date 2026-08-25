# Arazzo Validate Decomposition Plan

**Accepted:** 2026-08-16
**Baseline:** `e252c979af1eca064af757f0c71559bc172f116d`
**Status:** Accepted for the next epic; implementation has not started
**Scope:** Behavior-preserving decomposition of `crates/arazzo-validate`

## Decision

Stop further validator feature work until `arazzo-validate` has explicit source
and test ownership. The current behavior and tests are valuable and remain in
place, but the physical architecture is not acceptable for more accretion.

At the baseline:

- `crates/arazzo-validate/src/lib.rs` has 10,881 physical lines.
- About 3,700 lines are production code.
- About 7,200 lines are test-only code.
- The library unit-test target contains 184 executable items: 176 nested tests
  plus eight crate-root conformance adapters. The package also has one separate
  `no_stderr_printing` integration guard, so the package baseline is 185 tests
  and zero doctests.
- Four tests require private implementation access. The other 172 can test the
  public crate API.
- The production region owns diagnostics, the parse pipeline, raw wire checks,
  component resolution, structural provenance, and every typed validation
  domain.

The existing tests are not linked into normal library builds because they use
`#[cfg(test)]`. That fact does not make the source layout maintainable.

## Invariants

This epic is an isomorphic refactor. It must not:

- change accepted or rejected Arazzo documents;
- reinterpret the vendored specification;
- add an extension, fallback, or degraded mode;
- change a public function, type, variant, field, import path, error source, or
  display string;
- change diagnostic kind, severity, path, message, cardinality, or order;
- change raw-versus-typed diagnostic ownership;
- change component resolution, structural identity, provenance, legacy
  compatibility, or caller-mutation behavior;
- add a package or expose an internal resolver to make tests compile;
- rebaseline a golden, snapshot, schema, or conformance status.

The behavior-preservation boundary does not turn a known specification
violation into an invariant. Before work item 6 extracts parameter overlays, a
separate conformance ticket must correct workflow-level Parameter inheritance
for `workflowId` Steps in the current owner and add the new behavior to the
parity corpus. Work item 6 then preserves that corrected candidate. Its
post-correction candidate, rather than the original behavior of this one rule
at `e252c979`, is the semantic baseline for work items 6 through 12.

Diagnostic append order is observable through CLI and JSON output. It is a
contract, not an implementation detail.

### Normative freeze

This plan does not decide or extend document semantics. It moves the current
implementation of these vendored Arazzo 1.1.0 rules without changing it:

- `sourceDescriptions`: “REQUIRED. A list of source descriptions (such as an
  OpenAPI description) this Arazzo Description SHALL apply to. The list MUST
  have at least one entry.” (`spec/arazzo/v1.1.0.html#arazzoSources`)
- `workflows`: “REQUIRED. A list of workflows. The list MUST have at least one
  entry.” (`spec/arazzo/v1.1.0.html#workflows`)
- Workflow `steps`: “REQUIRED. An ordered list of steps where each step
  represents a call to an API operation or to another workflow.”
  (`spec/arazzo/v1.1.0.html#workflowSteps`)
- Step `successCriteria`: “If `successCriteria` is provided, it MUST contain at
  least one Criterion Object.” (`spec/arazzo/v1.1.0.html#stepSuccessCriteria`)
- Reusable Object `reference`: “REQUIRED. A Runtime Expression used to
  reference the desired object.” Its `value` says, “Sets a value of the
  referenced parameter. This is only applicable for parameter object
  references.” (`spec/arazzo/v1.1.0.html#reusableObjectReference`)

Current source and tests define the behavior that this isomorphic refactor must
preserve, including known deviations and compatibility rules. The vendored
specification prevents a move from silently inventing a new rule; it does not
authorize conformance changes in this epic.

## Target ownership

```text
crates/arazzo-validate/
├── src/
│   ├── lib.rs                         # declarations and public re-exports only
│   ├── diagnostics.rs                 # public diagnostic/error contract
│   ├── pipeline.rs                    # public parse/validate entry points
│   ├── action_context.rs              # shared pure action vocabulary
│   ├── raw/
│   │   ├── mod.rs
│   │   ├── access.rs
│   │   ├── reusable_values.rs
│   │   ├── required_values.rs
│   │   ├── actions.rs
│   │   └── required_collections.rs
│   ├── resolution/
│   │   ├── mod.rs
│   │   ├── provenance.rs
│   │   ├── inputs.rs
│   │   ├── parameters.rs
│   │   ├── actions.rs
│   │   └── tests/                     # private resolver tests + test-only support
│   └── validation/
│       ├── mod.rs                     # ordered delegation only
│       ├── document.rs
│       ├── sources.rs
│       ├── components.rs
│       ├── workflows.rs
│       ├── steps.rs
│       ├── version.rs
│       ├── dependencies.rs
│       ├── parameters.rs
│       ├── actions.rs
│       ├── expressions.rs
│       ├── identifiers.rs
│       ├── extensions.rs
│       └── tests/
└── tests/
    ├── validation.rs                  # one integration-test binary
    ├── no_stderr_printing.rs
    ├── source_layout.rs
    ├── layout_guard/
    │   ├── mod.rs                     # thin guard orchestration
    │   ├── rust_source.rs             # fail-closed Rust lexical scan
    │   ├── source_tree.rs             # file classification and limits
    │   ├── public_surface.rs          # root public allowlist
    │   ├── cargo_manifest.rs          # dependency and target contract
    │   └── module_graph.rs            # exact test-module reachability
    ├── support/mod.rs
    └── cases/*.rs
```

Use one integration harness with submodules. Do not create one Cargo test binary
per validation domain.

The dependency direction is:

```text
diagnostics + action_context + raw access
                  ↓
        raw boundary contracts
                  ↓
       resolution + provenance
                  ↓
        typed validation domains
                  ↓
               pipeline
                  ↓
              lib facade
```

Typed validation consumes provenance data but does not call resolution.
Resolution can use pure raw-presence helpers but does not call typed validation.
`pipeline` is the only owner of parse, resolve, validate, diagnostic composition,
and error/warning partition order.

Some legacy-action findings need both raw field presence and the resolved
component action type. They are not raw-only checks. `resolution/actions.rs`
owns their diagnostic emission, consumes only pure access/classification helpers
from `raw` and `action_context`, and records the findings in provenance for the
pipeline to compose. This is the only raw-to-resolution diagnostic seam.
Provenance keeps this parse-only channel separate from the direct-only reusable
value channel and exposes named single-consumer operations for each; it does
not expose a generic mutable diagnostics vector.

The observable phase order is mode-specific and must remain exact:

- parse APIs: raw YAML parse, typed parse, component resolution, typed
  diagnostics, resolution-time raw legacy-action diagnostics, raw required-value
  diagnostics, raw action-boundary diagnostics, the interleaved raw
  required-collection and success-criteria diagnostics, then stable
  error/warning partitioning;
- direct typed APIs: clone, component resolution, direct reusable-value
  diagnostics, typed diagnostics, then stable error/warning partitioning.

A parse or component-resolution error short-circuits before later diagnostic
phases. The dependency diagram is not a substitute for this runtime order.

Typed traversal also has nonuniform action schedules that must not be
normalized. Component action definitions append fixed-field findings before
parameter findings. Workflow and step use-site actions append parameter
findings before fixed-field, target, and criterion findings. Component and
workflow action lists visit success before failure; each step visits
`onFailure` before `onSuccess`.

State lifetime is part of the typed contract. Pre-collect all nonempty workflow
IDs for forward references but use a separate incremental uniqueness set. For
each workflow, pre-collect all nonempty step IDs before actions but use a
separate incremental step uniqueness set. Keep one run-scoped parameter
context state. Every Step target inherits workflow parameters in source order;
step declarations override inherited declarations by exact case-sensitive
`(name, in)` identity but cannot remove them. This includes `workflowId` Steps,
as required by Arazzo 1.1 Workflow and Step Parameter Objects. Preserve the
current source-type lookup behavior
in which every source entry updates the map, so the last duplicate or empty-name
entry wins.

## Durable source limits

The final layout guard must enforce these crate-scoped limits:

| Surface | Limit |
|---|---:|
| `src/lib.rs` | 150 physical lines |
| Other production Rust file | 600 physical lines |
| Test or test-support Rust file | 1,200 physical lines |
| Test bodies in implementation files | 0 |
| Inline `mod tests { ... }` bodies | 0 |

These are guardrails, not permission to create a catch-all file. Fresh review
must still confirm one clear concern per file. Work item 1 applies the test-file
limit immediately; it must not replace the inline test body with one temporary
7,000-line file.

No new public export and no new direct dependency are epic parity constraints,
not facts that a final-state source-layout scanner can infer without a named
baseline. Calibrate explicit durable allowlists from the immutable epic source
baseline: the six root functions and six diagnostic/error types, plus the
three normal dependencies `arazzo-spec`, `iri-string`, and `serde_yaml_ng` with
their exact specifications and no dev, build, or target-specific dependency
entries. The layout guard classifies `src/**/tests/**` and
top-level `tests/**` as test code; other `src/**/*.rs` files are production. It
also proves every non-root `tests/**/*.rs` module is reachable exactly once from
exactly one allowed integration root. Behavior `cases` and `support` belong only
to `validation`; `layout_guard` belongs only to `source_layout`; the no-stderr
root owns no child module. No child can be shared across roots. Every
`src/**/tests/**/*.rs` module is reachable exactly once from an allowed
owner-module declaration. The guard permits `#[cfg(test)]` in production only
on an external `mod tests;` declaration.

## Work sequence

Implementation is serial because each slice consumes the transient ownership
and test layout produced by its predecessor, and adjacent slices overlap on the
orchestration or test harness. Read-only audits and exact-candidate reviews can
run in parallel.

1. Externalize and physically shard the existing test module. Keep a small
   `src/tests/mod.rs` that expands ordered, domain-owned fragments with
   `include!`, so every current `tests::<name>` path, helper scope, and root
   adapter call remains exact without creating a replacement God file. Each
   fragment is a contiguous sequence of complete top-level items from the
   original body, is at most 1,200 lines, and has one clear test-domain owner.
   The ordered concatenation of fragment bytes must reproduce the original
   module body exactly. This include-only scaffold is temporary: work item 9
   converts ordinary public fragments to integration submodules and work item
   11 removes the remaining unit fragments and index.
2. Extract the public diagnostics contract.
3. Extract raw document-boundary validation and shared action vocabulary.
4. Extract component resolution and structural provenance.
5. Extract independent typed-validation leaf rules behind small owned modules.
   Keep `collect_diagnostics` in the legacy crate root until work item 8 can
   split its orchestration by domain; do not create an interim oversized
   `validation/mod.rs`.
6. Extract parameter validation while root orchestration still owns traversal.
7. Extract action validation while root orchestration still owns traversal.
8. Extract document/source/component/workflow/step traversal and ordered typed
   orchestration. Keep every public entry point and diagnostic composition in
   `lib.rs`; this slice does not also migrate the public pipeline.
9. Move 163 ordinary public baseline tests, plus all named additive public
   characterization tests, to the single integration harness. Keep the two
   already-direct conformance matrices, six non-test conformance matrix helpers,
   seven standalone tests called by the parameter matrices, their helper
   closure, and eight adapters in the sharded unit harness for work item 11.
   When a non-test helper or fixture is needed by both a moved test and the
   retained closure, keep an exact bounded unit copy and create the necessary
   integration-test copy. Record both consumers in the movement ledger. Do not
   duplicate a `#[test]` item, broaden production visibility, or use a cross-crate
   `#[path]` shortcut. The inheritance correction precondition temporarily adds
   `tests/workflow_parameter_inheritance.rs`; this item moves every test from
   that standalone target into `tests/cases/parameter_context.rs`, records each
   as an additive old-target-to-new-target mapping, and deletes the temporary
   root. It must not survive into the final three-root integration topology.
10. Move public parse/validate entry points, raw/typed parse coordination,
    resolution invocation, mode-specific diagnostic composition, and stable
    partitioning to `pipeline.rs`. Use the external integration contract to
    prove root re-exports. Leave the eight temporary adapters as the only
    non-facade items in `lib.rs`.
11. Move all eight conformance matrix bodies, the seven standalone tests called
    by the parameter matrices, and their helper closure from the remaining unit
    fragments to integration domain case files.
    Add eight stable evidence entrypoints directly to `tests/validation.rs`,
    remove `#[test]` from the two matrix bodies that were already tests, remove
    the eight crate-root adapters, update evidence to the integration-root
    entrypoints, remove every temporary unit-side helper/fixture copy, and
    complete the `lib.rs` facade.
12. Add the decomposed source, Cargo, and module-topology guard and run the
    final cumulative parity gate.

The three private resolution tests are
`parse_bytes_component_action_explicit_end_overrides_component_type`,
`direct_reusable_string_values_preserve_parameter_overrides`, and
`component_origin_indices_are_stable_across_btree_clones`; they move with work
item 4. `version_gate_reads_major_minor_not_a_prefix` moves with work item 5.
The three resolution tests call neutral test builders that also remain in use by
public/unit tests. Work item 4 copies only their minimal test-support closure into
`resolution/tests/support.rs`, records both owners, and leaves production
visibility unchanged. `version_gate_reads_major_minor_not_a_prefix` needs no
such shared support. Do not broaden visibility solely for tests.

The seven temporarily retained public tests are
`direct_validation_component_reference_positive_matrix`,
`direct_validation_component_reference_negative_matrix`,
`unused_component_actions_enforce_parameter_contract`,
`workflow_input_duplicates_use_name_only_and_report_every_in`,
`async_operation_parameters_fail_closed`,
`inherited_parameter_context_diagnostics_have_stable_cardinality`, and
`component_action_reference_provenance_is_stable`. The two parameter matrix
helpers call them directly, so moving them to a separate integration crate
before the matrix closure moves would not compile.

## Test accounting

Capture the baseline by Cargo target, not as one ambiguous package total:

- `cargo test -p arazzo-validate --lib -- --list`: 184 library executions;
- 176 are nested tests and eight are crate-root adapters;
- four nested tests need private access, 170 are ordinary public tests, and two
  are already-direct conformance matrices;
- six other conformance matrices are helper functions executed by adapters;
- `cargo test -p arazzo-validate --test no_stderr_printing -- --list`: one
  separate guard;
- doctests: zero.

The eight adapters execute six helper-only matrices and re-execute the two
already-direct matrices. Two parameter matrix helpers also call seven
standalone tests. The baseline count is therefore 184 top-level library test
items; it is not a count of unique assertion-body executions. The final 182
figure below is also a top-level baseline-item inventory, not a claim that
composite matrices execute no shared assertions.

- Work items 1 and 2 preserve the exact 184-item library list; work item 2 adds
  the separately inventoried external API contract target.
- Work items 3 through 8 may add only the named order, state-lifetime, lookup,
  and short-circuit characterization tests required by their tickets. Record
  every addition separately; never use one to hide a lost baseline test.
- Work items 3 through 8 preserve every baseline execution plus every earlier
  additive test. Work items 4 and 5 map the four private-test path changes.
- After work item 9, the baseline inventory is 163 ordinary integration tests
  plus 21 library tests: four private tests, seven standalone matrix-support
  tests, two direct matrices, and eight adapters. Named additive public tests
  move with the ordinary tests, including every inheritance-correction test
  moved from and removed with the temporary
  `workflow_parameter_inheritance` Cargo target.
- Work item 10 preserves that inventory while the public pipeline moves.
- After work item 11, the baseline inventory is 178 integration tests (170
  ordinary tests plus eight integration-root evidence entrypoints) and four
  private library tests. Each matrix has one top-level entrypoint; the existing
  parameter-matrix calls to seven standalone tests remain part of its semantics.
- The existing no-stderr guard and additive source-layout tests are separate
  non-behavior targets. Do not fold them into the 182 top-level baseline-item
  count.

Maintain a reviewed ledger with old Cargo target and fully qualified name,
ignored state, baseline-or-additive classification, disposition, and new target
and name. Every baseline item maps exactly once, no item is ignored, and every
additive item is explicit. A smaller aggregate count is not proof of
equivalence.

## Model and review policy

Use frontier implementers for the eight highest-risk slices:

- raw boundary ownership and cross-phase ordering;
- component resolution and provenance;
- typed-validation shell and diagnostic ordering;
- parameter context and run-scoped deduplication;
- action validation;
- document/workflow/step orchestration;
- the public pipeline and facade;
- the fail-closed source, Cargo, and module-topology guard.

Use Terra implementers for the remaining settled mechanical slices. Every
candidate receives an independent frontier exact-commit review. Any blocking
review correction is made by a frontier model and receives a fresh cumulative
frontier re-review.

## Verification contract

Every work item must pass:

- the item-specific focused tests;
- `cargo test -p arazzo-validate`;
- `cargo test -p arazzo-cli --test conformance_manifest`;
- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --workspace`;
- exact-candidate scope and cached-diff checks;
- fresh frontier review.

The final item also proves:

- an external compile-contract test covers all six public entry points and the
  complete root diagnostic/error type surface;
- for Workflow-parameter inheritance by `workflowId` Steps, the exact reviewed
  diagnostic tuples and test dispositions match the correction candidate that
  precedes work item 6; its evidence note enumerates every intentional
  kind/severity/path/message/cardinality/order delta from `e252c979`;
- for every unaffected rule, no differences are observed in diagnostic kind,
  severity, path, message, cardinality, and order against `e252c979` over the
  preserved tests and named differential corpus;
- no differences are observed in component-resolution output, caller
  non-mutation, raw/typed parity, or provenance identity over that corpus;
- complete test-inventory reconciliation;
- all CLI golden, snapshot, schema, and operation-path drift guards;
- no conformance status change;
- a cumulative frontier review from this plan's implementation base.

Supported public compatibility means downstream source imports, function
signatures, public fields and variants, methods, trait implementations,
`Display` text, and error-source behavior. Defining-module strings exposed only
through compiler diagnostics, rustdoc internals, or `type_name` are not stable
API and are outside this compatibility claim. Every new internal module remains
private; only the existing crate-root surface is re-exported.

At kickoff, verify that the affected source and evidence paths still match
baseline `e252c979af1eca064af757f0c71559bc172f116d`. After the inheritance
correction lands, record its exact reviewed candidate hash, tuple delta, and
test-root inventory before work item 6 starts. That candidate is the comparison
baseline only for the corrected inheritance rule; `e252c979` remains the
baseline for every unaffected surface. If either fence does not match, refresh
the plan and tickets before implementation. The cumulative reviewer receives
both baselines, every candidate hash and movement ledger, the explicit allowed
delta, and the base-to-head diff. A cumulative finding routes back to the owning
earlier ticket for remediation and fresh exact-candidate review before the final
gate runs again.

## Epic ordering

The cleanup epic is the next implementation epic. It does not depend on the
halted conformance epic.

After authoring the graph:

- gate `ac-33d48` on the cleanup epic;
- gate revised `ac-67bf5` on the cleanup final parity ticket;
- gate validator writer `ac-d15a3` on the cleanup final parity ticket;
- revise other open validator tickets against the final module paths before
  they start.

Do not start the cleanup epic or a member as part of planning.
