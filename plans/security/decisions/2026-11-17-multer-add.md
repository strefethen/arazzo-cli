---
type: add
ecosystem: cargo
package: multer
version_proposed: 3.1.0
decision: pass
decision_at: 2026-11-17
decision_by: claude
lockfile_delta: adds multer 3.1.0, mime 0.3.18, spin 0.9.9 (dev-only); futures-util dev edge in arazzo-cli on the locked 0.3.32
---

# Proposed dev-dependency: `multer` multipart parser

[ac-e8d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e8d48) must verify
assembled `multipart/form-data` bodies with an independent parser. The
recommendation is to approve `multer = "3.1"` as a dev-dependency of
`arazzo-runtime` and `arazzo-cli`.

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | Gap (not a warning) | `plans/security/supply-chain-policy.yml` is absent; default thresholds applied. |
| Review brief | PASS | `plans/security/reviews/cargo/multer.md` written 2026-11-17, covering the two new transitive packages. |
| Cooling-off | PASS | multer 3.1.0: 2024-05-04. spin 0.9.9: 2026-07-13. mime 0.3.18: 2026-11-05, 12 days old, past the 7-day threshold for a non-security addition. |
| Artifact trust | New, checksum-verified | Downloaded artifact checksums match the lockfile for multer, mime and spin. Dev-only: nothing reaches a release binary. |
| Install scripts | PASS | multer's `build.rs` (read in full) only emits `cargo:rustc-cfg=nightly` from `version_check`; mime and spin set `build = false`. No proc-macros. |
| Maintainer continuity | PASS | multer: every release by SergioBenitez (owner). mime: every release by seanmonstar (sole owner). spin 0.9.9: zesterer (owner, 24 releases). |
| Optional Socket query | Not run | No token in the agent environment. |

## Verdict

PASS on every check except the optional Socket query, which was not run.
`cargo audit` on the updated lockfile reports zero vulnerabilities; the spin
advisories (RUSTSEC-2023-0031, RUSTSEC-2019-0013) exclude 0.9.9.

## Install

Installed 2026-11-17 with ac-e8d48 by adding `multer = "3.1"` under
`[dev-dependencies]` of `crates/arazzo-runtime/Cargo.toml` and
`crates/arazzo-cli/Cargo.toml` (the latter also `futures-util = "0.3"`, already
locked), by hand, with no `cargo update`.
