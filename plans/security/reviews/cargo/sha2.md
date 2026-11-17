# cargo:sha2 — SHA-256 digest review

Reviewed 2026-11-17 for
[ac-23360](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-23360), which
records a bytes response body in `trace.v2` as a descriptor carrying its
SHA-256. **Version 0.10.9 (the artifact already locked) is acceptable for
adoption as a direct `arazzo-runtime` dependency `sha2 = "0.10"`.** It is
already compiled into `arazzo-runtime` through `rsasl 2.3.1` (`rskafka`, which
the runtime builds unconditionally); this review covers promoting it to a
direct edge, with no package or checksum change.

| Signal | Evidence / consequence |
|---|---|
| Need | The plan's bytes descriptor (`{kind: "bytes", size, sha256, ...}`) needs a SHA-256 digest. The standard library has none; hand-rolling a hash is the riskier path. `ring` and `aws-lc-rs` are heavier native builds; `sha2` is the RustCrypto reference implementation already in the graph |
| Version/MSRV | 0.10.9, published 2025-04-30 by newpavlov, not yanked, no declared `rust-version`, edition 2018; builds on the workspace MSRV 1.88 today (already in the build). Latest is 0.11.1 (2026-10-12, Rust 1.88); 0.11 is a breaking `digest` 0.11 line and would add packages, so it is not proposed |
| License | `MIT OR Apache-2.0`. Permissive |
| Maintenance | Registry owners tarcieri, newpavlov and the `github:rustcrypto:hashes` team; repository RustCrypto/hashes, active (0.11.1 published 2026-10-12). About 1.25 billion downloads |
| Maintainer continuity | 0.10.9 was published by newpavlov, a current owner; 0.11 pre-releases by tarcieri, also a current owner. No owner outside the RustCrypto organization. No prior brief existed to diff against, so the registry history stands in for it |
| Build execution | `build = false` in the published manifest. Default features `std` only; `asm` (native `sha2-asm`) is opt-in and not enabled in this lockfile. No proc-macros, no network |
| Dependencies | Non-optional: `cfg-if`, `digest` 0.10.7, and `cpufeatures` 0.2.17 on x86/aarch64; all already locked. Adds zero packages to `Cargo.lock` |
| Vulnerabilities | OSV query for crates.io `sha2`: one advisory, RUSTSEC-2021-0100 / CVE-2021-45696 (miscomputed AVX2 results), affected `>=0.9.7, <0.9.8`; 0.10.9 is outside the range. `cargo audit` against the workspace lockfile: zero vulnerabilities (6 allowed warnings, none naming sha2) |
| Unsafe | Uses `unsafe` for SIMD intrinsics and runtime CPU feature detection (`cpufeatures`). `arazzo-cli` forbids `unsafe` only in workspace code; residual risk is a backend miscomputation of the RUSTSEC-2021-0100 kind, covered by the crate's published test vectors |
| Trust/policy | Registry checksum `a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283` unchanged since the root lockfile first carried it (83bee8e, 2026-10-27). Checksum integrity, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Adoption notes:

- Use `Sha256::digest(bytes)` and lowercase-hex the 32-byte output; no streaming is needed while response bodies are buffered whole.
- Pin to the locked artifact by leaving the lockfile untouched; do not run a blanket `cargo update`.

Sources:
[registry](https://crates.io/api/v1/crates/sha2),
[owners](https://crates.io/api/v1/crates/sha2/owners),
[repository](https://github.com/RustCrypto/hashes),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-sha2-add](../../decisions/2026-11-17-sha2-add.md).
