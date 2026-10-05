# Pest parser kickoff

Date: 2026-10-01
Disposition: implementation handoff pending; no source or tracker changes made.
User intent: “We were looking at using pest with a new grammar can we make progress on that?”

The first useful slice is [ac-80673](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-80673): generate the standalone Runtime Expression parser in `arazzo-expr`, retaining the frozen borrowed API. Follow with [ac-67bf5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67bf5): parse simple-condition syntax using the same grammar. These slices add and verify syntax before later tickets switch evaluator and validator consumers.

## What is already settled

- The September 30 amendment in [the accepted conformance plan](../current/arazzo-1.1-conformance-evidence.md#runtime-expression-parsing-precedes-field-dispatch) records Steve's approval of Pest, shared grammar ownership, the condition syntax table, case-insensitive ABNF keyword literals, and a pre-parse nesting limit of 32.
- Runtime Expression grammar comes from `spec/arazzo/v1.1.0.html#runtime-expressions`. The specification says: “The runtime expression is defined by the following ABNF syntax”. Body references are `"body" ["#" json-pointer]`; condition property access is a separate concern.
- `pest` and `pest_derive` 2.9.1 and `abnf_to_pest` 0.6.0 are already locked through `iregexp-rs`. `crates/arazzo-expr/Cargo.toml` does not yet declare them directly. Promotion at those pins is intended to add dependency edges without adding locked packages.
- The locked `iregexp-rs` revision `e22a35b0d2c25f942f1e4e375525be10d342b7f6` supplies the concrete generation pattern: `build.rs` calls `parse_abnf` and `render_rules_to_pest`, writes `#[grammar_inline]` parser source to `OUT_DIR`, and declares grammar inputs for Cargo rebuilds.

## Live ticket gap

The source of desired behavior and the current ticket contracts disagree:

| Ticket | Current contract | Required reconciliation |
|---|---|---|
| [ac-80673](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-80673) | Hand-written linear scanner and prefix-end iterator | Build-time generated Pest parser; retain public borrowed types, remove prefix iterator, add grammar/build/manifest/test ownership |
| [ac-67bf5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67bf5) | Paused; consumes prefix ends; waits on evaluation decision and validator decomposition | Shared grammar and accepted deterministic operand boundary; follow the Runtime Expression parser without those unrelated gates |
| [ac-ec3a1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ec3a1) | Owns grammar plus evaluation decisions | Narrow to the remaining evaluation decisions |
| [ac-60b0b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-60b0b) | Evaluates parsed conditions | Explicitly depend on the narrowed evaluation decision before changing behavior |

The prior replacement ticket drafts are in [the parser assessment](expression-parser-restructure-draft-2026-09-30.md#6-ticket-drafts). They need a bounded technical review before installation or implementation dispatch. No lifecycle closures are implied by this kickoff assessment.

## First slice and proof

The standalone parser creates `crates/arazzo-expr/build.rs`, the vendored ABNF extraction and decision rules under `grammar/`, `src/runtime_expression.rs`, and a dedicated scaling test. It changes the crate manifest and adds module wiring/re-exports in `src/lib.rs`; it does not switch evaluator/runtime consumers. `lib.rs` is a recorded God module and must acquire no parsing implementation or policy.

The handoff must preserve the frozen public types and accessors, full consumption, exact namespace-specific forms, case-insensitive keywords with case-sensitive identifiers, structured UTF-8 byte errors, and pointer escape rejection. Verify positive and negative namespace tables, generated grammar provenance, API/accessor/error behavior, and adversarial scaling. The condition ticket then adds its grammar, private syntax view, accepted precedence/literals/postfix rules, and nesting boundary proof, while preserving existing evaluation.

Relevant commands: focused `arazzo-expr` tests, expression benchmark compilation, formatting, strict workspace Clippy, and workspace tests. An independent code review must inspect the exact candidate before closure.

## Evidence and remaining gates

- Live `tkt show` and `tkt lint --severity warn` checked both parser tickets on October 1. Lint passes, but the source-to-ticket disagreement prevents an implementation-readiness pass.
- Live reservations check for the expanded first-parser forecast reports `available: true`. No in-progress ticket or reservation is active. Existing dirty documents are preserved.
- The earlier spike's existing release binary was rerun: 96/140 extracted Runtime Expressions and 45/53 extracted conditions parse. The Runtime Expression rejects include dotted-body forms and one accidentally extracted JSONPath selector. This is feasibility evidence only: the existing binary predates the latest spike source, and its entry rules do not enforce the proposed namespace restrictions.
- [Pest dependency decision](../security/decisions/2026-09-30-pest-add.md) and [ABNF converter decision](../security/decisions/2026-09-30-abnf_to_pest-add.md) recommend the existing exact pins, but both remain `deferred`. Resolve the review disposition before executing the direct-dependency promotion.
- Dotted body access outside conditions remains the separate DEC-5 compatibility decision. The first standalone parser can be verified without switching consumers. Evaluator/validator migration must remain gated on that decision and any required fixture/generator/consumer migration.

## Independent review result

The bounded fresh-context review confirmed that the standalone parser slice is independent of DEC-5. It found one concrete correction needed in the existing first-parser draft: the proposed unanchored-prefix check precedes pointer diagnostics. Because `json-pointer` repeats reference tokens, `$response.body#/a~2` can stop successfully immediately before `~2`; the draft would incorrectly report `TrailingInput` instead of `InvalidJsonPointerEscape`.

For the rewritten handoff, anchored grammar failure information for a malformed pointer escape must take precedence over a successful shorter prefix. Add explicit cases pinning `$response.body#/a~2` to `InvalidJsonPointerEscape` over bytes `17..19`, `$inputs.x#/a~2` to the same kind over its offending escape, and `$response.body.status` to `TrailingInput` over the dotted suffix. Valid pointer escapes and valid unrestricted source-reference tails containing `~2` must retain their grammar-defined meaning. Classification must use grammar-derived rules/spans, without another expression scanner. The public error kinds and Display contract remain unchanged.

The later condition handoff also needs to define the pre-scan's depth accounting: distinguish unary `!` from `!=`, end a unary operand's depth when the operand ends, and ensure a flat chain of independent negations does not accumulate artificial nesting. The accepted depth limit remains 32. This is implementation-local clarification; it does not choose condition evaluation semantics.

Review disposition: the reviewer rechecked the affected contract corrections and found no remaining blocker in this bounded pass. The unchanged tickets still need reconciliation, including the diagnostic correction, and the dependency review disposition must be resolved before promotion. Condition syntax remains a follow-on. No implementation-readiness pass is claimed for the unchanged tickets; no candidate implementation has been verified.
