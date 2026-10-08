# Numeric feasibility evidence for the Pest evaluator cutover

Date: 2026-10-07. Initial probe source baseline: `eeb7b9707c59c2f76654c9f210e5397d4f9b2fab`.
Disposition: completed bounded investigation; no dependency or production change.
The accepted numeric contract is owned by [the conformance plan](../current/arazzo-1.1-conformance-evidence.md#numeric-representation-and-conversion).

## User direction

“the initial probe on exact ints is complete and I've approved a pragmatic approach vs and absolute "exact integers, and decimal limitations"”

The approved direction is pragmatic improvement within bounded scope, not a
universal exactness guarantee or permission to rewrite the value model.

## Evidence

The offline scratch project at `/private/tmp/arazzo-numeric-feasibility/`
used locked `serde_json` 1.0.149, `serde_yaml_ng` 0.10.0 and `serde_json_path`
0.7.2, the current `arazzo-spec`, the qualified vendored JSONPath core and
the current runtime input-validation module. It ran with default storage and
with only `serde_json/arbitrary_precision` enabled. No new package was installed.
Its source and raw `baseline.log` / `exact-storage.log` remain there; this
assessment preserves the decision-relevant results independently of temporary files.

| Probe | Default storage | Precise JSON storage |
|---|---|---|
| JSON `9007199254740993` | Retained | Retained |
| JSON `18446744073709551615` | Retained | Retained |
| JSON `18446744073709551616` | Rounded to floating point | Retained |
| JSON `0.10000000000000001` | Becomes `0.1` | Retained |
| YAML decimal with the same text | Becomes `0.1` | Still becomes `0.1` |
| Actual Arazzo default in JSON-shaped document text | Becomes `0.1` | Still becomes `0.1` |
| JSON `1e-400` | Becomes zero | Retained |
| JSON `1e400` | Parse error | Accepted |
| JSONPath filter for the second of adjacent large IDs | Two matches | Still two matches |
| JSONPath filter distinguishing the two close decimals above | Two matches | Still two matches |
| Serializing a JSON number through YAML | Numeric scalar | Private Number mapping |
| Existing input enum validation on accepted `1e400` | Not reachable by direct JSON loading | Panics |

The last probe caught the panic locally; no production process was exercised.
It exposes a representation assumption after feature expansion, not a claim
that existing ordinary JSON loading currently admits that value.

Scratch `cargo check` and strict Clippy passed. Both programs completed. The
included input-validation module's 22 tests passed under precise storage;
those tests do not qualify the feature across the workspace or cover every
newly admitted value. No performance comparison or new comparator was implemented.

## Why the boundary matters

- `arazzo-expr/src/lib.rs::{compare_values,compare_ordered,to_f64}` currently
  convert numbers to `f64` and apply magnitude-scaled approximate equality.
  `simple_condition.rs` delegates numeric comparisons to them. Distinct
  integers can be lost during comparison even when storage retained them.
- `arazzo-runtime/src/runtime_core/client.rs` loads response/replay JSON
  directly into `serde_json::Value`; CLI input handlers do likewise. Their
  existing value type can carry retained integers without a data-model change.
- `arazzo-spec::parse_unvalidated_bytes` uses the YAML loader even for
  JSON-shaped document text. `ValueSource::Literal`, property defaults and
  extensions contain `serde_yaml_ng::Value`, whose decimal representation
  can lose precision before evaluation. Runtime payload conversion,
  `Parameter::value_as_str`, generator JSON-to-YAML conversion and validator
  value walking depend on this model.
- The vendored JSONPath core's `number_equal_to` and `number_less_than` try
  `as_f64` first. Storage changes alone do not repair those comparisons.
- Runtime input enum validation already compares signed/unsigned integers
  carefully, demonstrating a bounded implementation technique. Its private
  `json_number_kind` assumes every number fits i64/u64/f64, which explains
  the newly admitted `1e400` panic in the probe.

Therefore full decimal preservation reaches document loading, serialization,
JSONPath qualification and input-validation assumptions. This is evidence
against quietly including it in the Pest migration, not proof that it would
require an entire codebase rewrite. A future initiative can assess that cost.

For this migration, preserving current storage and loaders avoids those
changes. A new simple-condition numeric comparison owner can eliminate
comparison-induced integer rounding and approximate equality without
promising recovery of digits already lost during ingestion.
