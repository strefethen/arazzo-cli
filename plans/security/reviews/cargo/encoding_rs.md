# cargo:encoding_rs — WHATWG charset decoder review

Reviewed 2026-11-17 for
[ac-88a70](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-88a70), which
decodes `$response.body` by the response `charset` parameter. **Version 0.8.35
(the artifact already locked) is acceptable for adoption as a direct
`arazzo-runtime` dependency `encoding_rs = "0.8"`.** It is already compiled into
the workspace through `yaml-rust2 0.11.0` (`arazzo-debug-adapter`); this review
covers promoting it to a runtime edge, with no package or checksum change.

| Signal | Evidence / consequence |
|---|---|
| Need | Needs the WHATWG Encoding Standard label table and decoders for fail-closed charset handling. The standard library has only UTF-8; hand-rolling the label table and legacy decoders is the riskier path. Alternative `encoding` is unmaintained |
| Version/MSRV | 0.8.35, published 2024-10-24, Rust 1.36, not yanked. Latest is 0.8.44 (2026-10-30, Rust 1.88, equal to this workspace MSRV); 0.8.36 to 0.8.39 do not exist. A `"0.8"` requirement permits a later `cargo update`, which is a separate review |
| License | `(Apache-2.0 OR MIT) AND BSD-3-Clause`; the BSD-3-Clause part covers WHATWG-derived index data (`LICENSE-WHATWG`). Permissive; no commercial trap |
| Maintenance | Single registry owner and sole publisher across all versions: hsivonen (Henri Sivonen), repository hsivonen/encoding_rs, not archived, last push 2026-10-30, about 475 stars. Author of the Encoding Standard implementation used by Firefox. Bus factor is one; mitigated by a 10-year publishing history under the same owner and renewed activity in Sep-Oct 2026 |
| Maintainer continuity | No added or removed owners; owner list is exactly the original publisher. No prior brief existed to diff against, so the registry history stands in for it |
| Build execution | `build = false` in the published manifest and no `build.rs` in the 0.8.35 archive. No native code, no network, no proc-macros |
| Dependencies | Non-optional: `cfg-if` only (already locked). Optional `serde` and `any_all_workaround` are off by default and not enabled in this lockfile. Adds zero packages to `Cargo.lock` |
| Vulnerabilities | OSV query for crates.io `encoding_rs` (exact 0.8.35 and unversioned): no records. `cargo audit` 0.22.1 against the workspace lockfile: zero vulnerabilities, none naming encoding_rs (6 allowed warnings, all other crates) |
| Unsafe | The crate uses `unsafe` heavily for SIMD and unchecked decoding paths. `arazzo-cli` forbids `unsafe` only in workspace code; a dependency's `unsafe` is not covered by that lint. Residual risk is memory-safety bugs in a crate with 650 million downloads and heavy fuzzing exposure (Firefox) |
| Trust/policy | Registry checksum `75030f3c4f45dafd7586dd6780965a8c7e8e285a5ecb86713e63a79c5b2766f3` unchanged since the root lockfile first carried it (3a0b580, 2026-03-12). Checksum integrity, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Adoption notes:

- Use `encoding_rs::Encoding::for_label` for label lookup and the `decode_without_bom_handling_and_without_replacement` family so invalid bytes fail closed instead of producing U+FFFD. The default `decode` replaces malformed input silently.
- Note `for_label` maps `utf-16*` and several legacy labels; the ticket must decide which labels are accepted rather than inheriting the whole table.
- Pin to the locked artifact by leaving the lockfile untouched; do not run a blanket `cargo update`.

Sources:
[registry](https://crates.io/api/v1/crates/encoding_rs),
[owners](https://crates.io/api/v1/crates/encoding_rs/owners),
[repository](https://github.com/hsivonen/encoding_rs),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-encoding_rs-add](../../decisions/2026-11-17-encoding_rs-add.md).
