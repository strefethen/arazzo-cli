# OpenAPI 3.1/3.2 Support & sourceDescriptions Auto-Loading — Options Assessment

**Date:** 2026-08-03
**Trigger:** [Issue #3](https://github.com/strefethen/arazzo-cli/issues/3) — `generate` fails on an OpenAPI 3.2 spec; `run` reports `operationId not found` unless `--openapi` is passed explicitly.
**Status:** Research complete. No code changes made. Decision needed on two independent tracks.

---

## 1. Problem framing

Issue #3 contains two separate problems:

1. **OpenAPI 3.1/3.2 ingestion.** `arazzo generate` deserializes with the
   `openapiv3` crate (2.2.0), which models OpenAPI 3.0.x only. 3.1/3.2
   documents use JSON Schema 2020-12 (`"type": ["null", "string"]`,
   numeric `exclusiveMinimum`, `examples` arrays), which fail
   deserialization outright. 0.3.0 turned this into a clear error; actual
   support is the open question.
2. **sourceDescriptions are not honored at run time.** The engine only
   indexes operationIds from specs passed via `--openapi`. It never loads
   the documents that `sourceDescriptions[].url` points at.

Product lens: this serves external adopters pointing arazzo-cli at
modern third-party specs (FastAPI, .NET 10, and most 2025+ generators
emit 3.1; 3.2 is growing since its 2025-09 release). Blast radius:
`generate`'s ingestion pipeline and the runtime's URL-building semantics
(every previously generated Arazzo file must keep running). Team: solo
dev + agents.

## 2. Codebase findings

### 2.1 The 3.2 blocker is confined to `generate`

- `openapiv3` is a dependency of exactly two crates:
  `arazzo-generate` and `arazzo-cli` (thin wrapper). All ingestion goes
  through one choke point: `arazzo_generate::parse_openapi_file`
  (`crates/arazzo-generate/src/lib.rs:10`), which is where the 0.3.0
  version guard lives.
- Typed-model surface, by file:
  - `crud.rs` (1,039 ln) — version check, `servers[0]` extraction with
    variable substitution, CRUD grouping over paths/operations,
    parameters, request bodies, responses, `detect_auth`
    (apiKey header/query/cookie, HTTP bearer/basic).
  - `examples.rs` (578 ln) — schema→example synthesis; the heaviest
    typed-schema consumer (`SchemaKind`, `Type`, `StringType`,
    `IntegerType`, `NumberType`, `ArrayType`, format variants).
  - `openapi_describe.rs` (377 ln) — powers MCP `describe_openapi` only.
  - `refs.rs` (143 ln) — internal `#/components/*` ref resolution.
  - `standalone_example.rs` (302 ln) — **already walks raw JSON Schema as
    `serde_json::Value`**, bridging into typed helpers. In-repo proof of
    the untyped pattern — and it shares the same gap: a 2020-12 `type`
    array falls through to `Value::Null` silently (MCP `generate_example`
    is quietly 3.0-only too).
- Consumers of the typed pipeline: CLI `generate`; MCP
  `generate_workflow`, `describe_openapi`, `generate_example`.

### 2.2 The runtime is already version-agnostic

`parse_openapi_into_index`
(`crates/arazzo-runtime/src/runtime_core/builder.rs:180`) walks untyped
YAML for `paths → method → operationId`. It indexes the reporter's 3.2
spec fine today (verified against their real spec). Keeping the runtime
untyped is the right architecture: it needs only the operationId→
(method, path) mapping (soon: `servers`), and wiring `openapiv3` in
would import the 3.0-only restriction into `run`, which currently does
not have it.

Hardening gaps in the untyped walk (all cheap, none blocked on crates):

- OpenAPI 3.2's new `query` HTTP method is not in the method set — those
  operations silently don't index.
- 3.2's `additionalOperations` map is ignored.
- Path-item `$ref`s are skipped silently.

### 2.3 MCP and DAP cannot load specs at all

- MCP `run_workflow` (`crates/arazzo-mcp/src/handlers.rs:215`) builds the
  engine with **no** `openapi_spec()` call and exposes no parameter for
  it — operationId-based workflows cannot run over MCP, period.
- The debug adapter has zero OpenAPI references — DAP sessions have the
  same gap.
- Consequence: sourceDescriptions auto-loading (implemented at
  `EngineBuilder::build` level) is the only clean fix for all three
  surfaces at once; per-surface flags would fix only the CLI.

### 2.4 sourceDescriptions.url is conflated with the API base URL

Two different semantics are currently fused:

- **Arazzo spec intent:** `sourceDescriptions[].url` is a URI-reference
  to the source *document* (the OpenAPI file), resolvable relative to
  the Arazzo document. The request base URL comes from the loaded
  OpenAPI document's `servers`.
- **Our convention:** `generate` writes `servers[0]` (the API base URL)
  into `url` (`crud.rs:59-64`), and the runtime uses
  `sourceDescriptions[0].url` directly as the HTTP base
  (`engine_http.rs:448-463`). Examples (`examples/*.arazzo.yaml`) use
  bare origins (`https://httpbin.org`).

These gaps are **coupled**: auto-loading alone would break URL building
(a document-pointing url concatenated with an operation path is
garbage). The fix must load the document *and* derive that source's
request base from its `servers` (reporter's spec declares
`https://localhost:5201/v1`), with a compatibility fallback for
bare-origin urls so every existing generated file keeps working.

### 2.5 Prior legwork exists and its premise has been overtaken

`docs/future/openapi-provider-grade-ingestion.md` (last updated
2026-05-03) is the floated-down provider-grade ingestion plan:
shared `arazzo-openapi` catalog crate (`OpenApiCatalog`,
`OperationContract`, `OperationKey`), `openapiv3` pinned underneath,
six phases, ~10–13 weeks. It deferred "comprehensive OpenAPI 3.1
strategy" behind a revisit condition — *"a 3.1 spec we actually want to
support hits a specific keyword we cannot diagnose-and-skip."*

Two updates from this research:

1. **The revisit condition has fired, and harder than anticipated.**
   Issue #3 is a real spec we want to support, and the failure is not a
   keyword to diagnose-and-skip — deserialization itself fails. The
   plan's Phase 1 acceptance ("3.1 fixture produces
   `OPENAPI_VERSION_31_UNSUPPORTED_KEYWORDS` and the catalog still
   constructs") is unachievable with `openapiv3` as the deserializer:
   you cannot diagnose a document you cannot deserialize. Any future
   catalog work needs a Value-based ingestion front-end regardless.
2. The plan predates the MCP/DAP run gap analysis and does not include
   sourceDescriptions auto-loading (its Phase 6 assumes `--openapi`
   flags).

## 3. Ecosystem findings (verified 2026-08-03)

### 3.1 Crate landscape

| Crate | Version / activity | 3.0 | 3.1 | 3.2 | Notes |
|---|---|---|---|---|---|
| `openapiv3` (current dep) | 2.2.0, 2025-06-02; **no commits in 14 months** | ✅ | ❌ | ❌ | 3.1 issue ([#50](https://github.com/glademiller/openapiv3/issues/50)) open since **Nov 2021**, no code ever landed, owner silent; 3.2 not on the radar |
| `oas3` | 0.22.0, 2026-05-06; active (robjtede/x52dev, pushed 2026-08) | ❌ (explicitly out of scope) | ✅ typed 2020-12 (type arrays, prefixItems, const, numeric exclusive bounds) | Promised ([#300](https://github.com/x52dev/oas3-rs/issues/300), maintainer commitment 2026-03), **not shipped** | Deserialization doesn't gate on version (`~3.1` check is opt-in), so 3.2 docs parse with new fields silently dropped. Silently drops unmodeled keywords (`unevaluatedProperties`, `$dynamicRef`, `jsonSchemaDialect`). **Open stack-overflow bug on recursive schemas since 2023** (oas3-rs #3) |
| `openapiv3-extended` | 6.0.1, 2025-08 | ✅ (+2.0 upgrade) | ❌ ("not currently supported nor being actively developed") | ❌ | |
| `openapiv3_1` | 0.1.5, 2026-03; young | ❌ | ✅ (utoipa-derived model + full 2020-12) | ❌ | 0.1.x maturity; small user base |
| `utoipa` | 5.5.0 | ❌ | authoring-only | ❌ | Not designed to ingest arbitrary specs |
| Down-convert tooling | `@apiture/openapi-down-convert` (npm) | — | — | — | Explicitly lossy, self-described "not fully robust", **no Rust equivalent exists** |

**Bottom line: no Rust crate ships typed 3.2 parsing today.** Waiting on
`openapiv3` upstream is betting on a repo that has been dormant 14
months with a 4.5-year-old open 3.1 issue. Migrating to `oas3` would
still require keeping `openapiv3` for 3.0 (oas3 explicitly does not
parse 3.0) — i.e., two typed models plus an abstraction layer over
both.

### 3.2 OpenAPI 3.2 parsing deltas (released 2025-09-19)

Additive over 3.1 for parsing purposes: root `$self`; path-item `query`
method + `additionalOperations`; parameter `in: querystring`; tag
`summary`/`parent`/`kind`; media-type `itemSchema`/`itemEncoding`
(streaming); components `mediaTypes`; OAuth `deviceAuthorization`;
discriminator `defaultMapping`. Three tripwires for strict parsers:
hard version-string gates, `Response.description` no longer required,
and unknown-field strictness. **A lenient 3.1-capable parser
structurally parses 3.2 documents.** (A widely-circulated claim that 3.2
removed `discriminator.mapping` is false per the spec text.)

### 3.3 Prior art

- **Go `libopenapi` (pb33f):** one unified model across
  Swagger 2/3.0/3.1/3.2 (+ Arazzo) with per-field union types and a
  lossless low-level node layer beneath the typed layer. Shipped 3.2
  within days of GA.
- **Go `kin-openapi`:** morphing its single typed model incrementally —
  official 3.1 support still "soon" after six years. Cautionary tale.
- **Rust `schemars` 1.x** (the flagship JSON Schema crate) deliberately
  abandoned its fully-typed schema model; `Schema` is now a thin wrapper
  around `serde_json::Value`. The Rust JSON-Schema ecosystem
  (`jsonschema` validator) is Value-based.
- **`openapi-to-rust`** (2026): ingests 3.0/3.1/experimental-3.2 with a
  hand-rolled serde skeleton, schemas as `Value`, no OpenAPI model crate
  — direct precedent for the approach recommended below.

## 4. Options — OpenAPI 3.1/3.2 ingestion

### Option A — Wait for / contribute to `openapiv3` upstream
Dormant repo, silent owner, incompatible schema model (bool vs numeric
exclusive bounds means a breaking redesign, not a patch). **Rejected.**

### Option B — Add `oas3` as a second parser (3.0 via openapiv3, 3.1/3.2 via oas3)
Active maintainer and typed 2020-12 model are real positives. But: oas3
cannot parse 3.0, so both crates stay forever; every consumer
(crud/examples/describe) then needs an abstraction over two typed
models — that abstraction **is** an internal representation, so Option
B degenerates into Option C plus two heavyweight dependencies; 3.2 is
promised, not shipped; the 2023 recursive-schema stack overflow is a
real risk for arbitrary user specs; silent dropping of unmodeled
keywords conflicts with the diagnose-and-report direction of the
ingestion plan. **Viable fallback, not the primary path.**

### Option C — Version-agnostic ingestion front-end (recommended)
Hand-rolled lenient serde skeleton for the OpenAPI *shape* we consume
(info, servers, paths/operations, parameters, request bodies,
responses, security schemes) with **schemas kept as
`serde_json::Value`** plus a small normalization view for example
synthesis (type-or-types, nullable, format, enum, const,
default/example(s), properties/required, items/prefixItems,
one/any/allOf, min/max — normalizing 3.0 `nullable` and 2020-12 type
arrays into one representation).

Why this is the proper fix rather than a workaround:

- It is the design the 2026 ecosystem converged on (schemars 1.x,
  libopenapi's lossless layer, openapi-to-rust), and the pattern the
  repo already uses twice (runtime indexer, `standalone_example.rs`).
- One pipeline for 3.0/3.1/3.2 — version differences become explicit
  normalization rules with diagnostics, exactly what the deferred
  ingestion plan's catalog needs as its front-end. The work is not
  throwaway; it is Phase-1-of-the-plan brought forward in minimal form.
- Kills the entire class of "our parser crate doesn't support version
  X" for the skeleton; 3.2's additions become fields we either read
  (`additionalOperations`, `query`) or deliberately ignore.
- Drops the `openapiv3` dependency once cut over.

Cost and risk: a real but bounded refactor of `crud.rs`/`examples.rs`/
`openapi_describe.rs`/`refs.rs` (~2.5k lines touched, much of it
mechanical). Migration safety comes from golden-output tests: run the
new front-end against all existing 3.0 fixtures and assert
byte-identical generated YAML before cutting over; add 3.1/3.2 fixtures
(a minimal hermetic one plus one shaped like the reporter's bank-api
spec). `refs.rs` gets simpler (JSON-pointer resolution over `Value`
with uniform cycle detection — also fixing the plan-documented
asymmetric-cycle-detection bug).

Placement: start as an `ingest` module inside `arazzo-generate` (MCP
already consumes that crate, so all surfaces benefit); extract into the
plan's `arazzo-openapi` crate when/if the full catalog work activates.
Creating the crate now is defensible but drags in catalog-API design
decisions this fix doesn't need.

### Option D — Down-convert 3.1/3.2 → 3.0, keep openapiv3
Recognized approach but explicitly lossy; no Rust implementation exists
(we would write ≈ the same normalization logic as Option C, in the
wrong direction, while keeping a dormant dependency). **Rejected.**

## 5. Options — sourceDescriptions

Design for the proper fix (spec-aligned, backward compatible):

1. **Load at engine build.** For each `type: openapi` source, resolve
   `url` relative to the Arazzo document's location (the engine needs
   the doc's path/base — new builder input). Local file → read and
   index. Remote `http(s)` → policy decision (below). `type: arazzo` /
   `asyncapi` sources: out of scope, same loader infra later.
2. **Per-source operation index and per-source request base.**
   `OperationEntry` gains its source; the request base for a source
   comes from its loaded document's `servers[0]` (with variable
   substitution, as `crud.rs` already does). operationId lookup scoped
   per source; cross-source duplicates get the plan's
   `RUNTIME_OPERATION_ID_AMBIGUOUS` treatment (warning in default mode,
   listing candidates).
3. **Compatibility rule — classify by URI shape, not by probing**
   (revised 2026-08-03; the earlier "if it loads as a document →
   document semantics, else legacy" draft made a field's *meaning*
   depend on filesystem/network state — same file, different
   environment, silently different semantics, and a parse error would
   demote a document source to base-URL mode and emit garbage requests
   instead of failing. Semantics must be decidable from the document
   text alone):
   - **Relative URI-reference (no scheme) → document semantics,
     always.** Resolve against the Arazzo document's directory, load,
     index, derive base from `servers`. Missing or unparseable → loud
     build error, never a fallback. Zero compat risk: relative urls
     cannot work as request bases today — `Url::parse` rejects
     relative targets (`crates/arazzo-runtime/src/lib.rs:488`), so no
     working file depends on them.
   - **Absolute `http(s)` url → legacy base-URL semantics** (exactly
     today's behavior; covers every generated file and example). A
     future opt-in remote-fetch flag may upgrade these to document
     semantics; that decision is severed from this fix.
   - Explicit `--openapi` files stay supported and win operationId
     conflicts (load order: sources first, then `--openapi`), with a
     warning naming both origins on duplicates.
4. **`generate` emits document-pointing urls** once runtime support
   lands (the `--spec` path, relativized). Effective behavior of
   generated files is unchanged — the runtime derives the same
   `servers[0]` base that generate bakes in today — but files become
   spec-conformant and self-contained. Coordinated release + CHANGELOG.
5. **MCP and DAP inherit the fix for free** (it lives in
   `EngineBuilder::build`). MCP file loads must route through its
   existing `check_path_allowed` gate.
6. Optional: `arazzo validate` warns when a `type: openapi` source url
   is not loadable (P3).

Remote source fetching — settled policy (2026-08-03, after full
deliberation; supersedes both earlier drafts of this note):

Ownership model: the user owns trust decisions, and the tool must
never make a use case unreachable. The mechanism of ownership is
secure-by-default with explicit per-invocation overrides (the curl
`-k` shape) — a default is not a prohibition, and every row below is
reachable.

Two flags with different jobs:

- `--fetch-remote-sources` — a **semantic disambiguator**, not a
  security gate: the installed base uses absolute urls as "request
  base" (legacy, preserved by ac-40602) while spec-conformant fetching
  reads them as "document to load". Endgame once `generate` emits
  document-pointing urls and legacy files age out: fetch on by
  default, `--no-fetch` for offline/CI determinism. The scheme table
  below persists even then.
- Destination policy when fetching (decidable from URL text alone, no
  DNS resolution in the decision):

  | Fetch target | Policy |
  |---|---|
  | `https://` any host | allowed |
  | `http://` loopback (`localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`) | allowed — W3C Secure Contexts / RFC 8252 precedent |
  | `http://` any other host | allowed with explicit `--insecure-http-sources` (curl `-k` pattern) |

Why the fetch layer is held to a stricter default than step execution
(deliberate, not inconsistency): step URLs are first-party content of
the document the user reviewed and chose to run; the fetched spec is
second-order content nobody reviews, and its `servers[]` silently
steers credentialed traffic between identical invocations of the same
reviewed file. The governing precedent is package managers fetching
dependencies — cargo/npm/pip require TLS (or explicit per-registry
opt-out) for the channel that determines subsequent privileged
behavior while restricting nothing about what installed code does at
runtime. One flag in a make target is the intended cost: the user's
signature on the trust decision.

Engineering hygiene (unchanged): fetch failures fail closed (error,
never a silent fallback to base-URL semantics); `--dry-run` stays
network-free so its output remains a trustworthy statement of what
would be sent (vendoring/`--openapi` as the offline remediation);
resolved base URLs visible in dry-run/trace; fetches reuse the
workflow `ClientConfig` (timeouts) plus a size cap; MCP exposes
fetching via server-side config rather than inheriting CLI flags —
there the config is how the absent owner exercises the trust decision
(the operator is an agent).

### Execution-layer transport trust (extension, 2026-08-03)

The step-execution layer currently has *implicit* transport policy —
raw reqwest defaults, chosen by nobody: `HttpClient::new` builds
`reqwest::Client::builder().timeout(..).build()` with no other
configuration (`crates/arazzo-runtime/src/runtime_core/client.rs:117-119`).
Concretely today: TLS verification on with **no override** (self-signed
`https://localhost` is a hard fail — the issue #3 reporter's spec
declares exactly that server); redirects **silently followed** (reqwest
default, limit 10) including https→http downgrades, where curl's
default is refuse-without-`-L`; no visibility of redirect chains in
trace; no signal when credentials travel over cleartext.

Proposed controls — same philosophy as the fetch table: defaults are
safe, every behavior reachable via explicit override, no prohibitions:

| Concern | Today (inherited) | Proposed default | Override |
|---|---|---|---|
| TLS verification | on, no escape hatch | on | `--insecure-host <host[:port]>` (repeatable, scoped); optionally blanket `--insecure` for curl parity |
| Redirects | follow ≤10, silent | follow, but **refuse https→http downgrade**; record chain in trace | `--allow-downgrade-redirects` (curl `--location-trusted` analog), `--max-redirects` |
| Credentials over cleartext `http://` (non-loopback) | silent | one stderr warning (not a block) | quiet flag |

Rationale for scoped-not-blanket insecure: curl is one URL per
invocation, so blanket `-k` is naturally scoped; a workflow run touches
many hosts in one invocation, and blanket `--insecure` would weaken the
production API call because the user wanted to accept a self-signed
localhost cert. Per-host is the workflow-engine analog of curl's
scoping. Implementation note: reqwest's `danger_accept_invalid_certs`
is client-global, so per-host needs either a second client pool keyed
by host or a custom rustls verifier — ticket-level detail, both
workable. Verification item for the ticket: pin down reqwest 0.12's
exact sensitive-header-stripping predicate on redirects (cross-host is
stripped; whether same-host scheme-downgrade strips is to be
verified).

Warnings model (owner-approved addition): using `--insecure-host` /
`--insecure` itself emits a one-line stderr notice at startup of live
runs (replay exempt — no live client is built), and an end-of-run note
names exceptions no request used (config-rot detection for stale flags
in Makefiles/CI). Squelch via `--no-transport-warnings` or
`ARAZZO_NO_TRANSPORT_WARNINGS=1` silences stderr text only — under
`--json`, structured warning entries persist regardless, because the
degraded-trust fact is audit data. The squelch in a recorded
invocation is a second signature on the trust decision, not a mute
button. Rationale for warning at all where curl `-k` stays silent:
arazzo invocations are built for repetition (CI/Make/test), the
config-rot habitat; curl's are ad-hoc.

This subsumes the earlier "related but separate
`danger_accept_invalid_certs`" note into one coherent
transport-trust-controls work item (ticketed: ac-fd376), independent
of both ac-40602 and the fetch ticket.

## 6. Recommendation & sequencing

Two independent slices, both proper fixes, neither activating the full
10–13-week plan:

- **Slice 1 — sourceDescriptions auto-load (runtime; small).**
  Steps 1–3 + 5 above, plus untyped-indexer hardening (`query` method,
  `additionalOperations`, path-item `$ref`). Unblocks the reporter's
  `run` scenario for **all** OpenAPI versions including 3.2 (their spec
  already indexes untyped), and makes operationId workflows work over
  MCP and DAP for the first time. No crate decisions involved.
- **Slice 2 — Option C ingestion front-end (generate; medium).**
  Unblocks `generate` (and MCP describe/generate tools) on 3.1/3.2,
  with golden-stability on 3.0. Removes the `openapiv3` dependency at
  cutover. Follow with the `generate` url-emission change (step 4).

Watch items: `oas3` #300 (if oas3 ships credible 3.2 + ever adds 3.0,
re-evaluate Option B's economics — unlikely per its README scoping) and
`openapiv3` #50 (no movement expected).

## 7. Verification notes for whoever implements

- Golden tests: new front-end vs existing fixtures must produce
  byte-identical Arazzo YAML before deleting the openapiv3 path.
- Real-world 3.2 shape to cover: `"type": ["null", "string"]`, numeric
  `exclusiveMinimum`, `examples` arrays, absent
  `Response.description`, `$self`, `additionalOperations`.
- Runtime compat matrix: (a) legacy base-URL sourceDescriptions,
  (b) document-pointing local relative url, (c) `--openapi` override
  alongside both, (d) multi-source specs with duplicate operationIds.
- Hermetic fixtures only (no network in tests), per repo convention.
