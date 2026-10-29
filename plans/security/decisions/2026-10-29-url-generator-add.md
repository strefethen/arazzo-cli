---
type: add
ecosystem: cargo
package: url
version_proposed: 2.5.8
decision: pending-user-approval
decision_at: 2026-10-29
decision_by: codex
lockfile_delta: proposed direct generator dependency edge only; retain package set and checksums
---

# Proposed generator dependency: existing `url` parser

[ac-72e07](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-72e07) asks Steve
to approve `url = "2"` for `arazzo-generate`, using the already-locked 2.5.8
artifact. This is the prerequisite for
[ac-923ea](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-923ea).
No dependency, lockfile or implementation changes have been made.

The recommendation is to approve the direct edge. Generation currently refuses
only slash-prefixed relative server URLs; the runtime also refuses `./v1` and
`api.example.test/v1`. Reusing `Url::parse` and its
`RelativeUrlWithoutBase` result aligns that existing limitation without a
second URI parser, a runtime dependency or broader URL validation. This parser
implements the WHATWG URL Standard; this repair does not introduce a complete
RFC 3986 validator or declare relative OpenAPI Server URLs invalid.

## Install-time evidence

| Check | Result | Evidence |
|---|---|---|
| Repository policy | WARN | `plans/security/supply-chain-policy.yml` is absent. |
| Review brief | WARN | `plans/security/reviews/cargo/url.md` is absent. |
| Cooling-off | PASS | 2.5.8 already appears in the root lockfile at [34a88b8](https://github.com/strefethen/arazzo-cli/commit/34a88b8dab323ac66046de92a81db179ee63b3c9), dated 2026-02-19. This is a lower bound on artifact age, not a claimed publication date. |
| Artifact trust | Unchanged | Existing checksum-backed crates.io artifact, already directly used by runtime and CLI; preserve checksum `ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed`. No package or version replacement is proposed. |
| Install scripts | PASS for this artifact | Local published Cargo manifest and locked offline metadata expose a library and tests, no custom-build target or build script. |
| Maintainer continuity | WARN | No prior review brief exists to compare maintainers. Local metadata identifies “The rust-url developers” and [servo/rust-url](https://github.com/servo/rust-url); no continuity claim is made. |
| Optional Socket query | Not run | No installation event yet; preserve query quota. Environment-token presence was checked without exposing its value. |

The local manifest declares MIT OR Apache-2.0 and Rust 1.63. Inspection used
`cargo metadata --locked --offline --format-version 1`, `Cargo.lock`, the cached
published `url-2.5.8/Cargo.toml.orig`, and the upstream repository. The registry
API was unavailable through the web tool; no registry publication date or
owner-history claim depends on that failed lookup. Evidence and the independent
architecture refinement are retained in
`/private/tmp/arazzo-maintainer-20261029/cycle19-server-diagnostic/`.

## Approval boundary

The `add-package` skill says: “Any warn or fail: do not install.” Missing policy,
missing review brief and unavailable maintainer comparison therefore require
Steve's approval. The existing
[iri-string decision](2026-07-26-iri-string-add.md) records the same explicit
approval for promoting a locked artifact to a direct dependency.

Approval would authorize only this direct edge and the bounded diagnostic
repair, with package-set/checksum preservation verified before review. It would
not authorize a package upgrade, new URL policy or any Kafka/AsyncAPI change.
