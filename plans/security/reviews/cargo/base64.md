# cargo:base64 — Base64 encoding review

Reviewed 2026-11-17 for
[ac-bdfbc](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bdfbc), which
converts an `--input-file` to a base64 or base64url string for an input whose
schema is `type: string` with that `contentEncoding`. **Version 0.22.1 (the
artifact already locked) is acceptable for adoption as a direct `arazzo-cli`
dependency `base64 = "0.22"`.** It is already compiled into the workspace
through `reqwest 0.12.28`, `hyper-util 0.1.20` and `rsasl 2.3.1`; this review
covers promoting it to a direct edge, with no package or checksum change.

| Signal | Evidence / consequence |
|---|---|
| Need | OAS 3.2 §4.24.4.3 names `contentEncoding` `base64` or `base64url` for encoded binary. The standard library has no Base64; hand-rolling the RFC 4648 alphabets is the riskier path |
| Version/MSRV | 0.22.1, published 2024-04-30 by marshallpierce, not yanked, `rust-version = "1.48.0"`, edition 2018. Latest is 0.23.1; that line would add a second package to the lockfile, so it is not proposed |
| License | `MIT OR Apache-2.0`. Permissive |
| Maintenance | Registry owners marshallpierce and alicemaz; repository marshallpierce/rust-base64, crate updated 2026-08-04. About 2.0 billion downloads |
| Maintainer continuity | 0.22.1 was published by marshallpierce, a current owner. No prior brief existed to diff against |
| Build execution | No `build.rs`, no proc-macros. Default features `std` → `alloc`; no optional dependencies |
| Dependencies | None (dev-dependencies only). Adds zero packages to `Cargo.lock` |
| Vulnerabilities | OSV query for crates.io `base64`: GHSA-x67x-vg9m-65c3 / RUSTSEC-2017-0004, fixed in 0.5.2; 0.22.1 is outside the range. `cargo audit` against the workspace lockfile: no vulnerability names base64 |
| Unsafe | `#![forbid(unsafe_code)]` in `src/lib.rs` |
| Trust/policy | Registry checksum `72b3254f16251a8381aa12e40e3c4d2f0199f8c6508fbecb9d91f575e0fbb8c6` unchanged since the root lockfile first carried the package (34a88b8, 2026-02-19). Checksum integrity, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger; absence is not approval |
| Scorecard | Not collected |

Adoption notes:

- Use `engine::general_purpose::STANDARD` for `base64` and `URL_SAFE` (padded, matching the OAS 3.2 base64url example that keeps `=` padding) for `base64url`.
- Pin to the locked artifact; do not run a blanket `cargo update`.

[ac-ab757](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ab757)'s broader
replay-evidence Base64 qualification is still open; this review does not
settle its alphabet/size-ledger questions.

Sources:
[registry](https://crates.io/api/v1/crates/base64),
[owners](https://crates.io/api/v1/crates/base64/owners),
[repository](https://github.com/marshallpierce/rust-base64),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-base64-add](../../decisions/2026-11-17-base64-add.md).
