# Qualified dependency decision: JSON Schema inputs

Date: 2026-08-31. Owner:
[ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90).
Status: **disposable qualification complete; schema-input adoption deferred**.

Steve approved a Rust 1.85 design target and temporary evaluation of the pinned
packages. This authorization did not add `jsonschema`, upgrade the debug
adapter's existing `yaml-rust2` 0.11.0 dependency, adopt either candidate on the
schema-input path, change the repository MSRV, accept the plan, or authorize
implementation/release/push.

- `cargo:yaml-rust2@0.11.1` is a qualified low-level codec substrate. Its event
  API and checked `Yaml::Real(String)` emission route passed the exact-value,
  marker, duplicate/key, tag/non-finite, alias/cycle/bomb and quoting matrix.
  Adopt only behind the Arazzo-owned exact-tree codec; never as a generic Serde
  replacement or public unchecked number type.
- `cargo:jsonschema@0.52.1` is the selected validator candidate for the accepted
  in-process execution contract, with defaults off. Exact 2020-12 numeric/
  reference behavior, offline registries, anchors,
  dynamic references, recursion, malformed-schema rejection and regex bounds
  passed for standalone schema resources. It failed the custom-meter hard-work
  boundary: schema-side compilation, applicator loops and eager error allocation
  bypass the instance budget. Direct registration of the exact Arazzo document
  resolved an absolute component pointer but did not index anchors below Arazzo
  container keys. A layered lower anchor catalog plus upper exact document and
  distinct compiler entry passed six pointer/anchor/private-path tests without
  rewriting authored refs. Diagnostic provenance, duplicate-anchor rejection
  and remaining resource/dynamic-scope cases still need proof. It is not approved
  for workspace adoption.
- `cargo:jsonschema-value@0.52.1` is required as an explicit direct dependency
  only if the tested custom representation is retained. It was already in the
  evaluated jsonschema lock and is not separately approved for workspace
  adoption. The wrapper is optional defense in depth, not a total-work limit.

The storage decision is no longer generic `serde_json::Value`. Use an
Arazzo-owned exact JSON tree and number lexeme, maintained `RawValue` JSON token
dispatch, yaml-rust2 events, and explicit bridges. This closes the private-number
marker collision at the schema codec boundary. It does not by itself make the
workspace feature safe: `openapiv3::OpenAPI` contains generic dynamic JSON
fields, and a focused probe reproduced document rejection and silent object-to-
number conversion through current CLI/MCP-reachable parsers. MCP's generic
request decoder also changes legal marker objects in both JSON-RPC `id` and
`params` into numbers. Arbitrary precision remains optional until one
coordinated activation after all dynamic JSON/numeric consumers, protocol
fields and typed-model crossings are safe, an OpenAPI 3.0 ingestion owner is
explicitly accepted, and the handwritten validator is gone.

Canonical evidence lives in the
[jsonschema brief](../reviews/cargo/jsonschema.md),
[YAML brief](../reviews/cargo/yaml-rust2.md), and their machine-readable records.
Both disposable locks passed refreshed RustSec audits with zero findings;
licenses/build scripts and harness costs are recorded. These checks do not
establish publisher signing, owner continuity, actual workspace cost, or final
supply-chain approval. yaml-rust2's approved fetch populated Cargo's ordinary
user registry cache; the repository dependency graph stayed unchanged.

The custom-meter experiment is closed as a hard-bound failure. A 200,000-branch
boolean `allOf` used zero instance-budget charges, and eager collection still
constructed 4,096 errors after exhaustion. Admission/regex caps remain
necessary but do not interrupt synchronous backend work. This result forbids
using `iter_errors` or claiming wrapper-based preemption; it does not make
native cancellation and process isolation necessary for this product.

On 2026-09-02 Steve selected synchronous in-process validation: compile once per
immutable document snapshot, enforce byte/node/depth/scalar/resource/alias and
regex admission limits, and use the first-error `validate` API. The product
accepts bounded non-preemptible residual work. The instance meter may be kept as
optional defense in depth. No worker process, alternate backend or native
cancellation work is required by this design. The initial matrix passed six
debug/release cases and 31 full-harness tests under illustrative caps. Production
implementation must calibrate those caps and record latency/RSS for representative
large documents and maximum-admitted adversarial inputs; this is implementation
proof, not another design gate.

Before workspace adoption, fully qualify the selected Arazzo-owned layered
compilation registry for embedded pointers, plain/dynamic anchors, IDs, siblings
and authored diagnostic locations. Also settle the existing OpenAPI 3.0 parser/
model owner, migrate MCP request `id` and `params` without changing their
protocol contract, and pass a feature-enabled repository-wide generic-Serde
reachability guard. Then obtain explicit whole-plan acceptance. Until those
remaining gates are accepted, retain the current repository MSRV/dependencies
and `unsupported` conformance status.
