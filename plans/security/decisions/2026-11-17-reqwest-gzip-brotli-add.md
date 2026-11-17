---
type: add
ecosystem: cargo
package: reqwest features gzip, brotli (async-compression 0.4.42, compression-codecs 0.4.38, compression-core 0.4.32, brotli 8.0.4, brotli-decompressor 5.0.3, alloc-stdlib 0.2.4, alloc-no-stdlib 2.0.4)
version_proposed: reqwest 0.12.28 features gzip, brotli
decision: pass
decision_at: 2026-11-17
decision_by: claude
lockfile_delta: seven new packages; tower-http 0.6.8 gains async-compression, futures-core, http-body-util, tokio, tokio-util edges; arazzo-runtime gains dev edges flate2 and brotli; no other package or edge changes
---

# Proposed runtime change: reqwest gzip and brotli decoding

[ac-24c42](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-24c42) decodes
gzip- and brotli-encoded responses in the transport, before the media-type
table runs, and keeps the decoded bytes under `--max-response-size`. The
recommendation is to enable reqwest's `gzip` and `brotli` features in
`arazzo-runtime`, which adds seven packages, and to add `flate2 = "1"` and
`brotli = "8"` as dev-dependencies so the hermetic tests can encode their own
fixtures.

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | Gap (not a warning) | `plans/security/supply-chain-policy.yml` is absent; default thresholds applied. |
| Review brief | PASS | `plans/security/reviews/cargo/async-compression.md` (async-compression, compression-codecs, compression-core) and `plans/security/reviews/cargo/brotli.md` (brotli, brotli-decompressor, alloc-stdlib, alloc-no-stdlib), written 2026-11-17. |
| Cooling-off | PASS | Newest artifact is alloc-stdlib 0.2.4 / brotli 8.0.4 (2026-06-14), five months old; the async-compression trio is from 2026-04-24; alloc-no-stdlib from 2022-09-12. All past the 7-day window. |
| Artifact trust | New packages, checksum-pinned | Seven new crates.io artifacts with checksums recorded in the briefs. `flate2` 1.1.10 (the gzip backend, and the flate2 dev edge) is already locked; the brotli dev edge uses the same 8.0.4 artifact the runtime edge locks. |
| Install scripts | PASS | `build = false` in every manifest that declares a build key; no `build.rs` in the alloc crates; no proc-macros or native code. |
| Maintainer continuity | PASS | async-compression family: trusted publishes from `Nullus157/async-compression` and releases by current owners. brotli family: every recorded release by danielrh, a current owner. |
| Optional Socket query | Not run | No token in the agent environment. |

`cargo tree -p arazzo-runtime -e features`: `reqwest/gzip` and `reqwest/brotli`
enable `tower-http` `decompression-gzip` and `decompression-br`, which enable
`async-compression` features `gzip`/`brotli` and `compression-codecs`
`flate2`/`brotli`; `brotli` resolves with its default `std` feature. `deflate`
and `zstd` stay off.

## Verdict

PASS on every check except the optional Socket query, which was not run.
`cargo audit` against the candidate lockfile reports zero vulnerabilities (6
pre-existing allowed warnings), and OSV has no records for any of the seven
crates.

## Install

Installed 2026-11-17 with ac-24c42 by editing
`crates/arazzo-runtime/Cargo.toml` by hand: `"gzip", "brotli"` appended to the
reqwest feature list and `flate2 = "1"`, `brotli = "8"` added under
`[dev-dependencies]`. A fresh index resolves the newest releases instead
(async-compression 0.4.52, compression-codecs 0.4.47 with brotli 9.1.3 and
brotli-decompressor 6.3.2, published 2026-11-15, inside the 7-day cooling-off
window), so the three async-compression crates were pinned to the reviewed
versions with `cargo update -p <crate> --precise`, and unrelated edge churn the
re-resolution introduced (`windows-sys` and `getrandom` edges of existing
packages) was reverted so the delta is additions only; `cargo metadata
--locked` accepts the result. No blanket `cargo update` was run.
