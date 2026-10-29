# Arazzo engine maintainer roadmap

Updated: 2026-10-28. Owner: Codex Chief of Staff, working with Steve.
Latest source cycle: [ac-89176](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-89176).
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

Latest delivery: effective non-action Step expression inventory
([ac-89176](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-89176)). One focused model owner supplies borrowed text,
declaration provenance, nested paths and field modes, reusing canonical
parameter inheritance and Selector admission. Independent review passed.
This is model groundwork; Action inventory and dependency consumers remain
outstanding. Next readiness assessment: the Action ordering decision
[ac-908bc](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-908bc), which gates
the Action overlay and [ac-70691](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-70691).
Runtime inheritance [ac-4074e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4074e)
remains paused for Steve's input-name collision decision
[ac-b9701](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b9701).
Publication remains held by Fable's earlier paused candidate. The separate
source-type decision [ac-bd367](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd367)
remains open.

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

## Delivered — cycle 5: version syntax and feature-set validation

[ac-e767c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e767c) delivers the
independent version-validation part of
[ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3), under the
existing conformance epic. The parent retains its full acceptance criteria and
testing obligations; the child is a prerequisite, not a replacement. One private
`version.rs` owns syntax, supported feature sets and the pre-1.1 vocabulary
classification. The validator facade delegates and shrinks; no runtime or public
output shape changes.

## Delivered — cycle 6: bounded regex cache ownership

[ac-e81bc](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e81bc) removes cache
resource and locking policy from Criterion evaluation. The new private sibling
retains the existing matching signature, error text and per-engine isolation.
Compile counters, eviction/boundary tests and a real overlapping HTTP workflow
prove reuse, bounds and serial/parallel semantic equivalence. This is a reliability
fix and ownership improvement, without a new condition language or throughput claim.

## Delivered — cycle 7: canonical Step-output reference syntax

[ac-2f84e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2f84e), in
[epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b), owns
one syntax-only adapter for the four accepted field modes. It reuses canonical
parsers and returns borrowed local Step-output occurrences in source order.
The expression-string owner shares its existing reserved-candidate iterator;
the crate root gains declarations and exports only. No graph, document model,
evaluation or scheduling concern enters this owner.

The vendored specification's [Tool Behavior](../spec/arazzo/v1.1.0.html#tool-behavior)
states: “Tools MUST also treat runtime expression output references (e.g.,
`$steps.stepId.outputs.field`) as implicit dependencies and ensure the referenced
step completes before the referencing step executes.” Its Runtime Expressions
section defines `steps-reference = step-id ".outputs." output-name [ "#" json-pointer ]`.
This cycle supplies syntax groundwork; later tickets must prove the scheduling
invariant. The accepted plan's literal-brace compatibility deviation remains
explicit and unchanged.

Frontier readiness confirmed the frozen API and four-file boundary. Decided by
recommendation: clarify that the adapter adds no allocation beyond its result/error
and the allocations already made by the mandated canonical parsers. Acceptance
criteria and testing obligations remain byte-identical. Root owns tkt, docs and
integration; Sol implements; a fresh frontier agent reviews the exact candidate.
One source slice, at most two review rounds; kickoff 23:37 UTC, three-hour ceiling.
Fable's source paths and paused decisions remain outside this cycle.

## Delivered — cycle 8: canonical effective Step parameters

[ac-0149e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0149e), under
[epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b), adds the
accepted borrowed Parameter overlay to a focused model module. Its exact
`(name, in_)` identity, source order, duplicate visibility and declaration
provenance apply uniformly to operationId, operationPath, channelPath and
workflowId Steps. The root adds four wiring/export lines only. Parsing,
validation, resolution and runtime behavior remain separate responsibilities.

The vendored [Workflow parameters](../spec/arazzo/v1.1.0.html#workflowParameters)
text says: “A list of parameters that are applicable for all steps described
under this workflow.” [Step parameters](../spec/arazzo/v1.1.0.html#stepParameters)
says: “If a parameter is already defined at the Workflow, the new definition
will override it but can never remove it.” The
[Parameter Object](../spec/arazzo/v1.1.0.html#parameter-object) says: “A unique
parameter is defined by the combination of a `name` and `in` fields.” Output
ordering and duplicate retention are the accepted repository choices, enabling
later validators to report the original declarations.

Frontier readiness confirmed the frozen API and three-file boundary. Decided by
recommendation: refresh owning helper anchors and prove location-case behavior
through negative deserialization, because `ParamLocation` has typed lowercase
variants. Original Goal, acceptance criteria and testing obligations are unchanged.
Root owns tkt/docs/Git, Sol implements, and a fresh frontier context reviews the
exact candidate. One source slice, at most two review rounds; kickoff
**2026-10-29 00:39 UTC** (17:39 PDT Oct 28), hard expiry **03:39 UTC**.
Fable’s source and reservations remain outside this slice.

## Delivered — cycle 9: validator inheritance and location context

[ac-475c6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-475c6), under
[epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b), replaces
the validator’s target-gated merge with the canonical model overlay. One local
adapter translates borrowed origins to existing paths and structural keys.
The single run-scoped suppression owner remains. The validator root shrinks
from 2,916 to 2,895 lines; no production module or responsibility is added.

Steve approved [ac-1e1d0](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1e1d0)
on **2026-10-28 PDT**. Valid `in` is now optional on workflow-target Step
parameters. OpenAPI query/querystring exclusivity applies only to synchronous
operation targets. Explicit Success/Failure Action prohibitions, existing
workflow-input name-only duplicate checks, and operation constraints remain.
The accepted plan’s “Workflow-target Step parameter locations” section owns
the vendored quotations and approved interpretation. Runtime input delivery,
input-name collision policy and child schema checks are outside this slice.

Frontier readiness confirmed the current five-file boundary. Decided by
recommendation: private borrowed selection/provenance assertions supplement
the unchanged approved public obligations, because clean validation alone
cannot prove inheritance after the old location prohibition is removed.
Root owns tkt/docs/Git, Sol implements, and a fresh frontier context reviews
the exact candidate. One source slice, at most two review rounds; kickoff
**2026-10-29 01:43 UTC** (18:43 PDT Oct 28), hard expiry **04:43 UTC**.
Fable’s source and reservations remain outside this slice.

## Delivered — cycle 10: Selector admission compatibility baseline

[ac-6572e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-6572e), under
[epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b), adds one
focused integration-test file in arazzo-spec (the existing model crate).
It retains 24 empirically observed fixtures plus missing-context/type siblings,
and compact OutputValue controls. Exact typed fields, defaults, extension values
and complete literal trees distinguish wire decoding, Value conversion and
direct Selector decoding. Tests record current admission, not normative validity.

Decided by recommendation: make this test-only baseline an independent
prerequisite to shared admission refinement. The offline probe exposed concrete
tag, duplicate-key and coercion differences; neither normalizing all YAML into
Value nor reusing borrowed owned-type decoding is a safe drop-in classifier.
One test owner adds no production responsibility, dependency or public API.
Root owns tkt/docs/Git, Sol implements, and a fresh frontier context reviews the
exact candidate. Cycle kickoff **2026-10-29 02:46 UTC** (19:46 PDT Oct 28),
hard expiry **05:46 UTC**; one source candidate, at most two review rounds.

## Paused consumer — child-workflow input-name collisions

[ac-4074e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-4074e) passed the
model/validator dependency gate, but frontier readiness found a semantic
decision explicitly excluded by Steve’s previous location approval. Canonical
selection retains distinct `(name,in_)` declarations, while child inputs use a
name-keyed map. An inherited `x/query=A` and Step `x/header=B` select both but
the current mapper yields `x=B`.

**Ask Steve for** [ac-b9701](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b9701):
recommend preserving current last-effective-declaration-wins mapping, evaluating
every selected declaration and documenting the result as a repository choice.
AGENTS.md’s Scope rule and the accepted plan’s explicit exclusion require Steve
to settle this policy. Runtime source work has not started; its original AC and
testing remain intact. Rejection or another representation needs revised scope.

## Delivered — cycle 11: shared Selector admission owner

[ac-8d855](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8d855) extracts owned
decoding and borrowed admission into the model crate's private
`selector_admission` module. Its visitors own required/default fields, duplicate
handling and type dispatch once. The borrowed path returns the original context
string without copying extension value trees. `ValueSource::from` uses that
path before existing admitted-value materialization. Wire parsing envelopes,
public models and serialization stay unchanged.

Decided by recommendation: use shared visitors after an executed prototype
showed that derived flattening copies extension trees. Focused capture preserves
malformed-extension diagnostics and their priority, including invalid byte-key
cases. The frozen original model and independent tests establish compatibility;
no speed claim or normative validity change is made.

Root owns tracker, Git and delivery documentation; Sol implements the settled
four-file slice after frontier readiness; a fresh frontier reviewer checks the
exact candidate. Kickoff **2026-10-29 03:42 UTC** (20:42 PDT Oct 28), hard expiry
**06:42 UTC**, one source slice and at most two review rounds.

## Next consumer — effective expression-site inventory

[ac-89176](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-89176) now has its
shared-admission prerequisite. Recheck its accepted contract, current source and
reservations before starting; graph eligibility alone is not implementation
readiness. Its frozen syntax-neutral API, provenance, ordering and no-value-tree-
cloning obligations remain intact. No inventory, graph or scheduling change is
delivered by the shared owner.

## Paused dependency checkpoint — source descriptors and ordinary diagnostics

The current dependency chain runs from ordinary Criterion error reporting
([ac-1946c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1946c)) through
interpolation warnings
([ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4)), exact runtime
action targets ([ac-cd36b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-cd36b)),
exact validated action targets
([ac-a7fe9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a7fe9)), and source
descriptors ([ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3)).
Do not bypass these dependencies merely because the downstream item is valuable.

The source-descriptor parent is paused for
[ac-bd367](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd367): **ask Steve
for the omitted source-type representation in CLI and MCP JSON**. Recommendation:
preserve the key with `"type": null` and show “unspecified” in human output,
without inferring OpenAPI or AsyncAPI. This changes a public string field and
needs Steve’s choice before implementation.

After that choice, refresh the parent’s consumer boundaries against current
DocumentSet binding, operation targets and Fable’s AsyncAPI applicability work.
Its historical URL-as-base fallback paragraph is stale; current binding no longer
has that fallback. Source URL validation and aggregate source-descriptor proof
remain outstanding. Preserve the full parent criteria, Fable’s live reservations
and the existing publication gate while selecting other independent ready work.

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
| Regex-cache ticket cited a deleted expression implementation | Resolved in cycle 6: [ac-e81bc](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e81bc) retains the selected policy and original criteria/tests, grounded in current runtime owners. | Exact-ticket migration and bounded refinement completed without a new Steve gate. |
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

## Checkpoint — cycle 5

- Source delivered in
  [32ebfed](https://github.com/strefethen/arazzo-cli/commit/32ebfede58c315a682022edb75c3b2bee0a4ef2a)
  **(local; not pushed)**. Fresh independent review **PASS**, one of two source
  review rounds used. Only the version-grammar/feature-set conformance claim
  moves to covered; source-descriptor obligations remain open under their parent.
- Six focused tests exercise YAML, JSON and direct validation. Five fail on the
  original implementation; the required-field control remains green. Validator
  tests: **212 passed**. Conformance manifest: **13 passed**. Full workspace:
  **1,520 passed, 0 failed, 35 ignored**. Formatting, compilation, strict Clippy
  and all four drift guards pass. All 272 tracked build inputs match the reviewed
  candidate; three vendored specifications remained unchanged during verification.
- The private classifier removes version responsibility from the validator facade,
  reducing it from 2,933 to 2,916 lines and lowering its size cap. Decimal
  components compare numerically without integer bounds; leading-zero handling
  is documented as an interpretation accepted by independent review.
- Decided by recommendation: correct the adjacent assertion that incomplete
  `1.0` selects a feature set, retaining the test and its other controls. Strict
  Clippy also caught two fixture `expect()` calls; explicit handling repaired
  them before the final full gates. No criterion, negative test or lint was weakened.
- Evidence: `/private/tmp/arazzo-maintainer-20261028/version-cycle/`. The single
  delivery log now explains the validation benefit, before/after examples and
  private ownership. Desktop 1280px and mobile 390px checks passed without page
  overflow or console errors; section navigation works and the temporary viewport
  was reset. Style and script blocks are byte-identical to the preceding journal.
- Publication remains held by Fable’s paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) candidate.
  Fable’s lane is untouched. The separate omitted-source-type decision is captured
  in [ac-bd367](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd367); its parent
  is paused with all original acceptance criteria and testing obligations retained.
  No nullable output schema, source-kind inference or fallback is implemented.

## Checkpoint — cycle 6

- Source delivered in
  [7dc80ba](https://github.com/strefethen/arazzo-cli/commit/7dc80bac8c957c0f8eb5b979b24ef91965fcc608)
  **(local; not pushed)**. Fresh independent review **PASS**, one of two source
  review rounds used. The fix bounds retained regex results to 256 per engine,
  excludes keys over 4,096 bytes from retention while still evaluating them,
  reuses failures, and compiles/matches outside the map mutex.
- Nine cache tests pass in debug and release. The new integrated HTTP workflow
  passes in both modes, proves three overlapping requests, and compares complete
  serial/parallel trace records with only elapsed duration normalized after an
  independent bound check. The release parallel-engine and recording suites add
  41 passing tests. Existing Criterion, helper and typed-interpolation tests pass.
- Full workspace: **1,530 passed, 0 failed, 35 ignored**. Formatting, compilation,
  strict all-target/all-feature Clippy and all four drift guards pass unchanged.
  All 274 tracked source/fixture inputs match the immutable candidate; four
  vendored specification files remained unchanged during the final gates.
- Private `regex_cache.rs` owns retention and locking; Criterion evaluation and
  existing tests are byte-identical after removing its old cache block. The
  evaluation owner shrinks from 731 to 704 lines. Engine construction keeps the
  existing compatibility re-export. No dependency, public API, diagnostic,
  accepted pattern or Fable path changed; no throughput gain is claimed.
- Decided by recommendation: replace historical references to the deleted
  expression-cache implementation with the already-selected explicit policy and
  current consumer paths. Keep every original acceptance/testing item. Compile
  counts prove sequential reuse; concurrent cold misses may duplicate compilation
  as the original design permits. Duration-only trace normalization preserves all
  semantic fields and asserts sequence/order, fixed headers and real overlap.
- Evidence: `/private/tmp/arazzo-maintainer-20261028/regex-cache-cycle/`.
  The initial sandbox socket denial was resolved by authorized localhost access;
  no failing verification was waived. The journal adds before/after behavior,
  responsibility boundaries and exact proof. Desktop/mobile checks and committed
  HTML hash binding are retained with the ticket’s final evidence.
- Publication remains held by Fable’s paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) candidate in
  main’s unpublished history. The runtime-audit epic retains its other obligations.
  Next refinement candidate: canonical Step-output reference extraction
  [ac-2f84e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2f84e). Its three
  prerequisites are completed, but current source grounding and live clearance
  must pass before a new start. The source-type decision and other pauses remain.

## Checkpoint — cycle 7

- Completed [ac-2f84e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2f84e)
  under [epic:ac-59b5b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-59b5b).
  Source: [f4c6e48](https://github.com/strefethen/arazzo-cli/commit/f4c6e484cf3b7d6de02371fec8722d61173c87d0)
  **(local; not pushed)**. The epic remains open; model inventories and dependency
  consumers are not delivered by this syntax-only slice.
- One new expression owner adapts the four accepted modes to canonical parsers.
  The shared reserved-candidate iterator preserves first-raw-brace boundaries and
  exact errors. The root adds only five declaration/export lines; evaluator,
  runtime, validator and Fable’s paths are unchanged.
- Ten public API tests prove canonical filtering, source order, duplicates,
  borrowed names, literal/quoted/native controls, case-insensitive keywords,
  pointers, Unicode and fail-closed malformed candidates. Expression crate:
  142 passed. Full workspace: **1,540 passed / 0 failed / 35 ignored**.
  Formatting, workspace check, strict all-target/all-feature Clippy, benchmark
  compilation, public API docs and all four drift guards passed.
- Fresh-context frontier review passed the exact source candidate and contract
  revision `638ee59d055526dcb1d4e2bc056631b8f5b7cab71dbac1e37374438462a56b6e`.
  The tracker’s evidence note and guarded close retain the final lifecycle fence.
  Review used one of two allowed rounds; no source changed after PASS.
- Raw evidence, source hashes, contract snapshots, scope, review and browser QA:
  `/private/tmp/arazzo-maintainer-20261028/step-reference-cycle/`.
  The delivery log labels this as groundwork and links the reviewed source.
- Publication remains held by paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f), whose earlier
  candidate is in the same unpublished main history. No push, release, or Kafka
  source mutation occurred. Existing unrelated untracked files are preserved.
- Next refinement: [ac-0149e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0149e),
  then the effective expression-site inventory after its prerequisites pass.
  The source-type decision and all existing pauses remain with their owners.

## Checkpoint — cycle 8

- Source: [6a07fba](https://github.com/strefethen/arazzo-cli/commit/6a07fba8b57c2b10fc2192807f804bb3f6f744e6)
  **(local; not pushed)**, independent review PASS in round 1. The model now
  supplies the accepted common view; current runtime/validator inheritance
  behavior is not changed. The effective-Step epic stays open for its consumers.
- A 40-case four-target matrix plus focused public/unit cases prove inheritance,
  exact overrides, duplicates, order, provenance, borrowing, invalid indexes and
  rejection of nonconformant location spellings. Model crate: 25 unit + 4 new
  integration + 23 serde integration tests. Full workspace: **1,545 passed,
  0 failed, 35 ignored**; formatting, compilation, strict all-target/all-feature
  Clippy, public docs and all four drift guards pass.
- 282 build/fixture/spec inputs stayed identical through verification; 278
  tracked inputs match the reviewed commit and four vendored specs are unchanged.
  Contract revision `4c799b1f42e322e12f9e4dd72553f5c3e585211840493c11920c190c7cfdf032`
  and raw evidence are retained in
  `/private/tmp/arazzo-maintainer-20261028/effective-parameters-cycle/`.
  The delivery log explains the model-only benefit, before/after declaration
  example, future consumer work and the explicit Steve decision.
- Static HTML behavior remains unchanged; desktop/mobile browser QA and the
  committed HTML hash are bound in the final ticket evidence. No Rust source
  changed after review. Revision-safe ticket closure releases this cycle’s leases.
- Publication remains held by Fable’s paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) candidate in
  unpublished main history. No push, release or Kafka source mutation occurs.
  Unrelated untracked files are preserved. The next independent refinement is
  expression-site inventory; the location-rule decision remains with Steve.

### Cycle 8 follow-up — approved parameter-location correction

Steve answered “Approve the correction (Recommended)” on 2026-10-28 PDT. This
supersedes the pending location-rule decision at the original checkpoint above.
The plan and validator AC/testing now reflect his choice; independent review
checks the amended contract before the decision ticket closes. No executable
source changed, and the existing Fable publication gate remains in force.

## Checkpoint — cycle 9

- Source: [8b65744](https://github.com/strefethen/arazzo-cli/commit/8b65744d1e7724c26cacb1f655ceb1e82327c6bb)
  **(local; not pushed)**, independent review PASS in round 1. Validator
  adoption and the approved location correction are delivered; runtime adoption
  and expression inventory remain outstanding. The effective-Step epic stays open.
- Original no-inheritance tests pass before edits; two private selection tests
  and six public behavior cases fail against original production code, then
  pass against this candidate. The 36-combination location matrix, exact parsed/
  direct diagnostics, reusable resolution, duplicate retention and operation/
  action negative controls pass. Validator: **222 passed**; conformance-manifest
  guard: **13 passed**. Full workspace: **1,555 passed, 0 failed, 35 ignored**;
  formatting, compilation, strict Clippy and all four drift guards pass.
- 283 build/fixture/spec inputs stay identical through verification; 279
  tracked inputs bind to the reviewed commit and four vendored specs remain
  unchanged. Frozen contract revision
  `b83e215293f98620797b9c8217af2fca2b0cac699a0a08b35aa71d36752e3464`
  and raw evidence are retained in
  `/private/tmp/arazzo-maintainer-20261028/validator-inheritance-cycle/`.
  The delivery log shows before/after validation and the ownership chain.
- Desktop/mobile QA and committed HTML hash are bound in final ticket evidence.
  No Rust source changed after review. Revision-safe closure releases the leases.
- Publication remains held by Fable’s paused
  [ac-18e6f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-18e6f) candidate in
  unpublished main history. No push, release or Kafka source mutation occurs.
  Unrelated untracked files are preserved. Next: runtime readiness and bounded
  Selector-admission refinement, subject to live ownership.

## Checkpoint — cycle 10

- Test candidate: [41bdf45](https://github.com/strefethen/arazzo-cli/commit/41bdf45b3319cbe358aff4f8f733172f5f5370bb)
  **(local; not pushed)**, independent review PASS in round 1. One new focused
  model integration-test file; production and Fable source unchanged.
- 26 named compatibility fixtures plus output controls: **29 focused
  tests passed**. Full workspace: **1,584 passed, 0 failed, 35 ignored**;
  formatting, compilation, strict Clippy and all four drift guards pass.
  Negative replacement controls prove duplicate-extension normalization and
  tagged-context borrowed decoding cannot substitute for current admission.
- 284 build/fixture/spec inputs stayed identical through gates; 280 tracked
  inputs bind to the candidate and four vendored specs are unchanged. Frozen
  contract `ff97d9e6c7675467a22adebe541b61d6a0166b015c8fccdb1459e42e917061ab`
  and raw evidence live in
  `/private/tmp/arazzo-maintainer-20261028/selector-compatibility-cycle/`.
  Desktop/mobile QA and committed delivery HTML hash are bound in final evidence.
- Runtime inheritance was parked before source work for ac-b9701; shared
  admission refinement remains open with its review marker, and the inventory
  remains blocked. The epic stays open. Revision-safe closure releases leases;
  unrelated untracked files remain untouched. Publication is still held by
  Fable’s paused ac-18e6f candidate in unpublished main history.

## Checkpoint — cycle 11

- Source: [048a130](https://github.com/strefethen/arazzo-cli/commit/048a1305b1153cf5a05b34a7b21b4386365b40fc) **(local; not pushed)**, independent review PASS.
  One private model owner and focused tests; public wire behavior and Fable's
  source are unchanged. The epic remains open for its consumers.
- The original 29-test admission baseline remains byte-identical. Six private
  borrowing tests and seven public diagnostic tests pass; the model package has
  94 passing tests. The frozen-original differential proof passes 423 exact
  value/error comparisons, including JSON and byte inputs, plus serialization.
- Full workspace: **1,597 passed, 0 failed, 35 ignored**. Formatting,
  workspace check, strict all-target/all-feature Clippy and all four drift guards
  pass. All 287 build/fixture/spec inputs remained unchanged through final gates.
- Evidence, contract and candidate bindings are retained under
  `/private/tmp/arazzo-maintainer-20261029/selector-shared-cycle/`. The delivery
  HTML receives desktop/mobile QA and a committed-blob hash binding.
- Inventory readiness is the next independent slice. Runtime collision and
  source-type decisions remain with Steve. Publication remains held by Fable's
  paused ac-18e6f candidate in unpublished main history; no push/release occurs.

## Checkpoint — cycle 12

- Source: [a5b4704](https://github.com/strefethen/arazzo-cli/commit/a5b4704207f14163a5493ea9bbe7950ddb0b41e1) **(local; not pushed)**; independent exact-candidate
  review PASS. One focused model inventory module plus public contract tests;
  the crate root gains declaration/export wiring only. Fable source is untouched.
- The public borrowed API preserves effective Parameter provenance and exact
  overrides, nested sequence/mapping locations, Selector contexts, Criterion
  modes, output order and correlation IDs. No expression grammar, validation,
  graph construction or scheduling was added to the model.
- Decided by recommendation: ground root-versus-child Selector traversal,
  opaque tags and normalized Criterion dispatch in existing owners. The frozen
  public API, Goal, Acceptance Criteria and Testing Obligations remain intact.
- Full workspace: **1,608 passed, 0 failed, 35 ignored**. Focused/model tests,
  formatting, workspace check, strict Clippy, public docs and four drift guards
  passed. Evidence and exact source/contract bindings:
  `/private/tmp/arazzo-maintainer-20261029/expression-inventory-cycle/`.
- The single delivery log explains the before/after benefit and remaining
  consumer work; desktop/mobile QA and committed HTML hashes bind its proof.
  The Step inheritance/dependency epic remains open. Next: bounded Action-order
  readiness assessment, with no policy decision inferred from this delivery.
- One coherent source slice under the 04:50–07:50 UTC cycle budget. Existing
  Steve decisions and Fable's publication gate remain unchanged. No push/release.
