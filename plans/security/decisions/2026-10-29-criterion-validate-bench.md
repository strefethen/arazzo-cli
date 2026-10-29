---
type: add
ecosystem: cargo
package: criterion
version_proposed: 0.5.1
decision: approved
decision_at: 2026-10-29
decision_by: Steve
lockfile_delta: proposed validate dev-dependency edge only; retain package versions and checksums
---

# Reuse Criterion for the root validation benchmark

Recommend inheriting the existing workspace `criterion` dependency in
`arazzo-validate`'s dev-dependencies for the proposed
[root-load benchmark](../../completed/arazzo-root-load-benchmark.md). The workspace
declares version `0.5` with `html_reports`; the lockfile resolves 0.5.1. Expression
and runtime benchmarks already use that same declaration. Preserve its current
features, package set, versions and checksums. No production dependency, package
upgrade, general benchmark framework or runtime API change is proposed.

[ac-8b448](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8b448) holds the
explicit dependency decision. Steve accepted the six-family benchmark design and unit-by-unit planning in
[ac-0be12](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0be12), explicitly
leaving dependencies and thresholds as separate decisions. This proposal is
that narrow dependency decision. Steve explicitly answered “Approve Criterion
reuse (Recommended)” on 2026-10-29, acknowledging the recorded warnings.
Approval retains the exact package and feature bounds below. The planning task is
[ac-9f790](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9f790).

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | WARN | `plans/security/supply-chain-policy.yml` is absent. |
| Review brief | WARN | `plans/security/reviews/cargo/criterion.md` is absent. This bounded install check is not a substitute for a full review brief. |
| Cooling-off | PASS | The exact 0.5.1 version/checksum already appears in [da1affd](https://github.com/strefethen/arazzo-cli/commit/da1affda03530895c0c4c735bf132b0dadb1c26d), dated 2026-03-10. This proves a lower bound on repository artifact age; no registry publication date is claimed. |
| Artifact trust | Unchanged | Cached `.crate` SHA-256 exactly matches the existing lockfile checksum: `f2b12d017a929603d80db1831cd3a24082f8137ce19c69e6447f54f5fc8d692f`. No replacement artifact or newly resolved package is proposed. |
| Install scripts | PASS for Criterion itself | The cached published Cargo manifests declare no custom build script, and `criterion-0.5.1/build.rs` is absent. This does not certify every transitive dependency; the existing resolved graph must remain unchanged. |
| Maintainer continuity | WARN | There is no prior review baseline. The cached manifest names the original authors and the upstream repository, but does not establish current registry owners or continuity. The registry version and owners endpoints were unavailable through the web tool and returned HTTP403 through curl; no current-owner claim is made. |
| Optional Socket query | Not run | No installation occurred and no Socket lookup was needed for this bounded proposal. No credential contents were inspected. |

The local package declares `Apache-2.0 OR MIT`, edition2018, and the upstream
repository [bheisler/criterion.rs](https://github.com/bheisler/criterion.rs).
These are artifact metadata, not a claim about current project maintenance.
The existing default features are `rayon`, `plotters` and `cargo_bench_support`,
with workspace `html_reports` already enabled. Dependency installation remains
separate from this planning record.

Evidence was read from the root Cargo manifests/lockfile, the local published
Criterion0.5.1 manifests and archive, and the historical lockfile. The packet is
`/private/tmp/arazzo-maintainer-20261029/cycle26-root-benchmark/`.

## Approval boundary

The `add-package` skill requires: “Any warn or fail: do not install.” Missing
policy, missing review brief and unverified maintainer continuity therefore
require Steve's explicit approval. The benchmark-design acceptance separately
reserved this decision. Steve's explicit approval now clears this narrow gate;
the warnings remain recorded rather than being represented as passing checks.

Approval covers only the validator dev-dependency on the already-locked version
and existing feature set. The implementation must demonstrate that all package
versions, checksums and unrelated dependency edges remain unchanged; any wider
resolution change returns for assessment. Approval does not authorize a version
upgrade, performance threshold, source-loading benchmark, OS measurement or CI
publication. The new root benchmark still needs its grounded unit ticket and
normal verification/review before completion.
