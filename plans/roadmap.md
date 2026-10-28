# Arazzo engine maintainer roadmap

Updated: 2026-10-28. Owner: Codex Chief of Staff, working with Steve.
Latest source cycle: [ac-50846](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-50846).
Initial roadmap: [ac-c45b9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c45b9).

The aim is a dependable, spec-faithful workflow engine whose internals remain
easy to understand and extend. Each delivery cycle should demonstrate a useful
improvement through the real CLI, runtime, or agent surface. `tkt` owns live
status, implementation contracts, evidence, and closure; this document owns
priority, team coordination, and the program checkpoint.

Visual delivery record: [delivery-log.html](delivery-log.html). Update this same
file at every completed cycle with the practical benefit, before/after examples
or architecture, proof and reviewed commits. Clearly distinguish added capability,
correctness fixes and architectural groundwork. `tkt` remains the live authority.

## Standing delegation and continuity

On 2026-10-28 Steve explicitly reaffirmed the maintainer/Chief of Staff mandate
after cycle 2 stopped at a self-imposed renewal boundary. That boundary was an
overly restrictive interpretation: **ordinary delivery cycles continue under
the standing delegation; Steve does not need to restart each one.** This
supersedes the renewal wording in the historical cycle descriptions below.
Existing pauses, substantive product/security/public-contract decisions and
release/deployment safeguards retain their owners.

Steve also requested a visual HTML explanation after every delivery cycle and
selected **hourly while there is ready work** for unattended follow-ups. The
active `Arazzo maintainer cycles` heartbeat continues this chat with the current
roadmap, live tickets and reservations. It reports meaningful deliveries,
failures and decisions, and stays quiet when nothing actionable changes.

- Choose the next bounded, independently useful outcome from accepted direction;
  refine it against current code before dispatch. Cycle sizes are checkpoints,
  not permission gates. Do not reopen paused work or exceed the existing scope
  and safeguard rules by calling it maintenance.
- Retain at most three active agents alongside the Chief of Staff, disjoint
  writer scopes, and independent review. Source candidates retain a two-round
  review budget; a repeated unresolved finding becomes an explicit gate.
- End each hourly run at a coherent completed outcome or a truthful parked
  checkpoint. The next run rechecks current state and continues available work;
  it does not reset blocked decisions or review budgets.
- Escalate real process or decision blockers as **ask Steve for…** tickets,
  with a concrete recommendation and evidence. Keep independent work moving.

Latest delivery: typed Criterion interpolation
([ac-50846](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-50846)). Regex,
JSONPath and explicit XPath 1.0 conditions can substitute workflow values before
evaluation; null context fails typed checks. The source passed independent review
and is committed locally. Publication is held by Fable’s earlier paused
conformance candidate. Next is refinement of ordinary Criterion error reporting
([ac-1946c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1946c)).

## Direction and initial cycle

Steve's direction on 2026-10-28, in Codex chat
`01a1840e-1005-7e90-bcb3-1fc565d1a082`:

> You are a maintainer of this repo along with me. I want to give you agency to continue improving this tool so that it becomes the best Arazzo spec workflow engine in the world. The changes should be incremental in value and each step offering meaningful improvements. We need to continue focusing on the structure of the codebase to be coherent with proper separation of concerns and folder/file layout that will be a continuum of our foundation for further development. You are free to use branches or worktrees to facilitate faster work on mulitiple features at one time. You are the Chief of Staff for this effort. Create your development team and organize them around features and coordinate them to maximize development velocity. Follow my SDLC and let me know when it's slowing you down by "ask Steve for..." issues.

Follow-up:

> Layout your roadmap so I can follow along, leverage tkt and epics so I can track what's happening.

Subsequent direction on the same date: Steve removed the numerical active-plan
limit and confirmed Claude Fable is continuing Kafka/AsyncAPI development on
`main`. That work continues; this team coordinates around it.

**Initial cycle v1:** apply that delegation to the two already-settled bug
contracts below, establish this roadmap, and capture the next structural
increments. This is a conservative execution boundary chosen by the Chief of
Staff, not a claim that Steve accepted every future proposal in this document.

- Repository: `arazzo-cli` only. Outcomes: dependency-safe test completion,
  credential-safe URL presentation, and a navigable roadmap/checkpoint.
- Authority: prioritize these existing contracts; make bounded implementation
  and ticket-maintenance decisions; implement, verify, obtain independent review,
  commit, close, and push when the existing SDLC push checks permit.
- Preserve external CLI/MCP contracts, wire schemas, error codes, and accepted
  spec decisions. Retain only the workspace-internal symbol changes already
  specified by the selected tickets. No dependency additions, new document syntax, releases, deploys,
  broader security-policy decisions, or resumption of previously paused work.
- Capacity: Chief of Staff plus at most three active agents; at most two source
  implementers, on explicitly disjoint paths. Frontier reasoning handles
  architecture and independent review; Sol handles settled implementation.
- Budget: two source tickets and this roadmap ticket; at most two candidate
  review rounds per source ticket. End at completion, a required gate, or three
  elapsed hours from cycle kickoff, whichever occurs first. Checkpoint partial
  work rather than widening scope or resetting the budget. Later cycles require
  Steve's renewal; none is scheduled by this document.
- Cycle completion: both fixes have focused integrated proof, required workspace
  gates, independent review of their exact candidates, and revision-safe ticket
  closure. Record push status separately. The roadmap and follow-up captures
  are linked and reviewed. Long-term ambition is not a completion claim.

## Delivered — cycle 2: CLI catalog ownership

Steve renewed execution on 2026-10-28 in the same chat:

> Continue with the next roadmap cycle, starting with CLI catalog separation while Fable works on Kafka. Keep tkt updated and carry the work through review and completion.

He also reinforced the architectural requirement:

> let's ensure the code base is growing using best practices, separation of concerns, logically organized fuctions and features. We're not creating nor growing god modules. Let's do this right like a rust senior architect would

**Cycle 2 v1:** complete [ac-df1d6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-df1d6)
under [epic:ac-d0d1d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d0d1d).
This contained outcome stays within `arazzo-cli`: a private `workflow_catalog`
owner handles catalog discovery and unique-workflow lookup; command dispatch
delegates, and `output` retains presentation. Only two internal entrypoints are
exposed. There is no callback into the handler facade, generic discovery layer,
new crate, or public contract expansion. Review assesses cohesion and dependency
direction, rather than treating a smaller file as proof of good architecture.

- Authority: Steve's explicit renewal covers refinement, implementation,
  verification, independent review, completion and the existing SDLC push checks.
  The first cycle's compatibility, release/deployment and paused-work boundaries
  continue to apply. No numerical active-plan limit applies.
- Team: Chief of Staff owns the ticket, roadmap, legacy-owner accounting and
  integration; a frontier architecture agent validates the boundary; a Sol
  implementation agent owns the three Rust files; a separate frontier reviewer
  assesses the exact candidate. At most three agents run alongside the Chief
  of Staff, with one source implementer in this team.
- Coordination: Fable retains the Kafka/AsyncAPI lane on `main`. This cycle's
  source writes are disjoint from Fable's output/docs/examples work. Shared
  builds and Git are coordinated; reviews identify exact commits.
- Budget: one source ticket plus its tracking documentation, at most two
  candidate review rounds, and three elapsed hours from **13:15 PDT / 20:15 UTC**
  on 2026-10-28 (conservative kickoff). Hard expiry **16:15 PDT / 23:15 UTC**.
  Stop at completion or a required gate; preserve the checkpoint on expiry.
- Completion proof: unchanged real CLI behavior, required workspace gates,
  independent review of the narrow ownership boundary and exact source,
  revision-safe ticket closure, and a publication checkpoint. The runtime
  extraction remains a later increment and this cycle does not close the epic.

## Delivered — cycle 1

| Outcome | Epic and ticket | Owner | Observable proof |
|---|---|---|---|
| Timed-out or cancelled test cases cannot satisfy a later case's `dependsOn` | [epic:ac-1fae2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1fae2) · [ac-9cc68](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9cc68) | Test reliability implementer | Timeout keeps its existing error; dependent sends no HTTP request; passing and ordinary-failure controls retain existing semantics. |
| URL credentials stay out of trace and dry-run output | [epic:ac-02c86](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-02c86) · [ac-d95a9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d95a9) | Credential redaction implementer | Persisted trace bytes, redirect URLs and MCP dry-run contain no raw userinfo; unrelated URL bytes and schemas stay stable; cleartext warnings recognize userinfo. |
| Steve can follow priorities, ownership and gates | [ac-c45b9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c45b9) | Chief of Staff | This roadmap links the live tracker and records a candid delivery checkpoint. |

These fixes have disjoint source ownership. The shared Git index and full
workspace gates are coordinated; a passing test of a mixed working tree is
identified as such. Review uses exact candidate commits. Claude Fable completed
[ac-dc3f2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-dc3f2) and was
implementing [ac-8ec91](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8ec91),
Kafka receive correlation filtering, at the cycle 1 checkpoint. That historical
reservation state is not a claim about the current queue. Fable owns the ongoing
Kafka/AsyncAPI development lane on `main`. Per Steve's updated preference,
`main` remains the default: run explicitly disjoint scopes concurrently and
serialize overlapping writes; use isolation only when it offers a concrete
coordination benefit. Any push that would include an unreviewed Fable
candidate is held until the existing SDLC push checks pass.

## Delivered — structure that supports further development

Completed epic: **Clarify runtime and CLI responsibility ownership**,
[epic:ac-d0d1d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d0d1d).
Both bounded extractions are delivered within the eight existing crates: CLI in
cycle 2, runtime in cycle 3. Each candidate passed its own independent review.

1. [ac-67f05](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67f05): moved
   resolved-operation projection and server-refusal helpers to the runtime's
   operation-target owner. At baseline [51e78d2](https://github.com/strefethen/arazzo-cli/commit/51e78d2c778f736dd8f28e2dfe417e93f5b7de3a),
   `runtime_core/operation_target.rs::resolve_operation_path` calls back into
   `engine_http::resolved_operation`. The move removes that backward dependency
   and preserves routing, server selection, refusal text and no-send behavior.
   Operation ID/path/server tests and drift guards passed. The larger HTTP
   decomposition draft remains unaccepted.
2. [ac-df1d6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-df1d6): delivered
   in cycle 2 above. Focused tests and identical before/after CLI transcripts
   establish preserved shallow discovery semantics, errors and rendering.

The older **Maintainability** [epic:ac-0c8d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0c8d)
has a specific RequestBody-field retirement contract and is not repurposed.

## Delivered — cycle 4: typed Criterion interpolation

[ac-50846](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-50846), under
[epic:ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48),
lets regex, JSONPath and explicit XPath 1.0 success checks incorporate braced
runtime expressions. The existing expression renderer owns conversion and
syntax; the existing Criterion owner prepares the completed string and dispatches
it. There is no duplicate parser, new public field or new God module.

- Null/undefined context fails each typed engine, including regex `^$`.
  JSONPath query admission errors retain their existing precedence over null.
- Unresolved or malformed interpolation fails with diagnostics before typed
  execution. Native dollar syntax, simple conditions, declared trace text and
  unsupported XPath versions retain their existing behavior.
- A frontier architecture agent confirmed the private owner boundary; one Sol
  agent implemented three disjoint files; the Chief of Staff handled the exact
  conformance-count guard and delivery record; a fresh frontier reviewer assessed
  the immutable source. Fable’s Kafka/AsyncAPI work remained separate on main.
- The legacy declaration migration and JSONPath wording clarification were
  resolved by the team before implementation. The only added test-file scope
  adjusts the existing expected totals for one claim moving from deviation to
  covered. No acceptance criterion or negative test was weakened.

## Next — ordinary Criterion error reporting

[ac-1946c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1946c),
**Report Criterion evaluation errors in ordinary traces**, is the next refinement
candidate from the accepted conformance lane. Its intended benefit is diagnosis
of failed evaluations outside the debugger. This error-projection capability is
not delivered by the interpolation change.

Refresh its current contracts, file declarations, dependencies and trace/output
ownership before implementation; do not infer readiness from priority alone.
Preserve existing schema/error contracts and route any substantive public-output
decision through its stated gate. Inspect Fable’s live reservations before
selecting writer paths.

## Following cycles — ordered by value and readiness

| Lane | Existing epic ownership | Next useful milestone | Entry condition |
|---|---|---|---|
| Spec conformance and effective Steps | [epic:ac-33d48](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-33d48), [epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b) | Turn settled semantics into positive/negative executable evidence, then close effective-Step gaps. | Respect current writer; verify vendored normative text and current ticket dependencies. |
| Offline authoring and honest dry-run | [epic:ac-40dcd](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-40dcd), [epic:ac-df418](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-df418) | Resolve operation targets before execution and preview composed workflows accurately. | Settle output/API contracts before implementation. |
| Safe agent execution | [epic:ac-852d1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-852d1) | Enforce the accepted MCP containment and resource policy through usable boundaries. | Finish remaining platform, network and memory contracts. |
| Broader OpenAPI and schema support | [epic:ac-31ca2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-31ca2) | Land the shared schema walker, then deliver the already-selected Value-based ingestion direction. | Refresh paused contracts and obtain explicit resumption. |
| Measurable runtime quality | [epic:ac-8ba62](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8ba62), [epic:ac-e20be](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e20be) | Reproducible capability/performance proof and bounded Kafka follow-ups. | Select one observable scenario at a time; preserve transport and dependency gates. |

This ordering is a priority proposal. Existing epic ownership and prerequisites
remain authoritative. A queue entry alone never establishes implementation
readiness, and future dates are not promised before scope and proof are settled.

## Decisions and SDLC friction

| Issue | Action and owner | Effect on delivery |
|---|---|---|
| Numerical active-plan limit removed by Steve | Updated [epic:ac-a0648](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a0648) / [ac-690a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-690a3) to remove the obsolete count/capacity requirement. They retain lifecycle disposition review and acceptance for actual moves. | Plan count does not block new work. No speculative mass archive or re-request for approval of the removed limit. |
| Old ticket file declarations need migration before reservations | Chief of Staff authorizes exact-ticket, reviewed operation-map migration with recovery evidence. | Mechanical overhead handled by the team; no Steve decision required. |
| Regex-cache ticket cites a deleted expression implementation | Refresh [ac-e81bc](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e81bc) against current runtime code before dispatch. | Investigation/refinement, not a reason to start from stale instructions. |
| Test dry-run contract conflicts with itself | [ac-e27f8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e27f8): resolve always-present `dryRun` versus unchanged ordinary JSON. **Ask Steve for the public-output choice with a concrete recommendation.** | Blocks that feature only. |
| MCP implementation policy has remaining gaps | [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8): prepare bounded platform/dependency, address-exception, backpressure and aggregate-memory decisions. **Ask Steve for remaining security choices**, not decisions already accepted. | Blocks hardening implementation; accepted root and size/time limits are retained. |
| OpenAPI/schema work is explicitly paused | [ac-6a778](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-6a778), [ac-93d90](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-93d90): retain Value-based ingestion and synchronous first-error validation decisions; finish missing contracts/proof, then **ask Steve for resumption**. | No automatic resumption from the broad maintainer request. |
| Runtime audit epic has a superseded member with replacement owned elsewhere | Reconcile [epic:ac-02c86](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-02c86) without moving [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f) away from its ingestion owner. | Does not block the independent redaction fix; prevents a false epic-complete claim. |

## Checkpoint — cycle 1

- Budget kickoff: **2026-10-28 12:47 PDT (19:47 UTC)**, conservatively rounded
  down to the first chat minute. Hard expiry: **2026-10-28 15:47 PDT (22:47
  UTC)**. Preserve these timestamps through later checkpoints; baseline
  [51e78d2](https://github.com/strefethen/arazzo-cli/commit/51e78d2c778f736dd8f28e2dfe417e93f5b7de3a).
  Checkpoint: **2026-10-28 13:05 PDT**, about 18 minutes consumed.
- Source outcomes: **2/2 completed and pushed**. Timeout dependency safety:
  [bd00935](https://github.com/strefethen/arazzo-cli/commit/bd009354d9083e850260408f34caba9be85f9141), independent review PASS in round 1.
  URL credential redaction:
  [f9abdd0](https://github.com/strefethen/arazzo-cli/commit/f9abdd01b16936fff37626f1337b1f1c4865b5b0) plus
  [f3cfd10](https://github.com/strefethen/arazzo-cli/commit/f3cfd102825127a93a7c4c8f34f8aa90d64adc1d), cumulative independent review PASS in round 2 after correcting the multiple-`@` leak.
  Both tickets are closed; source implementation reservations are released.
- Final reviewed source: **1,485 tests passed, 0 failed, 31 ignored**;
  formatting, strict workspace Clippy and all four drift guards passed.
  Frozen source hashes bind proof to the reviewed commits. Fable's subsequent
  Kafka edits are excluded from these claims. Its completed documentation
  candidate was checked before pushing the exact reviewed source tip.
- Architecture/backlog surveys and roadmap review are complete. Two structural
  captures are linked under the new ownership epic. Ticket declarations and
  adjacent consumer grounding were refreshed by recommendation; the numerical
  plan-capacity criteria were removed by Steve's explicit instruction.
- Evidence root for this local run:
  `/private/tmp/arazzo-maintainer-20261028/`. Ticket closeout records retain
  exact candidates, evidence pointers, review verdicts, and publication status.
- Historical next action: publish this roadmap checkpoint and await renewal.
  The roadmap landed as `35771c4`; Steve subsequently renewed cycle 2 above.
- No release, deployment, public API expansion, or paused-ticket resumption is
  authorized by this checkpoint. Unrelated untracked work is preserved.

## Checkpoint — cycle 2

- Started from `841aa5c` with Fable's Kafka candidate present and outside this
  ticket's mutation scope. Architecture review approved the private feature
  owner. Ticket lint, grounded readiness, queue eligibility and live reservation
  checks passed. The extraction removes catalog policy from handlers (758 to 617
  lines) into a 163-line private owner. These counts describe the move; the
  responsibility and dependency checks establish its architectural value.
- All 21 before/after CLI scenarios have identical stdout, stderr and exit status;
  focused discovery/catalog/show tests pass. Workspace verification passed:
  1,491 tests passed, 0 failed, 35 ignored, including all four drift guards;
  formatting, strict Clippy and `cargo check` passed. Hashes of 270 source/input
  files remained unchanged throughout the gates. Fresh independent review
  **PASS**, with no findings or evidence gaps, covers
  [b68625f](https://github.com/strefethen/arazzo-cli/commit/b68625fa8934c2e6aa6fbb8b253ff7d8bf94c2a1)
  **(pushed to main)**. The reviewer independently reran both retained
  binaries for all 21 scenarios and confirmed equivalent results. Evidence lives
  at `/private/tmp/arazzo-maintainer-20261028/ac-df1d6/` and the ticket retains
  the exact execution contract. The harness has no native per-criterion progress
  tool; `tkt` and the evidence packet remain the durable execution record.
- The repository intentionally ignores `god-files.md`; its local inventory is
  updated without changing that policy. Remaining replay/generation/validation/
  test/schema responsibilities are still debt, not declared resolved here.
- Checkpoint **2026-10-28 13:26 PDT / 20:26 UTC**: one source outcome delivered,
  one of two review rounds used, about 11 minutes consumed from conservative
  kickoff. `tkt` owns revision-safe closure and the final lifecycle state.
- Publication was briefly held by the existing SDLC check: Fable's preceding
  [841aa5c](https://github.com/strefethen/arazzo-cli/commit/841aa5ccb37afc9b84540565b696e2cbb7b67718)
  and [8e3d5cb](https://github.com/strefethen/arazzo-cli/commit/8e3d5cb8b968315458af8401145415a2679cea11)
  were still the candidate of active
  [ac-b0044](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b0044) at this
  checkpoint. Fable closed that ticket completed at **20:26:59 UTC**, with
  independent review evidence identifying both commits. The gate then cleared:
  the reviewed catalog source and checkpoint were pushed through
  [3e39566](https://github.com/strefethen/arazzo-cli/commit/3e395663bb9a6121126d2ec8943af96ad85085ec).
  No Fable interruption or Steve decision was needed.
- Next structural increment at that checkpoint: refine runtime operation projection ownership
  ([ac-67f05](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67f05)) against
  current code and reservations. Steve's standing-delegation clarification above
  subsequently activated that work without another per-cycle permission gate.
  The epic was still open at the cycle 2 checkpoint; its completion is recorded
  in cycle 3 below.

## Checkpoint — cycle 3

- Runtime source delivered in
  [1adaacb](https://github.com/strefethen/arazzo-cli/commit/1adaacb7c0910b95b53685821ee84f52ec6f62aa),
  independently reviewed **PASS** with no findings, pushed to `main`, and
  revision-safely closed completed. Four existing projection/refusal items now
  live in the existing `operation_target` owner. HTTP preparation consumes it;
  operation target resolution no longer depends back on HTTP execution.
- Observable behavior remains unchanged. All **65 focused tests passed before
  and after**; the moved block is byte-identical. Full gates passed:
  **1,491 tests passed, 0 failed, 35 ignored**, all four drift guards, formatting,
  compilation and strict Clippy. The final 270 source/input hashes match the
  reviewed candidate. A formatting-only import-order correction was completed
  before the final gates; no acceptance criterion or test was weakened.
- Both ownership boundaries are observed, so
  [epic:ac-d0d1d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d0d1d) is
  **closed completed**. This resolves the two bounded outcomes, not all remaining
  HTTP/CLI responsibility debt. No new crate, generic helper owner, or public
  contract was introduced.
- Evidence: `/private/tmp/arazzo-maintainer-20261028/ac-67f05/`. One of two
  source review rounds used. The accompanying visual log explains cycles 1–3;
  appearance, navigation and enlarged text are verified in a real browser.
- Hourly continuation is active. The next run refines typed Criterion
  interpolation through its named owning modules and the existing conformance
  plan. No Steve decision currently blocks that refinement.

## Checkpoint — cycle 4

- Source delivered in
  [be884e2](https://github.com/strefethen/arazzo-cli/commit/be884e2a0fd79eac540cc852824e133f08cd5594)
  **(local; not pushed)**. Fresh independent source review **PASS**, one of two
  review rounds used. The existing runtime claim now has positive and negative
  executable evidence; the conformance epic remains open for its other work.
- Nine focused Engine tests and ten Criterion unit tests pass. Seven of the
  initial eight focused tests fail on the old implementation; the compatibility
  control remains green. Adjacent runtime tests: 30 passed. Validator expression
  tests: 15 passed. Conformance manifest: 13 passed.
- Full workspace verification: **1,514 passed, 0 failed, 35 ignored**; all four
  drift guards, formatting, strict Clippy and compilation pass. All 270 tracked
  build inputs match the candidate Git objects, and three vendored specifications
  remained unchanged during verification. Fable’s preceding committed work is
  included in that checkpoint without modification by this ticket.
- Evidence: `/private/tmp/arazzo-maintainer-20261028/ac-50846/`. The visual
  delivery log adds the practical before/after example and exact publication
  status. Its settings and style blocks are byte-identical. Browser checks passed
  at 1280px and 390px with no page overflow or console errors; section navigation
  works and the temporary viewport override was reset.
- Publication hold: main contains Fable’s paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) candidate
  [7f83a9e](https://github.com/strefethen/arazzo-cli/commit/7f83a9e7fb0bc532ffa9181095f9d86749698594)
  **(local; not pushed)**. The existing review/scope gate must clear before a push
  includes it. Fable remains uninterrupted; no new approval request or duplicate
  decision ticket is created for the same gate. Local completion remains valid
  under the SDLC’s explicit publication-hold procedure.
- Hourly continuation remains active. Refine the next bounded, available ticket
  against current evidence and reservations; do not reopen paused work or treat
  this cycle as completion of the full conformance roadmap.
