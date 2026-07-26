---
type: add
ecosystem: cargo
package: iri-string
version_added: 0.7.12
decision: approved
decision_at: 2026-07-26T04:58:00Z
decision_by: codex
review_brief: plans/security/reviews/cargo/iri-string.md
evidence_bundle: plans/security/evidence/cargo/iri-string-add.json
lockfile_delta: direct dependency edge only; package set unchanged
ghsa_refs: []
dependabot_alerts: []
dependency_review: unavailable
overrides: []
skill_version: v0.1.0
apis_verified:
  - local-cargo-metadata
  - docs.rs
---

# Decision: approved - iri-string

## Reasoning
Approved after explicit user confirmation. The ticket needs RFC 3986 URI-reference validation, including relative references and fragment inspection. `iri-string` 0.7.12 provides that exact primitive and is already present in `Cargo.lock` through `reqwest` -> `tower-http`, so promoting it to a direct `arazzo-validate` dependency adds no new locked package or transitive package set.

## Evidence consulted
- Repository supply-chain policy: missing; no repository-specific approval policy was available.
- Review brief: missing at `plans/security/reviews/cargo/iri-string.md`.
- Cooling-off: allow; 0.7.12 was published 2026-03-29, more than seven days before review.
- Trust evidence: unchanged checksum-backed crates.io artifact already present in `Cargo.lock` with checksum `25e659a4bb38e810ebc252e53b5814ff908a8c58c2a9ce2fae1bbec24cbf4e20`.
- Install scripts: none; the published manifest declares a library target and optional `memchr`/`serde` dependencies, with no build script.
- Maintainer continuity: no prior review brief exists; published metadata identifies the existing `lo48576/iri-string` project and author YOSHIOKA Takuma.
- Socket.dev quick check: unavailable because the cache entry could not be parsed; shared cache state was not modified.
- Dependency delta: one direct dependency edge in the lockfile; the package set is unchanged because `cargo tree -i iri-string --locked` already showed 0.7.12 used by `tower-http` through `reqwest`.

## Machine-readable evidence
- Evidence bundle: plans/security/evidence/cargo/iri-string-add.json
- Lockfile delta: direct dependency edge only; package set unchanged

## Override decisions
None
