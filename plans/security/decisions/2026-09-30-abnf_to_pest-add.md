---
type: add
ecosystem: cargo
package: abnf_to_pest
version_added: 0.6.0
decision: approved
decision_at: 2026-10-01T21:46:49Z
decision_by: Steve (explicit approval; recorded by Codex)
previous_decision: deferred
previous_decision_at: 2026-09-30T21:42:00Z
previous_decision_by: claude
review_brief: plans/security/reviews/cargo/abnf_to_pest.md
evidence_bundle: inline (see Evidence consulted)
lockfile_delta: direct build-dependency edge only (arazzo-expr build -> abnf_to_pest); package set unchanged
ghsa_refs: []
dependabot_alerts: []
dependency_review: unavailable
overrides: []
skill_version: v0.1.0
apis_verified:
  - local-cargo-registry-sources
  - crates.io-api
  - cargo-audit-offline
  - github-dependabot-alerts
---

# Decision: approved - abnf_to_pest

## Approval
On 2026-10-01 Steve explicitly approved promoting the existing locked
`abnf_to_pest` build dependency at `=0.6.0` for the parser work. This approval
changes the prior deferred disposition; the evidence below remains the
September 30 review and has not been represented as a fresh security scan.
Scope is the direct build-dependency edge at this exact pin with no new locked
packages.

## Reasoning
This is a request to promote `abnf_to_pest` 0.6.0 from a transitive build dependency to a direct
`[build-dependencies]` entry of `crates/arazzo-expr`. `crates/arazzo-expr/build.rs` would use it to
convert the committed Arazzo 1.1 §5.9 ABNF into pest rules at build time, following the existing
`iregexp-rs` `build.rs`. It is build-time only and never linked into the shipped binaries. The crate
and its closure (`abnf` 0.13.0, `abnf-core` 0.5.0, `nom` 7.1.3, `indexmap` 1.9.3, `itertools` 0.10.5,
`pretty` 0.11.3, `arrayvec` 0.5.2, `typed-arena` 2.0.2) already resolve in `Cargo.lock` through
`iregexp-rs`'s own `[build-dependencies]` at the same pin.

Recommendation: **approve at `=0.6.0`**, matching the `iregexp-rs` pin.

## Evidence consulted
- Repository supply-chain policy: missing (`plans/security/supply-chain-policy.yml` does not exist). This is recorded, not treated as approval.
- Review brief: missing at `plans/security/reviews/cargo/abnf_to_pest.md` (a loud warning, not a block).
- Cooling-off: allow. `abnf_to_pest` 0.6.0 was published 2026-07-28. `abnf` 0.13.0 (2022-10-21) and `abnf-core` 0.5.0 (2021-07-16) are long-settled. None are yanked.
- Trust evidence: unchanged. The crates.io checksums equal the `Cargo.lock` entries: abnf_to_pest `25d0b038…`, abnf `087113bd…`, abnf-core `c44e09c4…`.
- Install/build-time code: none of `abnf_to_pest`, `abnf`, `abnf-core`, or `nom` has a `build.rs` or is a proc-macro. `indexmap` 1.9.3 has an autocfg probe `build.rs`, which already runs in this workspace's builds.
- Maintainer continuity: `abnf_to_pest` is owned by `Nadrieril` (who also published 0.6.0) and `levitte`. `abnf` and `abnf-core` are owned by `duesee`, a single maintainer. There is no prior brief to diff against.
- License: `MIT OR Apache-2.0` for abnf_to_pest, abnf, and abnf-core. The rest of the closure is MIT or MIT/Apache-2.0.
- Advisories: `cargo audit --no-fetch` (advisory DB as of 2026-09-14) reports 0 vulnerabilities in the workspace.
- Dependabot: 0 open alerts in `strefethen/arazzo-cli`.
- Socket.dev quick check: skipped. There is no cached `socket-v2/cargo` entry, and the check is optional.

## Machine-readable evidence
- Evidence bundle: inline (see Evidence consulted)
- Lockfile delta: direct build-dependency edge only (arazzo-expr build -> abnf_to_pest); package set unchanged

## Override decisions
None
