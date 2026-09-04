---
status: draft
epic:
created: 2026-08-31
---

# Lossless JSON Schema 2020-12 workflow inputs

**Seed:** Steve requested “work ticket
[ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90)”. Its goal is:
“Produce an accepted design for lossless JSON Schema 2020-12 modeling and
workflow-input validation before implementation tickets are authored.”

## Decision and open acceptance gates

Use an Arazzo-owned, opaque `InputSchema` backed by an Arazzo-owned exact JSON
tree and number lexeme. JSON uses maintained `serde_json::RawValue` token
boundaries; YAML uses `yaml-rust2` events and checked exact-number emission.
The compiler/validator stays behind an Arazzo-owned interface. Do not expose a
third-party schema model or continue adding assertions to the current
validator.

Steve approved a Rust 1.85 design target and disposable evaluation of
`jsonschema` 0.52.1 and `yaml-rust2` 0.11.1 on 2026-08-31. This does not change
the repository MSRV, add `jsonschema`, upgrade the debug adapter's existing
`yaml-rust2` 0.11.0 dependency, adopt either candidate on the schema-input path,
accept this plan, or upgrade conformance. The codec qualification passed and replaced the unsafe
generic `serde_json::Value` storage proposal. The backend passed exact numeric,
2020-12, standalone offline-reference/anchor, recursion, malformed-schema and
regex-limit probes; its resolved lock audit was clean and build cost is measured.
The exact Arazzo resource resolved absolute embedded pointers, while anchors
below container keys required a private compilation projection. The selected
layered projection passed six pointer/anchor/privacy probes but still needs
diagnostic-provenance, duplicate-identity and full resource/dynamic-scope proof.

The custom-meter total-work experiment failed as a hard preemption mechanism. A
custom `Json` representation successfully meters instance traversal,
exact-number access, equality, uniqueness, property names and materialization,
but it cannot count or interrupt schema-owned work. A 200,000-branch boolean
`allOf` compiled synchronously in 1.040 seconds and validated with zero
instance-budget charges; eager error collection still built all 4,096 missing-
property errors after a 64-unit budget exhausted. This remains useful evidence
about the backend's limits, but hard cancellation is not a product requirement.

On 2026-09-02 Steve selected synchronous in-process validation: compile once per
immutable document snapshot, enforce generous byte/node/depth/scalar/resource/
alias and regex admission limits, call the first-error `validate` API, and never
use eager `iter_errors`. The product accepts that admitted backend work cannot
be preempted. The custom instance meter is optional defense in depth, not a
required cancellation or total-work guarantee. A worker process, native
cancellation hook, or alternate backend is not an acceptance gate for this
design.

The follow-up first-error characterization passed six focused debug and release
tests and 31 tests across the full disposable harness. Under illustrative
16-KiB/1,000-node/depth-32/scalar caps, `validate` returned one of 900 missing
required-property errors without eager allocation and completed admitted
applicator, uniqueness, evaluated-property and recursive-ref cases; constrained
instance cases reported exhaustion. A 900-branch boolean fanout remained
unmetered but completed inside the admitted envelope. This supports the selected
execution design without making a cancellation guarantee. The tested limits are
illustrative; production values and latency/RSS evidence belong to implementation
calibration against representative large documents and maximum-admitted
adversarial inputs.

The high-level transition order is settled: prepare collision-safe dynamic JSON
and numeric consumers with precision off; land the dormant compiler; replace
the old validator while the wire domain is still bounded; activate arbitrary
precision once; then cut the public schema wire model over in the same release
embargo. Each intermediate commit compiles, and no old/new validator fallback
exists. Final feature-reachability review found that the readiness inventory was
incomplete, however. Existing OpenAPI ingestion passes a third-party model with
embedded dynamic JSON values through the arbitrary-precision private-number
protocol, and the MCP request migration named `params` without also naming the
dynamic JSON-RPC `id`. Both require feature-enabled proof before activation;
choosing the OpenAPI parser/model boundary is a material cross-initiative
decision and remains an acceptance gate.

This work changes only this proposal, its evidence, and conformance metadata.
The [evidence assessment](../assessments/arazzo-json-schema-2020-12-inputs-evidence.md)
owns the consumer inventory, characterization results, package evidence links,
qualification, and proposed follow-on scopes. No follow-on tickets have been
authored.

## Outcome and boundaries

An author can roundtrip workflow and reusable input schemas through JSON or
YAML without losing fields, nulls, booleans, arrays, or numeric value. At
execution, one compiled schema validates the complete input object before
any workflow request, including dry-run, execute-step, and nested workflows.
An invalid schema and invalid input are different failures.

“Lossless” means equality in the JSON data model, including exact numeric
value and presence; it does not promise comments, key order, whitespace,
quoting style, anchor names, or byte-identical output. Authored schemas remain
unchanged by reference resolution and default preparation. This is not a
promise of lossless roundtrip for every non-schema Arazzo object.

Non-goals: new OpenAPI generation behavior or OpenAPI 3.1 support, OpenAPI
parameter serialization, response validation, new Arazzo syntax, network
retrieval, arbitrary file loading through schema references, external Arazzo
workflow execution, and general `operationPath` fragment routing. Existing
OpenAPI 3.0 ingestion is in scope only to make workspace-wide precision
activation safe; its behavior otherwise remains unchanged. Existing
source-document compatibility behavior is unchanged.

## Governing text and current evidence

The vendored [Workflow Object](../../spec/arazzo/v1.1.0.html#workflow-object)
defines inputs as “A JSON Schema 2020-12 object representing the input
parameters used by this workflow.” The
[Components Object](../../spec/arazzo/v1.1.0.html#components-object) holds
“reusable JSON Schema objects to be referenced from workflow inputs.”
The [Reusable Object](../../spec/arazzo/v1.1.0.html#reusable-object) rule says
“Input Objects MUST use standard JSON Schema referencing via the `$ref`
keyword”. This is not `$components.inputs.*` expression evaluation.

[Parsing Documents](../../spec/arazzo/v1.1.0.html#parsing-documents) requires
each document to be “fully parsed in order to locate possible reference
targets before attempting to resolve references.”
[Section 5.6.2](../../spec/arazzo/v1.1.0.html#resolving-uri-fragments)
requires schema references to support “both JSON Pointer fragments and
plain-name fragments (defined by `$anchor`)”. Base precedence remains
[section 5.6.1](../../spec/arazzo/v1.1.0.html#establishing-the-base-uri).

Current source disproves two assumptions that a wrapper-only change would
make. `SchemaObject` captures unknown keywords but filters non-`x-` keys when
serializing; `arazzo-validate` also replaces root input `$ref` with a component
clone, discarding sibling constraints. Its parse APIs return that effective
model. JSON is currently parsed through the YAML parser too, whose numeric
values are bounded integers or `f64`. Merely enabling a JSON precision feature
cannot recover already rounded values.

The current enum fix is retained as regression evidence, not a second
validator. Required-null handling, type-null handling, numeric integer
semantics, and unconditional undeclared-property warnings all need an
intentional compatibility decision at the complete-validator cutover.

## Ownership and published boundaries

| Unit | Owns | Published boundary |
|---|---|---|
| New `arazzo-json-value` | Collision-safe exact JSON tree, number lexemes, mathematical numeric helpers, bounded and precise codecs | Opaque `JsonValue`/`JsonNumber`, `RawValue` JSON decoder, safe dynamic-value field adapters and optional metered validator view; no Arazzo or schema policy |
| `arazzo-spec` | Authored `InputSchema`, schema-root checks and exact document JSON/YAML decoding and encoding | Opaque boolean-or-object schema over `arazzo-json-value`; explicit `DocumentFormat` codec APIs and Arazzo-owned errors; no generic Serde promise or validation engine types |
| New `arazzo-document-context` | Pure containing-document base calculation extracted from existing routing | Optional absolute retrieval URI plus authored `$self` to established base; no I/O, source indexing, or workflow routing |
| New `arazzo-input-schema` | Dialect checking, schema resource registry, compilation, instance validation, diagnostic normalization and descriptive projection | Immutable `SchemaSet`, compiled workflow handle, schema/instance diagnostics and an explicitly descriptive input view |
| `arazzo-validate` | Structural Arazzo validation and mapping schema errors into validation diagnostics | Existing APIs plus contextual variants; authored input schemas survive all APIs |
| `arazzo-runtime` | Input preparation policy, strictness, execution ordering and diagnostic delivery | Prepared input map and shared input diagnostics; existing runtime error codes remain stable |
| `arazzo-generate` | Existing OpenAPI 3.0 ingestion and exact numeric consumers | Precision-safe parser/model boundary remains unresolved; no post-parse repair, reserved legal key, silent isolation or behavior expansion |
| CLI, MCP, DAP | Presentation and transport of shared results; collision-safe protocol fields including MCP JSON-RPC `id` and `params` | No adapter-specific validator or reference resolver; existing protocol contracts remain stable unless separately approved |

The dependency direction is `spec -> json-value`,
`input-schema -> spec + json-value + document-context`,
`validate -> input-schema`, and
`runtime -> input-schema + json-value + document-context`. Front ends consume
these published interfaces. Neither lower unit depends on runtime or a front
end. Each new unit must build and test without consumers. The pure context
extraction is the narrow integration prerequisite; it is not permission to
redesign the document-routing initiative.

### Wire model

`Workflow.inputs` becomes optional `InputSchema`; `Components.inputs` becomes
a map of the same type. `InputSchema` contains the Arazzo-owned `JsonValue`
tree: null, boolean, validated exact-number lexeme, string, array, or object
with unique string keys. It deliberately has no generic Serde contract.
Schema roots accept boolean and object forms and reject null, number, string
and array. Presence is tracked outside `Option` decoding so an authored null is
an invalid present schema, not an absent field. Nested type arrays, `null`
types, compositions, conditionals, unevaluated keywords, annotations and
unknown keywords survive without a keyword allowlist. Schema validity is a
separate compiler result.

JSON decoding first asks maintained `serde_json::RawValue` to validate and
retain each raw subtree. Dispatch uses the first JSON token: objects deserialize
through a duplicate-detecting `String -> Box<RawValue>` visitor, arrays through
raw elements, and only raw number tokens become `JsonNumber`. This preserves a
legal object whose key is `$serde_json::private::Number`; generic
arbitrary-precision `Value` decoding does not. JSON output and the validator
bridge construct object/array/number variants directly after token kind is
known. Generic Value/Serde conversion is outside the lossless guarantee.

YAML schema subtrees use `yaml-rust2` parser events; conversion from
`serde_yaml_ng::Value` is insufficient. The adapter applies the YAML JSON
ruleset, preserves scalar spelling until exact number conversion, rejects
duplicate/non-string keys, non-finite numbers, unsupported tags and multiple
documents, and bounds depth, nodes and alias expansion. YAML output privately
converts only a validated `JsonNumber` lexeme to `Yaml::Real(String)` and then
uses `YamlEmitter`; unchecked `Real(String)` accepts invalid numeric text and
must never cross the owner boundary. Numeric-looking strings are quoted.

`JsonNumber` preserves a valid JSON spelling for wire output, while equality,
integer classification, hashing, bounds and `multipleOf` use mathematical JSON
number semantics. Input order may be retained internally for diagnostics, but
key order and numeric spelling are not API promises; `1e400` and `1e+400` have
the same data-model value.

Unknown keywords are preserved as data; their contents are not indiscriminately
walked as schemas. Recognition of `$id` or anchors is limited to known schema
locations. Invalid schemas remain inspectable/roundtrippable through the model
but cannot produce a successfully compiled workflow validator.

### Public document codec contract

Exact schema data cannot remain behind the current generic root Serde contract.
`JsonValue`, `InputSchema`, and schema-bearing public document types
(`ArazzoSpec`, `Workflow`, and `Components`) do not implement public generic
`Serialize` or `Deserialize` after the wire cutover. A generic deserializer has
already chosen a numeric representation before `InputSchema` can protect it,
and a generic serializer cannot safely select the JSON marker protocol versus
YAML exact-number emission. Retaining those traits with format-dependent or
lossy behavior would make the public API contradict the lossless guarantee.
Ordinary leaf types that do not contain schemas may retain their existing Serde
contracts.

`arazzo-spec` replaces `parse_unvalidated_bytes` and direct document Serde with
named root APIs:

```text
decode_document(bytes, DocumentFormat) -> Result<ArazzoSpec, DocumentDecodeError>
encode_document(&ArazzoSpec, DocumentFormat, DocumentWriteOptions)
    -> Result<Vec<u8>, DocumentEncodeError>
```

`DocumentFormat` is exactly `Json` or `Yaml`; `DocumentWriteOptions` controls
presentation such as pretty JSON without changing data-model equality. Raw-byte
callers must supply the format. Path owners map `.json`, `.yaml`, and `.yml`
before calling the codec; an unknown extension is an explicit caller error.
There is no parse fallback from one format to the other and no content sniffing
inside `arazzo-spec`.

The public error types contain the format, a stable Arazzo-owned reason code,
phase (`syntax`, `data-model`, or `typed-model`), optional bounded line/column
and document JSON Pointer, and a bounded message. Third-party parser errors stay
private sources and their unrestricted text is not a stable contract. Decode
distinguishes malformed syntax, duplicate keys, non-JSON YAML values, resource
limits and typed Arazzo shape errors. Encode distinguishes invalid internal
number/data state from writer failure. Neither error contains an entire schema
or input value.

Every direct root-Serde consumer migrates in the wire-cutover owner slice:
`arazzo-validate` and its contextual parse entry points receive/propagate
`DocumentFormat`; CLI/runtime/MCP/DAP path loaders choose it from the known file
path; CLI and MCP generation call explicit YAML encoding; `arazzo-generate`
tests and `arazzo-spec` roundtrip tests use the root codec. Tests and a repository
search must prove there is no remaining
`serde_yaml_ng`/`serde_json` direct encode or decode of `ArazzoSpec`, `Workflow`,
or `Components`. Release notes name the removed traits, replacement functions,
format-selection rule and error-type migration for external Rust consumers.

### Workspace-wide precision activation boundary

`jsonschema/arbitrary-precision` enables `serde_json/arbitrary_precision` for
the unified production dependency graph, not just schema values. The exact
document codec therefore cannot protect a foreign type that contains
`serde_json::Value`. Feature-off tests cannot prove those paths safe.

Both `arazzo-generate::parse_openapi_file` and the duplicate CLI generation
parser generically deserialize raw YAML or JSON into `openapiv3::OpenAPI`.
That third-party model embeds `serde_json::Value` in extension maps, schema
defaults/examples/enums, media/parameter/header/example values, and link
parameters/request bodies. A Rust 1.85 probe placed a
legal single-key `$serde_json::private::Number` object in `info.extensions`.
With arbitrary precision enabled, a string value of `"123"` silently became a
JSON number; a non-number string rejected the complete OpenAPI document. An
ordinary object remained an object. MCP calls the shared library parser, while
the CLI currently bypasses it, so both entry points belong to one migration.

Post-parse repair is too late: generic deserialization has already lost the
distinction or rejected the document. Reserving the marker key would reject
legal OpenAPI extension data. Because `openapiv3::OpenAPI` is a third-party
Serde model with many embedded dynamic values, a narrow repair shim is not a
sound boundary. Before precision activation, select and qualify an owned,
read-only OpenAPI 3.0 ingestion boundary over the collision-safe tree, or obtain
separate approval for a design that isolates or removes the affected production
surface. This plan does not silently choose that larger architecture change.

OpenAPI examples/defaults and the MCP standalone-example input are already
numeric consumers: their current `i64`/`u64`/`f64` access and JSON-to-YAML
conversion can ignore, round or replace exact out-of-range values. They remain
in the pre-activation numeric-readiness slice; this finding does not authorize
new generator behavior.

MCP generically decodes `JsonRpcRequest` with both `id: Option<Value>` and
`params: Option<Value>`. A differential Rust 1.85 probe kept marker-shaped JSON
objects in both fields without arbitrary precision and changed them to numbers
with it. The MCP adapter migration therefore covers both fields and every
nested dynamic value through collision-safe field/root adapters. It must prove
marker-object and exact numeric-ID behavior while preserving the current ID
acceptance and echo contract; changing that protocol contract needs a separate
decision.

The activation gate inventories the resolved feature graph and every production
generic decode into a type that can contain dynamic JSON. It then runs marker-
shaped and exact-number cases through each actual surface with arbitrary
precision enabled. A repository drift guard owns an explicit allowlist for
those sites; a new site fails the gate until classified. Activation cannot
proceed while any third-party typed model can reach an embedded dynamic value
through generic Serde.

### Compilation and references

`SchemaSet` receives immutable authored schema roots, their exact document
pointers, the containing document context, and explicitly supplied schema
resources. The Arazzo document is a container, not itself a JSON Schema.
Container adaptation is an unresolved acceptance gate, not delegated to the
backend by assumption.

A Rust 1.85 probe registered the exact Arazzo JSON document at its retrieval
URI and compiled through a distinct entry schema. `jsonschema` 0.52.1 resolved
an absolute JSON Pointer into `components.inputs`, but did not index an
`$anchor` below Arazzo container keys; a workflow entered by pointer could not
resolve that sibling anchor. Its draft-aware registry correctly traverses only
JSON Schema subschema keywords.

The selected adapter uses a layered registry and a distinct compiler entry URI.
First, prepare a lower anchor-index resource whose `$id` is the containing
Arazzo URI and whose private `$defs` holds clones of every known input-schema
root. Then overlay the exact Arazzo document at the same URI and prepare again.
Resource lookup finds the exact upper document for JSON Pointers; anchor lookup
falls through to the lower prepared index. Finally, compile each workflow
through an internal entry schema whose sole `$ref` is the absolute document URI
plus its authored workflow-input pointer. Stored schemas, serialized output and
every authored `$ref` string remain unchanged, and the upper exact document
keeps the private `$defs` namespace inaccessible to authored pointers.

Six focused Rust 1.85 tests passed component/workflow pointers, component and
workflow anchors, private-path invisibility and the layered lookup. The temporary
report is `/tmp/ac-93d90-container-ref-qual/REPORT.md` (SHA-256
`cbd17c44159b688067bac0ea9e88c2af77da9f3bc0e671d1a839c5d9eb7b4093`).

Full adapter qualification remains an acceptance gate. The lower catalog copies
only known JSON Schema locations. Duplicate identities/anchors, non-schema
targets and annotation decoys are rejected before backend compile because the
backend silently overwrites same-name anchors.
The projection must preserve `$id`, `$ref`, `$dynamicRef`, `$anchor`,
`$dynamicAnchor`, siblings, recursion and external-resource bases, and every
backend location must map unambiguously to the original Arazzo document pointer.
The probe's projected-anchor failure reported only the document URI and
`/const`; the provenance-aware diagnostic normalizer must reconstruct the
component pointer from the evaluation path and authored reference graph. If
that mapping or the remaining resource/dynamic-scope invariants cannot be
proved, reject this projection/backend rather than ship a partial resolver.

After that adapter establishes the correct resources, the backend handles
ordinary in-schema reference and applicator evaluation. The containing Arazzo
URI is inherited until a schema `$id` establishes another base. Missing base
information is distinct from an unresolved target. A byte-only API can resolve
local container references without inventing a retrievable network URI; a
relative external reference without a base is a contextual schema error.

The adapter retrieves only from its immutable registry. Both library HTTP and
file retrieval features are disabled, and an explicit deny-unknown retriever
is mandatory even if another workspace dependency unifies library features.
An absolute HTTP identifier is permitted when its resource was supplied; it
does not authorize an HTTP request. No fallback to the working directory or
source URL as an API base exists in the schema path.

The current runtime's directory-only input is insufficient to identify the
containing Arazzo file. Contextual parse/build APIs receive its full retrieval
URI; existing `source_base_dir` source-loading semantics remain unchanged.
The shared pure base helper preserves existing routing behavior and is tested
against the current document-identity suite. External Arazzo execution remains
with [ac-83ty](https://sonos.scapedeck.com/docs/ac-tickets/ac-83ty).

The default dialect is 2020-12. An explicit unsupported dialect or required
vocabulary causes compilation failure, never a downgrade to a partial
validator. Ordinary unknown annotation keywords are not errors. Standard
format annotations and content annotations do not become assertions or
automatic content decoding. Unsupported required format-assertion vocabularies
are refused explicitly. Engine qualification must cover both positive and
negative cases; dependency marketing is not conformance evidence.

### Preparation and execution

Compilation is pure and never inserts defaults. Runtime retains a deliberately
bounded compatibility policy: insert defaults only for absent top-level
properties of a direct object schema, or an otherwise empty root `$ref`
wrapper chain ending at such an object. Do not overwrite supplied null, false,
zero or empty values. Preserve explicit `default: null`. Do not infer defaults
from composition, conditionals, nested objects, or multiple competing branches.
Root `$ref` with siblings no longer inherits the old replacement behavior.

After preparation, validate the complete input map. `required` tests presence;
null is allowed only when other constraints allow it. Use the backend's complete
assertions, including enum equality and numeric bounds; remove the hand-written
type/enum assertion pass at cutover. Properties allowed by the schema do not
receive the current unconditional undeclared-input warning.

Validation evaluates the JSON instance it receives; it cannot recover digits
lost before that boundary. The exact-token path must cover `--input-json` and
preserved schema defaults. Ordinary CLI `--input` coercion and YAML-backed
nested parameter literals currently round some numbers; do not claim those
ingress paths are exact. Qualification must trace each path through the actual
validator and settle its compatibility contract before acceptance. An eventual
change to those independent ingestion contracts requires a bounded owner.

Schema compilation, unsupported capability and resource-budget failures stop
execution regardless of strictness. Instance violations retain the existing
policy: strict mode refuses before any request; non-strict mode reports them
and continues. CLI run/test preserve their explicit flag, MCP remains strict,
and DAP retains its non-strict default. No new silent recovery is introduced.
Every invocation, including nested and execute-step paths, uses this same
boundary. Prepared state is per invocation; compiled schema state is immutable.

### Diagnostics and front ends

The schema unit owns a stable diagnostic record: phase, Arazzo-owned reason
code, workflow location, schema resource plus JSON Pointer, instance JSON
Pointer (absent for compilation), keyword, and a redacted message. Root instance
pointer is the empty string; pointer tokens escape `~` and `/`. Diagnostics
sort/deduplicate by phase and stable locations, independent of backend wording.
Required/additional-property diagnostics identify the parent instance and a
separate property name rather than inventing a path to a missing instance.

Never copy the backend's unrestricted error `Display`: it may contain input
values, enum members, patterns, credentials or file contents. URI labels remove
userinfo, query and fragment where not needed for the schema location;
schema pointers remain structured, not interpolated as unescaped log text.
Bound the number/size of diagnostics and emit an explicit truncation record.

Exact locations intentionally disclose workflow/property names and pointer
keys, including a `patternProperties` regex used as a key. This is distinct
from unrestricted backend messages or schema/instance payload disclosure;
there is no promise of name/path secrecy. Escape control characters and bound
every rendered field. Secret-bearing names remain visible under this policy;
requiring their concealment would need a separate opaque-location decision.
Deliberate schema inspection output is a separate surface from diagnostics.

Runtime projects failures through existing `RUNTIME_INPUT_VALIDATION` and
delivers non-strict diagnostics through a typed execution event instead of
raw process `eprintln!`. Nested/retry wrappers retain their existing outer
`RUNTIME_SUB_WORKFLOW_FAILED`/`RUNTIME_RETRY_REFERENCE_FAILED` codes and preserve
the input-validation cause's typed records. An Arazzo-owned error carrier and
accessor convey diagnostics through wrappers and pre-execution compilation
failure, when no event stream exists; adapters never parse strings or depend
on third-party error downcasts. CLI JSON run/test outcomes gain optional
`inputDiagnostics`, while stderr remains readable. MCP successful warnings and
failure tool results expose the same records. DAP forwards them as output
events with stable locations and retains its existing exit/termination order.
Validation CLI errors use explicit schema-invalid/reference/capability reasons.
Schema generators, exhaustive adapter matches and contract snapshots change
together; no existing `RUNTIME_*` string is renamed.

CLI show and MCP describe add the complete authored `inputSchema`. Existing
`inputs` summaries remain descriptive projections, never a schema substitute.
Simple schemas retain their current summaries. Complex unions/compositions use
an explicit incomplete-summary marker; no single type or requiredness is
invented. Non-resolving inspection still exposes the authored schema.

## Migration, security and rollout

This is a public Rust API break even though workspace crates are unpublished.
Replace `SchemaObject`/`PropertyDef` struct-literal access atomically across
the known generator, validator, runtime, CLI, MCP and test consumers. Keep
legacy conversion helpers only if they return an explicit error for unsupported
data; there is no lossy `From<InputSchema>` compatibility shim. The evidence
inventory is not a claim that external Rust consumers do not exist. Release
notes must describe constructor migration, preserved wire fields, defaults,
null behavior, summary changes and stronger input checks.

`serde_json/arbitrary_precision` activation is workspace-wide. The transition
therefore uses four explicit states; the detailed single-owner slices and
verification live in the evidence assessment.

1. **Releasable foundations, precision off.** Raise and prove the Rust 1.85
   baseline under separate authority. Add `arazzo-json-value` in bounded mode,
   move every dynamic CLI/runtime/MCP/DAP JSON ingress to its collision-safe
   adapters—including MCP JSON-RPC `id` and `params`—make
   expression/JSONPath/generator numeric operations total,
   settle and implement the existing OpenAPI 3.0 ingestion owner, add dormant
   `InputSchema` and document-context APIs, then add `arazzo-input-schema` with
   all backend defaults and precision disabled.
2. **Complete validator transition, precision off.** Preserve authored schema
   references, add typed diagnostics, switch runtime preparation/validation to
   exactly one compiled backend, and delete the handwritten validator. Move
   CLI/MCP/DAP presentation to shared records. This begins a release embargo.
3. **Single precision activation.** One `arazzo-input-schema` feature activates
   `jsonschema/arbitrary-precision` and `arazzo-json-value` precise mode. Every
   dynamic decoder, protocol field, third-party typed-model boundary and numeric
   consumer is already safe; the old validator is absent. A feature-enabled feature-graph/
   generic-Serde drift guard and production-path marker matrix pass.
   Conformance remains unsupported.
4. **Exact schema wire cutover.** Change the public spec fields to
   `InputSchema`, enable exact JSON/YAML codecs, remove legacy fields and
   conversions in owner-specific cleanup slices, then run full system proof.
   Only this complete candidate ends the embargo.

No state runs parallel validators or automatically falls back. Before state 3,
newly precise values are unreachable in production. After state 3, no number
may silently become zero, false, null, or lexical text in later execution.
`arazzo-expr/src/lib.rs` and `arazzo-validate/src/lib.rs` remain logged God
modules: new logic lives in focused modules and those façades receive only thin
delegation. Executing their necessary narrow wiring requires the normal human
approval for expansion; this plan does not authorize it.

Security mechanisms are registry-only retrieval; pre-parse byte caps; bounded
schema/instance depth, nodes, strings, number digits/exponents, resources and
alias expansion; backend regex backtrack/compiled-size/DFA limits; and bounded
Arazzo-owned diagnostics. The YAML adapter checks expansion before cloning and
never assumes its event receiver can abort the scanner after a semantic error.

The budget prototype metered node/member/element/number access, equality,
uniqueness, property names and value materialization. Exhaustion can override
the backend verdict and quickly stopped recursive, uniqueness,
`unevaluatedProperties`, large-string and large-number probes. It did not bound
schema-only applicator loops or eager error allocation, and compilation has no
instance hook. This closes the adapter experiment with a failed hard-bound
result. It also showed why the execution contract must never use
`iter_errors`: adapter truncation occurs after the backend has allocated the
complete collection. The selected in-process contract uses the
first-error `validate` path, rejects schemas/instances before parsing or
compilation at calibrated byte/node/depth/scalar/resource/alias caps, may retain
the instance meter as defense in depth, and documents that admitted synchronous
work is finite but not preemptible. Steve accepted that residual risk on
2026-09-02. Cap values are not invented from this disposable probe. The initial
qualification covered 900-branch schema-only
fanout, 900 missing required names, nested applicators, `uniqueItems`,
`unevaluatedProperties` and recursive local refs in debug and release. Production
implementation must calibrate caps and record latency/RSS against representative
large documents and maximum-admitted adversarial inputs. That is implementation
proof, not another design acceptance gate.
The temporary report is
`/tmp/ac-93d90-jsonschema-qual/validate-first-error-report.md` (SHA-256
`87a869063ad0fb0812eeae162f54fb67cebda067c1a1e762dab8c54a736a50e7`).

No worker process, alternate backend or native cancellation design is required
by this plan. If production measurements later show that the selected contract
misses a concrete performance target, that new evidence can reopen the decision.

There is no global mutable schema cache in the first release. Each engine owns
an immutable compiled snapshot; concurrent invocations own their prepared input
maps and diagnostics. New bytes/context produce a new snapshot, preventing
cross-tenant or stale-identity reuse. Repository writes remain serialized.

With the backend execution boundary selected, land consumer-neutral
wire/context foundations first, then compiler qualification, then atomic
production integration and surface contracts. Intermediate changes
must not start accepting newly representable schemas under a validator that
silently ignores their constraints: keep them explicitly unsupported until
the compiled path is available. Do not ship parallel old/new validators with
an automatic fallback. Conformance remains `unsupported` until the full
evidence matrix passes, regardless of plan or ticket status.

Rollback is a release rollback, not a runtime switch to weaker validation.
Newer schema documents cannot safely be sent through the old lossy model;
operators must retain the prior documents alongside the prior binary. There
is no persisted database migration or shared mutable cache to unwind.

## System proof and disposition

Prove exact schema data-model roundtrips in both formats, complete draft-2020-12
keyword/reference behavior, stable redacted errors, and zero I/O for denied
references. Exercise CLI validate/show/run/test, MCP describe/run and DAP,
including strict/non-strict, dry-run, execute-step and nested execution. With
arbitrary precision enabled, also exercise the CLI and MCP OpenAPI parsers,
generation and standalone-example surfaces using legal marker-shaped objects
and exact numbers, plus JSON-RPC `id` and `params` marker objects and exact
numeric IDs. The evidence assessment specifies representative cases and the
independent-unit and integration gates; the later ticket-writing gate must
warning-lint every authored Tier 2 scope.

For the selected execution contract, record latency and peak RSS for
representative real-world large OpenAPI/Arazzo documents and maximum-admitted
adversarial schemas and instances. Verify that admission-limit failures are
stable and occur before compilation or validation work begins.

**Disposition: Blocked draft — the execution contract is selected. The
container-reference adapter still needs provenance/resource proof, and the
workspace-wide precision gate still needs a precision-safe OpenAPI ingestion
owner plus the MCP request-ID/params migration. Close those reference and
precision gates, then obtain fresh review and explicit whole-plan acceptance.
Not transferable to implementation.**
Acceptance is not inferred from this ticket's ready state or from creating
this file. No complete JSON Schema support is claimed by this design ticket.
