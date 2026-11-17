# cargo:async-compression — streaming gzip/brotli decoder review

Reviewed 2026-11-17 for
[ac-24c42](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-24c42), which
turns on reqwest's `gzip` and `brotli` features so encoded responses are decoded
before the response size cap counts them. **`async-compression` 0.4.42 with
`compression-codecs` 0.4.38 and `compression-core` 0.4.32 are acceptable as
transitive `arazzo-runtime` dependencies.** No workspace crate depends on them
directly; reqwest 0.12.28 pulls them through `tower-http` 0.6.8 (already locked)
features `decompression-gzip` and `decompression-br`.

| Signal | Evidence / consequence |
|---|---|
| Need | reqwest's `gzip`/`brotli` features are its only content-decoding path, and they are implemented by this crate family. Hand-rolling decoding in `client.rs` is out of plan (D6) |
| Version/MSRV | async-compression 0.4.42 (2026-04-24), compression-codecs 0.4.38 (2026-04-24), compression-core 0.4.32 (2026-04-24); none yanked; `rust-version = "1.83"`, below the workspace 1.88. Latest are 0.4.52 / 0.4.47 / 0.4.34 (2026-11); the resolver chose the older trio from the cached index, and no blanket `cargo update` is proposed |
| License | `MIT OR Apache-2.0` on all three. Permissive |
| Maintenance | Repository Nullus157/async-compression, active (releases through 2026-11-02). Registry owners Nemo157, yoshuawuyts, robjtede, NobodyXu (the codec/core crates omit Nemo157). About 278 million / 140 million / 138 million downloads |
| Maintainer continuity | Every release from compression-core 0.4.28 onward, and all recent async-compression and compression-codecs releases, are crates.io trusted publishes from the `Nullus157/async-compression` repository workflow; 0.4.27 of compression-core was published by NobodyXu, a current owner. No owner outside the project. No prior brief existed, so registry history stands in for it |
| Build execution | `build = false` in all three published manifests. No proc-macros, no native code, no network |
| Dependencies | `compression-codecs` features `brotli` and `flate2` (`flate2` 1.1.10 already locked); `memchr`, `pin-project-lite`, `tokio` already locked. New packages: these three plus the brotli family (see [brotli](brotli.md)) |
| Vulnerabilities | OSV queries for crates.io `async-compression`, `compression-codecs`, `compression-core`: no records. `cargo audit` against the candidate lockfile: zero vulnerabilities (6 pre-existing allowed warnings, none naming these crates) |
| Unsafe | A few `unsafe` blocks (buffer initialization helpers). `arazzo-cli` forbids `unsafe` only in workspace code |
| Trust/policy | Checksums: async-compression `e79b3f8a79cccc2898f31920fc69f304859b3bd567490f75ebf51ae1c792a9ac`, compression-codecs `ce2548391e9c1929c21bf6aa2680af86fe4c1b33e6cea9ac1cfeec0bd11218cf`, compression-core `cc14f565cf027a105f7a44ccf9e5b424348421a1d8952a8fc9d499d313107789`. Checksum integrity plus trusted-publishing provenance, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Adoption notes:

- Enable only reqwest's `gzip` and `brotli`; `deflate` and `zstd` stay off (zstd carries the advisories recorded in `crates/arazzo-runtime/Cargo.toml`).
- Decoded output is still read through the runtime's `max_response_bytes` chunk loop, which bounds decompression bombs.

Sources:
[registry](https://crates.io/api/v1/crates/async-compression),
[owners](https://crates.io/api/v1/crates/async-compression/owners),
[repository](https://github.com/Nullus157/async-compression),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-reqwest-gzip-brotli-add](../../decisions/2026-11-17-reqwest-gzip-brotli-add.md).
