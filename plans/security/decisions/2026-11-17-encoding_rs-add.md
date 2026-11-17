---
type: add
ecosystem: cargo
package: encoding_rs
version_proposed: 0.8.35
decision: pass
decision_at: 2026-11-17
decision_by: claude
lockfile_delta: direct arazzo-runtime edge only; package set and checksums unchanged
---

# Proposed runtime dependency: existing `encoding_rs` decoder

[ac-88a70](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-88a70) decodes
`$response.body` by the response's `charset` parameter and fails closed on an
unknown label or invalid bytes. That needs a WHATWG Encoding Standard label
table and decoders. The recommendation is to approve `encoding_rs = "0.8"` as a
direct `arazzo-runtime` dependency, using the already-locked 0.8.35 artifact.

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | Gap (not a warning) | `plans/security/supply-chain-policy.yml` is absent; default thresholds applied. |
| Review brief | PASS | `plans/security/reviews/cargo/encoding_rs.md` written 2026-11-17 (review-package). |
| Cooling-off | PASS | 0.8.35 with checksum `75030f3c4f45dafd7586dd6780965a8c7e8e285a5ecb86713e63a79c5b2766f3` has been in the root lockfile since 3a0b580 (2026-03-12), a lower bound on artifact age. |
| Artifact trust | Unchanged | Same checksum-backed crates.io artifact already built into the workspace through `yaml-rust2 0.11.0` (`arazzo-debug-adapter`). Its only non-optional dependency, `cfg-if`, is already locked. No package or version change. |
| Install scripts | PASS | Published manifest sets `build = false`; no build script, no native code. |
| Maintainer continuity | PASS | Registry owner list is hsivonen alone and every published version (0.8.x history) was published by that account; no owner added or removed. Detail in the review brief. |
| Optional Socket query | Not run | No token in the agent environment; no install event. |

License per manifest: `(Apache-2.0 OR MIT) AND BSD-3-Clause` (the BSD-3-Clause
part covers WHATWG-derived index data), Rust 1.36.

## Verdict

PASS on every check except the optional Socket query, which was not run.
`cargo audit` reports zero vulnerabilities and OSV has no records for the
crate. The direct `arazzo-runtime` edge on the locked 0.8.35 artifact may be
installed with package set and checksums preserved; no manifest or lockfile
edit has been made by this review.

## Install

Installed 2026-11-17 with ac-88a70 by adding `encoding_rs = "0.8"` to
`crates/arazzo-runtime/Cargo.toml` by hand (no `cargo add`, no `cargo update`).
The `Cargo.lock` delta is the single `"encoding_rs"` entry in
`arazzo-runtime`'s dependency list; the locked package stays 0.8.35 with the
checksum above. Decoding uses `Encoding::for_label_no_replacement` and
`decode_without_bom_handling_and_without_replacement`, so malformed input is an
error rather than U+FFFD.
