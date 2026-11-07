---
status: draft
epic:
created: 2026-11-05
---

# Agent operability as a system property

**Seed:** Steve's 2026-11-05 request in this conversation:

> What would most enable you to do an awesome job understanding the situation accurately and optimally controlling everything to drive the best and most accurate results possible, with the least expenditure of resources?
>
> Don't just think of the project as an assemblage of various parts or components: really try to profoundly and deeply conceptualize it as a synthetic SYSTEM that is maximally coherent, cohesive, modular, and interconnected, forming a tower of linked abstractions that are maximally legible to you as an agent.

## Outcome and scope

An agent can establish what is true, select the smallest useful authorized
action, observe its result, explain discrepancies and retain reusable evidence
without reconstructing the system from prose or guessing across interfaces.
Human CLI, MCP and debugger users benefit from the same semantic owners.

Optimize **verified outcomes per unit of total work**: tool calls, context
bytes, latency, execution cost and recovery effort. Minimizing output while
concealing uncertainty is a regression. Adding machinery without demonstrating
a better operator outcome is also a regression.

This is a design proposal. No Arazzo syntax, expression meaning, accepted
conformance decision or existing artifact format changes here. It does not
introduce an autonomous planner, hosted memory service, remote session manager,
transparent retry, arbitrary resume, dependency installation or release work.
Frozen/decomposed plans and current ticket criteria remain authoritative.

## Operating loop

| Operator question | System answer | Next decision |
|---|---|---|
| What can this installed tool do? | Compact capabilities, versions and contract references | Choose one supported surface |
| What does this document require? | Scoped inspection, input requirements, diagnostics and support gaps | Supply data or repair the document |
| What would this invocation do? | Preview with effects, unresolved dependencies and limits | Execute only within authority and budget |
| What happened? | Runtime result plus effect and artifact outcomes | Stop, inspect or seek external confirmation |
| Why did it happen? | Bounded causal evidence tied to source and attempt | Correct the smallest demonstrated cause |
| What can be reused? | Explicitly retained, versioned evidence with validity conditions | Recheck assumptions before the next action |

The loop may stop at any boundary. Unsupported, denied, incomplete and unknown
are useful answers, not invitations to guess. Suggested next actions are
affordances, not instructions from workflow content and not additional authority.

## Grounding and ownership

The [system map](../../docs/system-architecture.md) owns the abstraction ladder. Relevant
checked evidence on 2026-11-05:

| Existing seam | Consequence for the design |
|---|---|
| CLI [`cli.rs`](../../crates/arazzo-cli/src/cli.rs) and [`main.rs`](../../crates/arazzo-cli/src/main.rs) | Agent commands/manifest/inspect are not implemented. CLI grammar and exit behavior already have owners. |
| CLI [`workflow_catalog.rs`](../../crates/arazzo-cli/src/workflow_catalog.rs) and MCP [`state.rs`](../../crates/arazzo-mcp/src/state.rs) | Catalog discovery has distinct shallow/recursive behavior; consolidation must preserve existing contracts. |
| Runtime [`events.rs`](../../crates/arazzo-runtime/src/runtime_core/events.rs) | Progress, capture and terminal result already exist; no second execution engine is needed. Public streaming access must be checked before promising an adapter. |
| Runtime [`source_document.rs`](../../crates/arazzo-runtime/src/runtime_core/source_document.rs) | The loader currently covers local OpenAPI, not every transitive source. Read-only does not imply policy-complete. |
| [`run.v1`](../../docs/run-export-v1.md), [`trace.v1`](../../docs/trace-schema-v1.md), [replay decision](../assessments/replay-fidelity-decision.md) | Artifact purposes, missing capture and post-execution failures must remain distinguishable. |
| [MCP hardening draft](mcp-security-hardening.md) | Security and resource policy have a separate owner and unfinished engineering gates. |

These are source-grounded facts, not claims about a released binary or completed
security controls. Existing uncommitted implementation work was not modified.

## Architectural decisions

### 1. One semantic owner, progressively richer projections

Document semantics stay in spec/validation/expression owners; admission,
execution and observation stay in runtime. CLI/MCP/DAP/TUI project those facts
according to their own protocol and trust boundary. No adapter infers success
from a log line or reimplements target classification or expression evaluation.

Discovery proceeds from a compact index to one command/workflow, then to its
schema or evidence. Full manifests are explicit exports, not the default prompt
payload. Arguments and schemas derive from their executable definitions;
effect descriptions are typed semantic annotations checked against behavior.
The [discovery plan](agent-discovery-and-inspection.md) owns this first slice,
including the proposed shared inspection unit and its dependency direction.

### 2. Describe an invocation, not a command's reputation

Effects are a set: file reads/writes, network activity, broker interaction,
credential use and external mutation uncertainty can coexist. Distinguish
capability, requested configuration and enforced policy. A restrictive MCP
configuration must not imply that ordinary CLI execution has the same policy.

Preview suppresses transport dispatch but can still read source files, evaluate
data, consume resources and write requested artifacts. A method name does not
establish idempotency. Generated examples are suggestions, never proof that
an input is valid or that an operation is safe.

Expose effective limits and their enforcement status; do not advertise a
limit that is merely planned. Distinguish exact structural counts from runtime
estimates and unknowns. Loops, retries, dynamic routing and server behavior can
make a total request/cost estimate unknown. Child work shares the parent budget;
it cannot reset a ceiling by nesting or retrying. Enforcement remains with the
resource owner specified by the accepted policy, not with discovery metadata.

### 3. Preserve knowledge state and identity

Observations identify their subject, producer/version, mode, relevant source
revision and completeness. A future evidence view must distinguish authored,
resolved, simulated and observed facts; present, missing, redacted, unavailable
and not-captured values; and errors from unsupported capability or denied access.
These are metadata distinctions, not new Arazzo values or a replacement for
accepted missing/null/Selector semantics.

A workflow is identified within its document, not globally by display name.
A run's child invocation, step and attempt are distinct identities. Additive
correlation must be versioned at each affected contract; existing `run.v1`
grouping cannot reconstruct child invocation identity it never captured.
Readers of old artifacts report that limitation rather than invent identifiers.

Inspection describes a captured snapshot. Execution validates the actual admitted
snapshot, inputs, configuration and policy; newly acquired dependencies are
checked at acquisition. MCP's startup-loaded root remains that admitted root,
not an implicit reload from disk. Reacquired bytes must be identified afresh.
A future strict
preview-binding mode must reject changed dependencies, never silently run a
different plan. No binding token claims that remote state stayed unchanged or
that preview and dispatch are a transaction. Secret-bearing inputs/headers must
not appear in public fingerprints or guessable unkeyed digests.

### 4. Make control and recovery match reality

The runtime owns execution state; adapters expose only controls they actually
support. Cancellation requested, cancellation settled, timeout and ordinary
failure are distinct. Debug pause/resume is not crash recovery. Process exit,
lost client, absent event or failed artifact write cannot establish that remote
effects did not happen.

For future result projections, distinguish runtime outcome, known dispatch
facts, uncertain external effects, and artifact production. Existing envelopes
remain intact. Recovery advice carries preconditions and evidence references.
After a send with no conclusive response, recommend checking external state;
never automatically rerun a mutating workflow. Existing document-directed retry
semantics remain unchanged; this proposal adds no adapter retry layer.

### 5. Explain causally and retrieve selectively

Connect diagnostics to document location, workflow/step/attempt, phase and an
existing stable code where available. Follow data provenance from a missing
request value to its producer; distinguish data dependencies from scheduling
and action-routing edges. This is an evidence view over existing semantics,
not a second graph that decides execution.

Default to a compact result with explicit counts and references to deeper
evidence. Fetch one failed attempt, unresolved input or selected output when
needed. A projection identifies omitted fields; omission is not absence.
Pagination, when introduced, binds to a snapshot and rejects stale cursors.
Transport-specific size policy wins: MCP's approved whole-response failure
must not quietly become truncation or partial success. New retrieval contracts
need their own accepted slice before changing an existing response.

Raw document descriptions, response text and saved artifacts are untrusted
data. Generated next actions use typed argument vectors or tool arguments,
not interpolated shell strings. Redaction and access checks apply before
material is retained or projected; an evidence reference cannot bypass them.

### 6. Accumulate evidence, not unquestioned conclusions

Use existing trace, run export and hermetic test fixtures as the initial durable
substrate. Retention is explicit and local; no automatic capture of bodies,
credentials or cross-project memory. A useful retained case records its source,
producer version, purpose, capture/redaction limits, expected observation and
conditions that invalidate it. Transient debugging artifacts need not become
permanent fixtures.

A reusable regression is a minimized, reviewed fixture with an independently
stated expected result. A live response is not automatically a test oracle.
Replay may answer a narrower question when bodies or identity are missing;
the system must state that question. Evidence cannot grant access or promote
untrusted text into agent instructions.

Cache only measured expensive pure work initially. Key reuse by relevant source
content/dependencies and semantic version, with bounded retention. Do not cache
authorization, credentials, live readiness or previous success as permission.
Prefer process-local caches; persistent caches and cross-session records need
separate privacy, invalidation and migration decisions. Existing performance
plans retain their workloads and completion conditions.

## Delivery order and connected plans

| Order | Complete operator improvement | Boundary and dependency |
|---|---|---|
| 1 | Discover supported syntax and inspect one document without guessing | [Discovery/inspection draft](agent-discovery-and-inspection.md); establish baseline first |
| 2 | Preview with honest unknowns and effective invocation effects | Build on accepted request admission; settle new metadata schema and runtime capability queries before decomposition |
| 3 | Locate a failure and choose recovery without replaying effects | Version correlation/evidence projections; retain existing trace/export compatibility and MCP limits |
| 4 | Reuse one reviewed case with fewer calls and no stale assumptions | Local artifact/fixture workflow; measure before adding persistent cache or a knowledge index |
| Optional consumer | Browse and control the same facts visually | [TUI draft](arazzo-workflow-explorer.md); no separate scheduler or admission policy |

Security hardening remains independently necessary; discovery can be designed
without waiting for all enforcement, but restricted MCP integration must use
its approved mechanisms and cannot declare them complete. Accepted conformance,
request-admission, benchmark and decomposition plans retain their scope and
criteria. Any material change to them routes through re-decomposition rather
than this umbrella silently superseding them.

## System verification and resource economics

Use one immutable candidate and hermetic endpoints for a repeatable operator
exercise: cold discovery, missing input, valid-but-unsupported document,
duplicate workflow IDs, preview with response-dependent data, controlled
success, ambiguous send/timeout, artifact-write failure, source change after
inspection and a second attempt using retained evidence. Include a permitted
message workflow and a denied destination when the relevant transport/policy
slice is integrated. Observe sends independently of reported success.

Compare the current CLI/MCP workflow with the proposed surface on the same
tasks. Record correctness, forbidden sends, unsupported guesses, duplicate
effects, command/tool calls, returned bytes, context tokens with named tokenizer,
wall time and retained artifact size. Record model/version and instructions for
agent trials; scripted protocol checks establish repeatability separately.
Separate cold/warm performance and valid samples; do not claim a timing win on
a noisy shared machine or compare mismatched source candidates.

**System success:** every scenario produces the correct decision or explicit
uncertainty, no unintended send or automatic duplicate execution, and sufficient
source/attempt evidence to explain it. The cold supported path needs only a
compact discovery call, scoped inspection and preview before execution;
schema detail is fetched only when needed. Across the fixed scenario set, total
discovery/diagnosis calls and returned bytes must both decrease from baseline
without lower task completion or missing required evidence. Retained evidence
must shorten the repeated investigation and be rejected when invalidated.
Baseline values and measured deltas accompany the final system proof, not an
invented speedup target. Latency/RSS regressions require explanation and an
explicit acceptance decision; fewer tokens alone do not establish success.

## Disposition

**Draft — 2026-11-05.** Documentation revision is authorized; implementation
acceptance and decomposition have not occurred. Proposed ownership and
progressive disclosure are decided by recommendation for this draft. Public
metadata/correlation contracts and new inspection-unit boundaries require
acceptance before implementation; later slices require bounded follow-on plans.
No existing pause, ticket criterion, security policy or publication gate changes.

Independent fresh-context design review completed with one bounded recheck.
Corrections made the canonical navigation portable to a tracked checkout and
connected qualified MCP inspection to preview/execution; snapshot and failure
envelope wording was tightened. No concrete defects remained in that recheck.
This is design review, not implementation acceptance or runtime verification.
