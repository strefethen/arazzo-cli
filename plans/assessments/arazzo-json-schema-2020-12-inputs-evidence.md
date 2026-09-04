# JSON Schema input design evidence

Date: 2026-08-31. Owner:
[ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90).
Supports the [draft plan](../current/arazzo-json-schema-2020-12-inputs.md);
grants no implementation authority. Baseline:
[16b96e7](https://github.com/strefethen/arazzo-cli/commit/16b96e71fdd9b13343ee61abdca5baa07e788212)
(local; not pushed). Existing unrelated working-tree changes are preserved.

## Producer and consumer inventory

Paths are repository-relative; external Rust consumers cannot be ruled out.

| Owner | Current behavior and migration seam |
|---|---|
| `crates/arazzo-spec/src/lib.rs`: `ArazzoSpec`, `Components`, `Workflow`, `SchemaObject`, `PropertyDef`, `JsonSchemaType` | Public schema fields and root/schema-bearing `Serialize`/`Deserialize` contracts. Replace fields and remove generic document Serde through a focused codec module/re-export; migrate constructors and callers as a Rust API break. Do not change ordinary Arazzo vendor-extension filtering. |
| Same file: `parse_unvalidated_bytes` | Both JSON and YAML use `serde_yaml_ng` and expose its error. Replace with explicit-format `decode_document`/`encode_document` and Arazzo-owned error types; no sniff/fallback. |
| Direct document Serde consumers: CLI and MCP generation handlers; spec/generator roundtrip tests; validate/runtime/front-end loaders | Direct `serde_yaml_ng::to_string(&ArazzoSpec)` and generic root decoding bypass an exact format boundary. Migrate generation to explicit YAML encoding, loaders to a propagated `DocumentFormat`, and tests to root codec APIs; prove no remaining direct document Serde use. |
| CLI `--input-json`/trace, runtime HTTP/replay, MCP JSON-RPC `id` and `params`, DAP arguments | Dynamic `serde_json::Value` ingresses share the arbitrary-precision private-marker hazard. Migrate all fields and nested values through new `arazzo-json-value` field/root adapters before feature activation; preserve MCP ID acceptance/echo behavior absent a separate protocol decision. |
| `crates/arazzo-generate/src/lib.rs::parse_openapi_file`; duplicate parser in CLI `handlers.rs`; `openapiv3::OpenAPI` | Generic OpenAPI deserialization reaches third-party `serde_json::Value` fields. Under arbitrary precision a legal marker-shaped `info` extension was rejected or changed into a number before callers could repair it. Select and qualify an owned read-only OpenAPI 3.0 ingestion boundary, or return a separately approved isolation/removal design; post-parse repair and marker reservation are forbidden. |
| `crates/arazzo-validate/src/lib.rs`: parse/validate APIs, `resolve_input_refs`, `resolve_components` | Parse returns effective model with root schema reference replaced by component clone; direct validate clones privately. Remove only schema replacement, preserve parameter/action resolution. |
| `crates/arazzo-validate/src/tests/cases/component_inputs.rs`, `unknown_fields.rs` | Tests explicitly enshrine cleared references and lost siblings. Change these intentionally; retain schema-keyword exemption from Arazzo unknown-field rules. |
| `crates/arazzo-generate/src/crud.rs`: `make_workflow` input construction | Only production struct-literal producer; required string auth/ID fields. Bounded constructor migration. |
| `crates/arazzo-cli/src/handlers.rs`: generation serialization | YAML output of generated model needs precise schema serialization. |
| `crates/arazzo-runtime/src/runtime_core/input_validation.rs` | Defaults/required/type/enum/undeclared-input passes, public `InputIssue`. Separate preparation policy from complete validation. |
| `crates/arazzo-runtime/src/runtime_core/engine_impl.rs` | Shared serial/parallel input helper; warnings are `eprintln!`. Thin delegation and typed delivery only. |
| `crates/arazzo-expr/src/lib.rs`: numeric comparisons/truthiness; runtime JSONPath/payload; CLI `handlers.rs`: input coercion; OpenAPI examples/defaults and MCP standalone examples | Indirect consumers of globally enabled precision. `f64` fallbacks can turn large exact values into false/lexical ordering; JSON-to-YAML can substitute null; legacy `--input` remains bounded. Migrate to total shared numeric helpers before activation. |
| `crates/arazzo-runtime/src/runtime_core/document_set.rs`, `builder.rs`, `state.rs` | Private base helper; only directory available, context consumed at build. Exact containing-file URI and immutable compiled snapshot need explicit interfaces. |
| `crates/arazzo-cli/src/output.rs` | List/catalog/show inspect properties/required/scalar type/description. Shared descriptive projection and authored schema output required. |
| `crates/arazzo-mcp/src/handlers.rs` | Duplicated introspection; run always strict. Share projection and diagnostics; retain strictness. |
| CLI `handlers.rs`, `test_runner.rs`; MCP `state.rs`; DAP `session.rs` | Indirect consumers of effective `arazzo_validate::parse`. Carry document context and verify component-based display/execution. |
| `crates/arazzo-debug-adapter/src/dap/session.rs` | Non-strict by default; failures emit code/message, exited(1), terminated. Non-strict warnings currently lack DAP events. |
| Runtime unit tests and `crates/arazzo-runtime/tests/input_validation.rs`; spec serde tests | Direct legacy constructors and already-parsed model comparisons. Compare original schema data to serialized output. |
| `docs/schemas/{show,run,test,validate}.schema.json` | CLI contracts; `InputDetail.type` is a required string. Update generated contracts with owning surface changes. |

Known God module: validator `lib.rs` is now 3,784 lines after test extraction,
but still combines parsing, diagnostics, raw checks, component resolution and
validation domains. It is already logged in `god-files.md`; the old line count
there is stale. No new concern belongs there. No newly identified God module.

## Disposable characterization and baseline

Harness used existing built libraries, without new dependencies. Full temporary
source/results: `/tmp/ac-93d90-characterization.rs`,
`/tmp/ac-93d90-characterization.json`; these are session evidence, not durable
build inputs. There are 57 schema cases: 24 cases in both JSON and YAML, plus
nine YAML-only cases. Raw Value and transparent wrapper agree on all 53 accepted
post-parser values; four parse errors. Agreement after parsing is not proof of
original numeric fidelity or schema validity.

| Family | Current model | Raw wrapper / implication |
|---|---|---|
| Boolean roots/properties, type arrays, null type | Rejected | Preserved; checked schema root still required |
| Composition, conditionals, unevaluated keywords, `$defs`, annotations | Captured then filtered from output unless `x-*` | Structurally preserved |
| `default: null`; absent property description/format | Loses null presence; inserts empty annotations | Raw data preserves these distinctions |
| Root `$ref` siblings | Validator replaces schema with component clone | Storage alone cannot fix parse API mutation |
| Beyond-u64 integers, long decimals | Numeric limits/rounding | Default JSON Value also rounds; precision must begin at ingestion |
| `1e-400`, `1e400` | Not exact | Underflow to zero; JSON overflow errors but YAML may parse same token as string |
| YAML `.nan`/`.inf`, non-string keys | Not JSON data | Naive conversion can create null or stringified keys; reject explicitly |
| Duplicate YAML keys | Ambiguous input | Direct Value parsing overwrites; YAML Value parsing rejects; use strict boundary |
| YAML custom tags | Outside JSON ruleset | YAML Value conversion can create tagged-key JSON objects; reject, do not normalize |
| Null/string/array root, malformed keyword shapes | Partly rejected by typed model | Raw wrapper accepts too much without shape checks; schema validity is a separate contract |

`Option<RawSchema>` also conflates absence with explicit null; the new optional
field needs a presence-aware deserializer. Proposed exact YAML codec and new
validation engine are **not implemented or proven** by this characterization.

Selected baseline results: 82 passing tests — spec 27 unit + 23 roundtrip,
runtime 22 unit + 5 integration, CLI 5 integration. The command log is in
`/tmp/ac-93d90-characterization-summary.md`. A redundant broad runtime filter
was interrupted after focused reruns passed; it is not counted as a full-suite
pass. All verification processes finished or were explicitly stopped.

| Exact baseline command | Passing tests |
|---|---:|
| `cargo test --offline -p arazzo-spec` | 50 |
| `cargo test --offline -p arazzo-runtime --lib input_validation` | 22 |
| `cargo test --offline -p arazzo-runtime --test input_validation` | 5 |
| `cargo test --offline -p arazzo-cli --test cli_integration input` | 5 |

These ran on installed Rust/Cargo 1.98.0, not a Rust 1.82 compatibility build.
The CLI filter covers existing input parsing, display, environment expansion,
redaction and integer preservation; it is not new-engine surface qualification.

`tkt lint --severity warn ac-93d90` passed intake. `tkt doctor` reports the
pre-existing generated-artifact ignore failure; store/reservations are healthy.
This design does not repair tracker infrastructure.

## Representation and package alternatives

| Approach | Fidelity / compatibility | Disposition |
|---|---|---|
| Public raw `serde_json::Value` | Generic arbitrary-precision decoding corrupts private-marker-shaped objects; generic YAML exposes the marker | Reject as storage/codec boundary |
| Arazzo-owned exact JSON tree + number lexeme | No keyword normalization; `RawValue` JSON tokens and YAML events preserve exact values; generic Serde absent | Selected and qualified as a disposable design |
| Schemars 0.8.22 | MSRV 1.60, MIT; numeric constraints f64, counts u32, required set, `$defs`/`definitions` rewrite | Reject for lossless storage |
| Schemars 1.2.2 | MSRV 1.74, MIT; Boolean/object JSON wrapper | No storage benefit that warrants generator dependency |
| jsonschema 0.33.0 | Declared MSRV 1.71.1; no arbitrary precision | Reject as complete exact-number validator |
| Boon 0.6.1 | No declared MSRV; f64 numeric constraints/equality | Does not solve precision blocker |
| jsonschema 0.52.1 | MSRV 1.85; exact standalone 2020-12 behavior passed; no native general work/cancellation budget; exact Arazzo resource resolves embedded absolute pointers but does not index anchors below container keys | Selected for synchronous in-process first-error validation; anchor/diagnostic and workspace precision gates remain; not approved for workspace adoption |
| yaml-rust2 0.11.1 | MSRV 1.65; exact event/checked-emitter probe passed; basic maintenance; 0.11.0 already serves the debug adapter | Selected codec substrate pending production/schema-codec acceptance |

Canonical [jsonschema review](../security/reviews/cargo/jsonschema.md),
[YAML review](../security/reviews/cargo/yaml-rust2.md), and
[package decision](../security/decisions/2026-08-31-json-schema-inputs.md)
own dated sources, resolved-lock audits, selected build-script/license reviews
and disposable build costs. These harness measurements are not workspace
incremental or final-binary deltas.

### Approved disposable qualification

Steve approved a Rust 1.85 design target and the two pinned temporary builds.
No workspace manifest, lockfile or source changed. The yaml-rust2 fetch
populated Cargo's ordinary user registry cache; project/audit artifacts stayed
under `/tmp`.

| Harness | Result | Cost / graph |
|---|---|---|
| `/tmp/ac-93d90-jsonschema-qual` | 31 tests passed: 14 backend probes, 11 custom-instance budget tests and six first-error tests. Exact behavior and instance metering passed; wrapper hard-preemption failed; capped synchronous first-error supports the selected execution contract, with production calibration assigned to implementation | Rust 1.85; 66 selected dependency packages excluding harness, 20 selected direct; clean linked release 22.64 s wall, 885.84 MiB max RSS, 6.20 MiB binary; RustSec zero findings |
| `/tmp/ac-93d90-yaml-qual` | 14 codec tests passed: exact numbers, boolean/object roots, marker object, order/duplicates/keys, tags/non-finite values, aliases/cycles/bombs, exact numeric and quoted-string emission | Rust 1.85; 13 active third-party packages; release 4.56 s, 246.7 MB max RSS, 744,992-byte binary; RustSec zero findings |
| `/tmp/ac-93d90-serde-probe` | Six-case follow-up proved `RawValue` subtree dispatch retains marker objects, exact numbers, Unicode surrogate pairs and empty strings and rejects duplicates without a handwritten JSON grammar | Existing cached serde packages; no adoption/cost claim |
| `/tmp/ac-93d90-openapi-precision` | Focused OpenAPI 3.0 parse probe proved generic `openapiv3::OpenAPI` ingestion changes a legal marker-shaped extension into a number or rejects the document under arbitrary precision. Post-parse repair is too late | Rust 1.85; existing pinned parser dependencies; report SHA-256 `e797f36c1fc2aba26e43f608421a7d9aac9064737ad5b16e89901c85caf9796a` |
| `/tmp/ac-93d90-openapi-value-matrix` | Twelve representative embedded `openapiv3::OpenAPI` value fields preserved marker objects without arbitrary precision and changed them to numbers with it | Rust 1.85; pinned `openapiv3` 2.2.0/serde packages; report SHA-256 `31e30af51a3fb3d5c9fd42c751f79f8ea575d99b9155c11dbc8f29a6caa5d454` |
| `/tmp/ac-93d90-global-ap-audit` | Bounded production conversion census proved MCP request `id` and `params` share the collision; quoted Arazzo parameter/default/selector conversion controls preserved objects in both builds, and no other material CLI-closure route was found | Rust 1.85 baseline/precision differential; report SHA-256 `bc49d20d53be5c0e4f407e2d272433f0d199923ff115948b608e9c5fe3bea536` |
| `/tmp/ac-93d90-container-refs` | Five Rust 1.85 tests: exact Arazzo registration resolves an absolute component pointer but not embedded anchors; an isolated `$defs` projection passed translated pointers, a plain anchor and recursive dynamic ref | Existing cached jsonschema graph; report SHA-256 `6658b49459975b0eebaf483ba2f4895eec7ee20a2533a65b2850fdba127f41b4` |
| `/tmp/ac-93d90-container-ref-qual` | Six tests qualified the layered adapter: lower private anchor catalog, upper exact Arazzo resource, and distinct compiler entry preserve authored pointer/anchor refs and hide synthetic paths. Diagnostic provenance and duplicate-anchor rejection remain gates | Rust 1.85; fmt/check/clippy/test pass; report SHA-256 `cbd17c44159b688067bac0ea9e88c2af77da9f3bc0e671d1a839c5d9eb7b4093` |

The selected storage is now `arazzo-json-value::JsonValue`, an owned exact tree
with validated number lexemes and no generic Serde implementation. JSON uses
`RawValue` recursion with duplicate-detecting raw object values. YAML uses
yaml-rust2 events and a private checked `JsonNumber -> Yaml::Real(String)`
emission route. The temporary test limits are characterization values, not
production defaults. Production needs corpora/fuzzing, byte encodings,
Unicode/escape properties, mathematical numeric helpers, full YAML JSON rules,
and measured workspace cost.

`jsonschema` is the selected validator candidate for the accepted in-process
execution contract; it is not approved as a workspace dependency or complete
engine. Its structured error paths are useful, but messages and operand-bearing
kinds expose values and its error iterator materializes all errors. It has no
native total work/deadline/cancellation limit, which is a known limitation of
the selected contract rather than an open product requirement.

The executed custom-`Json` prototype covered every identified instance hook:
containers and scalars, strings/numbers, equality, uniqueness, property names,
materialization and lazy values. It stopped recursive, 5,000-item uniqueness,
5,000-property `unevaluatedProperties`, large-string and large-number probes at
the budget boundary. `jsonschema-value@0.52.1` was required directly to override
the non-reexported `LazyInstance` return type.

The hard total-work contract nevertheless failed. A 200,000-branch boolean
`allOf` compiled synchronously in 1.040 seconds and validated in 2.388
milliseconds with zero instance charges. With 4,096 missing required properties,
the backend created every error after a 64-unit wrapper budget exhausted.
Schema byte/node/depth/scalar caps and regex limits make admitted inputs finite,
but they cannot interrupt compilation, schema-owned loops or post-access work.
The focused temporary report is
`/tmp/ac-93d90-jsonschema-qual/work-budget-report.md` (SHA-256
`5d01398e48ec71aa0392ec0f953f3c1579e485373549bba8d7d9e3254443f18c`).
This disqualifies the wrapper as a hard preemption mechanism and disqualifies
eager `iter_errors` for bounded diagnostics. On 2026-09-02 Steve selected the
synchronous in-process contract: compile once per immutable document snapshot,
apply byte/node/depth/scalar/resource/alias and regex admission limits, and call
the first-error `validate` path. Non-preemptible residual work within those
bounds is accepted. The instance meter may remain as defense in depth, but is
not required and must not be described as cancellation or a total-work bound.

The follow-up first-error matrix passed six tests in both debug and release and
31 tests across the full harness. Under illustrative 16-KiB/1,000-node/depth-32/
1,024-byte-scalar caps, one of 900 missing required-property errors returned
without materializing the other 899. Nested applicator, uniqueness,
`unevaluatedProperties` and recursive-ref cases completed with sufficient
budget or returned visible exhaustion. A 900-branch boolean `allOf` remained
schema-owned and unmetered but completed inside the admitted envelope. This
supports the selected design without establishing a production cap or
cancellation guarantee. Representative large-document and maximum-admitted
adversarial benchmarks, cap calibration, latency/RSS proof, and stable
admission-limit errors are implementation evidence rather than a remaining
design decision. Report SHA-256:
`87a869063ad0fb0812eeae162f54fb67cebda067c1a1e762dab8c54a736a50e7`.

The reference qualification exposed and narrowed a separate adapter boundary.
With a distinct compiler entry, an absolute JSON Pointer into a registered exact
Arazzo document resolves. Anchors below `components.inputs` do not, because the
backend deliberately traverses only JSON Schema subschema keywords. The
selected layered registry prepares a lower private `$defs` anchor catalog at the
document URI, overlays the exact Arazzo resource at that URI, and compiles from
a distinct internal entry. Exact upper resource lookup handles pointers; anchor
lookup falls through to the lower index. Six tests passed authored component and
workflow pointer/anchor refs while keeping the private path inaccessible. Full
qualification must preserve IDs, dynamic scope, siblings and external bases,
pre-reject duplicate anchors the backend would overwrite, reject decoy
locations, and reconstruct original component pointers for diagnostics whose
backend anchor path contains only the document URI and local keyword path.

The final feature-reachability audit exposed a separate workspace boundary.
`serde_json/arbitrary_precision` is unified into the CLI and MCP binaries, so
generic OpenAPI parsing is affected even though no schema codec calls it. The
focused probe exercised `Info.extensions`, and a second matrix exercised eleven
more representative `serde_json::Value` fields in `openapiv3` 2.2.0: marker
objects became numbers, while a non-number marker rejected the whole document.
The CLI contains a second direct parser rather than calling
`parse_openapi_file`; both must migrate.
Source inspection also found exact-number narrowing in generator example/
default consumption and the MCP standalone-example path. Those numeric routes
were already part of readiness, but now have named production proof. The audit
also proved that MCP's generically decoded request `id` is hazardous alongside
the already-owned `params` field. Both must use the safe adapter with their
existing protocol contract preserved. It did not prove equivalent corruption
in the quoted Arazzo parameter/default/selector conversion probes; they are not
recorded as defects.

## Follow-on boundaries, not authored Tier 2 tickets

The high-level transition order is settled, but these are decomposition seams rather
than authored tickets. Each row changes one named unit; repeated-unit work is
serialized. Warning-lint and transfer-audit every eventual ticket. State A is
releasable with precision off. States B through D form one release embargo:
commits remain buildable, but no intermediate binary ships.

| Slice / unit | Depends on | Write boundary | Required proof |
|---|---|---|---|
| Rust 1.85 baseline / workspace config | explicit authority | MSRV declaration and CI/toolchain configuration only | all existing gates on 1.85 and current stable |
| Safe JSON value / new `arazzo-json-value` | accepted codec/package decision | owned exact tree, bounded default codec, precise feature, mathematical numeric helpers | RawValue/event corpora, fuzzing, marker/duplicate/escape/byte/budget cases; consumer-free |
| Expression precision readiness / `arazzo-expr` | json-value, normative expression decision | new numeric module plus thin façade delegation | exact literal/equality/order/truthiness; no false/lexical fallback; explicit God-module expansion approval |
| Runtime JSON readiness / `arazzo-runtime` | json-value, expression | HTTP/replay decode, payload/YAML conversion and local JSONPath only | exact transport and numeric behavior; no validator switch |
| CLI, MCP and DAP dynamic JSON migrations / each adapter separately | json-value | input/trace, JSON-RPC `id` plus `params`, and DAP arguments respectively | marker-shaped objects and exact values; MCP numeric-ID acceptance/echo preserved; typed control numbers fail rather than default |
| OpenAPI ingestion precision readiness / `arazzo-generate`, then CLI adoption | accepted cross-initiative ownership decision, json-value | existing OpenAPI 3.0 parse/model boundary only; generation behavior unchanged; delete duplicate CLI parser | every dynamic OpenAPI field preserves legal marker objects and exact numbers under the production feature graph; no generic post-parse repair |
| Checked examples / `arazzo-generate`, then MCP adoption, then generator cleanup | OpenAPI readiness, json-value | additive faithful generator API; handler adoption; later legacy removal | OpenAPI and standalone-example exact numbers do not round, disappear or become YAML null; public break documented |
| Authored schema foundation / `arazzo-spec` | json-value | dormant `InputSchema`, checked legacy conversion/accessors; fields unchanged | boolean/object roots, exact model and explicit conversion failures |
| Document context / new unit, then runtime adoption | none | pure base calculation; separate thin runtime delegation/delete | current identity suite, no I/O or routing expansion |
| Schema compiler / new `arazzo-input-schema` | spec/context/json-value plus selected backend execution and accepted container-reference contracts | offline bounded compiler, Arazzo container projection/registry, diagnostics, descriptive projection; precision off | consumer-free dialect, exact container pointers, anchor/dynamic-ref/id/sibling/decoy mapping, redaction and work-contract cases |
| Schema preservation / `arazzo-validate` | compiler | focused module, remove input `$ref` replacement, contextual APIs | authored siblings survive; other component resolution unchanged; no God growth |
| Runtime diagnostic foundation / `arazzo-runtime` | compiler | immutable snapshot and typed carriers; execution unchanged | nested/retry cause preservation and concurrency isolation |
| Complete validator switch / `arazzo-runtime` | preservation/diagnostics | preparation + one compiled handle; delete handwritten validator in same slice | bounded legacy domain, strict/non-strict, all execution paths; old validator unreachable |
| Diagnostic adoption / CLI, MCP, DAP separately | runtime switch | transport/presentation only | snapshots, redaction, ordering, truncation, termination order |
| Precision activation / `arazzo-input-schema` | every readiness/adoption slice | single feature activates backend and json-value precision | all dynamic ingresses, third-party typed models and numeric consumers remain total; old validator absent; feature-graph/generic-Serde allowlist drift guard passes |
| Exact wire cutover / `arazzo-spec` | precision activation | public fields, remove schema-bearing root Serde, add explicit-format root codecs/errors, remove legacy field path | original JSON/YAML data-model equality; no direct document Serde consumers; format/error/public migration docs |
| Cleanup / each owning unit separately | wire cutover | delete only unused conversions/adapters | no behavior change |
| Conformance evidence / test and evidence owners | complete D candidate | fixtures, surface tests and manifest only | full matrix, four drift guards, zero reference I/O; only then change coverage |

New workspace membership and lock entries belong to the corresponding new-unit
slice. Before precision activation, newly precise production values are
unreachable. There is no runtime feature switch, parallel validator or fallback.
After release, rollback restores the entire precision/wire release range and
the prior document; reverting only the feature activation is unsafe.

## Required implementation proof

| Concern | Positive case | Negative / preservation case |
|---|---|---|
| Wire | Boolean/union/composition/conditional/unevaluated/annotations/unknown fields in both formats through explicit root codec APIs | Invalid root, duplicate key, non-JSON YAML, unknown format/path extension; exact presence/numbers; marker-like object keys retain meaning at every depth/key order; direct schema-bearing document Serde absent |
| Numeric | Large integers, long fractions, extreme exponents, const/enum/multipleOf through every claimed ingress and subsequent expression consumer | No old-enum panic, rounding-based acceptance or false/lexical conversion; legacy coercion limits explicit |
| Global precision | Legal marker-shaped OpenAPI extension/example/default, standalone-example exact numbers, and marker-shaped currently accepted MCP `id`/`params` traverse CLI/MCP production consumers with arbitrary precision enabled | Invalid marker numeral cannot reject a containing OpenAPI document or become a number; exact numeric IDs preserve the current protocol contract; no unclassified generic decoder into a typed dynamic-value holder remains |
| Registry | Forward workflow/component container refs, escaped pointers, id rebase, anchors/dynamic refs and siblings map back to authored locations | Missing base/resource, duplicate identity/anchor, non-schema target, synthetic-path access, annotation decoys and unsupported vocabulary |
| Preparation | Direct/simple-root-ref defaults; provided null/false/zero retained | Invalid default follows normal validation; no branch-dependent inference |
| Assertions | Full string/number/object/array/evaluated-location constraints | Required/null distinction; allowed additional/pattern properties; malformed schema fatal in non-strict mode |
| Execution | Serial/parallel/nested/retry-reference/dry-run/execute-step | Strict rejection before request; typed cause diagnostics survive wrapper codes; non-strict diagnostics delivered on every front end |
| Security | Supplied HTTP-identified resource resolves offline; first-error validation finishes ordinary recursive/evaluated cases | File/HTTP miss performs no I/O; pre-byte/number/depth/node/resource/alias caps; regex limits; no eager `iter_errors`; stable limit errors; representative large-document and maximum-admitted adversarial latency/RSS measurements; no hard-preemption claim |
| Diagnostics | Stable phase/code/resource/instance pointers; exact names/keys deliberately disclosed | No backend display or operand-bearing kind payloads; URI userinfo/query stripped; fields escaped/bounded; execution never calls eager `iter_errors`; secret-bearing names are not promised confidential |

Qualification executed the decoder hazard: `serde_json` 1.0.149/1.0.151
arbitrary-precision generic `Value` parsing turns a single-key
`$serde_json::private::Number` object into a number, and generic YAML output can
expose the marker map. The RawValue/event codec retained it as an object at
every tested depth and key order. Rejecting or reserving the key would reject
legal schema annotation data.

## Acceptance status

Not complete: Rust 1.85 and disposable package qualification are approved; the
exact-tree representation, high-level transition order, and synchronous
in-process first-error execution contract are settled. Hard preemption is not a
product requirement; the custom meter's failed hard-bound experiment remains a
known limitation. The plan is blocked on the lossless container-reference
adapter and a complete workspace precision-activation design, including an
accepted precision-safe OpenAPI ingestion owner and MCP `id`/`params` migration.
Follow-ons are deliberately not authored Tier 2 tickets before those decisions
and explicit whole-plan acceptance. Production cap calibration and representative
large/adversarial latency/RSS measurements belong to implementation proof.
Conformance remains unsupported. No production source, dependency manifest,
lockfile or repository MSRV changed, and no Git commit is part of this design
draft.

## Independent review and integration

Round 1 used three independent frontier review contexts: failure hunter,
security red team, and meta-critic (which read both other seats' findings).
All seven proposals were agreed, with the two numeric-feature findings merged.
No requested change was rejected and no production scope was added.

| Finding | Integrator disposition |
|---|---|
| Global precision activation and downstream expression behavior | Agree: shared feature activation is a gated transition, not an additive wire-only change; include numeric consumers in inventory |
| Input ingestion fidelity | Agree: distinguish received JSON values from already-rounded CLI/YAML coercion and require explicit ingress qualification |
| Nested/retry and pre-execution diagnostics | Agree: preserve cause records through existing outer runtime codes; errors carry records when no event stream exists |
| Serde private-number marker collision | Agree: require collision-safe token decoding and preserve all legal keys; raw wrapper is not a qualified decoder |
| Location metadata disclosure | Agree: exact names/keys intentionally public, unrestricted payloads prohibited; remove impossible absolute name/path secrecy promise |
| Per-unit transition architecture | Agree: settle published transition surfaces and numeric activation before ACCEPT, not during ticket sizing |

Temporary review reports: `/tmp/ac-93d90-review-failure-r1.md`,
`/tmp/ac-93d90-review-security-r1.md`, `/tmp/ac-93d90-review-meta-r1.md`.
The integrated draft remains unaccepted and not transferable.

Metadata verification: `cargo test --offline -p arazzo-cli --test conformance_manifest`
passed all 10 tests; `cargo fmt --all -- --check`, `git diff --check` and
warning-level ticket lint passed. JSON evidence parses and local document links
resolve. All 12 pre-existing dirty files matched their initial SHA-256 values.
`tkt scope` groups new untracked review/evidence directories as apparent drift;
file-level Git status confirms their four files exactly match declared writes.

Round 2: fresh failure-hunter and security contexts reported no new material
findings against draft SHA-256
`dc8e9ecea0d5667e93c4645e9e8893b3f4e00d08d83d342502edc1bac657cee7`.
The independent meta-critic reviewed both reports and confirmed the transition
gate was integrated, also with no new material findings. A fresh second-round
meta context could not spawn because the harness reached its cumulative agent
limit, so that seat reused its original independent review context. This is
two-round draft review, not fresh-context exhaustion, panel convergence, plan
acceptance or implementation authorization. Reports are
`/tmp/ac-93d90-review-{failure,security,meta}-r2.md`.

Round 3 used a fresh independent design-review context against the expanded
draft. It found three material omissions in sequence: the exact document model
had no viable public codec migration; the execution discussion treated native
cancellation and process isolation as a false exhaustive choice; and the
Arazzo-container anchor projection was asserted without a qualified public-API
mechanism or authored-location proof. The plan now owns explicit-format root
codecs, describes capped synchronous first-error validation as a viable
residual-risk contract, and records the tested layered registry plus its
remaining diagnostic/resource gates.

That reviewer then ran the bounded global arbitrary-precision audit summarized
above. The final material findings were the OpenAPI typed-model boundary and
the previously unnamed MCP JSON-RPC `id`; both are integrated. Stronger quoted-
marker controls withdrew preliminary claims about `Parameter::value_as_str`,
the old default/enum conversion and debug output projection, so those are not
presented as feature-activation defects. The review reports are
`/tmp/ac-93d90-global-ap-audit/REPORT.md` and
`/tmp/ac-93d90-openapi-value-matrix/REPORT.md`. The exact-candidate consistency
re-review matched all eight supplied SHA-256 hashes and reported no material
defects. This is review convergence, not plan acceptance or implementation
authority.

On 2026-09-02 Steve selected that synchronous in-process, first-error contract
and accepted its bounded non-preemptible residual work. This closes the
execution-design gate only. It does not accept the whole plan, approve package
adoption, set production limits, or supply implementation performance proof.

Current scope verification: exactly eight owned files, an empty Git index, no
production source/dependency-manifest/lockfile changes, and all twelve original
dirty files unchanged. `cargo fmt --all -- --check`, all ten conformance-
manifest tests, JSON parsing, `git diff --check`, sixteen local-link checks and
warning-level ticket lint passed. `tkt doctor` still reports only the pre-
existing generated-artifact ignore failure. The earlier 82 selected baseline
tests plus the final ten manifest tests total 92; disposable backend/codec/
adapter probes are recorded separately. A complete workspace test and Rust 1.85
workspace build are not claimed. No commit or push exists. The ticket remains
open pending the reference-adapter and workspace-precision design gates plus
explicit whole-plan acceptance.
