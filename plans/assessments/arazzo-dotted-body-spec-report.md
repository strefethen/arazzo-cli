# Arazzo 1.1.0 body/payload grammar and published examples

Local upstream-report draft, 2026-10-01. Not submitted or published.
Decision authority: [ac-f1e14](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-f1e14)
and the accepted plan's [DEC-5 A row](../current/arazzo-1.1-conformance-evidence.md#canonical-standalone-body-and-payload-references).
Steve agreed to “A: canonical syntax, helpful authoring feedback, and migration
before enforcement.” This report does not claim implementation or conformance
completion.

## Draft upstream report

Suggested title: **Align standalone body/payload examples with the 1.1.0 Runtime
Expression grammar**.

The published grammar and several published examples disagree about standalone
body/payload member access. All normative citations below refer to the vendored,
unmodified publication, not repository implementation or examples.

Section 5.9 [Runtime Expressions](../../spec/arazzo/v1.1.0.html#runtime-expressions)
states: “The runtime expression is defined by the following ABNF syntax:”

```abnf
body-reference = "body" ["#" json-pointer ]
payload-reference = "payload" ["#" json-pointer ]
json-pointer = *( "/" reference-token )
escaped = "~" ( "0" / "1" )
```

The publication comments that the escaped forms represent `'~' and '/',
respectively`. Section 5.9.1's [Examples table](../../spec/arazzo/v1.1.0.html#examples)
uses pointer forms, including `$response.body#/status`.

Conflicting examples in the same publication are:

| Published location | Exact expression | Required correction under the grammar |
|---|---|---|
| Section 5.8.11.3 [Runtime Expressions in Conditions](../../spec/arazzo/v1.1.0.html#runtime-expressions-in-conditions), regex example's `context` | `context: $response.body.status` | `context: $response.body#/status` |
| Section 5.8.1.2 [Arazzo Specification Object Example](../../spec/arazzo/v1.1.0.html#arazzo-specification-object-example), asynchronous `confirmPetPurchaseStep.outputs` | `orderId: $message.payload.orderId` | `orderId: $message.payload#/orderId` |
| Section 5.8.5.4 [Step Object Examples](../../spec/arazzo/v1.1.0.html#step-object-examples), asynchronous `confirmOrder.outputs` | `orderId: $message.payload.orderId` | `orderId: $message.payload#/orderId` |

These are complete standalone expressions in output/context fields. Their
location in a condition-related or asynchronous example does not make them
simple-condition operator expressions. Section 5.8.11.2
[Operators](../../spec/arazzo/v1.1.0.html#operators) separately labels `.` as
“Property de-reference” and `[]` as “Index (0-based)”. Section 5.8.11.4.1
[Simple Conditions](../../spec/arazzo/v1.1.0.html#simple-conditions) says the
condition “MUST be an expression that combines Runtime Expressions, literals,
and operators.” Therefore `$response.body.items[0].id == 7` can use condition
postfix operators without making `$response.body.items[0].id` a standalone
Runtime Expression.

Requested upstream clarification: correct the three standalone examples to
pointers and explicitly distinguish standalone body/payload suffix grammar
from simple-condition property/index operators. This local draft proposes no
new compatibility production. If upstream instead intends a broader standalone
grammar, it would need an explicit grammar and semantics change; that is not
assumed here.

## Accepted local behavior and author guidance

The following are desired outcomes, not claims that all current consumers
already enforce them. A validation `invalidExpression` is a structured field
error. The direct evaluator's existing null-plus-one-warning API is a separate
contract; retaining it does not permit silent output/context execution.

| Site | Input | Accepted outcome / migration |
|---|---|---|
| Step/Workflow output | `$response.body.status` | One `invalidExpression`; suggest `$response.body#/status` |
| Criterion/Selector context | `$request.body.customer.id` | One `invalidExpression`; suggest `#/customer/id` if these are nested members, or `#/customer.id` for one literal property |
| Message output | `$message.payload.orderId` | One `invalidExpression`; suggest `$message.payload#/orderId` |
| Any required expression | `$response.body`, `$response.body#`, `$response.body#/status` | Canonical syntax; bare or empty pointer selects the whole body |
| Parameter or payload value intended to execute | `$response.body.status` | Migrate to `$response.body#/status` before value-classifier cutover; unbraced invalid-expression text is otherwise a literal by the accepted field mode |
| Deliberate Parameter/payload literal | `$USD`, `$response.body.status`, `pay $inputs.x` | Preserve literal bytes; no blanket leading-dollar rejection |
| Parameter template | `status: {$response.body.status}` | Reserved malformed candidate; diagnostic with no partial evaluation; author may use `status: {$response.body#/status}` |
| Simple condition | `$response.body.items[0].id == 7` | Property/index syntax remains accepted; evaluation follows its separately accepted semantics |
| Pointer to punctuation-bearing key | `$response.body#/a.b` | Select one property named `a.b`; `#/a/b` instead selects nested `a` then `b` |
| Pointer to escaped key | `$response.body#/a~0b~1c` | Select one property named `a~b/c`; encode `~` before `/` |
| Standalone wildcard/filter/bracket path | `$response.body.items[*].id`, `$response.body.items.#`, `$response.body['a.b']` | Reject as executable standalone expressions; provide explicit-pointer/no-auto-conversion guidance, without guessing field identity or cardinality |

Only known property tokens can be safely encoded. Ordinary member-chain hints
are suggestions conditional on author intent, not automatic rewrites.
`#/a~2b` remains an invalid escape. Legitimate dotted named inputs/outputs such
as `$inputs.foo.bar` still mean one name under the accepted identifier grammar.
The canonical parser's error kind/range/Display remain unchanged; one shared
diagnostic-only helper supplies optional author guidance.

## Current migration ownership and safe order

Current-source inspection on 2026-10-01 confirms
`crates/arazzo-generate/src/crud.rs::build_step` emits dotted response outputs;
`README.md` Runtime Expressions guidance, `docs/index.html`'s auth example,
and authored workflow/test fixtures also contain standalone dotted references.
`crates/arazzo-expr/src/lib.rs::resolve_body_value` routes dot/bracket suffixes
to the legacy traversal. These facts identify migration work only.

1. [Generator outputs](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a9bea),
   [authored docs/fixtures](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e2aaa),
   and [shared diagnostic hints](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5c231)
   are prerequisites for evaluator and validator consumer enforcement.
2. [Evaluator cutover](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9eaf1)
   retains standalone legacy traversal removal and its intentional wildcard
   regression controls. [Classifier](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a48b1)
   preserves literal-value classification; [validator](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4cd56)
   owns required-field rejection and exact field diagnostics.
3. [Safe cutover ordering](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2997b)
   is a planning-only blocker: `simple_condition.rs::resolve_operand_with_diagnostics`
   currently passes raw postfix operands to the public evaluator. The planned
   [condition evaluator](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-60b0b)
   depends on the evaluator's parsed-dispatch seam. A reverse dependency would
   cycle, and a release-only gate would not protect intermediate commits.
   Resolve the boundary/order before code handoff; no fallback or condition
   semantics are selected by this report.
4. Exact grammar may be marked `covered` only after positive and negative
   executable integration evidence. Today's accepted policy leaves F13's
   current legacy behavior as deviation debt, with no full-conformance claim.

The September 30 [ecosystem assessment](expression-parser-restructure-draft-2026-09-30.md#ecosystem-check-2026-09-30)
is historical evidence. Its tool behavior, upstream issue positions and counts
were not refreshed in this decision; the vendored grammar/example contradiction
is sufficient to support the selected policy.

## Cross-repository impact: Sonos consumers

The September 30 assessment recorded **170 dotted output references across 20
sonos-hub provider files**, plus one condition reference and 168 pointer forms.
These are dated counts, not a current inventory or current runtime proof.

Before those consumers upgrade to a release enforcing exact standalone syntax,
their owning repository must separately authorize and track the migration,
recheck current provider documents and field modes, migrate executable output
and context references, and verify the affected outputs against their intended
response shapes. Preserve legitimate condition postfix and literal-value text.
Record the actual owning ticket and successful verification as release/upgrade
evidence there. Until that evidence exists, upgrading affected first-party
consumers is gated; do not infer cross-repository completion from local tests.

No cross-repository ticket edge, tracker write, code edit, deployment or
publication was performed by this decision. This impact note is not a substitute
for the owning repository's authorization or acceptance evidence.
