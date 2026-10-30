# Arazzo release candidate — 2026-10-29

**Disposition: PREPARED / HOLD.** CLI **0.9.0** and VSIX **0.0.10** are the
preparation targets. The committed source baseline is
`345e652438fdf8256edc40937e20b251cf6eef5f`; native artifact verification below passed.
Preparation does not authorize a tag, GitHub release, Marketplace publication,
or acceptance of the remaining security and correctness risks.

This assessment reconciles the [October 18 readiness report](arazzo-0.9.0-release-readiness-2026-10-18.md)
against committed source and current ticket evidence. The historical report is
preserved. The unrelated, incomplete schema-walker working-tree changes are
excluded from the candidate and every delivery claim here. Hourly maintainer
runs remain paused. This assessment introduces no production changes or new
implementation authority.

## Blocker reconciliation

| Finding | Current evidence and owner | Disposition |
|---|---|---|
| RustSec failures | Committed `Cargo.lock` resolves rustls **0.23.46**. [ac-d3602](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d3602) closed at ancestor `be99969`, recording unsuppressed canonical preflight, TLS tests and independent review PASS. | The original four rustls advisories are addressed. Fresh candidate audit remains part of artifact verification. |
| L1: disconnect at successive debug checkpoints | [ac-5eb26](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5eb26) closed at ancestor `53fa38a`. `debug/controller.rs::gate_step` checks invocation cancellation before and during the wait. The ticket records real-process criterion/output breakpoint regression proof, positive controls and independent review PASS. | Resolved in committed source; packaged VSIX acceptance is a separate verification step. |
| L2: EOF while paused | The same ticket delivers unconditional session cleanup after the DAP coordinator exits, including EOF and transport failures. Tests cover paused EOF, completed-session controls and original error preservation. | Resolved in committed source. |
| L3: form mapping sent as JSON | `runtime_core/engine_http.rs::prepare_http_request` still passes only non-JSON strings through; mappings reach `serde_json::to_vec`. [ac-9394b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9394b) closed as an accepted assessment, not implementation. | Remains. Implement the accepted encoding contract or obtain explicit known-defect release deferral. |
| L4: unresolved live values alter request targets | `arazzo-expr/src/resolution.rs::into_public` still projects missing values to null; `runtime_core/payload.rs::value_to_string` maps null to empty text; HTTP path/query construction substitutes that text or omits null queries. [ac-704a6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-704a6) closed as an accepted assessment. | Remains a CLI/runtime safety blocker. No delivered live-request admission mechanism was found. |
| L5: credential-name gaps | [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51), closed at ancestor `8768a20`, supplies separator normalization and schema-declared input secrets. Committed `redaction.rs` still lacks the approved additional exact names and matches raw query names without percent-decoding. [ac-2406c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2406c) closed as an accepted assessment. | Partly resolved. `api_key`/schema-secret coverage is delivered; `X-Auth-Key` and the accepted vocabulary/encoded-name extension remain outstanding. |
| L6: MCP file errors expose content | `arazzo-mcp/src/state.rs::check_path_allowed` still permits reads when roots are absent; `describe_openapi` returns downstream parser text through `tool_err`. [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8) records accepted policy decisions but remains open for detailed design, plan completion and dependent-ticket reconciliation. [ac-452ce](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-452ce), [ac-79d9f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-79d9f) and [ac-62191](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-62191) remain blocked. | Remains an MCP distribution blocker; accepted policy is not implemented. Remaining detailed contract and implementation gates stay open. |
| L7: example-generation resource exhaustion | Committed `arazzo-generate/src/examples.rs` still expands required properties without a shared output budget and pads strings with repeated `chars().count()`. [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f) is paused; its incomplete walker is dirty work, not a delivery. | Remains in CLI generation and unrestricted MCP generation. Completing the approved bounded walker requires authorized resumption and verification. |
| L8: templated child containers become strings | `runtime_core/engine_impl.rs::execute_subworkflow_step` still serializes a Parameter container before interpolation and returns a string. [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4) remains blocked on [ac-cd36b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-cd36b). | Remains. Explicit known-defect disposition is needed if deferred. |

The current GitHub release workflow builds `-p arazzo-cli`; its release assets
do not bundle the separate MCP or debug-adapter executable. VSIX preparation
does bundle the adapter and therefore needs its own artifact verification.
L6 is a separate MCP distribution concern. L7 also affects CLI generation, so
excluding an MCP executable does not eliminate that exposure.

## Accepted decisions and remaining gates

Steve already accepted the [form encoding contract](form-request-body-serialization.md),
[live-request admission contract](live-request-value-admission.md), and
[interim redaction vocabulary contract](redaction-vocabulary-contract.md).
Their closed assessment tickets do not establish implementation delivery.
These decisions need not be requested again:

- Live admission preserves zero-match Selector results as resolved null, a
  recorded residual limit; declared-but-unsupplied optional inputs count as
  authored null. Missing values caused solely by skipped dry-run transport need
  their own provenance. Genuinely unresolved request values fail deterministically
  without retry. [ac-fda34](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34)
  remains blocked and its older dry-run-only design must be reconciled with the
  accepted shared provenance owner before implementation.
- The interim credential contract adds five normalized exact names and decoded
  query-name matching. Generic `key` intentionally remains nonsensitive; this
  is an accepted limit, not an undecided name. A configurable redaction list
  remains separate work.
- The schema walker's strict numeric-error policy was explicitly approved.
  Its remaining implementation, review and delivery obligations remain open;
  no further approval of that same error policy is needed.

Recommend resolving L4 and the undelivered accepted L5 rules before CLI/runtime
release, and completing the bounded walker before claiming safe untrusted
generation. L3/L8 need explicit deferral if shipping unfixed. MCP failure projection, allowed-root composition (including absent-root denial),
race-free opening, public-only network direction, timeout bounds and final
256 KiB request / 4 MiB document / 64 KiB response ceilings are already accepted.
Exact filesystem APIs/dependency selection, network classifications and matching,
remaining resource details, plan completion and dependent-ticket reconciliation
remain outstanding in ac-950a8. Any new policy choice, broader release-risk
acceptance, and eventual tag/publication remain Steve's decisions. Candidate preparation alone clears none of them.

[ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) still records
unresolved conformance-evidence remediation and the live-broker evidence
acceptance decision. Steve explicitly authorized pushing its ancestor; that
does not establish a review PASS or accept release risks.
[ac-87a97](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-87a97) is closed:
reviewed main pushes with the existing Pages deployment are approved, while
hourly runs remain paused. The former general Pages/push hold is superseded.

## Existing evidence and limits

[ac-69cee](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-69cee) records
[successful CI for this exact baseline](https://github.com/strefethen/arazzo-cli/actions/runs/40999750541),
including all nine jobs, Windows and Security Audit. It records 1,738 passing
local workspace tests, 35 existing ignores, 594 byte-matched candidate inputs
and exact-candidate review PASS. This is retained evidence for the committed
baseline, not a fresh proof of newly packaged artifacts.

Committed `Cargo.lock` still includes `unsafe-libyaml 0.2.11` and
`chacha20 0.10.1`. Earlier audits reported these as unmaintained/yanked
warning-only entries, distinct from the four fatal rustls advisories. Fresh
audit results must identify their current status without suppression.

This reconciliation inspected committed source and current tickets. It did not
rerun the October 18 loopback probes or claim a new full security audit. Old
“eight live findings,” rustls 0.23.45, unaccepted form/admission/vocabulary
contracts, ready generator status, and general unpushed/Pages-hold statements
are historical; the dispositions above supersede them for this candidate.

## Current repair checkpoint

The native artifacts below predate the current blocker repairs. They remain
verified for their stated baseline only; rebuild and reverify final artifacts
before release. No tag, publication or hourly resumption is authorized.

- Structured credential-name delivery is reviewed and closed in
  [ac-ff226](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ff226), local commit
  [6b444c4](https://github.com/strefethen/arazzo-cli/commit/6b444c4c21a2d6d70f068993075f1f6844c956c9).
  This is partial L5 progress, not full blocker clearance.
- Encoded-query/free-text follow-on
  [ac-e1040](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e1040) passed
  automated checks but independent review found export URL field loss. It is
  paused for the component-aware masking decision in
  [ac-208df](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-208df).
- L4 now has a reviewed decomposition in
  [the request-admission plan](../current/arazzo-request-value-admission.md),
  owned by [epic:ac-3d8bf](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3d8bf).
  The earlier dry-run ticket has been reconciled with shared provenance; the
  system success condition has not yet been demonstrated. Expression provenance
  [ac-48b58](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-48b58) is reviewed
  and closed at local [9a6f832](https://github.com/strefethen/arazzo-cli/commit/9a6f832bdc34a4718146dd8ff7956cb5e3930c8b); recursive-value provenance is also reviewed and closed in
  [ac-b3093](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b3093), local
  [56cd5f1](https://github.com/strefethen/arazzo-cli/commit/56cd5f18eab70d33bebeaa803af4b4fc16f5b5b9).
  Child parameter unification is reviewed and closed in
  [ac-0b8c2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0b8c2), local
  [3664ac6](https://github.com/strefethen/arazzo-cli/commit/3664ac6c46fce4661b1eb35ac41913dfae2c8c8b).
  Shared runtime propagation is reviewed and closed in
  [ac-1af66](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1af66), local
  [74b00a0](https://github.com/strefethen/arazzo-cli/commit/74b00a04a4c7f960a7053d12603a6a0082f24ae6).
  Refusal is in progress; the foundation alone does not clear L4.
- Tokio 1.51.6 / macros 2.7.1 is reviewed and closed in
  [ac-a3f4f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a3f4f), local
  [5d91f2b](https://github.com/strefethen/arazzo-cli/commit/5d91f2bc7aa79a76d9cb3d770996f8f23db855b5).
  Canonical readiness and Rust 1.88 locked build passed; fresh audit no longer
  reports RUSTSEC-2026-0386. The two unrelated warnings remain. This proof does
  not refresh the retained VSIX or establish a final release candidate.

## Artifact verification — PASS for native preparation

Source was exported from baseline `345e652438fdf8256edc40937e20b251cf6eef5f`,
with only the two changelogs and extension package/lock version overlaid.
The candidate commit containing this assessment identifies the final metadata;
its production inputs match the tested export. The schema-walker changes were
excluded. Full input manifest SHA-256:
`93004560fc1dd229f1fad36dc7cc5917df6786faaacb59c081d909d144ae59a3`.

| Check | Result |
|---|---|
| Canonical `scripts/release/verify-readiness.sh` | PASS: fmt, strict all-feature Clippy, workspace tests, schema drift, extension lint/build, release CLI smoke and audit |
| Rust test invocations | 1,764 passed, zero failed, 35 ignored; includes explicit schema-drift rerun |
| Extension tests | 43 passed, zero failed |
| Retained CLI artifact | Version 0.9.0; native arm64; validate/list/dry-run JSON smoke passed |
| VSIX archive | Version 0.0.10, darwin-arm64; CRC, contents, internal checksum and built/extracted adapter equality passed |
| Extracted release adapter | Native DAP transcript passed using the binary extracted from the VSIX |
| Original dirty source | All 13 non-changelog dirty paths retain their baseline hashes; walker changelog entry stays uncommitted |

Local evidence and artifacts are retained under
`/private/tmp/arazzo-release-candidate/`: `artifact-results.md`,
`artifact-results.json`, command logs, manifests and `SHA256SUMS.txt`.
The source export is retained in its `source/` directory. These temporary
paths are local evidence, not published download links or durable hosting.

| Artifact | SHA-256 |
|---|---|
| `arazzo-cli-0.9.0-345e652-aarch64-apple-darwin` | `f2b65a13125eb7025aee4c8dbb221f9ddaeb84d68ce282edebdc1f9f0c64c91c` |
| `arazzo-debug-0.0.10-darwin-arm64.vsix` | `b4cf8d6af04fefe0720b443c5b6187aba887712fbda82807d0f31d6e2ab8facd` |
| Bundled release adapter | `c71b43ea3181b1c423fc34470b8efc919aa0b84023c6825567178b16a9dacf1a` |

Fresh audit exited zero with three warnings and no added suppression:
`unsafe-libyaml 0.2.11` unmaintained (RUSTSEC-2024-0449), `tokio 1.50.0`
unsound AsyncFd constructors (RUSTSEC-2026-0386), and `chacha20 0.10.1`
yanked. [ac-a3f4f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a3f4f)
tracks assessment of the Tokio advisory before publication. A successful audit
exit does not accept these risks.

Coverage is Darwin arm64 with Node 25.2.1/npm 11.6.2 and cached vsce 3.9.2.
The release workflow uses Node 20. Linux, Windows, macOS Intel, live ignored
Kafka tests, fresh MSRV proof and installed VS Code activation were not verified
in this preparation. Prior CI is not proof of those release packages.

The remaining behavioral and policy gates retain **HOLD**. No tag, push,
release workflow dispatch, publication, installed-tool replacement or hourly
resume occurred. Independent exact-candidate review is recorded in
[ac-70755](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-70755), not inferred
from the successful builds.
