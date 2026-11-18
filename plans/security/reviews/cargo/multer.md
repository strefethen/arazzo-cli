# cargo:multer — multipart parser review (dev-dependency)

Reviewed 2026-11-17 for
[ac-e8d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e8d48), whose
plan requires multipart bodies to be "verified by parsing them back with an
independent multipart parser, not by string comparison". **Version 3.1.0 is
acceptable as a dev-dependency of `arazzo-runtime` and `arazzo-cli`**
(`multer = "3.1"`, default features off by default). It never reaches a
release binary.

| Signal | Evidence / consequence |
|---|---|
| Need | An independent `multipart/form-data` parser for hermetic tests. multer is the parser behind axum and Rocket; the alternatives (`multipart`, `mime-multipart`, `formdata`) are unmaintained or synchronous wrappers over the same ideas |
| Version/MSRV | 3.1.0, published 2024-05-04 by SergioBenitez, not yanked; no declared `rust-version`; builds on the pinned toolchain |
| License | `MIT`. Permissive |
| Maintenance | Registry owners SergioBenitez and rousan; repository rwf2/multer (Rocket organization). About 138 million downloads. Stable API since 3.0 |
| Maintainer continuity | Every 2.x and 3.x release was published by SergioBenitez, a current owner. No prior brief to diff against |
| Build execution | A five-line `build.rs` calls `version_check::is_feature_flaggable()` and only emits `cargo:rustc-cfg=nightly`; read in full from the downloaded artifact. `version_check 0.9.5` is already locked. No proc-macro, no network, `#![forbid(unsafe_code)]` |
| Dependencies | `bytes`, `encoding_rs`, `futures-util`, `http`, `httparse`, `memchr`, `version_check` (all already locked) plus two new packages: `mime 0.3.18` and `spin 0.9.9`. Optional `log`, `serde`, `tokio` features stay off |
| `mime 0.3.18` | Published 2026-11-05 by seanmonstar (sole owner, publisher of every release; hyperium/mime), 12 days before adoption, past the 7-day cooling-off. `MIT OR Apache-2.0`, `build = false`, no dependencies. Checksum `d5848f5a9e8117cf94f2f90faac24a87ff81600ff8628c1e3deaab572e76bfa1`. OSV: no records |
| `spin 0.9.9` | Published 2026-07-13 by zesterer (current owner, publisher of 24 releases); 0.9.0–0.9.8 are yanked. `MIT`, `build = false`, used with `spin_mutex` only. Checksum `3763264f6b73151db08c50ff20d7d8a0b8796e021cdea7ceedad07b80155fa0e`. OSV/RustSec: RUSTSEC-2023-0031 affects `>=0.9.3, <0.9.8` and RUSTSEC-2019-0013 affects `<0.5.2`; 0.9.9 is outside both |
| Vulnerabilities | OSV query for crates.io `multer`: no records. `cargo audit` against the updated lockfile: zero vulnerabilities (6 pre-existing allowed warnings, none naming multer, mime or spin) |
| Trust/policy | multer registry checksum `83e87776546dc87511aa5ee218730c92b666d7264ab6ed41f9d215af9cd5224b` matches the downloaded artifact and the lockfile. Checksum integrity, not signed publisher identity. No `supply-chain-policy.yml`, no cargo-vet ledger |
| Scorecard | Not collected |

Adoption notes:

- Test-only: `[dev-dependencies]` in `arazzo-runtime` and `arazzo-cli`. Feed the captured body as a one-item `futures_util::stream::once` and read `name()`, `file_name()`, `headers()` and `bytes()` per field.
- multer returns `name`/`filename` attribute text without percent-decoding, so tests assert the escaped `%22`/`%0D`/`%0A` forms.
- `arazzo-cli` gains a `futures-util` dev edge on the already-locked 0.3.32 for the same stream.

Sources:
[registry](https://crates.io/api/v1/crates/multer),
[owners](https://crates.io/api/v1/crates/multer/owners),
[repository](https://github.com/rwf2/multer),
[OSV](https://api.osv.dev/v1/query),
[RustSec](https://github.com/RustSec/advisory-db).
Decision log: [2026-11-17-multer-add](../../decisions/2026-11-17-multer-add.md).
