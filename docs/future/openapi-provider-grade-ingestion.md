# Provider-Grade OpenAPI Ingestion Plan

## Status Snapshot

**Last updated:** 2026-04-30

`arazzo-cli` is already useful as an Arazzo runtime and as a CRUD workflow
scaffolder for OpenAPI 3.x specs. It is not yet a provider-grade OpenAPI
ingestion tool.

The current OpenAPI surface is intentionally narrow:

- `arazzo generate --scenario crud` calls `generate_crud(...)` and emits CRUD
  workflows from collection/item resource shapes.
- MCP `generate_workflow` calls the same CRUD generator.
- MCP `describe_openapi` returns lightweight endpoint/schema/auth summaries.
- Runtime `--openapi` support indexes `operationId -> METHOD + path` only.

This plan closes the gap by adding a shared OpenAPI contract layer first, then
teaching generation, runtime execution, MCP tools, and validation to consume
that same contract.

## Decision

Build provider-grade OpenAPI support around a new normalized ingestion contract,
not by continuing to grow `crud.rs` directly.

The core design is:

1. Add a shared `arazzo-openapi` crate that parses, resolves, normalizes, and
   diagnoses OpenAPI specs.
2. Keep `arazzo-generate` responsible for Arazzo workflow generation.
3. Keep `arazzo-runtime` responsible for executing Arazzo, while allowing it to
   use OpenAPI contracts when resolving `operationId` targets.
4. Make CLI and MCP surfaces expose the same contract data and diagnostics.
5. Add contract-driven validation so generated workflows are checked against the
   OpenAPI operations they claim to exercise.

## Non-Goals

- Do not turn this CLI into an OpenAPI server/client code generator.
- Do not infer every arbitrary business workflow from OpenAPI alone.
- Do not perform OAuth browser/device flows inside the runtime in this plan.
  OAuth/OpenID metadata should generate explicit token inputs and diagnostics.
- Do not add network-dependent tests. Provider-shaped fixtures must be hermetic.
- Do not silently ignore unresolved references, unsupported security, or
  unsupported serialization. Emit structured diagnostics and fail closed when
  correctness depends on the missing contract.
- Do not assume new third-party dependencies will be added. The default plan is
  to build on the crates already in the workspace and add external dependencies
  only after a specific implementation ticket proves the need.

## Dependency Posture

Prefer a lean dependency graph.

The `arazzo-openapi` crate in this plan is an internal workspace crate, not a
new third-party dependency. Its first implementation should use dependencies
already present in the workspace:

- `openapiv3` for the existing OpenAPI 3.0 model.
- `serde_yaml_ng` and `serde_json` for raw document access, diagnostics, and
  OpenAPI 3.1 fields that `openapiv3` does not model.
- `url` and `percent-encoding` for URL and path/query encoding helpers.
- `reqwest` for the existing HTTP runtime, bounded remote `$ref` fetching, and
  future multipart support if needed.

External dependency additions are exception cases. A ticket proposing one must
include:

1. The provider-shaped fixture or failing case that motivated it.
2. Why the existing workspace crates are insufficient.
3. The behavioral contract the dependency would own.
4. The fallback-free failure behavior if the dependency cannot parse or model
   the input correctly.
5. Maintenance risk: crate activity, API stability, transitive dependencies,
   licenses, and security posture.

Do not add a JSON Schema validator, OpenAPI dereferencer, multipart helper, or
3.1 parser preemptively. Revisit those only when a concrete fixture proves that
the internal implementation is becoming risky or materially incomplete.

## Resolved Design Decisions

### Inspection Command Shape

Use a verb-first command group:

```bash
arazzo inspect openapi --spec <openapi> --json
```

This matches the existing CLI style better than `inspect-openapi`, and it leaves
a natural future path for other API description formats:

```bash
arazzo inspect smithy --spec <smithy> --json
```

Do not make `arazzo openapi inspect` the primary documented command. It is a
reasonable future alias if users strongly expect a protocol-first shape, but the
canonical command should be action-first.

### Provider Recipe Storage

Start with checked-in declarative recipe files plus a small typed Rust recipe
engine.

World-class API tooling should keep provider workflow knowledge reviewable,
versioned, testable, and separate from generator control flow. Hardcoding every
provider recipe directly in Rust makes contribution and review harder. Pulling
recipes from a remote registry is too early and weakens reproducibility.

The recommended shape is:

- recipe files live in the repository and ship with the binary
- recipes are declarative YAML or JSON parsed with existing workspace parsers
- Rust owns matching, validation, diagnostics, and Arazzo emission
- every recipe has provider-shaped fixture tests and golden output
- no remote recipe registry in this plan

The Rust engine should stay intentionally small: match operations by
operationId, tags, path/method patterns, required parameters, media types, and
security requirements; then emit a sequence of Arazzo steps from explicit
templates.

### Remote `$ref` Fetching

Remote HTTP(S) `$ref` fetching is in scope, but must be explicit and bounded.

Initial implementation should support remote refs only when enabled through an
explicit resolver option or CLI flag. Remote fetching must use:

- HTTP(S) only
- request timeout
- maximum fetched document size
- redirect limit
- cycle detection across local and remote refs
- stable diagnostics for fetch, parse, size, timeout, and unsupported-scheme
  failures

Tests must remain hermetic by using a local mock HTTP server, not public network
dependencies.

### Contract-Aware Runtime Execution

Contract-aware runtime `operationId` execution starts opt-in.

The default `--openapi` behavior remains the existing method/path resolution
until contract-aware execution has enough real fixture coverage. Add an explicit
mode or flag for the first version, such as:

```bash
arazzo run spec.yaml workflow --openapi api.yaml --openapi-mode contract
```

After the contract-aware path is stable, automatic contract mode can be
considered as a later feature addition.

### OAuth2/OpenID

Stop at token input wiring in this plan.

Generated workflows may expose token and scope inputs and explain the relevant
OAuth2/OpenID metadata through diagnostics. Token acquisition helpers,
loopback flows, device-code flows, and refresh-token management require a
separate future plan.

## Current System Map

### OpenAPI Generation

Files:

- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/src/generate.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-generate/src/openapi_describe.rs`
- `crates/arazzo-generate/src/refs.rs`
- `crates/arazzo-mcp/src/handlers.rs`
- `crates/arazzo-mcp/src/tools.rs`

Flow:

```text
CLI generate / MCP generate_workflow
  -> parse OpenAPI with openapiv3
  -> arazzo_generate::crud::generate_crud(...)
  -> Arazzo YAML
```

Current limitations:

- Only the `crud` scenario exists.
- Path item `$ref` values are skipped.
- `$ref` resolution handles only a few internal component families.
- Auth detection uses the first global security requirement.
- Server handling uses the first root server and substitutes variable defaults.
- Request body generation is `application/json` only.

### Runtime `operationId`

Files:

- `crates/arazzo-runtime/src/runtime_core/builder.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/src/runtime_core/state.rs`
- `crates/arazzo-cli/src/run_context.rs`
- `crates/arazzo-cli/src/test_runner.rs`

Flow:

```text
CLI run/test --openapi <file>
  -> EngineBuilder::openapi_spec(raw bytes)
  -> parse_openapi_into_index(...)
  -> BTreeMap<operationId, OperationEntry { method, path }>
  -> StepTarget::OperationId resolves to METHOD + path
  -> Arazzo step parameters/body/auth still drive the request
```

Current limitations:

- The OpenAPI operation contract is not imported into runtime request
  preparation.
- Required operation parameters are not validated against Arazzo steps.
- Request bodies, media types, examples, response schemas, security, and
  operation-level servers are not used by runtime `operationId` execution.

### Arazzo Runtime Strengths To Preserve

The runtime already supports useful authored-workflow behavior:

- Multiple parameter locations: path, query, header, cookie.
- Request body content type handling.
- Raw string payloads for non-JSON content such as XML.
- Dry-run, trace, replay, strict inputs, response-size guards, and hermetic test
  infrastructure.

Provider-grade OpenAPI ingestion should reuse these strengths rather than
replace the Arazzo runtime with an OpenAPI client.

## Gap Matrix

| Gap | Current state | Target state |
|-----|---------------|--------------|
| Operation contract | Runtime stores `operationId -> method/path` only | Shared `OperationContract` with parameters, bodies, responses, auth, servers, examples, and diagnostics |
| `$ref` resolution | Partial internal component refs | Internal refs, path item refs, nested refs, local external file refs, cycle detection, clear unsupported remote-ref diagnostics |
| OpenAPI versions | 3.0 works, 3.1 best effort, 2.x rejected | Explicit 3.0 and 3.1 compatibility profiles; 2.x import adapter deferred or fail-closed with conversion guidance |
| Servers | First root server only | Effective root/path/operation server resolution with variable handling |
| Security | First global requirement only | Global and per-operation security alternatives, compound requirements, scopes, API key/http/oauth/openid metadata |
| Parameters | Arazzo-level simplified serialization | OpenAPI style/explode/allowReserved/deepObject/matrix/label/form handling where contract-aware execution is enabled |
| Request bodies | Generated JSON only | JSON, form-urlencoded, multipart, XML/text; examples and schema-derived payloads per media type |
| Responses | Success status heuristic only | Response status/content/schema/examples imported and used for generation and validation |
| Generation | CRUD resource heuristic only | CRUD remains, plus operation catalog workflows, provider recipes, and LLM-authoring support |
| Validation | Generated YAML parses | Generated Arazzo steps are validated against OpenAPI operation contracts |

## Shared Contract Model

Add a new crate:

```text
crates/arazzo-openapi/
```

Initial public API:

```rust
pub struct OpenApiCatalog {
    pub sources: Vec<OpenApiSource>,
    pub operations: Vec<OperationContract>,
    pub diagnostics: Vec<OpenApiDiagnostic>,
}

pub struct OperationContract {
    pub source_name: String,
    pub operation_id: Option<String>,
    pub method: HttpMethod,
    pub path_template: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub servers: Vec<ServerContract>,
    pub parameters: Vec<ParameterContract>,
    pub request_bodies: Vec<RequestBodyContract>,
    pub responses: Vec<ResponseContract>,
    pub security: Vec<SecurityRequirementAlternative>,
    pub extensions: BTreeMap<String, serde_json::Value>,
}
```

Supporting contracts:

- `ParameterContract`
  - `name`
  - `location`: path, query, header, cookie
  - `required`
  - `schema`
  - `style`
  - `explode`
  - `allow_reserved`
  - `examples`
  - `deprecated`
- `RequestBodyContract`
  - `required`
  - `content_type`
  - `schema`
  - `examples`
  - `encoding`
- `ResponseContract`
  - `status`
  - `description`
  - `headers`
  - `content`
  - `examples`
- `SecuritySchemeContract`
  - `apiKey`
  - `http/basic`
  - `http/bearer`
  - `oauth2`
  - `openIdConnect`
- `OpenApiDiagnostic`
  - `severity`: error, warning, info
  - `code`: stable machine-readable code
  - `path`: OpenAPI document path
  - `message`
  - `operation_id`

Design rules:

- Diagnostics are first-class output, not stderr-only warnings.
- Unresolved refs are errors when they affect operation execution or generation.
- Unsupported features are explicit diagnostics with stable codes.
- The catalog preserves original ordering for deterministic CLI/MCP output.
- Contract structs should be serializable for `--json`, tests, MCP, and future
  caching.

## Phased Implementation Plan

### Phase 0 - Lock The Contract And Fixture Corpus

Goal: create the shared contract target before writing a large resolver.

Files:

- `docs/future/openapi-provider-grade-ingestion.md`
- `testdata/openapi/`
- `crates/arazzo-openapi/README.md` after the crate exists

Tasks:

1. Define the exact `OpenApiCatalog` and diagnostic JSON shape.
2. Add hermetic provider-shaped fixtures:
   - `stripe-checkout-shaped.openapi.yaml`
   - `cloudflare-zone-onboarding-shaped.openapi.yaml`
   - `github-repos-shaped.openapi.yaml`
   - `deep-object-query.openapi.yaml`
   - `multipart-upload.openapi.yaml`
   - `oauth2-security.openapi.yaml`
   - `external-local-refs.openapi.yaml`
3. Document which real provider behaviors each fixture models.
4. Add golden JSON snapshots for catalog output once Phase 1 exists.
5. Add the dependency posture above to the acceptance checklist for every
   implementation ticket in this plan.

Acceptance criteria:

- The contract can represent all current CRUD generation needs.
- The contract can represent Stripe-style multi-step payment setup, Cloudflare
  zone onboarding, GitHub REST operations, OAuth2 metadata, multipart upload,
  and deepObject query parameters.
- The fixtures are hermetic and small enough to maintain in-repo.

Testing obligations:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace`

### Phase 1 - Add `arazzo-openapi` With Real Ref Resolution

Goal: parse OpenAPI into a normalized catalog with reliable diagnostics.

Files:

- `Cargo.toml`
- `crates/arazzo-openapi/Cargo.toml`
- `crates/arazzo-openapi/src/lib.rs`
- `crates/arazzo-openapi/src/catalog.rs`
- `crates/arazzo-openapi/src/diagnostics.rs`
- `crates/arazzo-openapi/src/refs.rs`
- `crates/arazzo-openapi/src/servers.rs`
- `crates/arazzo-openapi/src/security.rs`
- `crates/arazzo-openapi/src/parameters.rs`
- `crates/arazzo-openapi/src/bodies.rs`
- `crates/arazzo-openapi/src/responses.rs`
- `crates/arazzo-openapi/tests/catalog.rs`

Tasks:

1. Move OpenAPI parsing helpers out of `arazzo-generate` into
   `arazzo-openapi`, using the existing `openapiv3`, `serde_yaml_ng`, and
   `serde_json` dependencies.
2. Support internal refs for:
   - path items
   - parameters
   - request bodies
   - responses
   - schemas
   - headers
   - examples
   - security schemes
3. Support local external file refs with path sandboxing relative to the root
   spec file, using `std::fs` plus the existing YAML/JSON parser stack.
4. Detect ref cycles and emit stable diagnostics.
5. Add remote HTTP(S) ref support behind an explicit resolver option or CLI
   flag, using the remote `$ref` fetching policy in the Resolved Design
   Decisions section.
6. Preserve OpenAPI document paths in diagnostics.
7. Emit stable diagnostics for unsupported schemes, remote fetch failures,
   timeout, document-size overflow, parse failure, and ref cycles.
8. Do not add a dereferencing crate in this phase. If the internal resolver
   fails a fixture in a way that would require a third-party resolver, stop and
   write a dependency-evaluation note before proceeding.

Acceptance criteria:

- Path item refs are included in the operation catalog.
- Internal component refs resolve recursively with cycle protection.
- Local external file refs resolve when they stay under the allowed root.
- Remote HTTP(S) refs resolve when explicitly enabled and produce stable
  diagnostics when disabled or unsupported.
- Existing CRUD generation can be ported to the catalog without behavioral
  regressions.
- No new third-party dependencies are introduced for parsing or dereferencing
  unless a separate dependency-evaluation note is approved.

Testing obligations:

- Unit tests for each ref family.
- Unit tests for cycle diagnostics.
- Unit tests for local external refs.
- Hermetic remote ref tests with a local mock HTTP server.
- Golden catalog snapshots for provider-shaped fixtures.
- Workspace fmt, clippy, and tests.

### Phase 2 - Replace Lightweight OpenAPI Description With Catalog Output

Goal: expose the normalized operation contract through CLI/MCP without changing
runtime execution yet.

Files:

- `crates/arazzo-generate/src/openapi_describe.rs`
- `crates/arazzo-mcp/src/handlers.rs`
- `crates/arazzo-mcp/src/tools.rs`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/src/output.rs`
- `docs/schemas/`
- `README.md`

Tasks:

1. Port `describe_openapi` to `arazzo-openapi::OpenApiCatalog`.
2. Add a CLI inspection surface:

   ```bash
   arazzo inspect openapi --spec <openapi> --json
   ```
3. Emit stable JSON with:
   - operations
   - parameters
   - request bodies
   - responses
   - security alternatives
   - server candidates
   - diagnostics
4. Update MCP `describe_openapi` to return the same shape.
5. Add schema output for the new JSON contract.

Acceptance criteria:

- CLI and MCP describe the same catalog.
- `--json` output is stable and schema-covered.
- Diagnostics are visible to agents and humans.
- Existing MCP clients still receive useful endpoint/schema/auth data, even if
  new fields are added.

Testing obligations:

- CLI integration tests for `--json`.
- MCP integration tests for `describe_openapi`.
- Schema drift test for the new JSON output.
- Workspace fmt, clippy, and tests.

### Phase 3 - Contract-Aware Parameter Serialization

Goal: make OpenAPI parameter style/explode semantics reusable by runtime and
generation.

Files:

- `crates/arazzo-openapi/src/parameters.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/src/runtime_core/url.rs`
- `crates/arazzo-runtime/tests/engine_execution.rs`
- `crates/arazzo-generate/src/crud.rs`

Tasks:

1. Implement serializers for:
   - path `simple`
   - path `label`
   - path `matrix`
   - query `form`
   - query `spaceDelimited`
   - query `pipeDelimited`
   - query `deepObject`
   - header `simple`
   - cookie `form`
2. Preserve current Arazzo-authored behavior unless an OpenAPI contract supplies
   a stricter serialization rule.
3. Add `allowReserved` handling for query parameters.
4. Keep deterministic query ordering for traces and tests.
5. Emit diagnostics when a parameter shape cannot be serialized safely.

Acceptance criteria:

- Contract-aware execution can prepare URLs matching OpenAPI style/explode
  rules.
- Existing Arazzo parameter behavior remains stable for workflows without
  OpenAPI contracts.
- Arrays and objects no longer collapse into one JSON string when the OpenAPI
  contract says `deepObject`, `pipeDelimited`, or `spaceDelimited`.

Testing obligations:

- Unit tests for every supported style/explode combination.
- Runtime dry-run tests that assert exact URLs.
- Trace/replay stability tests for serialized query/path values.
- Workspace fmt, clippy, and tests.

### Phase 4 - Contract-Aware Request Bodies And Media Types

Goal: generate and execute requests using OpenAPI media-type contracts instead
of assuming JSON.

Files:

- `crates/arazzo-openapi/src/bodies.rs`
- `crates/arazzo-generate/src/examples.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-runtime/src/runtime_core/payload.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/tests/engine_workflow.rs`
- `crates/arazzo-runtime/tests/engine_soap.rs`

Tasks:

1. Normalize request body content by media type:
   - `application/json`
   - `application/x-www-form-urlencoded`
   - `multipart/form-data`
   - XML media types
   - `text/*`
2. Generate payload examples from:
   - explicit examples
   - schema examples
   - schema defaults
   - schema-derived placeholder examples
3. Support OpenAPI `encoding` metadata for form and multipart bodies.
4. Preserve existing raw XML/text runtime behavior.
5. Add clear diagnostics for unsupported binary upload shapes.

Acceptance criteria:

- Generated workflows choose the best supported request media type rather than
  hardcoding JSON.
- Multipart and form-urlencoded specs generate runnable Arazzo starting points.
- Authored XML/SOAP workflows continue to execute as raw body payloads.
- Unsupported encodings fail with actionable diagnostics.

Testing obligations:

- Generator tests for JSON, form, multipart, XML/text.
- Runtime tests for exact content type and body bytes.
- Dry-run JSON output tests for non-JSON payload visibility.
- Workspace fmt, clippy, and tests.

### Phase 5 - Security And Server Resolution

Goal: import provider auth/server structure accurately without hiding security
choices.

Files:

- `crates/arazzo-openapi/src/security.rs`
- `crates/arazzo-openapi/src/servers.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-runtime/src/runtime_core/builder.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-mcp/src/handlers.rs`

Tasks:

1. Resolve effective servers in OpenAPI precedence order:
   - operation
   - path item
   - root
2. Preserve server variables as explicit choices:
   - default value
   - enum values
   - generated input option when needed
3. Resolve effective security in OpenAPI precedence order:
   - operation security
   - root security
   - explicit empty security means no auth
4. Represent alternatives and compound requirements:
   - OR across security requirement objects
   - AND across schemes inside one requirement object
5. Generate workflow inputs for required auth values.
6. For OAuth2/OpenID, generate token/scopes inputs and diagnostics; do not
   implement token acquisition in this phase.

Acceptance criteria:

- Per-operation security overrides global security.
- No-auth operations stay no-auth even when the API has global security.
- Compound auth requirements can generate multiple parameters.
- Server variables are not silently defaulted without diagnostics.
- Runtime contract-aware `operationId` execution can select the effective
  server when the Arazzo step does not explicitly route through a
  `sourceDescription`.

Testing obligations:

- Catalog tests for root/path/operation server precedence.
- Catalog tests for global, per-operation, empty, alternative, and compound
  security.
- Generator tests for API key, bearer, basic, OAuth2 metadata, and no-auth
  operation overrides.
- Runtime dry-run tests for operation-level server selection.
- Workspace fmt, clippy, and tests.

### Phase 6 - Contract-Aware Runtime `operationId` Execution

Goal: make runtime `operationId` support useful for OpenAPI-heavy workflows
without making OpenAPI mandatory for Arazzo execution.

Files:

- `crates/arazzo-runtime/src/runtime_core/builder.rs`
- `crates/arazzo-runtime/src/runtime_core/state.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/src/runtime_core/error.rs`
- `crates/arazzo-cli/src/run_context.rs`
- `crates/arazzo-cli/src/test_runner.rs`
- `crates/arazzo-runtime/tests/engine_execution.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Tasks:

1. Replace `OperationEntry { method, path }` with contract-backed operation
   entries.
2. Keep existing method/path-only resolution for workflows that do not supply an
   OpenAPI contract.
3. Add an explicit opt-in CLI/runtime mode for contract-aware execution, such as
   `--openapi-mode contract`.
4. When `operationId` resolves through a contract:
   - apply effective server selection
   - apply parameter serialization
   - validate required parameters
   - choose content type for request bodies
   - apply contract-derived auth parameters when generated/declared by Arazzo
5. Emit structured runtime errors for:
   - missing operation
   - missing required parameter
   - unsupported serialization
   - unsupported media type
   - ambiguous security requirement
6. Preserve dry-run and trace visibility.

Acceptance criteria:

- Existing `operationId` tests still pass.
- A provider-shaped Arazzo workflow can reference `operationId` and rely on the
  OpenAPI contract for method/path/server/parameter serialization when
  contract-aware mode is enabled.
- Existing `--openapi` behavior remains method/path-only unless the explicit
  contract-aware mode is selected.
- Missing required operation parameters fail before an HTTP request is sent.
- Dry-run output shows the exact prepared provider request.

Testing obligations:

- Runtime unit and integration tests for contract-backed operationId execution.
- CLI dry-run tests with `--openapi` and provider-shaped fixtures.
- Replay tests for contract-backed requests.
- Workspace fmt, clippy, and tests.

### Phase 7 - Contract-Driven Workflow Generation

Goal: make generation useful beyond CRUD without pretending OpenAPI alone
contains all business workflows.

Files:

- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-generate/src/lib.rs`
- `crates/arazzo-generate/src/scenarios.rs`
- `crates/arazzo-generate/src/recipes.rs`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-mcp/src/tools.rs`
- `crates/arazzo-mcp/src/handlers.rs`
- `README.md`

Tasks:

1. Port CRUD generation to consume `OpenApiCatalog`.
2. Add operation-level generation:

   ```bash
   arazzo generate --spec api.yaml --operation createCheckoutSession
   arazzo generate --spec api.yaml --tag checkout
   ```

3. Add `--scenario catalog` to generate one workflow per selected operation.
4. Add recipe support for provider-shaped multi-step flows:
   - recipes live as checked-in declarative files bundled with the repo/binary
   - Rust owns recipe parsing, matching, validation, diagnostics, and emission
   - recipes match operations by stable IDs, tags, path patterns, and required
     inputs
   - recipes emit diagnostics when required operations are absent
   - no remote recipe registry in this plan
5. Keep CRUD generation as a named scenario.
6. Add MCP arguments for operation/tag/scenario selection.

Acceptance criteria:

- CRUD output is behaviorally stable after moving to the catalog.
- Users can generate a runnable one-operation workflow for any operation with a
  supported contract.
- Users can generate tag-filtered workflow sets.
- Provider recipes can produce multi-step workflows when the operation catalog
  contains the required operations.
- Failed recipe matching explains exactly what is missing.

Testing obligations:

- Golden YAML tests for CRUD before/after catalog migration.
- Golden YAML tests for operation and tag generation.
- Provider-shaped recipe tests for Stripe checkout and Cloudflare onboarding.
- MCP integration tests for new generation arguments.
- Workspace fmt, clippy, and tests.

### Phase 8 - Contract Validation And Quality Gates

Goal: prove generated workflows match the OpenAPI contracts they target.

Files:

- `crates/arazzo-generate/src/validation.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/src/output.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`
- `docs/schemas/`

Tasks:

1. Add `validate_generated_workflow_against_catalog(...)`.
2. Validate each generated step:
   - target operation exists
   - required path/query/header/cookie params are supplied or generated
   - request body content type is supported by the operation
   - generated success criteria reference declared response statuses
   - generated auth inputs match the selected security requirement
   - no unresolved refs remain in the operation contract
3. Emit `GenerateOutput.diagnostics` in both text and JSON modes.
4. Fail generation by default on contract errors.
5. Keep warnings for lossy-but-runnable choices, such as using a schema-derived
   example because no example was supplied.

Acceptance criteria:

- Generated workflows cannot silently target missing operations.
- Missing required parameters fail generation.
- Unsupported media types fail generation unless the user explicitly selects a
  future permissive mode.
- CLI and MCP expose diagnostics with stable codes.

Testing obligations:

- Unit tests for each validation rule.
- CLI tests for success, warning, and error diagnostics.
- JSON schema tests for diagnostic output.
- Workspace fmt, clippy, and tests.

### Phase 9 - OpenAPI 3.1 And Swagger 2.x Strategy

Goal: make version behavior explicit and trustworthy.

Files:

- `crates/arazzo-openapi/src/version.rs`
- `crates/arazzo-openapi/src/schema.rs`
- `crates/arazzo-openapi/tests/version.rs`
- `README.md`

Tasks:

1. Define support profiles:
   - OpenAPI 3.0: supported
   - OpenAPI 3.1: supported where the existing parser plus raw YAML/JSON access
     can preserve the needed semantics; emit diagnostics for 3.1-only JSON
     Schema features not yet modeled
   - Swagger/OpenAPI 2.x: unsupported directly unless a dedicated adapter is
     added
2. Decide whether 2.x support should be:
   - a built-in adapter,
   - an optional conversion command,
   - or documented as "convert before ingestion".
3. Add 3.1 fixtures for:
   - `type: ["string", "null"]`
   - `const`
   - `unevaluatedProperties`
   - `$schema`
   - `webhooks`
4. Preserve unsupported 3.1 features as diagnostics, not silent drops.
5. Do not add a dedicated OpenAPI 3.1 crate unless a provider fixture requires
   semantics that cannot be represented with the existing parser plus raw
   document access.

Acceptance criteria:

- Version diagnostics are deterministic and visible.
- 3.1-only features do not silently become incorrect 3.0 assumptions.
- 2.x failure messaging points to the chosen conversion path.
- No new 3.1 parser dependency is introduced without an explicit
  dependency-evaluation note.

Testing obligations:

- Version fixture tests.
- Snapshot tests for diagnostics.
- Workspace fmt, clippy, and tests.

## Verification Matrix

Every implementation ticket in this plan should include at least:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

High-risk phases should also include targeted gates:

```bash
cargo test -p arazzo-openapi
cargo test -p arazzo-generate
cargo test -p arazzo-runtime
cargo test -p arazzo-cli --test cli_integration
cargo test -p arazzo-mcp
```

Provider-shaped smoke checks should use dry-run and never depend on external
network calls:

```bash
cargo run -p arazzo-cli -- generate --spec testdata/openapi/stripe-checkout-shaped.openapi.yaml --scenario catalog --json
cargo run -p arazzo-cli -- run testdata/openapi/generated/stripe-checkout.arazzo.yaml checkout --openapi testdata/openapi/stripe-checkout-shaped.openapi.yaml --dry-run --json
```

## Recommended Ticket Breakdown

1. Create `arazzo-openapi` crate and contract model.
2. Add hermetic provider-shaped OpenAPI fixtures.
3. Implement internal and local external `$ref` resolution with existing
   workspace dependencies.
4. Implement explicit, bounded remote HTTP(S) `$ref` fetching with hermetic
   local-server tests.
5. Port MCP `describe_openapi` to catalog output.
6. Add `arazzo inspect openapi --spec <openapi> --json`.
7. Implement OpenAPI parameter serialization helpers.
8. Wire opt-in contract-aware parameter serialization into dry-run/runtime.
9. Implement request body media-type normalization.
10. Implement server and security resolution.
11. Replace runtime `operationId` index with contract-backed entries behind
    opt-in contract-aware mode.
12. Port CRUD generation to `OpenApiCatalog`.
13. Add operation/tag/catalog generation scenarios.
14. Add bundled declarative recipe format and typed Rust recipe engine.
15. Add first provider recipe: Stripe checkout shaped fixture.
16. Add second provider recipe: Cloudflare zone onboarding shaped fixture.
17. Add contract-driven generation validation.
18. Add OpenAPI 3.1 diagnostics profile.
19. Decide and implement/document Swagger 2.x strategy.
20. Only if blocked by a concrete fixture: write a dependency-evaluation note
    for the smallest external crate that solves the proven gap.

## Open Questions

None for the initial implementation shape. The remaining decision points should
be handled as follow-up feature plans after the opt-in contract-aware path and
the bundled recipe format have real fixture coverage.

## Success Definition

This plan is successful when an OpenAPI-heavy user can:

1. Inspect a provider OpenAPI spec and see accurate operations, auth, servers,
   parameters, bodies, responses, examples, and diagnostics.
2. Generate runnable Arazzo workflows for CRUD resources, individual
   operations, tags, and known provider-shaped recipes.
3. Execute `operationId` workflows with opt-in OpenAPI-backed request
   preparation.
4. Catch missing parameters, unsupported media types, unresolved refs, and
   security ambiguities before a live request is sent.
5. Trust `--json` and MCP outputs as stable contracts for agents and CI.
