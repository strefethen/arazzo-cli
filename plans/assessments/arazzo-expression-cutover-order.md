---
status: assessed
epic: epic:ac-33d48
created: 2026-10-07
---

# Pest evaluator cutover: evidence and approval record

Steve approved the full package on 2026-10-07, including bounded numeric
representation, the explicit null ambiguity, structural comparison order, and
private resolver → literal renderer → atomic public/condition/runtime/debug
cutover. Ongoing authority is solely in the
[accepted conformance plan](../current/arazzo-1.1-conformance-evidence.md#accepted-simple-condition-evaluation-contract-2026-10-07).
This assessment is evidence/history, not a second implementation contract.

**Seed:** “Prepare one complete, recommended evaluation contract and safe cutover
sequence, keeping scope bounded by the probe findings. Bring that package back
for approval, then implement it through testing and independent review.” Steve
also clarified: “I've approved a pragmatic approach”.

## Source evidence

`simple_condition.rs::resolve_operand_with_diagnostics` sends raw postfix
operands to the public standalone evaluator. Tightening standalone grammar first
would reject valid `$response.body.items[0].id == 7` conditions. The condition
AST separates Runtime Expression operands from postfix operators, but needs
private grammar-derived source spans for evaluation errors. The parsed Runtime
Expression form is private to its module and needs crate-private form access.
The [numeric probe](arazzo-numeric-feasibility-2026-10-07.md) records why precise
storage would widen the migration; its findings bound the approved contract.

The full evaluation matrix, normative quotes, safe sequence, ownership,
verification, rollback and system success now live in the accepted plan.
The semantic and ordering decision tickets retain audit/closure evidence.

## Review history

Independent GPT-6 Astra high review, 2026-10-07, fresh context: two concrete
findings corrected (literal-classifier candidate ordering and nested equality
error precedence). One bounded recheck found no remaining findings and verified
runtime/debug adapter transfer. This was architecture review, not implementation
verification. Local document checks verified five normative quotes, links/HTML
anchors, whitespace and no unresolved placeholders. The approved re-decomposition
requires its separate fresh-context transfer audit before implementation handoff.
