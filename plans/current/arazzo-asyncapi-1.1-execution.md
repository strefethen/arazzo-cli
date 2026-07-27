# Plan: Arazzo 1.1 Runtime Expressions, Data Selection, and Conformance

## Goal

Bring `arazzo-cli` toward the current Arazzo 1.1 surface area without diverging from the open ticket tree.

This is a reconciliation plan for adversarial review. It deliberately does not implement code. It aligns the current plan with the P0/P1/P2 epics and related tickets, and it narrows the scope away from AsyncAPI broker or transport execution until a separate review and ticket set exists.

## Research Sources

- Arazzo Specification v1.1.0, latest published version as of 2026-06-20: <https://spec.openapis.org/arazzo/latest.html>
- AsyncAPI Specification v3.1.0, latest published version as of 2026-06-20: <https://www.asyncapi.com/docs/reference/specification/v3.1.0>
- Local ticket tree from `tkt epic list`, `tkt ready`, `tkt blocked`, and `tkt show` on 2026-06-20.
- Local implementation files read for current behavior:
  - `crates/arazzo-spec/src/lib.rs`
  - `crates/arazzo-validate/src/lib.rs`
  - `crates/arazzo-expr/src/lib.rs`
  - `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
  - `crates/arazzo-runtime/src/runtime_core/engine_actions.rs`
  - `crates/arazzo-runtime/src/runtime_core/payload.rs`
  - `docs/arazzo-1.0.1-compliance-plan.md`

## Scope Boundary

The current ticket set is an Arazzo 1.1 conformance and runtime-semantics rollout. It is not an AsyncAPI transport-execution rollout.

In scope:

- Arazzo 1.1 AsyncAPI source-description typing and async step fields, with fail-closed runtime behavior.
- `$self`, source-description expressions, JSON Pointer suffixes, and `$message` expression context.
- Selector Object modeling and value evaluation.
- `querystring` parameters.
- `onSuccess` and `onFailure` action parameters for workflow handoff.
- `requestBody.replacements` selector-type and selector-value extensions.
- Remaining 1.1 criterion/action conformance details.

Out of scope for these tickets:

- AsyncAPI broker/client execution.
- WebSocket, AMQP, MQTT, Kafka, or any other concrete AsyncAPI transport driver.
- Implicit remote fetching of AsyncAPI source documents.
- Treating unsupported AsyncAPI operations as HTTP, dry-run success, or no-op execution.
- A new retry hook execution model beyond the action-parameter work explicitly called out in the tickets.

Any transport execution plan needs a future epic after this conformance rollout is stable.

## Current Grounding

The local codebase is still shaped as an Arazzo 1.0/1.0.1 HTTP executor:

- `SourceType` accepts `openapi` and `arazzo`, not `asyncapi`.
- `StepTarget` supports `operationId`, `operationPath`, and `workflowId`; it has no `channelPath`.
- `Step` has no `action`, `timeout`, `correlationId`, or explicit `dependsOn`.
- `ParamLocation` has `path`, `query`, `header`, and `cookie`; it has no `querystring`.
- `RequestBody.replacements` exists locally, but only with `target` and `value`; it does not model `targetSelectorType` or selector-object replacement values.
- `OnAction` has no `parameters` field, and action execution cannot pass evaluated inputs to a `goto workflowId` target.
- `ExpressionEvaluator` has `$request.*`, `$response.*`, `$steps.*`, `$workflows.*`, `$sourceDescriptions.<name>.url`, `$method`, `$url`, and `$statusCode`, but it has no `$self`, source-description type metadata, JSON Pointer suffix support for all value expressions, or `$message.*` namespace.
- `prepare_http_request()` is the only operation execution path. `operationId` resolution builds an OpenAPI HTTP method/path index, and every executable operation is converted into an HTTP request.

The existing `docs/arazzo-1.0.1-compliance-plan.md` says the 1.0.1 replacement work is complete. This rollout must extend that base rather than re-open it as missing work.

## Ticket Reconciliation

### P0 Prerequisite: AsyncAPI-Aware Modeling, Not Execution

Epic: [epic:ac-fc428](https://sonos.scapedeck.com/docs/ac-tickets/ac-fc428)

Tickets:

- [ac-a3656](https://sonos.scapedeck.com/docs/ac-tickets/ac-a3656): support Arazzo 1.1 `sourceDescriptions[].type: asyncapi`.
- [ac-45adb](https://sonos.scapedeck.com/docs/ac-tickets/ac-45adb): model async step fields and fail closed at runtime.

Plan alignment:

- Proceed first. These tickets establish the model, validation, output, and runtime error boundary needed by the P1 tickets.
- Keep runtime behavior explicit and conservative: async steps parse and validate structurally, but execution fails with a typed unsupported error.
- Do not add AsyncAPI source loading flags, source document fetching, operation indexing, or transport execution in this phase.

Recommended order:

1. [ac-a3656](https://sonos.scapedeck.com/docs/ac-tickets/ac-a3656)
2. [ac-45adb](https://sonos.scapedeck.com/docs/ac-tickets/ac-45adb)

Review note: P1 tickets are currently ready in the tracker, but logically they should not be started ahead of this P0 foundation unless the implementer manually preserves the same boundaries.

### P1 Runtime Expressions

Epic: [epic:ac-e167f](https://sonos.scapedeck.com/docs/ac-tickets/ac-e167f)

Tickets:

- [ac-43f15](https://sonos.scapedeck.com/docs/ac-tickets/ac-43f15): support `$self` and source-description expressions.
- [ac-f0140](https://sonos.scapedeck.com/docs/ac-tickets/ac-f0140): support Arazzo 1.1 JSON Pointer suffixes and `$message` expressions.

Plan alignment:

- Proceed after the P0 source-description model exists.
- Add `$self` and source-description metadata without adding source-document fetch or transport behavior.
- Add `$message.header.*` and `$message.payload` evaluation against a synthetic or runtime-provided message context; do not require a live AsyncAPI driver.
- Share pointer-suffix logic across inputs, outputs, steps, and workflows instead of adding namespace-specific parsing branches.

Recommended order:

1. [ac-43f15](https://sonos.scapedeck.com/docs/ac-tickets/ac-43f15)
2. [ac-f0140](https://sonos.scapedeck.com/docs/ac-tickets/ac-f0140)

Adversarial review questions for this epic:

1. Should `$message.body` be rejected, warned on, or treated as an alias for `$message.payload`?
2. What exact error should be emitted when `$message.*` is evaluated without message context?
3. Should source-description expressions expose only `.url` and `.type`, or should the model reserve room for additional metadata without promising it yet?

### P1 Data Selection

Epic: [epic:ac-fc412](https://sonos.scapedeck.com/docs/ac-tickets/ac-fc412)

Tickets:

- [ac-87a4c](https://sonos.scapedeck.com/docs/ac-tickets/ac-87a4c): implement Selector Object evaluation.
- [ac-3a7f4](https://sonos.scapedeck.com/docs/ac-tickets/ac-3a7f4): support `querystring` and action workflow parameters.

Plan alignment:

- Proceed after or alongside P1 expression work, but use one shared value-resolution abstraction.
- Keep selector evaluation separate from criterion evaluation: selectors return values, criteria return pass/fail.
- Add `querystring` as a distinct parameter location representing the whole query fragment, with explicit conflict handling against normal `query` parameters and existing URL query strings.
- Add `OnAction.parameters` and evaluate them as workflow inputs when the action targets `workflowId`.
- Do not expand this ticket into a new retry hook execution model.

Recommended order:

1. [ac-87a4c](https://sonos.scapedeck.com/docs/ac-tickets/ac-87a4c)
2. [ac-3a7f4](https://sonos.scapedeck.com/docs/ac-tickets/ac-3a7f4)

Parallel option:

- The `querystring` part of [ac-3a7f4](https://sonos.scapedeck.com/docs/ac-tickets/ac-3a7f4) can proceed independently.
- The action-parameter value path should reuse the selector/value resolver if [ac-87a4c](https://sonos.scapedeck.com/docs/ac-tickets/ac-87a4c) has landed; otherwise it should stay limited to literals and runtime-expression strings until the resolver exists.

Adversarial review questions for this epic:

1. What is the canonical JSONPath return contract for zero, one, and many matches?
2. Should Selector Object values preserve arrays for multi-match selections, or should any scalar-only field reject multi-match results?
3. Where should selector diagnostics be surfaced so CLI JSON output remains stable and machine-parseable?

### P2 Conformance Polish

Epic: [epic:ac-aeca8](https://sonos.scapedeck.com/docs/ac-tickets/ac-aeca8)

Tickets:

- [ac-f8ceb](https://sonos.scapedeck.com/docs/ac-tickets/ac-f8ceb): extend replacements for Arazzo 1.1 selector semantics.
- [ac-80a8f](https://sonos.scapedeck.com/docs/ac-tickets/ac-80a8f): align Arazzo 1.1 criterion and action conformance.

Plan alignment:

- Proceed after the P1 selector/value resolver exists.
- Extend the existing 1.0.1 replacement implementation; do not duplicate the base replacement engine.
- Add `targetSelectorType` and selector-object replacement values while preserving default JSON Pointer/XPath behavior when `targetSelectorType` is absent.
- Allow Arazzo 1.1 expression-type versions and defaults without silently accepting versions that the runtime cannot evaluate.
- Preserve decimal `retryAfter` semantics, reject negative values, and update trace/runtime serialization deliberately.
- Make simple string equality, inequality, and `in` comparisons case-insensitive while preserving numeric comparison behavior.

Recommended order:

1. [ac-f8ceb](https://sonos.scapedeck.com/docs/ac-tickets/ac-f8ceb) after [ac-87a4c](https://sonos.scapedeck.com/docs/ac-tickets/ac-87a4c)
2. [ac-80a8f](https://sonos.scapedeck.com/docs/ac-tickets/ac-80a8f)

Review note: [ac-f8ceb](https://sonos.scapedeck.com/docs/ac-tickets/ac-f8ceb) depends on [ac-36bi](https://sonos.scapedeck.com/docs/ac-tickets/ac-36bi), which the tracker reports as closed and satisfied. Its body still says the base replacement ticket is in progress; that copy is stale but not a conceptual blocker.

## Cross-Cutting Architecture Decisions

### Shared Value Resolver

Add one value-resolution layer that can handle:

- scalar literals;
- runtime expression strings;
- runtime expression strings with JSON Pointer suffixes where Arazzo 1.1 allows them;
- Selector Objects;
- nested sequences and maps containing any of the above.

Use this resolver for outputs, parameters, payload maps, action parameters, and replacement values. Avoid one-off resolver branches in each runtime caller.

### Selector Object Contract

Selector Objects have:

- `context`: runtime expression that evaluates to structured data;
- `selector`: selector string;
- `type`: selector type, either a string or Expression Type Object.

Supported selector families for this rollout:

- `jsonpointer` / `rfc6901`: implement through JSON Pointer support.
- `xpath`: implement only for versions backed by the existing XML extraction capability, and fail closed for unsupported versions.
- `jsonpath`: implement only after the ticket defines a deterministic return contract.

No hidden "first item wins" behavior is allowed. Multi-match behavior must be deliberate and tested.

### Querystring Contract

`in: querystring` represents the complete query fragment.

Rules:

- It must not coexist with normal `query` parameters for the same operation.
- It must not be appended to a URL that already has a query string unless the ticket explicitly validates and documents that behavior.
- It must not be form-encoded a second time after value resolution.
- It should fail with a clear validation or request-preparation error when the conflict is statically visible.

### Action Parameter Contract

`OnAction.parameters` should be evaluated as target workflow inputs when the action uses `workflowId`.

Rules:

- `in` is invalid for action workflow parameters.
- Parameters are meaningful only when `workflowId` is present.
- Existing action decisions need a place to carry evaluated workflow inputs.
- The current ticket should not invent a broader retry hook execution model unless the ticket is explicitly widened.

### Message Context Contract

`$message.*` evaluation must work without a real transport driver by accepting an explicit message context in tests and future runtime paths.

Minimum surface:

- `$message.header.<name>` with deterministic header lookup.
- `$message.payload` for the whole decoded payload.
- `$message.payload#/json/pointer` for JSON Pointer access.

Do not force message exchange data into the existing HTTP `Response` type. If later transport work lands, message context should remain distinct from `$response.*`.

### Replacement Contract

Keep existing base replacement ordering and behavior:

- payload resolution happens before replacements;
- replacements apply in authored array order;
- later replacements win when they target the same location.

Arazzo 1.1 additions:

- `targetSelectorType` can explicitly select `jsonpointer`, `jsonpath`, or `xpath` semantics.
- replacement `value` can be a Selector Object.
- unsupported target write semantics, especially JSONPath mutation, must fail closed with a visible diagnostic instead of silently doing nothing.

## File Impact

Expected implementation scope across tickets:

- `crates/arazzo-spec/src/lib.rs`: Arazzo 1.1 model fields, selector types, expression-type object, action parameters, decimal retry fields.
- `crates/arazzo-validate/src/lib.rs`: 1.1 structural validation, query/querystring incompatibility, action parameter rules, async field rules, expression-type version validation.
- `crates/arazzo-expr/src/lib.rs`: `$self`, `$message.*`, source-description metadata, pointer suffix support, selector hooks, case-insensitive string comparisons.
- `crates/arazzo-runtime/src/runtime_core/state.rs`: evaluation context additions and message/source metadata.
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`: querystring request preparation, fail-closed async step handling, payload/replacement integration points.
- `crates/arazzo-runtime/src/runtime_core/engine_actions.rs`: action parameters and workflow input handoff.
- `crates/arazzo-runtime/src/runtime_core/payload.rs`: replacement selector-type dispatch and selector-value resolution.
- CLI/MCP output surfaces: expose Arazzo 1.1 model metadata and unsupported async behavior truthfully, while preserving `--json` contracts.

## Verification Obligations

Every implementation ticket should finish with the repository gate:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Targeted tests by area:

- serde round-trip tests for Arazzo 1.1 source descriptions, async step fields, selector objects, action parameters, replacement selector types, and decimal retry fields;
- validator tests for bad target combinations, query/querystring conflicts, action parameter misuse, invalid selector versions, and unsupported async execution shapes;
- expression tests for `$self`, source-description metadata, pointer suffixes, `$message.*`, and case-insensitive string comparisons;
- selector tests for JSON Pointer, XPath, and JSONPath behavior where implemented;
- runtime tests for `querystring`, action parameter workflow handoff, selector-valued payloads, selector-valued replacements, fail-closed async steps, and retryAfter behavior;
- CLI integration and schema-drift tests for stable JSON output.

## Proceed / Further Review

Proceed with the ticket set after linking this plan path into the tickets. The ticket lint result for the eight child tickets currently reports only `plan-not-set` warnings.

Proceed now:

- [ac-a3656](https://sonos.scapedeck.com/docs/ac-tickets/ac-a3656)
- [ac-45adb](https://sonos.scapedeck.com/docs/ac-tickets/ac-45adb)
- [ac-43f15](https://sonos.scapedeck.com/docs/ac-tickets/ac-43f15)
- [ac-f0140](https://sonos.scapedeck.com/docs/ac-tickets/ac-f0140)
- [ac-87a4c](https://sonos.scapedeck.com/docs/ac-tickets/ac-87a4c)
- [ac-3a7f4](https://sonos.scapedeck.com/docs/ac-tickets/ac-3a7f4)
- [ac-f8ceb](https://sonos.scapedeck.com/docs/ac-tickets/ac-f8ceb), after selector resolution is in place
- [ac-80a8f](https://sonos.scapedeck.com/docs/ac-tickets/ac-80a8f)

Needs further review before ticketing or implementation:

- AsyncAPI operation/source-document loading beyond preserving source-description metadata.
- AsyncAPI transport execution and any concrete protocol driver.
- JSONPath mutation semantics for replacement targets if full write support is desired instead of fail-closed diagnostics.
- A broader retry hook execution model for `retry workflowId` or `retry stepId`.

## Done Definition

This plan is coherent when:

- every listed ticket points at this plan path;
- P0 is treated as the prerequisite foundation for P1;
- P2 replacement work is sequenced after Selector Object evaluation;
- no current ticket implements AsyncAPI broker/client execution;
- unresolved review questions are either encoded as ticket acceptance criteria or explicitly deferred to a future epic.
