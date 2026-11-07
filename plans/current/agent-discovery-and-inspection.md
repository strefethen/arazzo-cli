---
status: draft
epic:
created: 2026-11-05
---

# Agent discovery and scoped inspection

**Seed:** Steve's system-wide agent-ergonomics request is retained in the
[umbrella plan](agent-operability.md#agent-operability-as-a-system-property).
This replaces the stale implementation recipe formerly in
`docs/future/agent-first-command-discovery.md`. It is a draft, not a claim that
the commands below exist or that prior ticket contracts have been revised.

## Outcome

An agent discovers supported invocations, then obtains the minimum reliable
context for one workflow without scraping help, scanning unrelated documents
or reading the entire schema bundle. Human help remains ordinary Clap help.
Machine discovery is additive; existing command outputs and errors stay stable.

## User-visible workflows

Proposed surfaces, with exact flags/schema finalized at acceptance:

| Surface | Default answer | Explicit expansion |
|---|---|---|
| `agent commands` | Compact command IDs, intent, effect categories and detail references | One command's arguments, constraints and output schema reference |
| `agent manifest` | Explicit full export of the same catalog and protocol facts | Full bundle for offline tooling; never required on every invocation |
| `schema --index` | Stable names and schema versions/retrieval references | `schema <name>` or explicit `schema --all` |
| `inspect <file>` | Document identity, validation/support status and compact workflow summaries | One qualified workflow's inputs, steps, diagnostics and dependency summary |
| `inspect <directory>` | Deterministic bounded catalog plus scan completeness | Explicit workflow/document selection; no implicit live execution |

Machine output is versioned JSON. Successful summaries include enough identity
and detail references to continue without reconstructing an invocation from
prose. No TOON format, agent-specific help injection, live credential check,
network probe or automatic execution is part of this slice.

## Current facts and compatibility

[`cli.rs`](../../crates/arazzo-cli/src/cli.rs) owns Clap definitions; these new
commands are absent. Existing agent notes are committed source, not an
uncommitted patch to revert. Help cleanup must be a deliberate reviewed change
with refreshed relevant snapshots, not instructions to discard someone else's
edits. [`main.rs`](../../crates/arazzo-cli/src/main.rs) owns process exits:
JSON usage errors exit 2; ordinary command failures exit 1. Do not publish a
manifest claiming that every failure exits 1 or every command emits JSON.

[`workflow_catalog.rs`](../../crates/arazzo-cli/src/workflow_catalog.rs) owns
shallow CLI catalog/lookup; MCP discovery has different recursive behavior.
Preserve those legacy contracts. The new inspect mode explicitly declares its
discovery scope. Source descriptions may require additional files; the current
runtime loader is not a universal authorization boundary.

## Owners and published surfaces

**CLI grammar and metadata:** a focused CLI command-discovery owner derives
syntax from Clap's command graph, including argument arity/types, required and
conditional constraints, conflicts, defaults and enum domains. Secrets and
environment-derived values are never exported as defaults. Typed semantic
annotations supply intent, mode-dependent effects, protocol use and recovery
references that Clap cannot know. Missing annotations fail completeness checks;
they never default to safe. Existing output models generate schemas; index and
bundle enumerate that same registry.

**Shared read model:** propose `arazzo-inspect` as a small library unit owning
document/workflow identities, discovery results, inspection summaries and their
diagnostic references. CLI and MCP are its first consumers in one coherent
delivery; it is not added for a hypothetical TUI alone. It consumes `arazzo-spec`,
`arazzo-validate` and only published read-only runtime admission/capability queries
needed to report support. It owns no workflow execution, network transport,
credentials, protocol envelopes, shell rendering or persistent store.

Dependency direction is adapters → inspect → spec/validate/runtime → expr.
Runtime must not depend on inspect or either adapter. Runtime retains target
binding/admission facts; inspect does not infer executability merely from a
successful parse or duplicate runtime classifiers.

**Acquisition and policy:** inspect consumes bytes plus opaque document identity
from a caller-provided bounded acquisition/discovery interface. The adapter
owns directory traversal and authorization; the library never takes a checked
path and reopens it. CLI supplies local reads under its declared scope. MCP
supplies its approved pinned-root acquisition and safe diagnostic projection.
Shared semantic results may differ in detail visibility after that projection.
MCP wire limits and whole-operation failure policy remain unchanged.

**Protocol adapters:** CLI owns JSON/schema/human rendering; MCP owns tool
definitions and MCP result projection. Transport argument names may differ,
but qualified workflow selection, input requirements and support reasons must
agree for the same admitted bytes and policy. Do not make CLI call MCP handlers
to obtain a read model, or add introspection responsibilities to runtime's
execution facade. New public runtime query signatures and inspection DTOs must
be settled in a bounded contract refinement before this draft is accepted.

**Selection-to-execution handoff:** inspection returns an adapter-usable target
reference. CLI uses its existing spec path plus workflow ID. For MCP, propose an
additive optional `document_id` argument on `describe_workflow` and
`run_workflow`, selecting only a server-authorized loaded registration. It is an
opaque session reference, never an arbitrary path or permission to load another
document. Without it, retain current unique-ID lookup and ambiguity rejection.
With it, an unknown/stale registration fails; never fall back to another matching
name. Inspection of a file outside the loaded execution catalog must explicitly
report that it cannot be run through that MCP session. Existing bare-ID callers
and result envelopes retain their contracts. Exact reference representation,
discovery projection, tool-schema evolution and stale-reference errors are part
of acceptance, and the shared inspection slice must prove selection → preview
→ execution for duplicate IDs using the intended admitted root snapshot.

## Behavioral contract

**Validity is not readiness.** Return separate validation, capability and
configuration assessments, each with checked/not-checked/failed scope. Without
inputs or loaded source bytes, dependent conclusions remain unknown. No global
`ready: true` inferred from structural validity. Separate warnings from errors;
strict validation continues to mean what the existing CLI defines.

**Qualified identity.** Use document identity plus workflow ID. Duplicate names
are explicit candidates; selection must never take the first match. Relative
paths are display/access locators, not immutable identity. Within one inspection,
read each authorized document once and derive its views from those bytes.
Reinspection identifies newly read bytes; execution validates its actual admitted
snapshot and dependencies. MCP references select startup-loaded roots rather
than causing a filesystem reload. No durable cross-session reference is promised.

**Bounded scope.** Direct files are never discovered by extension filtering.
Directory inspection uses an explicitly reported pattern/depth policy; defaults
follow `.arazzo.yaml`/`.arazzo.yml` discovery and do not silently widen existing
catalog behavior. It reports included, invalid, inaccessible and excluded counts
where the adapter is authorized to disclose them. The new inspection traversal
does not follow symlink descendants. It accepts caller-enforced document/byte
budgets; exact CLI defaults and matching MCP charge points are an acceptance
gate, not an invented universal limit.

**Honest completeness.** Compact metadata is the first response; full input
schemas, steps and values require selection. A diagnostic-bearing directory
scan can report discovered entries but must mark incomplete/invalid status and
exit nonzero when requested coverage was not achieved. Every CLI failure uses
the existing `CliError` envelope (`kind:error`, `error`, `code`); bounded per-file
diagnostics and coverage may be optional command-specific fields on that object,
never a separate success-shaped result. Root failure or budget exhaustion returns
a complete command error, never a successful prefix. No
pagination in this slice; oversized selections must be narrowed explicitly.
Detailed per-file diagnostic projection is a CLI contract; it must not weaken
MCP's whole-operation failure or leak denied paths.

**Effective effects.** Catalog metadata describes possible effects and the
conditions enabling them. `run --dry-run` suppresses external dispatch; requested
trace/export can still write files. `serve` reserves stdout for a protocol, not
an ordinary JSON command response. Execution support, configured access and
unknown external mutation risk are separate facts. Read-only inspection does
not resolve environment secrets or initiate transport to test readiness.

**Recovery references.** Advice uses structured tool arguments/argv templates
with typed placeholders, existing diagnostic codes and preconditions. It never
embeds secrets or interpolates a document description into a shell command.
Existing error envelopes receive no new required fields. A hint is advisory;
the caller's authority and current observed state still govern its use.

## Required order and verification

First record a baseline operator walkthrough using existing catalog/list/show,
steps/schema and preview surfaces. Settle the shared read API, runtime query
surface, exact inspection limits, qualified selectors and new JSON versions.
Then deliver grammar-derived discovery plus schema indexing, followed by the
shared inspection unit with CLI and MCP consumers, qualified target handoff and
parity proof. Human help
cleanup follows the usable replacement; documentation must distinguish installed
capability from planned syntax throughout rollout.

Verify discovery against the real Clap graph, schemas against their generating
types, and semantic effect annotations against representative invocations.
Hermetic tests exercise duplicate IDs, malformed/inaccessible documents,
unknown support, strict warnings, bounds, literal hostile text, changed source
bytes and side-effect absence during inspection. Test both allowed and denied
source acquisition, including source-kind limitations rather than assuming them
away. Existing CLI/MCP contracts and required drift guards remain controls.

**System success:** from a cold start an agent selects a supported command and
one qualified workflow, discovers missing data and actual limitations, then
forms a correct preview invocation using compact discovery plus scoped
inspection. Full manifest/schema bundles are optional. CLI/MCP semantic
answers agree for identical evidence/policy; protocol privacy differences are
explicit. Required context calls and bytes decrease against the same baseline
without lost diagnostics or unintended file/network effects.

## Disposition

**Draft — 2026-11-05.** New unit ownership is decided by recommendation for
review; shared APIs, limits and exact new wire contracts remain named acceptance
gates. This replaces an obsolete design recipe, not accepted ticket criteria.
It neither authorizes implementation nor reopens paused MCP work.
