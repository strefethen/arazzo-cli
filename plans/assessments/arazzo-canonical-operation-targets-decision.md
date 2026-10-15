# Canonical operation targets and portable generation

Assessment for [ac-3557b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3557b), 2026-10-15. **Design deliverable; not an accepted implementation plan.** No runtime or generator behavior changes in this assessment.

## Recommended contract

Use standard Arazzo targets exclusively. Steve has settled removal of custom operation-target syntax: no compatibility fallback, legacy mode, or retained shortcut. This supersedes the open F1/F5 compatibility question in the [conformance audit](arazzo-spec-conformance-audit.md); it does not close those implementation debts.

Generate `$sourceDescriptions.<name>.<operationId>` when the actual OpenAPI operation defines a unique ID representable by the runtime-expression grammar. When the operation has no ID, or its otherwise valid ID cannot be represented by that grammar, generate `{$sourceDescriptions.<name>.url}#/paths/<escaped-path>/<method>` using the operation's real document location. Duplicate IDs are an OpenAPI validation failure; generation must report that failure rather than conceal it with pointer output. These are target contracts, not migration examples. Do not invent an ID, operation, method, or server. The preference for an ID is a SHOULD, not a MUST.

Import and generation converge on one runtime result: the selected source/document, operation location, request method, path template, and effective OpenAPI server. The source URL identifies the description document. It never supplies an API request base by itself.

## Decisive specification evidence

All normative authority below is the vendored published text, not repository examples or existing implementation.

[Arazzo 1.1.0, operationPath](../../spec/arazzo/v1.1.0.html#stepOperationPath):

> A reference to a Source Description Object combined with a JSON Pointer to reference an operation.

> The operation being referenced MUST be described within one of the sourceDescriptions descriptions. A Runtime Expression syntax MUST be used to identify the source description document. If the referenced operation has an operationId defined then the operationId SHOULD be preferred over the operationPath.

[Arazzo operationId](../../spec/arazzo/v1.1.0.html#stepOperationId):

> The name of an existing, resolvable operation, as defined with a unique operationId and existing within one of the sourceDescriptions.

> If multiple (non arazzo type) sourceDescriptions are defined, then the operationId MUST be specified using a Runtime Expression (e.g., $sourceDescriptions.\<name\>.\<operationId\>) to avoid ambiguity or potential clashes.

[Arazzo Source Description URL](../../spec/arazzo/v1.1.0.html#sourceURL):

> A URL to a source description to be used by a workflow.

[Arazzo fragment resolution](../../spec/arazzo/v1.1.0.html#resolving-uri-fragments):

> If a URI contains a fragment identifier, then the fragment MUST be resolved per the fragment resolution mechanism of the referenced document.

The same section expressly gives `{$sourceDescriptions.petstore.url}#/paths/~1pets/get` and requires JSON Pointer syntax for fragments in these runtime expressions. Its further-reference rule delegates OpenAPI references to OpenAPI; it does not authorize a second URI resolver.

[Arazzo API URLs](../../spec/arazzo/v1.1.0.html#relative-references-in-api-urls):

> When Step Objects reference API operations via operationId or operationPath, the actual API endpoint URL is determined by the OpenAPI description’s Server Object, not by the Arazzo Description’s base URI.

[OpenAPI 3.2 Operation servers](../../spec/oas/v3.2.0.html#operation-servers):

> An alternative servers array to service this operation. If a servers array is specified at the Path Item Object or OpenAPI Object level, it will be overridden by this value.

[Path Item servers](../../spec/oas/v3.2.0.html#path-item-servers) similarly override the document level. [Operation ID](../../spec/oas/v3.2.0.html#operation-id) is optional and case-sensitive. [Path Item](../../spec/oas/v3.2.0.html#path-item-object) defines method-bearing operation positions, including `query` and `additionalOperations`; for the latter:

> The map key is the HTTP method with the same capitalization that is to be sent in the request.

## Current facts and the resulting boundary

The shared [`arazzo-spec::classify_operation_path`](../../crates/arazzo-spec/src/operation_path.rs) accepts bare paths, absolute request URLs, `{source}./path`, and optional method prefixes, while classifying the standard expression-plus-pointer as unsupported. [`Engine::build_url_from_path`](../../crates/arazzo-runtime/src/runtime_core/engine_http.rs) refuses the standard form before URL assembly today. The audit's original garbage-URL observation is historical, not current behavior. [`operation_path_forms`](../../crates/arazzo-runtime/tests/operation_path_forms.rs) deliberately pins the shortcuts.

[`DocumentSet::bind`](../../crates/arazzo-runtime/src/runtime_core/document_set.rs) already establishes identity from provided documents and local files. It nevertheless returns no bound document for an unmatched absolute non-file URL; [`EngineBuilder::build`](../../crates/arazzo-runtime/src/runtime_core/builder.rs) then uses `sd.url` as the request base. `index_bound_document` parses the bound bytes, builds an ID-only index, and drops the parsed document. `index_operations` skips operations without IDs and recognizes only eight methods. `operation_server` already owns operation-over-path-over-document selection.

[`OperationIndex::bare`](../../crates/arazzo-runtime/src/runtime_core/state.rs) gives unclaimed explicitly provided documents precedence over source-bound ones. This permits an ID outside every declared sourceDescription and must disappear with the shortcuts. This is separate from document identity precedence, which remains unchanged. Standard bare IDs remain valid when source-count rules permit and lookup identifies exactly one operation within the declared source; qualified lookup stays isolated to its named source. Valid operation IDs may contain punctuation: do not invent a restrictive ID alphabet while removing extensions.

[`crud::build_step`](../../crates/arazzo-generate/src/crud.rs) always emits the custom method/source/path string, despite receiving the selected OpenAPI operation. Its source URL already uses the provided document URL. The generator can select the standard representation without changing parameter serialization or adopting a new OpenAPI model.

## One runtime pipeline

1. **Parse the target once.** `arazzo-spec` remains the shared syntax owner. Its operationPath result identifies a source reference and encoded pointer, or a malformed value; it carries no authored method override. Validation rejects custom forms as invalid Arazzo, checks the named source, and distinguishes valid syntax from unavailable runtime capabilities.
2. **Bind and retain the document.** Keep `DocumentSet`'s identity precedence, provided-set-first/local-file behavior, and no network retrieval. For OpenAPI targets, an unmatched absolute URL now fails rather than becoming a base URL. Retain a parsed bound document and diagnostic identity per source in runtime state. Unclaimed supplied documents remain binding inputs, not an alternative operation namespace.
3. **Resolve and recognize the operation.** Resolve the URI fragment as JSON Pointer, including URI-fragment decoding and `~0`/`~1` handling. Require an actual operation position and its path-item context; an arbitrary mapping with operation-like fields is insufficient. Keep recognition independent of `operationId`. Both pointer and ID lookup select the same operation record, preserving source isolation and ambiguity refusal. Include spec-supported method positions rather than copying the current eight-method filter; `additionalOperations` preserves its method key's case.
4. **Construct one request target.** Both routes use the same `operation_server`/server-reading policy and return an internal resolved target to request preparation. `build_url_from_path` consumes that result, never reparses authored syntax. Remove request-body-dependent GET/POST guessing. Parameter and body construction stay downstream and unchanged.

Keep existing `RUNTIME_*` categories. Malformed/custom paths use `UnsupportedOperationPathForm`; missing sources/bindings use `SourceDescriptionNotFound`; unresolved or non-operation pointers use `SourceDescriptionParse`, naming source, pointer, and bound document. Preserve ID ambiguity/missing categories and distinguish unsupported source types from invalid documents. Fail before creating a planned request or sending traffic; never retry another source, guessed method, or URL.

## Ownership and removal consequences

After acceptance, refine [ac-ff633](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ff633) as the existing **runtime** owner: document retention, pointer resolution, complete operation recognition without IDs, shared server selection, and removal of unbound-ID/url-as-base execution. Do not create a second fragment ticket.

The shared parser contract belongs to **arazzo-spec**: replace exported `OperationPathForm`, classifier results, supported-form text, method splitting, and presentation semantics in an atomic coordinated cutover with the necessary runtime, validator, and presentation call-site adaptations, so no planned stage leaves consumers uncompilable. The enums/functions are public Rust surfaces; `OperationIndex` and bound-document types are internal. Retain `Engine::resolve_operation_id`'s public method/path purpose while tightening its resolution contract. Runtime depends on the shared parser; **arazzo-generate** then owns target selection/encoding and generated-document proof. No new production crate or general reference framework is needed.

CLI/MCP describe must present the canonical target without inventing a method when no document is loaded. CLI run/validate/dry-run, MCP execution, DAP/debug paths, examples, fixtures, schemas/snapshots where affected, diagnostics and conformance claims all need coordinated follow-through. Existing shortcut documents will fail; URL-only source declarations will need actual document binding. Update repository examples and error guidance in the same release cutover, with no shim and no period advertising the new generator as executable before runtime support exists. Broad fixture updates are a consequence, not permission to re-baseline unexplained changes.

## Decisive proof and remaining limits

The release proof is a canonical imported document and generated output validated and planned against the **same provided OpenAPI source**, producing the same method/path/server. Include one operation with an ID and one without, escaped pointer segments, and operation/path/document server precedence. A hermetic request capture verifies that planned and sent targets agree. Missing/non-operation pointers, custom target forms, unbound URLs/IDs, and wrong-source matches produce no request; a second source with overlapping IDs/paths cannot satisfy the first source's target.

Full reference traversal, AsyncAPI transport, JSON Schema anchors, and parameter serialization remain outside this assessment. Existing relative-server and missing-root-server limitations are not specification requirements: pointer and ID routes must share honest refusals, and ac-ff633 refinement must state their limits. Referenced path items and operations outside directly routable paths need explicit capability accounting before broad conformance claims; a pointer alone cannot invent their request context. No new compatibility decision remains. The proposed published-surface changes and bounded owner refinement still require acceptance; this assessment neither authorizes implementation nor declares those capabilities delivered.
