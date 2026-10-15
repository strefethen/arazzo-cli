---
status: completed
epic: ac-33d48
created: 2026-10-15
completed: 2026-10-15
---

# Standard Arazzo operation targets

**Acceptance:** On 2026-10-15 Steve directed: “Turn the ac-3557b design into Tier 2 tickets, reusing ac-ff633, and have right-sized agents implement it. No compatibility shims or migration examples, and keep reviews and testing proportional.” This accepts the independently reviewed [assessment](../assessments/arazzo-canonical-operation-targets-decision.md), including its nine verbatim normative quotations and vendored Arazzo/OpenAPI anchors. The assessment remains evidence; this plan owns the implementation direction.

## Outcome and boundaries

Imported and generated workflows use standard Arazzo operation targets and resolve the same bound OpenAPI operation to the same method, path, and server. Remove bare request paths, absolute request URLs, `{source}./path`, optional method prefixes, url-as-request-base behavior, and unbound supplied-document operation-ID lookup. There is no legacy mode, shim, test-only translator, or migration example. Legitimate standard negative fixtures remain useful and must be labeled as rejection evidence.

This is not full OpenAPI conformance. Preserve existing explicit refusals for unsupported relative/missing-root-server configurations; do not invent request context for referenced path items, callbacks, webhooks, or non-routable operation locations. No network document retrieval, general reference framework, AsyncAPI execution, JSON Schema anchors, parameter-serialization changes, CLI offline-validation feature, OpenAPI model upgrade, or unrelated refactor is included. The generator continues to use its existing OpenAPI 3.0 model and version boundary.

## Contract and ownership

The assessment verifies the current mismatch: the shared classifier accepts private forms and refuses standard pointers; `DocumentSet::bind` has a url-as-base escape; builder drops parsed bound documents after ID-only indexing; bare ID lookup prefers unclaimed provided documents; `crud::build_step` emits private targets. Those source anchors and the nine normative quotations are retained in the assessment rather than duplicated here.

`arazzo-spec` owns one standard target parser. A successful operationPath parse identifies the named source and URI-fragment JSON Pointer; malformed/custom forms are rejected, and no authored method override exists. Runtime and structural validation consume that published result. CLI/MCP static presentation retains the raw canonical target and reports no method until document resolution, regardless of request-body presence; existing output shapes remain unchanged. Necessary public parser and all caller adaptations land atomically.

`arazzo-runtime` retains source-bound documents under the existing identity and local/provided-document binding rules. Unclaimed provided documents cannot satisfy targets; absolute unmatched document URLs fail without retrieval. Pointer lookup and operation-ID lookup project one internal operation record carrying source/document identity, operation location, method, path template, and effective server. IDs are optional for pointer lookup. Direct operation positions under `paths` include the standard fixed methods and OAS 3.2 `query`/`additionalOperations`; additional method keys preserve their specified case. Non-operation or unresolved pointers fail rather than being interpreted as request paths. Both target forms use the existing operation/path/document server-selection policy. API request assembly consumes the resolved result, never guesses GET/POST from a request body or reparses authored target syntax.

Keep public `RUNTIME_*` categories: malformed/custom path → `UnsupportedOperationPathForm`; missing source/binding → `SourceDescriptionNotFound`; missing/non-operation pointer → `SourceDescriptionParse` with source, pointer, and document; existing ID ambiguity/missing and unsupported-source categories retain their purposes. Valid but unsupported standard locations fail explicitly. The structural validator stays document-only; it gains no OpenAPI loader or duplicate runtime resolver.

`arazzo-generate` emits source-qualified operationId for a unique, grammar-representable ID; absent IDs or otherwise valid grammar-unrepresentable IDs use the standard source-expression JSON Pointer to the actual operation. Duplicate IDs are input errors, never a pointer fallback. Escape pointer tokens and URI fragments correctly; retain the document URL as the source URL and never fabricate operations. Keep `generate_crud`'s existing fallible API and output shape.

## Execution and coordination

Three assignments form one coordinated change: the atomic syntax/runtime/validator/presentation cutover (existing ac-ff633); canonical generation; and repository fixtures/examples/docs plus cross-surface integration proof. Each has explicit disjoint file ownership. Production files and their inline tests stay with the cutover owner; generator files stay with generation; remaining standalone test fixtures, examples, contract baselines, and conformance data belong to the integration owner. Shared test helpers may build real bound OpenAPI documents from explicit test facts, never translate legacy target strings.

| Assignment | Owner |
|---|---|
| Atomic parser/runtime/validator/presentation cutover | [ac-ff633](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ff633) |
| Standard generation and duplicate-ID refusal | [ac-1a904](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1a904) |
| Authored corpus, surface integration and claims | [ac-fc81d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fc81d) |

All three may begin after the transfer audit because their authoring inputs are settled and scopes are disjoint. They have no artificial inter-ticket completion dependency: generation and integration are verified against the assembled runtime, and the runtime cannot close while repository fixtures still depend on removed behavior. Any temporary failing tests during the coordinated working-tree edit are unfinished work, not a shippable intermediate candidate. Root owns the shared index, integration fixes routed to the owning agent, and one integrated source candidate. Agents report focused proof and do not independently stage, commit, or close. No ticket closes until the shared gate passes; record the same candidate and gate evidence on each.

Reuse epic [ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48) without changing its unrelated frozen conformance plan. Retire stale legacy-presentation work with explicit supersession, not completed-implementation claims. Offline `validate --openapi`, broader ID diagnostics, and generator ingestion upgrades remain with their existing owners.

## Verification and risks

Use focused semantic tests for parser rejection, escaped pointers, operations without IDs, source isolation, server precedence, generator selection/duplicate IDs, and no-traffic refusal. Converted existing tests must still assert their original unrelated behavior using legitimate bound descriptions. Preserve public JSON shapes and explain baseline changes; do not bless unexplained drift or weaken unrelated assertions.

Root runs the mandatory format, strict workspace Clippy, and workspace test suite once on the final assembled candidate, plus affected public-crate documentation builds for changed Rust APIs. A failure is repaired by its owner and only affected checks repeated until the shared gate is sound. One fresh independent review of the final integrated change covers all coupled tickets; no separate review/full-suite cycle per agent. The cross-unit decomposition receives one fresh-context transfer audit before dispatch.

This deliberately breaks documents using shortcuts. Shipped examples, diagnostics, README/changelog, and conformance evidence must agree in the same candidate; no release or push is authorized. Existing source identity and no-network behavior remain invariants. Retaining parsed documents adds memory proportional to the bound documents; avoid redundant full copies beyond what pointer lookup and diagnostics require.

## System success condition

One canonical imported workflow and generated workflows for operations with and without IDs validate and plan against the same provided OpenAPI source. Method, path, effective server, and hermetic captured request agree. Escaped pointers and multiple-source overlap work without cross-source selection. Custom syntax, unbound document URLs/IDs, missing/non-operation pointers, and duplicate generator IDs fail before traffic. CLI/MCP presentation is honest about unresolved methods, DAP execution consumes the same runtime, relevant conformance evidence names passing tests, and the integrated repository gates pass.

**Observed result:** All three implementation tickets are closed/completed. The imported/generated same-source execution, escaped-pointer/source-isolation, and zero-traffic refusal tests in `crates/arazzo-cli/tests/canonical_operation_targets.rs` passed against the assembled source committed as [a13c357](https://github.com/strefethen/arazzo-cli/commit/a13c35739aca1bc393c27406b961c0af62b9929c) (local; not pushed). Runtime, generator, CLI/MCP/DAP and conformance evidence agree; formatting, strict Clippy, public-crate docs, and one workspace run with passing scoped fixture repairs establish the integrated gate. One fresh independent review passed without findings. After ticket closure, root confirmed that the observed system proof still names the unchanged reviewed source. The ticket closeout notes retain the shared evidence packet at `/tmp/standard-arazzo-targets/shared/evidence.md` and exact review identities.

**Disposition:** Completed on 2026-10-15. The broader [epic:ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48) remains open for its unrelated conformance work.
