# cargo:yaml-rust2 — precise schema token review

Reviewed and qualified 2026-08-31 for
[ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90).
**Candidate 0.11.1 is approved for disposable evaluation only; it is not
installed or approved for the schema-input path.** The workspace already uses
yaml-rust2 0.11.0 in `arazzo-debug-adapter`; that pre-existing DAP use is outside
this review. Candidate 0.11.1 exposes YAML scalar tokens before `f64` conversion;
the existing `serde_yaml_ng` path cannot recover precision later.

| Signal | Evidence / consequence |
|---|---|
| Version/MSRV | 0.11.1, published 2026-08-18, Rust 1.65. Latest 0.12.0 requires Rust 1.85 but was not part of the approved pinned evaluation |
| License | MIT OR Apache-2.0. Active transitive licenses are permissive MIT/Apache, BSD-3-Clause, Zlib, and Unlicense/MIT variants |
| Maintenance | Two registry owners, Ethiraric and davvid; historical continuity unknown. Basic maintenance only; new features move to Saphyr |
| Fidelity | `Event::Scalar` retains token, style, anchor and tag. A 14-test adapter preserved exact numbers, marker-like keys, order and quoted strings and rejected duplicate/non-string keys, non-finite values, tags, cycles and alias bombs |
| Output | Private checked conversion to `Yaml::Real(String)` plus `YamlEmitter` preserved tested exact number spellings. `Real(String)` itself accepts invalid numeric text, so it cannot be public or unchecked |
| Limits | Event receiver cannot stop the scanner after a semantic error. A pre-parse byte cap and before-clone alias expansion accounting are mandatory; characterization limits are not production defaults |
| Build execution | yaml-rust2 has no build script. Active serde/serde_core/serde_json/zmij scripts were inspected; they perform compiler/config probes and `OUT_DIR` generation, not network retrieval |
| Vulnerabilities | Refreshed RustSec audit of the 18-dependency disposable lock: zero vulnerabilities and zero warnings |
| Cost | Active graph: 13 third-party packages. Cold warm-cache release harness: 4.56 s, 246.7 MB max RSS, 744,992-byte binary; 330,288 bytes and 4.39 s over a disposable hello baseline. Workspace delta unmeasured |
| Trust/policy | Registry checksum integrity, not signed publisher identity. No repository policy or Cargo-vet ledger found; absence is not approval |

The package is a viable low-level substrate, not a complete codec and not a
Serde replacement. The qualified boundary is an Arazzo-owned exact JSON tree:

- JSON uses maintained `serde_json::RawValue` syntax/token boundaries, explicit
  first-token dispatch, duplicate-detecting object entries, and direct primitive
  decoding. Generic arbitrary-precision `Value` decoding is prohibited.
- YAML consumes yaml-rust2 events into the same tree, applies the YAML JSON
  ruleset, and enforces key, tag, depth, node and alias boundaries.
- JSON/validator bridges construct value variants directly only after token
  kind is known. YAML output privately converts a validated exact-number lexeme
  to `Yaml::Real(String)`.

The disposable proof passed format, check, clippy, 14 tests, clean debug/release
builds and RustSec audit on Rust 1.85. Production still needs conformance corpora,
fuzzing, Unicode/escape and byte-encoding coverage, mathematical number
semantics, full YAML JSON-rules verification, resource measurements, and actual
workspace cost. The approved fetch populated Cargo's ordinary user registry
cache; no manifest or lockfile changed, and the existing debug-adapter dependency
remained at 0.11.0.

Temporary artifacts live under `/tmp/ac-93d90-yaml-qual`. Sources:
[registry](https://crates.io/api/v1/crates/yaml-rust2),
[exact archive](https://crates.io/api/v1/crates/yaml-rust2/0.11.1/download),
[scalar event source](https://github.com/Ethiraric/yaml-rust2/blob/v0.11.1/src/parser.rs),
[upstream maintenance statement](https://github.com/Ethiraric/yaml-rust2),
[RustSec](https://github.com/RustSec/advisory-db).
See [machine-readable evidence](../../evidence/cargo/yaml-rust2-review.json).
