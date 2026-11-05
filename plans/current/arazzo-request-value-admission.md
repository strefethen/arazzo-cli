---
status: decomposed
epic: ac-3d8bf
created: 2026-10-29
---

# Preserve request values through admission

**Seed:** The current release-blocker repair request in the parent chat authorizes implementation, beginning with unresolved request values and credential redaction; hourly work remains paused and release tagging/publication is excluded. The exact accepted request-value decisions are recorded in `plans/assessments/live-request-value-admission.md`, opening acceptance record and §§3–7, and [ac-704a6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-704a6). This plan operationalizes those decisions; credential redaction retains its separate owner.

## Outcome and non-goals

An HTTP step whose request depends on unresolved data fails before transport, with a source/location diagnostic and `RUNTIME_UNRESOLVED_REQUEST_VALUE`. Intentional nulls and empty values retain their serialization. Transport-skipped dry-run data remains separately identifiable, so an incomplete plan is shown honestly rather than being rejected as a live-resolution error.

No new expression grammar, syntax, OpenAPI parameter-metadata lookup, fallback data, public output-value representation, mock response behavior, or message-transport admission policy is introduced. The accepted selector zero-match residual remains: a zero-match Selector produces resolved null and can still create an empty path segment. This initiative must not claim universal prevention of such requests.

## User-visible workflows

An upstream `$response.body#/id` miss remains unresolved through Step outputs, Workflow outputs, nested workflow inputs/returns, and ordinary execution composition. A later request using it fails before RequestPrepared or RequestSent. The same holds for direct misses, nested payload leaves, templates and replacement values across all parameter locations. A real null, empty string, or Selector zero-match result remains allowed. Replacement-target misses retain current warn-and-skip behavior.

Declared unsupplied optional inputs become null only for request construction; missing required or undeclared inputs and supplied unresolved inputs fail at their consuming request. Strict input validation retains its earlier concrete-input failures. `$method` resolves in path/query/querystring as well as headers and body.

Admission failure uses ordinary failure routing. Retry actions are inapplicable; goto remains available and end preserves the original admission code. Default/warn/error diagnostics do not alter live admission. Dry-run rejects genuinely unresolved data but retains transport-skipped requests, rendering markers and warnings through the existing dry-run ticket contract. Ordinary public values, criteria, actions, trace/debug scopes and synthetic dry-run response values remain unchanged.

## Decision-relevant facts

At `58de5ac`, `arazzo-expr::resolution` already retains Missing internally; its public projection and `interpolation::resolve_value` erase it. The canonical expression-string classifier already distinguishes expressions, templates and literals, so it remains the grammar owner. Invalid reserved templates are failures; literal `$USD`, `$env.X`, and standalone dotted body text are not reclassified.

`runtime_core::payload` recursively resolves ValueSources but retains only a private JSONPath failure flag. `VarStore` stores JSON values without provenance. HTTP preparation precedes events and dispatch. Current subworkflow output publication uses replace/restore operations, so rollback must restore provenance with the corresponding values. Current message Step outputs also enter the same stored-value graph.

The vendored Arazzo 1.1 text at `spec/arazzo/v1.1.0.html#requestBodyPayload` states that payload values “can contain Runtime Expressions or Selector Objects which MUST be evaluated prior to calling the referenced operation.” At `#parameterValue`, “The value can be a constant, a Runtime Expression, or a Selector Object to be evaluated and passed to the referenced operation or workflow.” The selected missing-value and zero-match policies are the accepted repository choices recorded by the assessment, not invented normative requirements.

## Architecture

`arazzo-expr` owns canonical parsing and absence-preserving string resolution. It publishes one shared result through thin complete-expression and literal/template-capable entrypoints mirroring the existing APIs. The result retains ordered occurrences for every evaluated expression: authored source, namespace, typed stored-value coordinate/pointer, and missing status, alongside the value, diagnostics and syntax-failure status. Canonical parsed form access builds these occurrences in the existing pass; no second scanner, parser or evaluation is introduced. Existing methods project that result without behavior drift. No runtime state or dry-run policy enters this package; no EvalContext field or public parser accessor is added.

`arazzo-runtime` owns recursive ValueSource resolution, value provenance, and admission. The existing payload owner carries resolved values, diagnostics and unresolved leaf sources; hard Selector failures propagate, while zero matches remain resolved. A private value-provenance module owns metadata operations and internal value carriers; VarStore is their sole storage owner. Values and provenance travel together through all writes, merges, recursive calls, action input forwarding and replace/restore operations. Public runtime values remain JSON.

A private request-admission module owns request-only context preparation and refusal diagnostics. It consumes the canonical resolution result, excludes unresolved stored entries from the request context, materializes optional omissions only there, and supplies method consistently. Existing HTTP orchestration delegates to this owner rather than accumulating admission policy. Value resolution happens once per request occurrence; there is no separate preflight parser or duplicate evaluator pass.

The shared provenance has two classes: unresolved and transport-skipped. A genuinely unresolved dependency remains a refusal even when another dependency is transport-skipped. Dry-run rendering and warning formatting belong to the focused dry-run owner and consume the shared metadata. They never replace stored values used by controls or observers. Existing output producers, including received-message outputs subsequently consumed by HTTP, use the common provenance result without introducing message admission.

Required order is expression result → recursive ValueSource result → shared propagation/request context → HTTP refusal and routing → integrated surface proof. The existing subworkflow parameter resolver must be unified before shared propagation. Its narrowly transferred prerequisite preserves the complete original obligations of [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4); unrelated action-warning work stays there. [ac-fda34](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34) is refined onto the shared owner and may proceed after propagation, serializing overlapping edits with admission.

Intermediate foundation commits preserve current public behavior; they are not advertised as the L4 repair. Replacement-value refusal is activated together with the pre-transport admission boundary and failure-routing support. The final integrated candidate establishes the complete accepted behavior.

## Verification and operational risks

Focused tests establish exact/null/missing/template classification, nested Selector outcomes, provenance propagation, parallel snapshots, action forwarding and transactional rollback. Hermetic loopback proof covers sequential execution, parallel execution, run-step dependencies, nested workflows and authored retry/end/goto outcomes. Refused steps emit neither prepared nor sent events and never reach the server. Authored-null/empty/zero-match controls remain positive cases.

Dry-run composition proves zero transport, retained producer/consumer requests, honest markers and ordered diagnostics, including strict child input validation. Existing control values and public schemas remain unchanged. CLI diagnostic modes, MCP error passthrough, DAP error display and the four drift guards establish affected consumer behavior. Explain every changed golden row; do not rebaseline unrelated differences.

Credential redaction is a separate release-blocker owner. Admission diagnostics report authored source references and locations, not resolved credential values; final source-error surfaces participate in that owner's canary verification. Existing dirty generator/MCP/CHANGELOG work must be preserved. Use one writer and live reservations, and coordinate the eventual release-note edit. No tag or release publication is authorized.

## System success condition

After all implementation concerns close, rerun the assessment's loopback producer/missing-id/DELETE reproduction against the final candidate across supported execution paths. No unsafe DELETE arrives; the consuming step reports the admission code and precise source/location. Null/empty/zero-match controls retain accepted behavior; failure routing and dry-run transport-skipped composition satisfy their contracts. Record a single evidence pointer covering candidate identity, integrated results and the known selector residual. Only that proof clears the unresolved-request release blocker.

**Disposition:** Accepted for implementation under Steve’s current instruction to clear release blockers and the prior explicit acceptance of the referenced contract. Independent plan review and its bounded API-completeness recheck passed; decomposition is tracked by [epic:ac-3d8bf](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3d8bf). No new policy authority or publication approval is inferred.

## Execution ownership

[epic:ac-3d8bf](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3d8bf) owns the serial implementation: [expression provenance](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-48b58) → [recursive values](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b3093) → [child Parameters](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0b8c2) → [shared runtime state](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1af66) → [request admission](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fd849) → [dry-run presentation](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34). The last ticket retains its original epic ownership and is referenced here for system verification. Original action-warning obligations remain in [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4).

Independent transfer audit passed after one bounded file-scope correction pass; its coverage record is in the epic. Foundation tickets do not clear the release blocker.

Final integrated verification completed on 2026-11-04 against the recorded shared tree based on `71d0fdd`, including its preserved dirty generator/MCP build inputs. The single [system evidence packet](/private/tmp/arazzo-rel09/fda34-final/review-packet.md) records the final candidate binding, exact contract snapshots, loopback missing-id/DELETE matrix (sequential, parallel, run-step and nested workflows), diagnostic modes, failure routing, live/null/empty/zero-match controls, strict/non-strict dry-run composition, all four drift guards and full required checks. The accepted Selector residual remains observable: a zero-match Selector can still send `DELETE /pets/`. No production behavior or criteria changed in this verification pass. Fresh independent review of the final candidate remains pending; this evidence does not itself authorize ticket closure or release publication.
