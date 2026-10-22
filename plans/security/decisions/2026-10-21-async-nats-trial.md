---
type: add
ecosystem: cargo
package: async-nats
version_proposed: 0.50.0
decision: approved
decision_at: 2026-10-21
ticket: ac-ee786
---

# Dependency decision: local NATS trial

Proposed addition: `async-nats = { version = "=0.50.0", default-features = false }`
under `crates/arazzo-runtime/Cargo.toml` dev-dependencies only. If needed for
`Subscriber::next`, expose the already-locked `futures-util` as a dev-dependency.
No production dependency or workspace version change is proposed.

## Bounded install-time review

- PASS — cooling-off: published July 20, 2026, more than seven days ago; not
  yanked. License Apache-2.0. Declared Rust version 1.88.0 matches repository MSRV.
- PASS — artifact identity: crates.io names the official `nats-io/nats.rs`
  repository and publisher Jarema (Tomasz Pietrek). Registry SHA-256:
  `d83a251fa1a4c9d0fe6e816b7acd60549e473e08d14f27a1d992c2675abff05f`.
- PASS (direct crate only) — published source lists no `build.rs`. Default
  features are disabled to avoid unused JetStream/service/websocket features.
- WARN — `plans/security/supply-chain-policy.yml` is absent.
- WARN — no previous `async-nats` review brief exists in this repository.
- WARN — maintainer continuity has no prior local baseline for comparison.
- WARN — transitive build scripts have not yet been verified against a resolved
  trial dependency graph. Dependency resolution and MSRV compilation remain
  mandatory implementation checks; this review does not claim those passed.
- SKIPPED — optional Socket check; no credential lookup performed.

Evidence: [registry metadata](https://crates.io/api/v1/crates/async-nats/0.50.0),
[published manifest](https://docs.rs/crate/async-nats/0.50.0/source/Cargo.toml.orig),
[published source](https://docs.rs/crate/async-nats/0.50.0/source/).

Steve explicitly approved this test-only dependency addition on October 21,
2026: “Approved. Add the test-only dependency and continue implementing
ac-ee786 with the sub-agents.” This clears the reported warnings for the bounded
trial. Dependency-graph and MSRV verification still apply during implementation.

## Implementation constraints

The [versioned connection documentation](https://docs.rs/async-nats/0.50.0/async_nats/struct.ConnectOptions.html#method.max_reconnects)
defines zero reconnect attempts as unlimited. The trial must explicitly bound
connection lifetime and treat first loss as terminal, rather than relying on zero.
The official NATS Server release artifact and checksum must be verified before
use; this dependency decision does not certify a server binary.
