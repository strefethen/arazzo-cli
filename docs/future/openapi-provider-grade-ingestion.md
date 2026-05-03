# Provider-Grade OpenAPI Ingestion Plan

## Status

**Last updated:** 2026-05-03

This plan is the floated-down implementation target derived from earlier
audit-driven design rounds. The exhaustive prior version (twelve phases,
extensive defensive scope) is recoverable from git history at commit `719ceaf`
and earlier. This version trades thoroughness for confidence: fewer phases,
each one shipping visible improvement, decisions made inline, speculative
scope explicitly out.

## Goal

`arazzo-cli` already runs Arazzo workflows and scaffolds CRUD workflows from
OpenAPI 3.x specs. This plan closes the gap to a tool that can ingest
provider-shaped OpenAPI documents, generate runnable Arazzo with contract-aware
request preparation, and fail closed when the contract does not support a
correct request.

When this plan is done, an OpenAPI-heavy user can:

1. Inspect a provider OpenAPI spec and see normalized operations, parameters,
   bodies, responses, security, and diagnostics through stable JSON.
2. Use the MCP server interactively on large unchanged specs without
   per-call re-parse latency.
3. Generate Arazzo workflows for CRUD resources, individual operations, and
   tag-filtered sets, validated against the spec.
4. Execute `operationId` workflows in opt-in contract mode with correct
   server selection, parameter serialization, body media type, and security
   binding.
5. Trust `--json` output as a stable contract for agents and CI.

## Decision

Add a shared OpenAPI contract layer instead of growing `crud.rs` directly.

```text
crates/arazzo-openapi/   # new internal workspace crate
```

Owns: source identity, version detection, ref resolution, normalized operation
contracts, operation identity, schema preservation, contract diagnostics.

Does NOT own: Arazzo workflow generation, HTTP execution, OAuth token
acquisition, recipe registries, OpenAPI client/server code generation.

Generation, runtime, CLI, and MCP all consume the same catalog.

## Out Of Scope (explicit, with revisit conditions)

Everything in this section is deliberately not in the plan, not deferred to a
later phase. Each item has a named condition for when to revisit.

| Out of scope | Revisit when |
|---|---|
| Remote `$ref` HTTP fetching | A real provider spec we want to support uses remote refs. Until then, keep the policy types modeled but unimplemented. |
| Declarative recipe engine (provider templates) | At least three concrete user-supplied recipe scenarios exist. Until then, generation covers CRUD + per-operation + per-tag, which is enough for the immediate audience. |
| Comprehensive OpenAPI 3.1 strategy | A 3.1 spec we actually want to support hits a specific keyword we cannot diagnose-and-skip. Phase 1 emits one bucketed `OPENAPI_VERSION_31_UNSUPPORTED_KEYWORDS` diagnostic; richer per-keyword work waits for that pressure. |
| Swagger / OpenAPI 2.x adapter | A real 2.x spec we want to support shows up. Until then: README documents convert-before-ingestion via `swagger2openapi`. |
| MCP `legacy` compatibility shim | This is a small-team project; the MCP client and the developer iterate together. CHANGELOG documents the cutover; consumers update or pin a tag. |
| Trimmed real-provider fixtures with attribution policy | Hand-shaped fixtures demonstrably miss something the test suite needs to catch. Until then, hermetic shaped fixtures are sufficient. |
| OAuth2 token acquisition (loopback / device-code / refresh) | Always out of scope for this plan; separate future plan if the runtime needs first-class auth. |
| JSON Schema validator dependency | Phase 4 validation hits a fixture that needs deeper JSON-Schema semantics than structural checks can reach. Until then, validation is structural. |

## Inline Decisions (resolved here, not deferred to Phase 0)

- **Source-id derivation:** filename stem (`stripe.openapi.yaml` → `stripe`).
  Collisions get a numeric suffix (`stripe-2`). MCP file inputs go through
  `check_path_allowed` first; CLI uses the user-supplied path.
- **JSON path privacy:** `source_id` in JSON is the filename stem, not a
  path. Absolute paths appear only in human-readable error text and
  diagnostic `display_name` fields.
- **Arazzo binding for OpenAPI-derived facts:** vendor extension
  `x-arazzo-cli.openapiBinding` on Arazzo steps. `arazzo-spec` schema growth
  considered only if Phase 6 proves an extension is unworkable.
- **Strict validation default:** off in CLI (warnings); `--strict` opts into
  fail-closed. `arazzo` library callers default to strict.
- **MCP catalog cache:** content-hash keyed (SHA-256 of root file), LRU 16
  entries, configurable via `ARAZZO_MCP_CATALOG_CACHE_SIZE`. Local external
  refs included in the fingerprint so changes to a referenced file invalidate
  the root catalog.
- **`openapiv3` upgrade policy:** pinned in workspace `Cargo.toml`. Bump only
  when a fixture forces it; bumps require golden-snapshot reruns.
- **Duplicate `operationId` policy:** emit `OPENAPI_OPERATION_ID_DUPLICATE`
  at catalog time as a warning. Lookup is fatal only when the lookup itself
  is ambiguous (`RUNTIME_OPERATION_ID_AMBIGUOUS`,
  `GENERATE_OPERATION_AMBIGUOUS`).
- **`generate --json` envelope:** stable `kind: "generate"`,
  `schemaVersion: "generate.v1"`, `yaml` field present without `--output`.
  This fixes the existing bug where YAML leaks to stdout.

## Current System Map

| Concern | File | Limitation |
|---|---|---|
| OpenAPI parsing | `crates/arazzo-generate/src/refs.rs` | Internal refs only; cycle detection only on schemas; path-item refs skipped |
| CRUD generation | `crates/arazzo-generate/src/crud.rs` (~1000 lines) | First global security only; first root server only; JSON bodies only |
| MCP describe | `crates/arazzo-mcp/src/handlers.rs` (`describe_openapi`) | Re-parses on every call; lightweight summary only |
| Runtime operationId | `crates/arazzo-runtime/src/runtime_core/state.rs` (`OperationEntry`) | `BTreeMap<String, {method, path}>` collapses duplicates |
| `generate --json` | `crates/arazzo-cli/src/handlers.rs` | Bug: YAML printed to stdout when `--output` omitted |

## Architecture

### Catalog types

```rust
pub struct OpenApiCatalog {
    pub schema_version: &'static str,    // "openapi.catalog.v1alpha1"
    pub sources: Vec<OpenApiSource>,
    pub operations: Vec<OperationContract>,
    pub by_operation_id: BTreeMap<String, Vec<OperationKey>>,
    pub diagnostics: Vec<OpenApiDiagnostic>,
}

pub struct OpenApiSource {
    pub source_id: String,        // filename stem, deduplicated
    pub display_name: String,
    pub file_path: Option<PathBuf>, // None for in-memory test inputs
    pub openapi_version: OpenApiVersion,
}

pub struct OperationKey {
    pub source_id: String,
    pub method: HttpMethod,
    pub path_template: String,
}

pub struct OperationContract {
    pub key: OperationKey,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub servers: Vec<ServerContract>,           // resolved per precedence
    pub parameters: Vec<ParameterContract>,
    pub request_bodies: Vec<RequestBodyContract>,
    pub responses: Vec<ResponseContract>,
    pub security: Vec<SecurityRequirementAlt>,
    pub extensions: BTreeMap<String, serde_json::Value>,
}

pub struct OpenApiDiagnostic {
    pub severity: Severity,
    pub code: &'static str,    // e.g., "OPENAPI_REF_UNRESOLVED"
    pub source_id: Option<String>,
    pub pointer: Option<String>,
    pub operation_key: Option<OperationKey>,
    pub message: String,
}
```

Schema fidelity is preserved with `Option<OpenApi30SchemaSummary>` plus the
diagnostics list. If Phase 4 or Phase 6 needs a richer enum, add it then —
not before.

### JSON envelopes

`arazzo --json inspect openapi --spec <path>`:

```json
{
  "kind": "openapiCatalog",
  "schemaVersion": "openapi.catalog.v1alpha1",
  "catalog": { "sources": [], "operations": [], "by_operation_id": {} },
  "diagnostics": []
}
```

`arazzo --json generate ...` (with or without `--output`):

```json
{
  "kind": "generate",
  "schemaVersion": "generate.v1",
  "yaml": "arazzo: 1.0.0\n...",
  "file": null,
  "workflows": 1,
  "steps": 3,
  "diagnostics": []
}
```

Both surfaces participate in `docs/schemas/` drift checks once committed.

## Phases

Six phases. Each ships a visible improvement. Sequential except where noted.

### Phase 1 — Catalog foundation (~3-4 weeks)

**Goal:** Land `arazzo-openapi` with the catalog types, port existing ref
resolution, add path-item refs, fix cycle detection asymmetry, emit
duplicate-operationId and 3.1-keyword diagnostics.

**Files (new unless noted):**

```
crates/arazzo-openapi/Cargo.toml
crates/arazzo-openapi/src/{lib,catalog,source,refs,version,diagnostics}.rs
crates/arazzo-openapi/src/{servers,security,parameters,bodies,responses}.rs
crates/arazzo-openapi/tests/catalog.rs
Cargo.toml                                 (existing — add member)
testdata/openapi/                          (new directory)
testdata/openapi/duplicate-operation-ids.openapi.yaml
testdata/openapi/openapi-31-raw-schema.openapi.yaml
testdata/openapi/external-local-refs.openapi.yaml
```

**Tasks:**

1. Create the crate using existing workspace deps (`openapiv3`,
   `serde_yaml_ng`, `serde_json`, `url`, `percent-encoding`).
2. Implement `OpenApiCatalog` construction from a `Vec<OpenApiSource>`.
3. Port internal `$ref` resolution from `arazzo-generate/src/refs.rs`. Apply
   uniform cycle detection across schemas, request bodies, responses,
   parameters, headers, examples, security schemes — fixes the existing
   asymmetric-cycle-detection bug.
4. Add path-item `$ref` resolution.
5. Add local-external `$ref` resolution sandboxed to the root file's
   directory. Reject absolute paths and `..` traversal.
6. Detect `openapi: 3.1.x` and emit `OPENAPI_VERSION_31_UNSUPPORTED_KEYWORDS`
   listing keywords found that the typed model cannot represent. One bucketed
   diagnostic per spec is enough at this stage.
7. Build `by_operation_id`. Emit `OPENAPI_OPERATION_ID_DUPLICATE` (warning)
   for repeats.
8. Reject `SourceUri::RemoteUrl` with `OPENAPI_REMOTE_REF_DISABLED`.

**Acceptance:**

- Path-item refs and component refs (schemas, parameters, request bodies,
  responses, headers, examples, security schemes) resolve with cycle
  protection.
- Duplicate `operationId` produces a stable diagnostic; lookups via
  `by_operation_id` return all candidates.
- 3.1 fixture produces `OPENAPI_VERSION_31_UNSUPPORTED_KEYWORDS` and the
  catalog still constructs (no panic, no abort).
- External refs cannot escape the root sandbox.
- `cargo test -p arazzo-openapi` passes.
- Workspace fmt/clippy/test green.

### Phase 2 — Catalog inspection + JSON fix (~1-2 weeks)

**Goal:** Expose the catalog through CLI and MCP. Add MCP caching. Fix the
`generate --json` bug.

**Files (existing unless noted):**

```
crates/arazzo-cli/src/{cli,handlers,output}.rs
crates/arazzo-mcp/src/{handlers,tools,state}.rs
docs/schemas/inspect-openapi.schema.json   (new)
docs/schemas/generate.schema.json          (new)
README.md
```

**Tasks:**

1. Add `arazzo --json inspect openapi --spec <path>` returning the catalog
   envelope. Add `arazzo schema inspect-openapi` for drift coverage.
2. Replace the existing MCP `describe_openapi` shape with the catalog
   envelope. CHANGELOG documents the cutover.
3. Cache parsed `OpenApiCatalog` in `ServerState` keyed by absolute path +
   SHA-256 of file bytes + fingerprints of resolved local external refs +
   resolver options. LRU 16, env-configurable. Cache invalidates when any
   keyed input changes.
4. Route MCP file inputs through `check_path_allowed` before parsing.
5. Fix `generate --json`: always emit the JSON envelope, with `yaml` field
   present when `--output` is omitted. Add the `generate.schema.json` drift
   test.

**Acceptance:**

- CLI and MCP produce byte-identical catalog JSON for the same input.
- `arazzo --json generate ...` never prints raw YAML to stdout.
- Repeated MCP `describe_openapi` calls on an unchanged spec parse the file
  at most once.
- Modifying the spec contents (even with mtime preserved) triggers a
  re-parse on the next call.
- Modifying a local external ref triggers a re-parse of the root catalog.
- Schema drift tests pass.

### Phase 3 — Generator port (~1-2 weeks)

**Goal:** Move CRUD generation onto the catalog. Add per-operation and
per-tag generation.

**Files:**

```
crates/arazzo-generate/src/crud.rs          (existing, ~1000 lines)
crates/arazzo-generate/src/lib.rs           (existing)
crates/arazzo-generate/src/scenarios.rs     (new — per-op / per-tag)
crates/arazzo-cli/src/{cli,handlers}.rs     (existing — add flags)
crates/arazzo-mcp/src/{tools,handlers}.rs   (existing — add args)
```

**Tasks:**

1. Port `generate_crud` to consume `OpenApiCatalog` instead of parsing OpenAPI
   directly. Keep golden-output stability; behavioral parity is the bar.
2. Add `--operation <id>` and `--tag <name>` scenarios. Per-operation emits
   one workflow targeting the named operation; per-tag emits one workflow
   per matching operation.
3. Add `--scenario catalog` that emits one workflow per operation in the
   catalog (useful for surveying a spec).
4. Surface catalog diagnostics in `GenerateOutput.diagnostics`.
5. MCP `generate_workflow` accepts the new `--operation` / `--tag` /
   `--scenario` parameters.

**Acceptance:**

- Existing CRUD golden tests pass unchanged.
- New per-operation tests against `petstore.openapi.yaml` produce expected
  workflows.
- Catalog diagnostics flow through to CLI human and JSON output.
- MCP integration test covers the new scenarios.

### Phase 4 — Contract validation (~1 week)

**Goal:** Validate generated workflows against the catalog. Fail closed on
real correctness gaps.

**Files (new unless noted):**

```
crates/arazzo-generate/src/validation.rs
crates/arazzo-cli/src/{handlers,output}.rs  (existing)
crates/arazzo-cli/tests/cli_integration.rs  (existing)
```

**Tasks:**

1. Implement `validate_generated_workflow_against_catalog(workflow, catalog)`.
2. Validate each generated step:
   - target operation exists
   - target operation is unique (uses `by_operation_id`)
   - all required `path` / `query` / `header` / `cookie` parameters are
     supplied or generated
   - request body content type is in the operation's declared media types
   - success criteria reference declared response statuses
   - auth inputs match the selected security requirement
3. Emit `GENERATE_CONTRACT_*` diagnostics for each rule.
4. CLI `--strict` flag fails on any validation diagnostic at error severity;
   default keeps them as warnings unless they affect emitted-step
   correctness.

**Acceptance:**

- Generated workflows targeting nonexistent operations fail by default.
- Missing required parameters fail by default.
- `petstore.openapi.yaml` regenerates without validation errors.
- A fixture with an intentional missing-required-param produces
  `GENERATE_CONTRACT_MISSING_REQUIRED_PARAM`.

### Phase 5 — Better request preparation (~2-3 weeks)

**Goal:** Catalog construction populates real request facts: parameter
serialization, body media types, per-operation security/server overrides.
No runtime changes yet; this phase feeds the catalog only.

**Files (existing unless noted):**

```
crates/arazzo-openapi/src/{parameters,bodies,security,servers}.rs
crates/arazzo-generate/src/{crud,examples,scenarios}.rs
crates/arazzo-generate/src/validation.rs       (extend Phase 4)
testdata/openapi/deep-object-query.openapi.yaml   (new)
testdata/openapi/multipart-upload.openapi.yaml    (new)
```

**Tasks:**

1. **Parameters:** populate `ParameterContract.style`, `explode`,
   `allow_reserved`. Implement serializers for path simple/label/matrix,
   query form/spaceDelimited/pipeDelimited/deepObject, header simple,
   cookie form. Generators use them; runtime uses them in Phase 6.
2. **Request bodies:** populate `RequestBodyContract` for `application/json`,
   `application/x-www-form-urlencoded`, `multipart/form-data`, XML media
   types, `text/*`. Preserve existing raw XML/text runtime behavior.
   Generators choose the best supported media type per operation.
3. **Servers:** resolve per OpenAPI precedence (operation → path-item → root).
   Preserve server variables with diagnostics for substituted defaults.
4. **Security:** resolve per OpenAPI precedence (operation → root). Empty
   security means no auth. Represent OR across requirement objects, AND
   across schemes inside one requirement. Generators emit auth inputs;
   ambiguous compound requirements fail closed in `--strict`.
5. Phase 4 validation extends to media-type compatibility, parameter
   shape against schema, security-input matching.

**Acceptance:**

- A `deepObject` query parameter generates and serializes correctly.
- A `multipart/form-data` operation generates a runnable workflow that emits
  multipart bytes (verified via dry-run JSON output).
- A spec with operation-level security overriding global security generates
  the operation-level auth, not the global default.
- Generation against a spec with multiple servers picks the operation-level
  server when present.

### Phase 6 — Runtime contract mode (~1 week)

**Goal:** Make `--openapi` runtime resolution use the full catalog when
explicitly enabled. Default behavior unchanged.

**Files (existing):**

```
crates/arazzo-runtime/src/runtime_core/{builder,state,engine_http,error}.rs
crates/arazzo-cli/src/{run_context,handlers}.rs
crates/arazzo-runtime/tests/engine_execution.rs
```

**Tasks:**

1. Replace `OperationEntry { method, path }` with a contract-backed entry
   referencing `OperationContract` from the catalog.
2. Keep `BTreeMap<String, OperationEntry>` keyed by `operationId` for
   default-mode compatibility, but populate it with disambiguation: the
   first operationId match keeps the legacy behavior, duplicates emit
   `RUNTIME_OPERATION_ID_DUPLICATE_DEFAULT_MODE` warnings.
3. Add `--openapi-mode contract` to CLI `run` / `replay` / `test`. In
   contract mode:
   - lookup by `(operationId)`; if `by_operation_id` returns multiple keys,
     fail with `RUNTIME_OPERATION_ID_AMBIGUOUS`.
   - apply effective server selection (from Phase 5).
   - apply parameter serialization (from Phase 5).
   - validate required parameters before HTTP.
   - choose request content type from the operation's declared media types
     (or validate the workflow's chosen type).
   - apply selected security binding when generated/declared by Arazzo.
4. `sourceDescription` routing: `{stripe}.createCheckoutSession` resolves
   the source first, then performs the operationId lookup within that
   source's operations only. Same `RUNTIME_OPERATION_ID_AMBIGUOUS` if the
   source-scoped lookup is itself duplicate.
5. Preserve dry-run, trace, replay behavior.

**Acceptance:**

- Existing `operationId` runtime tests pass in default mode.
- Contract mode correctly serializes a `deepObject` query and a
  `multipart/form-data` body.
- Contract mode fails before HTTP on missing required parameters.
- Contract mode fails before HTTP on duplicate operationId.
- Dry-run JSON output shows the prepared request as it would be sent.

## Verification

Every phase ticket runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

High-risk phases also run targeted gates:

```bash
cargo test -p arazzo-openapi
cargo test -p arazzo-generate
cargo test -p arazzo-runtime
cargo test -p arazzo-cli --test cli_integration
cargo test -p arazzo-mcp
```

Smoke checks (dry-run only, no network):

```bash
cargo run -p arazzo-cli -- --json inspect openapi --spec testdata/petstore.openapi.yaml
cargo run -p arazzo-cli -- --json generate --spec testdata/petstore.openapi.yaml --tag pets
cargo run -p arazzo-cli -- --json run <generated.arazzo.yaml> <workflow> \
  --openapi testdata/petstore.openapi.yaml --openapi-mode contract --dry-run
```

## Phase Dependencies

```
1 ──> 2
1 ──> 3 ──> 4 ──> 5 ──> 6
```

Phases 2 and 3 can land in either order after 1. Phase 5 depends on 4 because
the validator extends with each new contract surface. Phase 6 is the final
runtime integration and depends on 5.

## Open Questions

These are the questions that genuinely remain after inline decisions:

1. **`generate --json --output file.yaml` shape**: include the YAML string
   in the `yaml` field, or only `file` + summary? Recommend including
   `yaml` for streaming consumers; revisit if it hurts performance on
   large outputs.
2. **`OperationKey.pointer` field**: should the catalog include a JSON
   Pointer to the operation in the source document for deep-link
   diagnostics? Cheap to add; defer until a diagnostic actually needs it.
3. **MCP cache eviction signal**: do we need an explicit
   `clear_catalog_cache` MCP tool, or is restart-the-server good enough?
   Defer until users report needing it.

Resolved by inline decisions: source-id derivation, Arazzo binding,
`openapiv3` upgrade, MCP cache shape, strict default, JSON path privacy,
duplicate-operationId severity, generate JSON envelope, 2.x strategy.

## Why This Plan Is Smaller Than Its Predecessors

The plan that produced this one (recoverable at git `719ceaf`) tried to be
provider-grade in two senses simultaneously: a quality bar (no silent
failures, fail-closed correctness) and a feature scope (remote refs,
declarative recipes, comprehensive 3.1, MCP legacy compat). Holding both
made every phase carry defensive scope that the project's audience does
not need today.

This plan keeps the quality bar — every acceptance criterion still fails
closed on real correctness gaps — and cuts the speculative feature scope.
What was Phase 9 (remote refs) and Phase 11 (declarative recipes) became
"out of scope with revisit conditions" because the conditions are testable
and the work is real engineering, not a deferral.

The result is six phases of focused work, each one shipping a thing the
user can demo, totaling roughly 10–13 weeks of implementation.
