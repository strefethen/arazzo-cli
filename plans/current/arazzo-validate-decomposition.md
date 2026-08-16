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
- The file contains 184 executable test items: 176 nested tests plus eight
  crate-root conformance adapters.
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

Diagnostic append order is observable through CLI and JSON output. It is a
contract, not an implementation detail.

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
│   │   └── success_criteria.rs
│   ├── resolution/
│   │   ├── mod.rs
│   │   ├── provenance.rs
│   │   ├── inputs.rs
│   │   ├── parameters.rs
│   │   ├── actions.rs
│   │   └── tests/
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

## Durable source limits

The final layout guard must enforce these crate-scoped limits:

| Surface | Limit |
|---|---:|
| `src/lib.rs` | 150 physical lines |
| Other production Rust file | 600 physical lines |
| Test or test-support Rust file | 1,200 physical lines |
| Test bodies in implementation files | 0 |
| Inline `mod tests { ... }` bodies | 0 |
| New public exports | 0 |
| New dependencies | 0 |

These are guardrails, not permission to create a catch-all file. Fresh review
must still confirm one clear concern per file.

## Work sequence

All implementation is serial because every slice changes the crate root or
shared test scaffolding. Read-only audits and exact-candidate reviews can run in
parallel.

1. Externalize the existing test module verbatim.
2. Extract the public diagnostics contract.
3. Extract raw document-boundary validation and shared action vocabulary.
4. Extract component resolution and structural provenance.
5. Establish the typed-validation shell and extract independent leaf rules.
6. Extract parameter validation.
7. Extract action validation.
8. Extract document/source/component/workflow/step orchestration and the public
   pipeline; make `lib.rs` a facade.
9. Move public-contract tests to the single integration harness.
10. Retire stale conformance adapters and update evidence references.
11. Add the source-layout guard and run the final cumulative parity gate.

Private resolver/provenance tests move with work item 4. The private version
test moves with work item 5. Do not broaden visibility solely for tests.

## Test accounting

Capture `cargo test -p arazzo-validate -- --list` before work item 1.

- Work items 1 through 3 preserve the exact executable test list.
- Work items 4 and 5 provide one-to-one mappings for the four private-test path
  changes. Their count, fixtures, and assertions remain unchanged.
- Work items 6 through 8 preserve the executable test list established by work
  item 5.
- Work item 9 maps all 172 public tests to integration-test paths.
- Six non-test conformance matrix helpers become direct tests in work item 10.
- Two matrices are already direct tests.
- Removing eight adapters removes two duplicate executions and replaces six
  adapter executions with the six direct matrices.
- The expected final behavioral inventory is 182 tests before additive
  source-layout tests.

Every rename or split needs a one-to-one mapping. Passing fewer tests is not
proof of equivalence.

## Model and review policy

Use frontier implementers for the four highest-risk slices:

- component resolution and provenance;
- typed-validation shell and diagnostic ordering;
- action validation;
- final document/workflow/step orchestration and public pipeline.

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

- exact public signature and type-path compatibility;
- exact diagnostic kind, severity, path, message, cardinality, and order;
- component-resolution output and caller non-mutation;
- raw/typed parity and provenance identity;
- complete test-inventory reconciliation;
- all CLI golden, snapshot, schema, and operation-path drift guards;
- no conformance status change;
- a cumulative frontier review from this plan's implementation base.

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
