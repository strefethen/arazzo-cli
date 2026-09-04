# cargo:jsonschema — workflow input validation review

Reviewed and qualified 2026-08-31 for
[ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90).
**Selected for the accepted in-process execution design after disposable
evaluation; not approved or installed in the workspace.** Need: complete JSON
Schema 2020-12 validation behind an Arazzo-owned interface, including exact
numeric constraints.

| Signal | Evidence / consequence |
|---|---|
| Version/age | Registry 0.52.1, published 2026-08-30; under-seven-day cooling warning remains |
| MSRV/license | Rust 1.85.0 and MIT. Steve approved a Rust 1.85 design target. Selected transitive licenses are permissive; one lock-only target expression includes LGPL as an alternative, not a requirement |
| Maintenance | Active releases and correctness fixes; one registry owner, Stranger6667; historical ownership continuity not established |
| Draft/features | 2020-12 and exact huge integer/fraction/exponent probes passed with optional `arbitrary-precision`; it is off in a dormant compiler and activates only after a coordinated workspace-wide Serde reachability gate |
| Retrieval | `default-features = false`; supplied standalone resources and anchors resolved from a registry with zero retriever calls. Missing HTTP/file references reached the deny retriever and failed without I/O. A registered exact Arazzo document exposes absolute embedded pointers, but anchors below container keys need a private schema projection |
| Limits | Regex backtracking is bounded. A custom instance meter passed its own coverage probes but schema-owned applicator/compiler/error loops bypass it. No native general work budget, deadline, cancellation token, or bounded error collection exists; hard-bound qualification failed |
| Diagnostics | Structured paths/kinds exist, but direct and `masked()` messages can expose schema operands. Arazzo must map only allowlisted structure and never forward backend text/operand data |
| Build execution | Package has no build script. Fourteen selected transitive packages have build targets; inspection found platform/compiler probes and `OUT_DIR` generation, no network retrieval |
| Vulnerabilities | Refreshed RustSec lock audit: zero vulnerabilities and zero warnings across 87 dependencies in the disposable lock |
| Cost | Exact macOS tree: 66 dependency packages excluding the harness, including 20 selected direct dependencies. Clean linked release build: 22.64 s wall, 885.84 MiB max RSS, 6.20 MiB harness binary; 5.80 MiB over the unlinked harness baseline |
| Trust/policy | Registry checksum integrity, not publisher signing. No repository Cargo-vet ledger or supply-chain policy found; absence is not approval |

The original fourteen probes passed on Cargo/Rust 1.85: draft keywords, huge integers, long
fractions, extreme exponents and `multipleOf`, offline supplied resources,
anchors, dynamic references, finite recursion, malformed-schema rejection,
diagnostic inspection, regex limits, feature-off compilation, marker-collision
characterization, and eager error-count characterization. Eleven follow-up
budget tests and six synchronous first-error tests also passed, for 31 total.
Instance-side recursion, uniqueness,
evaluated properties, strings, numbers, property names and exact comparisons
exhausted the wrapper budget as designed.

The validator is the selected candidate for the accepted synchronous,
in-process, first-error execution contract. Generic arbitrary-precision
`serde_json::Value` decoding corrupts
a legal single-key `$serde_json::private::Number` object. The design now avoids
that storage/ingress path with an Arazzo-owned exact tree and explicit codecs.
A final production-reachability review proved that package feature unification
also affects non-schema paths: current CLI/MCP-reachable parsing into
`openapiv3::OpenAPI` reaches embedded dynamic JSON values through generic
Serde, changing a legal marker object into a number or rejecting the document.
MCP's typed request decoder also changes marker objects in both JSON-RPC `id`
and `params` into numbers. Package adoption is therefore blocked until the
OpenAPI ingestion owner and MCP field migration are accepted and a feature-
enabled allowlist/drift guard proves no equivalent production typed-model path
remains. The focused reports have SHA-256
`31e30af51a3fb3d5c9fd42c751f79f8ea575d99b9155c11dbc8f29a6caa5d454` and
`bc49d20d53be5c0e4f407e2d272433f0d199923ff115948b608e9c5fe3bea536`.
A custom `Json` wrapper counted every identified instance hook, including
materialization and `with_string_node`, but the decisive hard-bound probes
failed: 200,000 boolean `allOf` branches compiled in 1.040 seconds and validated
with zero wrapper charges, while 4,096 missing `required` properties were all
materialized after a 64-unit budget exhausted. Admission caps make inputs
finite, not cancellable. These results disqualify the wrapper as a hard
preemption mechanism and disqualify `iter_errors` for bounded diagnostics. They
are compatible with the selected contract because hard preemption is not a
product requirement. On 2026-09-02 Steve accepted bounded non-preemptible
residual work behind admission caps and first-error `validate`. Six debug/release
probes support this design under illustrative 16-KiB/1,000-node/depth-32/scalar
caps: one of 900 required errors returned without eager allocation, and admitted
applicator/unique/evaluated/ref cases completed or exhausted visibly. The
instance meter is optional defense in depth, not cancellation or a total-work
guarantee. Production must calibrate caps and record latency/RSS for
representative large documents and maximum-admitted adversarial inputs. No
alternate backend, native cancellation, or worker-process design remains an
acceptance gate.

If retained, the wrapper requires `cargo:jsonschema-value@0.52.1` as an explicit
direct dependency to override `LazyInstance`; `jsonschema` reexports the
representation traits but not that return type. It was already in the disposable
resolved graph and is not separately approved for workspace adoption.

The focused first-error report has SHA-256
`87a869063ad0fb0812eeae162f54fb67cebda067c1a1e762dab8c54a736a50e7`.

A follow-up Arazzo-container probe found a separate reference-adapter boundary.
From a distinct compiler entry, the exact registered document resolves an
absolute pointer into `components.inputs`, but the registry does not index an
anchor below Arazzo container keys. A six-test follow-up qualified a layered
public-API seam: prepare a lower private `$defs` anchor catalog at the document
URI, overlay the exact document, then compile from a distinct internal entry.
Pointers use the exact upper resource, anchors fall through to the lower index,
authored refs remain unchanged, and the private path stays inaccessible. Full
proof must reconstruct authored diagnostic pointers, pre-reject duplicate
anchors the backend silently overwrites, and preserve IDs, dynamic scope,
siblings and external bases. The focused report has SHA-256
`cbd17c44159b688067bac0ea9e88c2af77da9f3bc0e671d1a839c5d9eb7b4093`.

0.33.0 declares MSRV 1.71.1 but lacks arbitrary precision. 0.34.0 raised the
minimum to 1.83; precision arrived in 0.35.0. Later numeric/cycle fixes make
freezing the old line inappropriate. Boon 0.6.1 converts numeric constraints
and equality paths through `f64`. Schemars is a generation/modeling library and
does not provide the required lossless validation boundary.

Temporary reproducible qualification and machine evidence live under
`/tmp/ac-93d90-jsonschema-qual`; the durable result is summarized here and in
the [design evidence](../../../assessments/arazzo-json-schema-2020-12-inputs-evidence.md).
The focused work-budget report has SHA-256
`5d01398e48ec71aa0392ec0f953f3c1579e485373549bba8d7d9e3254443f18c`.

Sources: [registry](https://crates.io/api/v1/crates/jsonschema),
[exact archive](https://crates.io/api/v1/crates/jsonschema/0.52.1/download),
[versioned docs](https://docs.rs/jsonschema/0.52.1/jsonschema/),
[RustSec](https://github.com/RustSec/advisory-db).
See [machine-readable evidence](../../evidence/cargo/jsonschema-review.json).
