---
type: add
ecosystem: cargo
package: getrandom
version_proposed: 0.2.17
decision: pass
decision_at: 2026-11-17
decision_by: claude
lockfile_delta: direct arazzo-runtime edge only; package set and checksums unchanged
---

# Proposed runtime dependency: existing `getrandom` 0.2

[ac-e8d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e8d48) gives each
`multipart/form-data` request attempt a fresh random boundary, which needs OS
randomness. The recommendation is to approve `getrandom = "0.2"` as a direct
`arazzo-runtime` dependency on the already-locked 0.2.17 artifact, the version
ring already links into the runtime.

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | Gap (not a warning) | `plans/security/supply-chain-policy.yml` is absent; default thresholds applied. |
| Review brief | PASS | `plans/security/reviews/cargo/getrandom.md` written 2026-11-17. |
| Cooling-off | PASS | 0.2.17 was published 2026-01-11 and has been in the root lockfile since 34a88b8 (2026-02-19). |
| Artifact trust | Unchanged | Same checksum-backed crates.io artifact already built into `arazzo-runtime` through `ring` and `rand_core 0.6`. No package or version change. |
| Install scripts | PASS | `build = false`; no build script, no proc-macro. |
| Maintainer continuity | PASS | Published by newpavlov of the rust-random maintainers team, which owns the crate. |
| Optional Socket query | Not run | No token in the agent environment. |

## Verdict

PASS on every check except the optional Socket query, which was not run.
`cargo audit` reports zero vulnerabilities and OSV has no records for the crate.

## Install

Installed 2026-11-17 with ac-e8d48 by adding `getrandom = "0.2"` to
`crates/arazzo-runtime/Cargo.toml` by hand. Cargo's re-resolution also moved
`tempfile`'s edge from getrandom 0.3.4 to 0.4.2; that unrelated edge was
restored by hand and `cargo metadata --locked` confirmed the lockfile. The
`Cargo.lock` delta for this package is the single `"getrandom 0.2.17"` entry
in `arazzo-runtime`'s dependency list.
