# cargo:getrandom — OS randomness review

Reviewed 2026-11-17 for
[ac-e8d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e8d48), which
gives every `multipart/form-data` request attempt a fresh random boundary.
**Version 0.2.17 (the artifact already locked) is acceptable for adoption as a
direct `arazzo-runtime` dependency `getrandom = "0.2"`.** It is already
compiled into `arazzo-runtime`'s normal build through `ring 0.17.14` (rustls)
and `rand_core 0.6.4` (rskafka); this review covers promoting it to a direct
edge, with no package or checksum change.

| Signal | Evidence / consequence |
|---|---|
| Need | A boundary must be unpredictable per attempt. The standard library has no public randomness API; seeding from `RandomState` is not a randomness contract. `getrandom` is the OS-entropy crate every Rust CSPRNG builds on |
| Version choice | Three versions are locked: 0.2.17 (normal graph, via ring and rand 0.8), 0.3.4 (tests only, via tempfile and rand 0.9) and 0.4.2 (rand 0.10). 0.2.17 is the only one already linked into the shipped runtime, so choosing it adds nothing to release binaries |
| Version/MSRV | 0.2.17, published 2026-01-11 by newpavlov, not yanked; builds on the workspace MSRV 1.88 today (already in the build) |
| License | `MIT OR Apache-2.0`. Permissive |
| Maintenance | Registry owners dhardy and the `github:rust-random:maintainers` team; repository rust-random/getrandom, active (0.4.x line current) |
| Maintainer continuity | 0.2.17 was published by newpavlov, a rust-random maintainer. No prior brief existed to diff against, so the registry history stands in for it |
| Build execution | `build = false` in the published manifest; no build script, no proc-macro, no network. Default features only (none enabled by this edge) |
| Dependencies | `cfg-if`, `libc`, and wasm/wasi target-only crates; all already locked. Adds zero packages to `Cargo.lock` |
| Vulnerabilities | OSV query for crates.io `getrandom`: no records. `cargo audit` against the workspace lockfile: zero vulnerabilities (6 allowed warnings, none naming getrandom) |
| Unsafe | FFI to the OS entropy syscall (`getentropy` on macOS, `getrandom` on Linux). Workspace code stays `unsafe`-free |
| Trust/policy | Registry checksum `ff2abc00be7fca6ebc474524697ae276ad847ad0a6b3faa4bcb027e9a4614ad0` matches the downloaded artifact and the lockfile, which has carried it since 34a88b8 (2026-02-19). No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Adoption notes:

- Call `getrandom::getrandom(&mut [u8; 16])` once per assembled body and hex the result; a failure is a `RuntimeError`, never a fallback boundary.
- Leave the lockfile's resolution alone: adding the edge must not move `tempfile` to 0.4.2 (cargo tried; the lockfile edge was restored to 0.3.4).

Sources:
[registry](https://crates.io/api/v1/crates/getrandom),
[owners](https://crates.io/api/v1/crates/getrandom/owners),
[repository](https://github.com/rust-random/getrandom),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-getrandom-add](../../decisions/2026-11-17-getrandom-add.md).
