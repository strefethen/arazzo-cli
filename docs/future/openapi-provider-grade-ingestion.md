# Provider-Grade OpenAPI Ingestion Plan

## Status Snapshot

**Last updated:** 2026-05-01

This is the authoritative merged plan for making `arazzo-cli` a
provider-grade OpenAPI ingestion tool. It incorporates the Codex and Claude
revision reviews while preserving the original direction: add a shared OpenAPI
contract layer instead of growing `crud.rs` directly.

`arazzo-cli` is already useful as an Arazzo runtime and CRUD workflow
scaffolder for OpenAPI 3.x specs. It is not yet provider-grade OpenAPI
ingestion.

The current OpenAPI surface is intentionally narrow:

- `arazzo generate --scenario crud` calls `generate_crud(...)` and emits CRUD
  workflows from collection/item resource shapes.
- MCP `generate_workflow` calls the same CRUD generator.
- MCP `describe_openapi` returns lightweight endpoint/schema/auth summaries.
- Runtime `--openapi` support indexes `operationId -> METHOD + path` only.
- CLI `run`, `replay`, and `test` currently pass OpenAPI inputs as raw bytes
  into `EngineBuilder::openapi_spec(...)`.
- Current `generate --json` emits structured JSON only when `--output` is used;
  without `--output`, generated YAML can leak to stdout even when global
  `--json` is present.

This plan closes the gap by adding a source-aware OpenAPI contract layer first,
then teaching generation, runtime execution, MCP tools, and validation to
consume that same contract.

## Merged Direction

The merged plan adopts these architectural decisions:

- Source identity is first-class: every ingestion call starts from
  `OpenApiSourceInput`, `SourceUri`, and stable `source_id` values.
- Operation identity is source-qualified: `OperationKey` is authoritative, and
  duplicate `operationId` values produce diagnostics and fail closed when a
  lookup depends on the ambiguous ID.
- `generate --json` gets a stable JSON envelope early, before new scenarios or
  declarative recipe support is added.
- Schema fidelity uses `SchemaContract`, not a field-level `FieldSource` flag.
  The contract variant, diagnostics, and coverage summary preserve whether a
  fact is typed, raw, unresolved, unsupported, or synthesized.
- Remote `$ref` support is a late security-gated phase. It is disabled by
  default, denies internal network targets by default, and is not exposed to MCP
  without an explicit allowlist.
- MCP `describe_openapi` keeps a temporary `legacy` compatibility block while
  exposing the same catalog envelope as the CLI.
- Contract validation lands before declarative recipe support.
- OpenAPI 3.1 diagnostics begin in Phase 1, while broader 3.1 strategy remains
  a later phase.
- MCP catalog caching is a hard Phase 2 requirement.
- Provider-shaped fixtures include both hand-shaped hermetic specs and at least
  one trimmed real provider fragment with attribution.
- Recipe schema design is locked early, but recipes are implemented only after
  source identity, validation, runtime contract mode, and security/body/parameter
  semantics are strong enough to prove correctness.

## Central Invariant

No generated or runtime request may depend on unresolved, ambiguous, raw-only,
unsupported, or synthesized OpenAPI contract facts without:

1. a structured diagnostic,
2. a visible JSON representation,
3. a fail-closed decision when correctness depends on that fact,
4. a test proving the behavior.

This invariant applies to source identity, `$ref` resolution, `operationId`
lookup, schema handling, parameter serialization, request bodies, servers,
security, generated examples, runtime dry-run, trace, replay, CLI JSON, and MCP
responses.

## Decision

Build provider-grade OpenAPI support around a new source-aware, diagnostic-first
contract crate:

```text
crates/arazzo-openapi/
```

The crate owns:

- source identity and resolver policy
- OpenAPI version profile detection
- internal and local `$ref` resolution
- normalized operation contracts
- operation identity and duplicate detection
- schema contract representation
- contract diagnostics with stable codes
- request, response, parameter, server, and security summaries

The crate does not own:

- Arazzo workflow generation
- HTTP execution
- OAuth token acquisition
- remote recipe registries
- general OpenAPI client/server code generation

The core design is:

1. Add `arazzo-openapi` to parse, resolve, normalize, and diagnose OpenAPI
   documents.
2. Keep `arazzo-generate` responsible for Arazzo workflow generation.
3. Keep `arazzo-runtime` responsible for executing Arazzo, while allowing it to
   use OpenAPI contracts in opt-in contract mode.
4. Make CLI and MCP surfaces expose the same contract data and diagnostics.
5. Add contract-driven validation before declarative recipe output is considered
   successful.

## Non-Goals

- Do not turn this CLI into an OpenAPI server/client code generator.
- Do not infer arbitrary business workflows from OpenAPI alone.
- Do not perform OAuth browser/device/refresh flows inside the runtime in this
  plan.
- Do not add data-related fallback or degraded-mode behavior.
- Do not add network-dependent tests. Provider-shaped fixtures must be hermetic.
- Do not silently ignore unresolved refs, duplicate operation IDs, unsupported
  security, unsupported serialization, unsupported media types, or unsupported
  schema semantics.
- Do not enable remote `$ref` fetching by default.
- Do not claim OpenAPI 3.1 support through `openapiv3` alone.
- Do not add broad third-party dependencies without a dependency-evaluation note
  tied to a failing fixture.

## Dependency Posture

Prefer a lean dependency graph.

The `arazzo-openapi` crate is an internal workspace crate, not a new third-party
dependency. Its first implementation should use dependencies already present in
the workspace:

- `openapiv3` for the existing OpenAPI 3.0 model.
- `serde_yaml_ng` and `serde_json` for raw document access, diagnostics, and
  OpenAPI 3.1 fields that `openapiv3` does not model.
- `url` and `percent-encoding` for URL and path/query encoding helpers.
- `reqwest` only where the existing async HTTP stack is already appropriate.

External dependency additions are exception cases. A ticket proposing one must
include:

1. The provider-shaped fixture or failing case that motivated it.
2. Why the existing workspace crates are insufficient.
3. The behavioral contract the dependency would own.
4. The fallback-free failure behavior if the dependency cannot parse or model the
   input correctly.
5. Maintenance risk: crate activity, API stability, transitive dependencies,
   licenses, and security posture.

Do not add a JSON Schema validator, OpenAPI dereferencer, multipart helper, or
3.1 parser preemptively. Revisit those only when a concrete fixture proves that
the internal implementation is becoming risky or materially incomplete.

## Provider-Grade Definition

Provider-grade means the tool can ingest provider-shaped OpenAPI documents and
either produce a correct, inspectable, runnable starting point or fail with
diagnostics before generating or executing a misleading workflow.

A feature is not provider-grade if it:

- loses the source path needed to resolve local external refs
- cannot distinguish two operations with the same `operationId`
- silently drops security alternatives or operation overrides
- generates request bodies that violate required schema shape
- serializes deep object or array parameters incorrectly without diagnostics
- treats 3.1 JSON Schema keywords as safe 3.0 semantics
- exposes different CLI and MCP contracts for the same OpenAPI input
- re-parses large specs on every MCP tool call

## Resolved Design Decisions

### Inspection Command Shape

Use a verb-first command group:

```bash
arazzo --json inspect openapi --spec <openapi>
```

This matches the existing CLI style better than `inspect-openapi`, and it leaves
a natural future path for other API description formats:

```bash
arazzo --json inspect smithy --spec <smithy>
```

Do not make `arazzo openapi inspect` the primary documented command. It is a
reasonable future alias if users strongly expect a protocol-first shape, but the
canonical command should be action-first.

### JSON Envelopes

Every new machine-readable surface gets a `schemaVersion` and participates in
schema drift checks.

`generate --json` must always emit JSON, with or without `--output`:

```json
{
  "kind": "generate",
  "schemaVersion": "generate.v2alpha1",
  "yaml": "arazzo: 1.0.0\n...",
  "file": null,
  "workflows": 1,
  "steps": 3,
  "resources": [],
  "diagnostics": []
}
```

If `--output` is present, `file` is set. `yaml` may be omitted only if the schema
explicitly documents that behavior. Human-mode YAML output can remain unchanged.

### Schema Contract And Provenance

Use `SchemaContract` as the source of truth for schema fidelity. Do not add a
field-level `FieldSource` flag to every consumer.

The plan still preserves the underlying provenance invariant:

- Schema consumers must inspect the `SchemaContract` variant before making
  correctness-sensitive decisions.
- Raw or unsupported 3.1 schema facts are visible in diagnostics and coverage.
- Synthesized defaults and placeholders are visible through diagnostics or
  summary coverage, not silent behavior.
- Runtime and validation fail closed when unsupported schema semantics affect
  request correctness.

### MCP Compatibility

MCP `describe_openapi` returns the same catalog envelope as the CLI and keeps a
temporary compatibility object until clients migrate:

```json
{
  "kind": "openapiCatalog",
  "schemaVersion": "openapi.catalog.v1alpha1",
  "catalog": {},
  "diagnostics": [],
  "legacy": {
    "endpoints": [],
    "schemas": [],
    "auth_schemes": []
  }
}
```

The `legacy` object can be removed only through a later compatibility plan.

### MCP Catalog Caching

MCP `describe_openapi` must not re-parse unchanged large specs on every call.

Phase 2 caches parsed catalogs in `ServerState`, keyed by at least:

- absolute path
- file mtime with the highest available precision
- file size
- dependency fingerprint for every local external ref resolved during cataloging
- resolver policy
- relevant CLI/MCP options that affect catalog shape

If the filesystem cannot provide precise mtimes, the cache must either include a
content digest or re-parse on uncertainty. LRU sizing remains a human decision
before Phase 2 merges.

### Declarative Recipe Format

Design a generic declarative recipe format plus a small typed Rust recipe engine,
but implement recipe execution only after validation and runtime contract mode
are strong enough to prove emitted workflows.

This repository must stay a generic Arazzo executor. Provider-shaped recipe
fixtures are useful for tests and examples, but the default binary must not ship
provider-specific workflow logic such as "Stripe checkout" or "Cloudflare zone
onboarding" as built-in behavior.

The recommended shape is:

- the recipe schema and generic engine live in the repository
- provider-shaped recipes live under fixtures/examples or are supplied by users,
  not compiled into the default command behavior
- recipes are declarative YAML parsed with `serde_yaml_ng`
- Rust owns matching, validation, diagnostics, and Arazzo emission
- every checked-in example recipe has provider-shaped fixture tests and golden
  output
- no remote recipe registry in this plan

Recipe schema draft:

```yaml
# testdata/openapi/recipes/stripe-checkout.example.recipe.yaml
recipe:
  id: stripe-checkout
  version: 1
  description: One-time Stripe Checkout payment flow.
  match:
    requires:
      - operationId: createCheckoutSession
        method: POST
        path: /v1/checkout/sessions
      - operationId: retrieveCheckoutSession
        method: GET
        path: /v1/checkout/sessions/{id}
    prefers:
      - tag: checkout
      - server-pattern: "^https://api\\.stripe\\.com"
  inputs:
    - name: api_key
      from: security.bearer
      required: true
    - name: success_url
      type: string
      required: true
  steps:
    - id: create-session
      operation: createCheckoutSession
      body:
        success_url: "{$inputs.success_url}"
        mode: payment
    - id: poll-session
      operation: retrieveCheckoutSession
      params:
        path:
          id: "{$steps.create-session.outputs.id}"
```

Matching rules:

1. A recipe matches a spec only when every `requires` clause is satisfied.
2. When multiple recipes match, the recipe with the most satisfied `prefers`
   clauses wins.
3. Ties are broken by recipe `id` lexicographic order with a diagnostic noting
   the ambiguity.
4. Recipes do not extend other recipes in this plan.
5. Recipes match by selectors that resolve to exactly one `OperationKey`, not
   ambiguous `operationId` alone.
6. Recipes are rejected at load time when their required operation selectors are
   internally ambiguous.

Recipes are validated at compile time against a checked-in JSON Schema once the
recipe phase lands.

### Remote `$ref` Fetching

Remote HTTP(S) `$ref` fetching is a late, explicit, bounded, security-gated
phase. It is not implemented with local refs.

Default behavior:

- Remote refs are disabled everywhere.
- CLI can enable remote refs only through explicit flags and resolver policy.
- MCP remote refs remain disabled unless the server is started with an explicit
  remote-ref allowlist.
- Tests remain hermetic and use a local mock HTTP server.

Remote fetching must enforce:

- HTTP(S) only
- request timeout
- maximum fetched document size
- total fetch document count
- total fetched byte budget
- redirect limit
- re-check every redirect target against policy
- check resolved IP addresses for the original host and every redirect target
- deny private, loopback, link-local, multicast, and unspecified IP targets by
  default
- optional host allowlist
- content-type validation for JSON/YAML media types
- reject URLs with embedded credentials and redact sensitive URL components in
  diagnostics
- cycle detection across local and remote refs
- stable diagnostics for disabled remote refs, unsupported schemes, policy
  denial, DNS/connection failure, timeout, size overflow, redirect overflow,
  parse failure, and cycles

The implementation must decide the async boundary before coding:

1. make cataloging async,
2. isolate remote fetching in CLI/MCP async handlers,
3. or write a dependency note for a blocking client.

### Contract-Aware Runtime Execution

Contract-aware runtime `operationId` execution starts opt-in.

The default `--openapi` behavior remains existing method/path resolution until
contract-aware execution has enough real fixture coverage. Add an explicit mode
or flag for the first version:

```bash
arazzo run spec.yaml workflow --openapi api.yaml --openapi-mode contract
```

Automatic contract mode can be considered only after the opt-in path is stable.

### Arazzo Binding Strategy

Generation and runtime need a way to preserve OpenAPI-derived facts that Arazzo
1.0 cannot currently carry directly.

Phase 0 must decide the initial binding strategy:

1. vendor extension only: `x-arazzo-cli.openapiBinding`
2. `arazzo-spec` schema model expansion
3. both, with extensions first

This plan recommends extensions first, with `arazzo-spec` changes only when a
specific contract fact needs to become portable Arazzo model surface.

### OAuth2/OpenID

Stop at token input wiring in this plan.

Generated workflows may expose token and scope inputs and explain the relevant
OAuth2/OpenID metadata through diagnostics. Token acquisition helpers, loopback
flows, device-code flows, and refresh-token management require a separate future
plan.

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
- Cycle detection exists in schema refs but not uniformly across every ref
  family.
- Auth detection uses the first global security requirement.
- Server handling uses the first root server and substitutes variable defaults.
- Request body generation is `application/json` only.
- `generate --json` lacks a stable always-JSON envelope when no output file is
  used.
- MCP `describe_openapi` re-parses the spec on every tool call.

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

- OpenAPI source identity is lost before ref resolution.
- `operationId` is treated as globally unique even though repeatable
  `--openapi` inputs can collide.
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
| Source identity | Runtime gets raw bytes; generator parses from file path ad hoc | `OpenApiSourceInput` and `source_id` flow through catalog, CLI, MCP, and runtime contract mode |
| Operation identity | Runtime stores `operationId -> method/path` only | `OperationKey` with source-qualified lookup and duplicate diagnostics |
| `$ref` resolution | Partial internal component refs, asymmetric cycle detection | Internal refs, path item refs, local external file refs, uniform cycle detection, late security-gated remote refs |
| OpenAPI versions | 3.0 works, 3.1 best effort, 2.x rejected | Phase 1 3.1 diagnostics, raw schema preservation, fail-closed unsupported semantics, 2.x conversion guidance |
| Servers | First root server only | Effective root/path/operation server resolution with variable handling |
| Security | First global requirement only | Global and per-operation security alternatives, compound requirements, scopes, API key/http/oauth/openid metadata |
| Parameters | Arazzo-level simplified serialization | OpenAPI style/explode/allowReserved/deepObject/matrix/label/form handling where contract-aware execution is enabled |
| Request bodies | Generated JSON only | JSON, form-urlencoded, multipart, XML/text; examples and schema-derived payloads per media type |
| Responses | Success status heuristic only | Response status/content/schema/examples imported and used for generation and validation |
| Generation | CRUD resource heuristic only | CRUD remains, plus operation catalog workflows, generic declarative recipes, and agent-readable diagnostics |
| Validation | Generated YAML parses | Generated Arazzo steps are validated against OpenAPI operation contracts before recipes land |
| MCP performance | Re-parses spec on every `describe_openapi` call | Catalog cached in `ServerState`, invalidated on root file, local ref, or policy change |

## Shared Contract Model

### Source Identity

Every ingestion call starts with source-aware inputs, never anonymous raw bytes.

```rust
pub struct OpenApiInput {
    pub sources: Vec<OpenApiSourceInput>,
    pub resolver_policy: ResolverPolicy,
}

pub struct OpenApiSourceInput {
    pub source_id: String,
    pub display_name: String,
    pub uri: SourceUri,
    pub bytes: Vec<u8>,
}

pub enum SourceUri {
    FilePath(String),
    InlineName(String),
    RemoteUrl(String),
}
```

Rules:

- CLI file inputs use `SourceUri::FilePath`.
- MCP file inputs use `SourceUri::FilePath` only after `check_path_allowed`.
- In-memory tests use `SourceUri::InlineName`.
- `source_id` values must be unique within one `OpenApiInput`.
- CLI/MCP source-id derivation must be deterministic and must not leak absolute
  paths into stable JSON IDs unless a human explicitly chooses that behavior.
- `SourceUri::RemoteUrl` is rejected until the remote-ref phase enables an
  explicit policy.
- Local external refs resolve relative to the root source file, not process CWD.
- Diagnostics always include `source_id` and a JSON Pointer where possible.

### Resolver Policy

```rust
pub struct ResolverPolicy {
    pub local_external_refs: LocalExternalRefPolicy,
    pub remote_refs: RemoteRefPolicy,
    pub max_ref_depth: usize,
}

pub enum LocalExternalRefPolicy {
    Disabled,
    RootRelativeSandbox,
}

pub enum RemoteRefPolicy {
    Disabled,
    ExplicitAllowList(RemoteRefAllowList),
}

pub struct RemoteRefAllowList {
    pub allowed_hosts: Vec<String>,
    pub allow_private_networks_for_tests: bool,
    pub timeout_ms: u64,
    pub max_document_bytes: usize,
    pub max_total_bytes: usize,
    pub max_documents: usize,
    pub max_redirects: usize,
}
```

Remote policy is modeled now but implemented later. This prevents local ref
support from baking in assumptions that cannot survive remote refs.

An empty `allowed_hosts` list denies all remote fetches. Wildcard host matching
is out of scope for the first remote-ref implementation unless a later security
review explicitly approves it.

`allow_private_networks_for_tests` is for hermetic local-server tests only. A
production CLI/MCP path must not silently turn that test escape hatch on.

### Operation Identity

`operationId` is not globally unique across repeatable `--openapi` inputs.

```rust
pub struct OperationKey {
    pub source_id: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub pointer: String,
}

pub struct OperationLookup {
    pub by_key: BTreeMap<OperationKey, usize>,
    pub by_operation_id: BTreeMap<String, Vec<OperationKey>>,
}
```

Rules:

- `OperationContract.key` is the stable identity.
- `operationId` is metadata on `OperationContract`, not part of the stable key.
- `pointer` points to the operation location in the root source document; external
  refs keep their own diagnostic `external_uri` instead of changing identity.
- `operationId` lookup succeeds only when exactly one operation matches.
- Duplicate `operationId` values produce diagnostics during cataloging.
- Runtime contract mode fails with `RUNTIME_OPERATION_ID_AMBIGUOUS` if an
  Arazzo `operationId` maps to more than one operation.
- Generation recipes match by selectors that resolve to exactly one
  `OperationKey`, or by `operationId` only when the match is unique.

### Catalog Contract

```rust
pub struct OpenApiCatalog {
    pub schema_version: String,
    pub sources: Vec<OpenApiSource>,
    pub version_profile: VersionProfile,
    pub operations: Vec<OperationContract>,
    pub lookup: OperationLookup,
    pub diagnostics: Vec<OpenApiDiagnostic>,
    pub coverage: CatalogCoverage,
}

pub struct CatalogCoverage {
    pub typed_schema_count: usize,
    pub raw_schema_count: usize,
    pub unresolved_ref_count: usize,
    pub synthesized_fact_count: usize,
}

pub struct OperationContract {
    pub key: OperationKey,
    pub operation_id: Option<String>,
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

`schema_version` starts at `openapi.catalog.v1alpha1` until the JSON contract is
covered by schema drift tests.

### Schema Contract

The first implementation must preserve schema facts without pretending to fully
validate JSON Schema.

```rust
pub enum SchemaContract {
    OpenApi30(OpenApi30SchemaSummary),
    RawJsonSchema {
        dialect: Option<String>,
        raw: serde_json::Value,
        unsupported_keywords: Vec<String>,
    },
    Unsupported {
        reason: String,
        raw: Option<serde_json::Value>,
    },
    UnresolvedRef {
        reference: String,
    },
}
```

Rules:

- 3.0 schemas can be summarized through `openapiv3` plus raw fields.
- 3.1 schemas that use keywords not modeled by `openapiv3` remain raw and emit
  diagnostics until a real dialect strategy exists.
- Request-body validation may start as structural validation, but must not claim
  full JSON Schema validation unless a validator dependency is approved.
- Runtime and validation may rely on `OpenApi30` structural facts.
- Runtime and validation must fail closed when `RawJsonSchema`, `Unsupported`,
  or `UnresolvedRef` affects correctness.
- Generated placeholder examples are not schema facts. They are recorded through
  diagnostics and coverage, then validated against whatever schema facts are
  available.

### Diagnostics

```rust
pub struct OpenApiDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub source_id: Option<String>,
    pub pointer: Option<String>,
    pub external_uri: Option<String>,
    pub operation_key: Option<OperationKey>,
    pub message: String,
}
```

Diagnostic code families:

- `OPENAPI_PARSE_*`
- `OPENAPI_VERSION_*`
- `OPENAPI_REF_*`
- `OPENAPI_OPERATION_*`
- `OPENAPI_SCHEMA_*`
- `OPENAPI_SECURITY_*`
- `OPENAPI_SERVER_*`
- `OPENAPI_SERIALIZATION_*`
- `OPENAPI_REMOTE_REF_*`
- `GENERATE_CONTRACT_*`
- `RUNTIME_CONTRACT_*`

Design rules:

- Diagnostics are first-class output, not stderr-only warnings.
- Unresolved refs are errors when they affect operation execution or generation.
- Unsupported features are explicit diagnostics with stable codes.
- The catalog preserves original ordering for deterministic CLI/MCP output.
- Contract structs should be serializable for `--json`, tests, MCP, caching, and
  future CI drift checks.

## JSON Output Contracts

### Inspect OpenAPI

Canonical command:

```bash
arazzo --json inspect openapi --spec <openapi>
```

JSON envelope:

```json
{
  "kind": "openapiCatalog",
  "schemaVersion": "openapi.catalog.v1alpha1",
  "catalog": {},
  "diagnostics": []
}
```

Schema command:

```bash
arazzo schema inspect-openapi
```

### Generate

`generate --json` must always emit JSON, with or without `--output`.

```json
{
  "kind": "generate",
  "schemaVersion": "generate.v2alpha1",
  "yaml": "arazzo: 1.0.0\n...",
  "file": null,
  "workflows": 1,
  "steps": 3,
  "resources": [],
  "diagnostics": []
}
```

Rules:

- If `--output` is omitted, generated YAML appears in the `yaml` field.
- If `--output` is present, `file` is set and `yaml` may be omitted only if the
  schema explicitly documents that behavior.
- Contract errors return `kind: "generateError"` with stable diagnostics and a
  non-zero exit.
- Existing human-mode YAML output can remain unchanged.

### MCP Compatibility

MCP `describe_openapi` should return the catalog envelope and keep a
compatibility object until clients migrate:

```json
{
  "kind": "openapiCatalog",
  "schemaVersion": "openapi.catalog.v1alpha1",
  "catalog": {},
  "diagnostics": [],
  "legacy": {
    "endpoints": [],
    "schemas": [],
    "auth_schemes": []
  }
}
```

The `legacy` object can be removed only through a later compatibility plan.

## Phase Dependencies

Phases use a 12-phase shape: Phase 0 through Phase 11.

```text
0 -> 1
1 -> 2, 5, 6, 7, 10
2 -> 3, 9
3 -> 4
4 + 5 + 6 + 7 -> 8
4 + 5 + 6 + 7 -> 11
```

Dependency rules:

- Phase 0 locks decisions and fixtures.
- Phase 1 creates source-aware cataloging, local refs, operation identity, and
  initial 3.1 diagnostics.
- Phase 2 exposes the catalog through CLI/MCP and adds MCP caching.
- Phase 3 fixes `generate --json` and ports CRUD to the catalog.
- Phase 4 validates generated workflows before declarative recipe output lands.
- Phases 5, 6, and 7 can proceed after Phase 1 for catalog-side helpers and may
  be parallelized. Their generation-validation integration depends on Phase 4;
  their runtime use waits for Phase 8.
- Phase 8 runtime contract mode depends on validation plus parameter, body, and
  security/server semantics.
- Phase 9 remote refs cannot start until source identity, CLI/MCP boundaries, and
  the remote security policy are stable.
- Phase 10 expands version strategy; Phase 1 still emits the first 3.1
  diagnostics.
- Phase 11 recipes land only after validation and contract semantics are strong
  enough to prove generated workflows.

## Phased Implementation Plan

### Phase 0 - Decisions, Contract, Fixtures, And Recipe Schema

Goal: lock the contract and blocking decisions before implementation.

Files:

- `docs/future/openapi-provider-grade-ingestion.md`
- `testdata/openapi/`
- `testdata/openapi/real-fragments/`
- `docs/schemas/` after JSON surfaces exist
- `crates/arazzo-generate/recipes/recipe.schema.json` after recipe schema lands

Tasks:

1. Define `OpenApiInput`, `OpenApiSourceInput`, `ResolverPolicy`,
   `OperationKey`, `OpenApiCatalog`, `SchemaContract`, and diagnostics.
2. Define exact JSON envelopes for:
   - `--json inspect openapi`
   - `generate --json`
   - MCP `describe_openapi`
3. Add hermetic provider-shaped fixture descriptions before writing code:
   - `stripe-checkout-shaped.openapi.yaml`
   - `cloudflare-zone-onboarding-shaped.openapi.yaml`
   - `github-repos-shaped.openapi.yaml`
   - `deep-object-query.openapi.yaml`
   - `multipart-upload.openapi.yaml`
   - `oauth2-security.openapi.yaml`
   - `external-local-refs.openapi.yaml`
   - `duplicate-operation-ids.openapi.yaml`
   - `openapi-31-raw-schema.openapi.yaml`
4. Add at least one trimmed-but-real provider spec fragment under
   `testdata/openapi/real-fragments/` only after license/attribution is approved.
   If no provider license allows a checked-in excerpt, add a documented substitute
   fixture that is generated from public shape knowledge rather than copied text.
5. Document which real provider behaviors each fixture models.
   Fixtures may use provider-shaped names, but implementation must not branch on
   provider identity or fixture names.
6. Lock the recipe schema draft enough that Phase 11 does not invent it from
   scratch.
7. Decide whether initial Arazzo binding is:
   - vendor extension only: `x-arazzo-cli.openapiBinding`
   - `arazzo-spec` schema model expansion
   - both, with extensions first
8. Decide source-id derivation and path privacy for CLI/MCP JSON output.
9. Decide the initial Swagger/OpenAPI 2.x strategy. This plan recommends
   fail-closed with conversion guidance.
10. Add the dependency posture above to the acceptance checklist for every
   implementation ticket in this plan.

Acceptance criteria:

- No implementation ticket starts while a dependent decision is open.
- The contract can represent current CRUD generation needs.
- The contract can represent provider-shaped multi-step setup flows, GitHub-style
  REST operations, OAuth2 metadata, multipart upload, duplicate operation IDs,
  3.1 raw schemas, and deepObject query parameters without baking provider logic
  into the product.
- Fixtures are hermetic and small enough to maintain in-repo.
- At least one real-fragment fixture is committed with an attribution note, or a
  documented no-copy substitute is committed with the reason copying was rejected.
- The JSON shape for `generate --json` is defined before diagnostics are added.

Testing obligations:

- Documentation review only until code exists.
- Once fixture files are added, `cargo test --workspace` must remain green.

### Phase 1 - Source-Aware `arazzo-openapi` With Local Refs And 3.1 Diagnostics

Goal: create the crate, make local source identity real, and emit early OpenAPI
3.1 diagnostics before remote refs or runtime contract mode.

Files:

- `Cargo.toml`
- `crates/arazzo-openapi/Cargo.toml`
- `crates/arazzo-openapi/src/lib.rs`
- `crates/arazzo-openapi/src/source.rs`
- `crates/arazzo-openapi/src/catalog.rs`
- `crates/arazzo-openapi/src/diagnostics.rs`
- `crates/arazzo-openapi/src/refs.rs`
- `crates/arazzo-openapi/src/version.rs`
- `crates/arazzo-openapi/src/schema.rs`
- `crates/arazzo-openapi/src/servers.rs`
- `crates/arazzo-openapi/src/security.rs`
- `crates/arazzo-openapi/src/parameters.rs`
- `crates/arazzo-openapi/src/bodies.rs`
- `crates/arazzo-openapi/src/responses.rs`
- `crates/arazzo-openapi/tests/catalog.rs`
- `crates/arazzo-openapi/tests/version_diagnostics.rs`

Tasks:

1. Add the internal crate using existing workspace dependencies only.
2. Parse root OpenAPI documents from `OpenApiSourceInput`.
3. Preserve `source_id`, `display_name`, and root file location.
4. Detect OpenAPI version before deserializing into `openapiv3` where possible.
5. Detect `openapi: "3.1.x"` early and emit diagnostics for 3.1-only constructs:
   - JSON Schema array `type: ["string", "null"]`
   - `const`
   - `unevaluatedProperties`
   - `prefixItems`
   - top-level `webhooks`
   - `$schema`
6. Preserve affected schemas as `SchemaContract::RawJsonSchema` when the typed
   model cannot represent the construct.
7. Implement internal `$ref` resolution for:
   - path items
   - parameters
   - request bodies
   - responses
   - schemas
   - headers
   - examples
   - security schemes
8. Implement local external file refs under `RootRelativeSandbox`.
9. Detect cycles across internal and local external refs.
10. Emit diagnostics for unresolved refs, cycles, external path escape,
    unsupported schemes, duplicate `operationId`, parse failure with source
    identity, and raw 3.1 schema facts.
11. Do not implement remote HTTP(S) refs in this phase.
12. Do not add a dereferencing crate in this phase. If the internal resolver
    fails a fixture in a way that would require a third-party resolver, stop and
    write a dependency-evaluation note before proceeding.

Acceptance criteria:

- Local external refs cannot resolve outside the allowed root.
- Duplicate `operationId` values are visible in catalog diagnostics.
- `OperationKey` is stable and deterministic.
- Path item refs are included in the operation catalog.
- Internal component refs resolve recursively with uniform cycle protection.
- A 3.1 fixture using `type: ["string", "null"]` produces a stable diagnostic and
  a raw schema contract.
- Catalog construction for a trimmed 50K+ line real-fragment or no-copy
  substitute fixture has a recorded benchmark. The initial target is under 2
  seconds on a developer machine, but the first implementation ticket must record
  measured hardware and decide whether this becomes a CI-enforced threshold or a
  regression-tracking benchmark.
- Existing CRUD generation is not changed in this phase.

Testing obligations:

- `cargo test -p arazzo-openapi`
- Unit tests for internal refs, local external refs, path escape, and cycles.
- Version-diagnostics tests for every 3.1 construct enumerated above.
- Benchmark or release-mode measurement for the Phase 1 performance budget.
  Enforce it in CI only after the measured baseline is approved.
- Workspace fmt, clippy, and tests.

### Phase 2 - CLI And MCP Catalog Inspection With Caching

Goal: expose the catalog through CLI/MCP without changing runtime execution or
generation, and prevent repeated MCP parses of unchanged specs.

Files:

- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/src/output.rs`
- `crates/arazzo-mcp/src/handlers.rs`
- `crates/arazzo-mcp/src/tools.rs`
- `crates/arazzo-mcp/src/state.rs`
- `docs/schemas/`
- `README.md`

Tasks:

1. Add `arazzo --json inspect openapi --spec <openapi>`.
2. Add `arazzo schema inspect-openapi`.
3. Route CLI OpenAPI file inputs through `OpenApiSourceInput`.
4. Route MCP file inputs through `check_path_allowed` before cataloging.
5. Port MCP `describe_openapi` to the catalog envelope with `legacy` fields.
6. Cache parsed `OpenApiCatalog` values in `ServerState`.
7. Key the cache by absolute path, precise mtime, file size, dependency
   fingerprint for resolved local external refs, resolver policy, and
   catalog-affecting options.
8. Invalidate when mtime, file size, any dependency fingerprint, relevant policy,
   or relevant options change. If precise mtime is unavailable, use a content
   digest or re-parse.
9. Add JSON schemas and schema drift checks for the new output.
10. Keep human output concise but include diagnostic counts.

Acceptance criteria:

- CLI and MCP expose the same catalog data.
- JSON output is schema-covered.
- MCP compatibility fields still include endpoint/schema/auth summaries.
- Local external refs work through CLI and MCP with source-aware diagnostics.
- Repeated MCP `describe_openapi` calls on the same unchanged spec parse the file
  at most once.

Testing obligations:

- CLI integration tests for `--json inspect openapi`.
- MCP integration tests for `describe_openapi`.
- MCP cache test: two consecutive calls with identical args parse once.
- MCP cache invalidation test: modifying spec mtime triggers a re-parse.
- MCP cache invalidation test: modifying a local external ref triggers a
  re-parse of the root catalog.
- Schema drift tests for `inspect-openapi`.
- Workspace fmt, clippy, and tests.

### Phase 3 - Fix `generate --json` And Port CRUD To Catalog

Goal: make generation use the shared catalog and fix the JSON contract before new
generation scenarios are added.

Files:

- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-generate/src/lib.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/src/output.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`
- `docs/schemas/generate.schema.json`

Tasks:

1. Define `GenerateOutput` v2 with `schemaVersion`, `yaml`, optional `file`,
   summary counts, resources, and diagnostics.
2. Make `arazzo --json generate --spec ...` emit JSON even without `--output`.
3. Add `arazzo schema generate` to schema drift tests.
4. Port CRUD generation to consume `OpenApiCatalog`.
5. Preserve existing human-mode YAML output.
6. Preserve existing CRUD behavior through golden before/after output tests.
7. Surface duplicate operation and unresolved-ref diagnostics in generation.

Acceptance criteria:

- `generate --json` never writes raw YAML to stdout.
- CRUD output is behaviorally stable after catalog migration.
- Contract diagnostics appear in both CLI and MCP generation results.
- Generation fails by default on catalog errors that affect emitted steps.

Testing obligations:

- CLI tests for `generate --json` with and without `--output`.
- Golden YAML CRUD tests.
- Schema drift test for `generate.schema.json`.
- Workspace fmt, clippy, and tests.

### Phase 4 - Contract Validation And Arazzo Binding Strategy

Goal: prove generated steps match operation contracts before broader provider
generation is added. This phase starts with structural validation and a single
validator entry point; Phases 5, 6, and 7 extend that same validator as
serialization, body, server, and security semantics mature.

Files:

- `crates/arazzo-generate/src/validation.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-cli/src/output.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`
- `crates/arazzo-spec/src/lib.rs` only if the chosen binding strategy requires
  model changes

Tasks:

1. Add `validate_generated_workflow_against_catalog(...)`.
2. Validate each generated step:
   - target operation exists
   - target operation is unique
   - required path/query/header/cookie parameters are supplied or generated
   - parameter value shape is compatible with the parameter schema summary
   - request body content type is supported
   - generated body includes required top-level object fields when known
   - generated body respects enum/default/example choices when known
   - success criteria reference declared success response statuses
   - output expressions that claim generated IDs point at plausible response
     schema fields when known
   - selected auth inputs match the chosen security requirement
   - no unresolved refs remain in contract fields used by the generated step
3. Add `x-arazzo-cli.openapiBinding` when Arazzo's public model cannot carry a
   provider-specific contract fact.
4. Emit diagnostics for lossy-but-runnable choices.
5. Fail generation by default on validation errors.

Acceptance criteria:

- Generated workflows cannot silently target missing or ambiguous operations.
- Missing required parameters fail generation.
- Unsupported or shape-invalid request bodies fail generation unless a future
  explicit permissive mode is designed.
- Validation results are present in `GenerateOutput.diagnostics`.
- Raw, unresolved, or synthesized schema facts cannot be treated as authoritative
  without diagnostics.

Testing obligations:

- Unit tests for each validation rule.
- Fixtures for missing required param, invalid enum, unsupported body media type,
  duplicate operation ID, unresolved body schema ref, and raw 3.1 schema fact.
- Workspace fmt, clippy, and tests.

### Phase 5 - Parameter Serialization Helpers

Goal: implement OpenAPI parameter serialization once and share it across
generation validation and runtime contract mode.

Files:

- `crates/arazzo-openapi/src/parameters.rs`
- `crates/arazzo-runtime/src/runtime_core/url.rs`
- `crates/arazzo-runtime/tests/engine_execution.rs`
- `crates/arazzo-generate/src/validation.rs`

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
2. Implement `allowReserved` for query parameters.
3. Keep deterministic ordering for traces and tests.
4. Preserve current Arazzo-authored behavior when no OpenAPI contract is active.
5. Emit diagnostics or runtime errors when a value cannot be serialized safely.

Acceptance criteria:

- Arrays and objects do not collapse into JSON strings when the contract requires
  deepObject, pipeDelimited, or spaceDelimited serialization.
- Existing authored Arazzo workflows without contract mode remain stable.
- Contract validation and runtime contract mode use the same serializer.

Testing obligations:

- Unit tests for every supported style/explode combination.
- Runtime dry-run tests asserting exact URLs.
- Trace/replay stability tests.
- Workspace fmt, clippy, and tests.

### Phase 6 - Request Bodies And Media Types

Goal: normalize request body media types for generation, validation, and runtime
contract mode without breaking authored XML/text workflows.

Files:

- `crates/arazzo-openapi/src/bodies.rs`
- `crates/arazzo-openapi/src/schema.rs`
- `crates/arazzo-generate/src/examples.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-runtime/src/runtime_core/payload.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/tests/engine_workflow.rs`
- `crates/arazzo-runtime/tests/engine_media_types.rs`
- `crates/arazzo-runtime/tests/engine_soap.rs`

Tasks:

1. Normalize body content for:
   - `application/json`
   - `application/x-www-form-urlencoded`
   - `multipart/form-data`
   - XML media types
   - `text/*`
2. Preserve raw XML/text runtime behavior.
3. Generate body examples from explicit examples, schema examples, defaults, and
   schema-derived placeholders.
4. Support OpenAPI `encoding` metadata for form and multipart bodies.
5. Add clear errors for unsupported binary upload shapes.
6. Keep request-body validation aligned with the schema contract from Phase 4.

Acceptance criteria:

- Generated workflows choose a supported media type instead of hardcoding JSON.
- Multipart and form-urlencoded specs generate runnable starting points or fail
  with diagnostics.
- Authored XML/SOAP workflows continue to execute as raw body payloads.
- Unsupported encodings fail before generation or execution.

Testing obligations:

- Generator tests for JSON, form, multipart, XML/text.
- Runtime tests for exact content type and body bytes.
- Dry-run JSON output tests for non-JSON payload visibility.
- SOAP regression tests remain green.
- Workspace fmt, clippy, and tests.

### Phase 7 - Server And Security Resolution

Goal: import provider auth and server structure accurately without hiding
choices.

Files:

- `crates/arazzo-openapi/src/security.rs`
- `crates/arazzo-openapi/src/servers.rs`
- `crates/arazzo-generate/src/crud.rs`
- `crates/arazzo-generate/src/validation.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-mcp/src/handlers.rs`

Tasks:

1. Resolve effective servers in OpenAPI precedence order:
   - operation
   - path item
   - root
2. Preserve server variables as choices, defaults, and diagnostics.
3. Resolve effective security in OpenAPI precedence order:
   - operation security
   - root security
   - explicit empty security means no auth
4. Represent security alternatives and compound requirements:
   - OR across requirement objects
   - AND across schemes inside one requirement object
5. Generate auth inputs for required auth values.
6. For OAuth2/OpenID, generate token and scope inputs plus diagnostics. Do not
   implement token acquisition.
7. Fail generation or runtime contract mode on ambiguous security unless the
   workflow or recipe selects a requirement.

Acceptance criteria:

- Per-operation security overrides global security.
- No-auth operations stay no-auth even when global security exists.
- Compound requirements can generate multiple inputs.
- Server variables are not silently defaulted without diagnostics.
- Ambiguous security alternatives are visible and fail closed when execution
  correctness depends on selection.

Testing obligations:

- Catalog tests for server precedence.
- Catalog tests for security alternatives, empty overrides, and compound auth.
- Generator tests for API key, bearer, basic, OAuth2 metadata, and no-auth
  operation overrides.
- Runtime dry-run tests for operation-level server selection.
- Workspace fmt, clippy, and tests.

### Phase 8 - Contract-Aware Runtime `operationId` Mode

Goal: make runtime `operationId` support useful for OpenAPI-heavy workflows
without changing default behavior.

Files:

- `crates/arazzo-runtime/src/runtime_core/builder.rs`
- `crates/arazzo-runtime/src/runtime_core/state.rs`
- `crates/arazzo-runtime/src/runtime_core/engine_http.rs`
- `crates/arazzo-runtime/src/runtime_core/error.rs`
- `crates/arazzo-cli/src/run_context.rs`
- `crates/arazzo-cli/src/test_runner.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-runtime/tests/engine_execution.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Prerequisites:

- Phase 4 validation is merged.
- Phase 5 parameter serialization is merged.
- Phase 6 body/media semantics are merged.
- Phase 7 server/security resolution is merged.

Tasks:

1. Add source-aware OpenAPI inputs to runtime builder APIs.
2. Keep existing raw-byte helper only as a test/compatibility shim if needed.
3. Add explicit mode:

   ```bash
   arazzo run spec.yaml workflow --openapi api.yaml --openapi-mode contract
   ```

4. Keep default `--openapi` method/path behavior unchanged.
5. In contract mode:
   - resolve operation by unique `operationId`
   - fail on ambiguous `operationId`
   - apply effective server selection
   - apply parameter serialization
   - validate required parameters before HTTP
   - choose or validate request content type
   - apply selected security binding only when generated or declared by Arazzo
   - refuse strict contract execution when required request facts are raw,
     unresolved, unsupported, or synthesized
6. Emit structured runtime errors for missing operation, ambiguous operation,
   missing required parameter, unsupported serialization, unsupported media type,
   ambiguous security requirement, and unsupported schema semantics.
7. Preserve dry-run, trace, replay, strict inputs, and response-size behavior.

Acceptance criteria:

- Existing `operationId` tests still pass in default mode.
- Contract mode fails before HTTP on missing required parameters.
- Contract mode fails before HTTP on ambiguous operation IDs.
- Contract mode fails before HTTP when unsupported/raw schema semantics affect
  required request correctness.
- Dry-run output shows the exact prepared provider request.
- Replay can re-execute contract-backed requests without live network access.

Testing obligations:

- Runtime unit and integration tests for contract mode.
- CLI dry-run tests with provider-shaped fixtures.
- Replay tests for contract-backed requests.
- Workspace fmt, clippy, and tests.

### Phase 9 - Remote `$ref` Fetching Security Gate

Goal: add remote refs only after source identity, local refs, CLI/MCP boundaries,
and the remote security policy are stable.

Files:

- `crates/arazzo-openapi/src/refs.rs`
- `crates/arazzo-openapi/src/remote.rs`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-mcp/src/state.rs`
- `crates/arazzo-mcp/src/tools.rs`
- `crates/arazzo-openapi/tests/remote_refs.rs`

Tasks:

1. Decide async boundary before coding:
   - make cataloging async, or
   - isolate remote fetching in CLI/MCP async handlers, or
   - write a dependency note for a blocking client.
2. Keep remote refs disabled by default.
3. CLI may enable remote refs only through explicit flags.
4. MCP remote refs remain disabled unless `serve` is started with an explicit
   remote-ref allowlist.
5. Enforce:
   - HTTP(S) only
   - timeout
   - maximum fetched document size
   - total document and byte budget
   - redirect limit
   - re-check every redirect target against policy
   - check resolved IP addresses for the original host and every redirect target
   - deny private, loopback, link-local, multicast, and unspecified IP targets
     unless explicitly allowed for local tests
   - optional host allowlist
   - content-type validation
   - rejection of URLs with embedded credentials and diagnostic redaction for
     sensitive URL components
   - cycle detection across local and remote refs
6. Emit stable diagnostics for disabled remote refs, unsupported scheme, policy
   denial, DNS/connection failure, timeout, size overflow, redirect overflow,
   parse failure, and cycles.

Acceptance criteria:

- No remote request occurs unless explicit policy enables it.
- Hermetic tests use a local mock HTTP server.
- MCP cannot fetch arbitrary remote or internal network targets by default.
- Remote diagnostics include the source ref that triggered the fetch.
- Redirect targets are re-evaluated against policy.

Testing obligations:

- Unit tests for policy decisions.
- Hermetic local-server tests for success, redirect, timeout, size, content type,
  and denial.
- Tests proving no request is made when remote refs are disabled.
- Workspace fmt, clippy, and tests.

### Phase 10 - OpenAPI 3.1 And Swagger 2.x Strategy Expansion

Goal: make version behavior explicit and trustworthy across the full surface.
Phase 1 already emits initial 3.1 diagnostics; this phase expands support
profiles and decides conversion/adapters.

Files:

- `crates/arazzo-openapi/src/version.rs`
- `crates/arazzo-openapi/src/schema.rs`
- `crates/arazzo-openapi/tests/version.rs`
- `README.md`

Tasks:

1. Define support profiles:
   - OpenAPI 3.0: supported.
   - OpenAPI 3.1: accepted for inspect mode when raw-schema preservation can
     retain needed fields; generation/runtime fail closed on unsupported schema
     features.
   - Swagger/OpenAPI 2.x: fail closed with conversion guidance for the initial
     plan unless a dedicated adapter is approved.
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
6. If semantic 3.1 generation or execution is required, stop and write a
   dependency-evaluation note for the parser/schema strategy.

Acceptance criteria:

- Version diagnostics are deterministic and visible.
- 3.1-only features do not silently become incorrect 3.0 assumptions.
- 3.1 parse failures become deterministic diagnostics where possible.
- Generation/runtime contract mode fail closed when unsupported 3.1 semantics
  affect correctness.
- 2.x failure messaging points to the chosen conversion path.
- No new 3.1 parser dependency is introduced without an explicit
  dependency-evaluation note.

Testing obligations:

- Version fixture tests.
- Snapshot tests for diagnostics.
- Workspace fmt, clippy, and tests.

### Phase 11 - Declarative Recipes And Broader Generation

Goal: add useful provider-shaped generation only after the contract, validation,
runtime mode, parameter/body/security semantics, and recipe schema are strong
enough to prove correctness, while keeping provider-specific recipes outside the
default product behavior.

Files:

- `crates/arazzo-generate/src/scenarios.rs`
- `crates/arazzo-generate/src/recipes.rs`
- `crates/arazzo-generate/src/validation.rs`
- `crates/arazzo-generate/src/recipe_schema.rs`
- `testdata/openapi/recipes/`
- `docs/schemas/recipe.schema.json`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-mcp/src/tools.rs`
- `crates/arazzo-mcp/src/handlers.rs`
- `README.md`

Tasks:

1. Add operation-level generation:

   ```bash
   arazzo generate --spec api.yaml --operation createCheckoutSession
   arazzo generate --spec api.yaml --tag checkout
   ```

2. Add `--scenario catalog` to generate one workflow per selected operation.
3. Add generic support for user-supplied declarative recipes.
4. Validate every checked-in example recipe against the recipe JSON Schema in
   tests.
5. Recipes match operations by selectors that resolve to exactly one
   `OperationKey`, not ambiguous `operationId` alone.
6. Recipes emit diagnostics when required operations, media types, auth schemes,
   or response outputs are absent.
7. Recipe ambiguity emits a stable diagnostic.
8. Every recipe output runs through contract validation.
9. Add initial example recipes:
   - Stripe checkout shaped fixture
   - Cloudflare zone onboarding shaped fixture
10. Add MCP arguments for operation/tag/scenario selection.

Acceptance criteria:

- CRUD output remains behaviorally stable after catalog migration.
- Users can generate runnable one-operation workflows for supported operations.
- Users can generate tag-filtered workflow sets.
- Example and user-supplied recipes fail with exact missing-contract diagnostics.
- Recipe ambiguity produces a deterministic pick plus diagnostic, or fails closed
  if deterministic selection would be misleading.
- No recipe bypasses contract validation, and no provider-specific recipe ships as
  built-in default behavior.

Testing obligations:

- Golden YAML tests for CRUD, operation, tag, and catalog generation.
- Provider-shaped recipe tests.
- Recipe tie-breaking/ambiguity tests.
- MCP integration tests for new generation arguments.
- Workspace fmt, clippy, and tests.

## Verification Matrix

Every implementation ticket in this plan includes:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

High-risk phases also include targeted gates:

```bash
cargo test -p arazzo-openapi
cargo test -p arazzo-generate
cargo test -p arazzo-runtime
cargo test -p arazzo-cli --test cli_integration
cargo test -p arazzo-mcp
```

Provider-shaped smoke checks use dry-run and local fixtures only:

```bash
cargo run -p arazzo-cli -- --json inspect openapi --spec testdata/openapi/stripe-checkout-shaped.openapi.yaml
cargo run -p arazzo-cli -- --json generate --spec testdata/openapi/stripe-checkout-shaped.openapi.yaml --scenario catalog
cargo run -p arazzo-cli -- --json run testdata/openapi/generated/stripe-checkout.arazzo.yaml checkout --openapi testdata/openapi/stripe-checkout-shaped.openapi.yaml --openapi-mode contract --dry-run
```

Real-fragment smoke check:

```bash
cargo run -p arazzo-cli -- --json inspect openapi --spec testdata/openapi/real-fragments/stripe-checkout-trimmed.openapi.yaml
```

Remote-ref smoke checks, when Phase 9 exists, must use a local mock server and
must prove no request is made when remote refs are disabled.

## Recommended Ticket Breakdown

1. Lock source-aware catalog contract, schema contract, and JSON envelopes.
2. Add provider-shaped fixture corpus, including duplicate IDs and 3.1 raw schema
   cases.
3. Add real-fragment provider spec with attribution.
4. Lock the initial recipe schema draft.
5. Create `arazzo-openapi` crate with source-aware inputs.
6. Implement internal ref resolution and diagnostics.
7. Implement local external refs with root-relative sandboxing.
8. Implement operation identity and duplicate `operationId` diagnostics.
9. Implement OpenAPI version detection and Phase 1 3.1 diagnostics.
10. Add Phase 1 performance budget benchmark.
11. Add `--json inspect openapi` and schema drift coverage.
12. Port MCP `describe_openapi` to catalog output with legacy compatibility.
13. Add MCP catalog caching and invalidation.
14. Fix `generate --json` envelope and schema coverage.
15. Port CRUD generation to `OpenApiCatalog`.
16. Add generated workflow contract validation.
17. Add binding strategy support through `x-arazzo-cli.openapiBinding` or
    approved spec model changes.
18. Implement parameter serialization helpers.
19. Implement request body media-type normalization.
20. Implement server and security resolution.
21. Add opt-in runtime `--openapi-mode contract`.
22. Add remote-ref security policy and async-boundary decision note.
23. Implement remote refs only after the policy is approved.
24. Expand OpenAPI 3.1 diagnostics profile and document initial 2.x guidance.
25. Add operation/tag/catalog generation.
26. Implement generic user-supplied recipe execution from the locked schema.
27. Add provider-shaped example recipe fixtures and require contract validation
    for each output.
28. Only if blocked by a concrete fixture: write a dependency-evaluation note for
    the smallest external crate that solves the proven gap.

## Explicit Human Decisions Needed

1. Should remote `$ref` fetching ever be allowed in MCP, or should it remain
   CLI-only?
2. What default remote-ref allowlist policy should ship for CLI use?
3. Should duplicate `operationId` be fatal at catalog time, or only fatal when a
   lookup depends on the ambiguous ID?
4. Should initial Arazzo/OpenAPI binding use only `x-arazzo-cli` extensions, or
   should `arazzo-spec` grow first-class fields?
5. Is a JSON Schema validator dependency acceptable once validation needs exceed
   structural checks?
6. Should OpenAPI 2.x remain convert-before-ingestion for the first release?
7. Should `generate --json --output file.yaml` include the YAML string, or only
   the output file path plus summary and diagnostics?
8. What MCP cache size/eviction policy should Phase 2 use?
9. What attribution pattern is acceptable for trimmed real provider fragments?
10. Should strict validation be on by default in CI-like environments?
11. Should stable JSON expose absolute file paths, or should source IDs be
    redacted/hash-derived with paths limited to human text output?

## Open Questions

- Exact remote-ref CLI flag names.
- Exact `schemaVersion` strings once the JSON contracts are ready to stabilize.
- Whether 3.1 inspection should expose raw schemas directly or summarize them
  behind a smaller schema contract.
- How long MCP `legacy` fields stay in `describe_openapi`.
- Where user-supplied recipe files are loaded from, and whether checked-in
  example recipes live only under `testdata/openapi/recipes/`.
- Whether the Phase 1 2-second performance target should be lower for common
  real-provider specs after measurement.
- Whether catalog migrations need feature flags for rollback or hard cutovers.

## Success Definition

This plan is successful when an OpenAPI-heavy user can:

1. Inspect a provider OpenAPI spec and see source-aware operations, auth, servers,
   parameters, bodies, responses, examples, duplicate-ID diagnostics, version
   diagnostics, schema coverage, and ref diagnostics.
2. Use MCP interactively on large unchanged specs without per-call re-parse
   latency.
3. Generate JSON-described Arazzo workflows with stable diagnostics and no
   `--json` raw-YAML leakage.
4. Generate CRUD, one-operation, tag, catalog, and generic declarative-recipe
   workflows only when contract validation proves the emitted steps are coherent.
5. Execute `operationId` workflows in opt-in contract mode with correct server
   selection, parameter serialization, body media type, security binding, dry-run
   output, trace, and replay behavior.
6. Fail closed before live HTTP when required parameters, unsupported media types,
   unresolved refs, ambiguous security, ambiguous operation IDs, unsafe remote
   refs, or unsupported 3.1 semantics affect correctness.
7. Trust `--json` and MCP outputs as stable contracts for agents and CI.
