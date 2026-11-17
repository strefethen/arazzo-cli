# cargo:brotli — Brotli codec review

Reviewed 2026-11-17 for
[ac-24c42](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-24c42), which
turns on reqwest's `brotli` feature. **`brotli` 8.0.4 with
`brotli-decompressor` 5.0.3, `alloc-stdlib` 0.2.4 and `alloc-no-stdlib` 2.0.4
are acceptable as transitive `arazzo-runtime` dependencies, and `brotli` as a
direct `arazzo-runtime` dev-dependency** (tests encode their own fixtures; the
dev edge adds no package or feature beyond what the runtime edge locks).

| Signal | Evidence / consequence |
|---|---|
| Need | `compression-codecs` (see [async-compression](async-compression.md)) decodes `br` through this crate. It is the pure-Rust Brotli port used across the ecosystem |
| Version/MSRV | brotli 8.0.4 (2026-06-14, `rust-version = "1.59.0"`), brotli-decompressor 5.0.3 (2026-06-14), alloc-stdlib 0.2.4 (2026-06-14), alloc-no-stdlib 2.0.4 (2022-09-12); none yanked. Newer majors exist (brotli 9.1.3, brotli-decompressor 6.3.2); `compression-codecs` 0.4.38 requires brotli 8, so they are not proposed |
| License | brotli `BSD-3-Clause AND MIT`; brotli-decompressor `BSD-3-Clause/MIT`; alloc crates `BSD-3-Clause`. Permissive |
| Maintenance | Repositories under the dropbox GitHub organization (rust-brotli, rust-brotli-decompressor, rust-alloc-no-stdlib), active (releases 2026-11-15). Registry owners: danielrh on all four; brotli also SimonSapin and the `github:servo:cargo-publish` team. 300 to 340 million downloads each |
| Maintainer continuity | Every recent release of all four crates was published by danielrh, a current owner (alloc-stdlib 0.2.0 to 0.2.1 predate publisher recording). No owner added outside the project. No prior brief existed, so registry history stands in for it |
| Build execution | `build = false` in brotli and brotli-decompressor manifests; no `build.rs` in the alloc crates. No proc-macros, no native code, no network. The published `bin` targets are not built as dependencies |
| Dependencies | brotli: `alloc-no-stdlib`, `alloc-stdlib` (feature `std`), `brotli-decompressor`; optional `sha2` not enabled. Adds four packages to `Cargo.lock` |
| Vulnerabilities | OSV queries for crates.io `brotli`, `brotli-decompressor`, `alloc-stdlib`, `alloc-no-stdlib`: no records. `cargo audit` against the candidate lockfile: zero vulnerabilities (6 pre-existing allowed warnings, none naming these crates) |
| Unsafe | Uses `unsafe` in hot loops and allocator glue; `alloc-no-stdlib`'s `unsafe` feature (uninitialized allocation) is not enabled. Residual risk is a decoder memory-safety bug on hostile input, the main reason the decoded size cap matters |
| Trust/policy | Checksums: brotli `5cc91aac060a7a1e25823bdccbfb6af1875b88f17c6daac97894eed8207166b3`, brotli-decompressor `3a32acac15fe1967bc3986b2a6347dffc965602354ea6f450ad07e8bfd253583`, alloc-stdlib `0e76a019e91224d279006ff972f1e984179a6e9feb050adba6ce8274aef23195`, alloc-no-stdlib `cc7bb162ec39d46ab1ca8c77bf72e890535becd1751bb45f64c597edb4c8c6b3`. Checksum integrity, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Sources:
[registry](https://crates.io/api/v1/crates/brotli),
[owners](https://crates.io/api/v1/crates/brotli/owners),
[repository](https://github.com/dropbox/rust-brotli),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-reqwest-gzip-brotli-add](../../decisions/2026-11-17-reqwest-gzip-brotli-add.md).
