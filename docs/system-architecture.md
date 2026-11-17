# Arazzo as an observable execution system

This is the system map, grounded in the checkout on 2026-11-05. It replaces the
obsolete synchronous-engine description. It describes existing ownership;
proposed additions are explicitly linked as drafts. Source inspection is not
release verification, especially while the checkout contains uncommitted work.

## Start with the question

| What do I need to know? | Read next |
|---|---|
| What does the system mean, and who owns each decision? | This document |
| How do execution, capture and queries connect today? | [Runtime architecture](architecture.md) |
| How should an agent operate and improve the system? | [Agent operability plan](../plans/current/agent-operability.md), **Draft** |
| How will machine discovery and inspection work? | [Discovery and inspection plan](../plans/current/agent-discovery-and-inspection.md), **Draft** |
| What is the execution/replay evidence contract? | [Run export](run-export-v1.md), [trace](trace-schema-v2.md), [internal API](internal-api-v1.md) |
| What is permitted Arazzo? | [Vendored specifications](../spec/README.md), then the [conformance audit](../plans/assessments/arazzo-spec-conformance-audit.md) |
| What is the delivery direction? | [Roadmap](../plans/roadmap.md); live work state belongs to `tkt` |

Read one level deeper only to answer an unresolved question. Generated schemas
and current implementation establish available behavior; a plan establishes
desired behavior only within its recorded authority. A successful parse, an
accepted plan and a verified execution answer different questions.

## The abstraction ladder

The product interprets a workflow into effects and evidence. Its interfaces
should let an operator travel in both directions: from intent to an action, and
from an observed failure back to the decision and source that produced it.

| Level | Meaning | Current owner |
|---|---|---|
| Document | Authored workflows, steps, expressions and references | `arazzo-spec`; published semantics in `spec/` |
| Admitted document | Parsed structure, resolved components and diagnostics | `arazzo-validate` |
| Resolved value | Evaluated data, selection, missing-value provenance | `arazzo-expr`; runtime supplies context and consumes provenance |
| Execution decision | Dependency scheduling, target binding, request admission, criteria and action routing | `arazzo-runtime` |
| Controlled effect | HTTP/message exchange with cancellation and configured transport constraints | Runtime transport owners |
| Observation | Events, settled attempts, declared outputs, warnings and terminal result | Runtime event/capture owners |
| Presentation and retention | CLI JSON, MCP results, DAP views, redacted trace/export artifacts | Respective adapters |

These are responsibilities, not instructions to create seven new frameworks.
Each layer consumes the narrow published surface below it. Presentation never
becomes a second evaluator, scheduler or authority over whether a request may
be sent. An observation cannot retroactively authorize an effect.

## Units and dependency direction

The workspace has eight members. The manifests are the authority for exact
dependency edges; this table identifies responsibility and direction.

| Unit | Owns | Consumes |
|---|---|---|
| `arazzo-spec` | Document model, byte parsing, canonical operation-target classification | External serialization/parsing libraries |
| `arazzo-validate` | Structural and semantic diagnostics | Spec model |
| `arazzo-expr` | Expression classification, evaluation, typed JSONPath and provenance | Evaluation inputs; no runtime dependency |
| `arazzo-runtime` | Source binding, execution, state, transports, capture and debug control | Spec, validation, expressions |
| `arazzo-generate` | OpenAPI ingestion and workflow/example generation | Spec model and OpenAPI libraries |
| `arazzo-mcp` | MCP framing, tool arguments, server configuration and result projection | Runtime, validation, spec, generation |
| `arazzo-debug-adapter` | DAP sessions and protocol projection | Runtime debug API, spec and validation |
| `arazzo-cli` | Command arguments, use-case orchestration, stdout and artifact persistence | Runtime, validation, spec, generation and MCP hosting |

The CLI hosts MCP; MCP does not depend on CLI handlers. The proposed shared
inspection unit is described in its draft plan and is **not** a ninth member
today. New cross-surface behavior must not be hidden in a binary's handler file
or in an unrelated model crate to avoid declaring its real owner.

## One execution, several views

The CLI starts Tokio and delegates command behavior to its command owners.
`EngineBuilder` binds source documents and configures execution. `Engine`
returns an `ExecutionHandle`; events and the final result have distinct channels,
and the handle carries cancellation. Debug control is inside the runtime;
DAP translates it. HTTP and message steps share the workflow's execution
semantics while retaining distinct transport implementations.

Runtime evidence has several purposes:

- Events describe progress and decisions. Their presence alone does not prove
  terminal success or that an external service committed a change.
- Trace records support diagnosis and replay within captured fidelity.
- Run records retain settled attempts and declared outputs without implicitly
  retaining undeclared bodies.
- The terminal runtime result answers how execution ended. A subsequent output
  or artifact-write failure can still make the CLI command fail.

The [runtime architecture](architecture.md) owns the detailed flow.
The [run export contract](run-export-v1.md) owns `run.v1`; trace/replay have
their own contracts. Do not merge these formats just because they describe
the same run. Reuse facts and correlation, not incompatible wire envelopes.

## Boundaries that make the system understandable

**Specification versus capability.** A document may be valid yet require an
unsupported capability. The vendored specification determines validity;
implementation support and configured policy determine executability. Version
strings, CLI help and old examples are not conformance evidence.

**Values versus knowledge about values.** Intentional null, missing data,
transport-skipped preview data and redacted presentation are different states.
Keep that distinction at the owning boundary. Follow the accepted
[request-value contract](../plans/current/arazzo-request-value-admission.md);
do not infer new document semantics from a presentation marker.

**Preparation versus observation.** Dry-run constructs simulated execution data;
it does not establish a response-dependent branch, external authorization,
future server behavior or replay completeness. Prepared-request metadata is
not necessarily a full wire request. Label those limits where decisions are made.

**Policy versus description.** A tool description informs the caller. The
resource owner enforces access, deadlines and limits. MCP's
trust model is stated in [SECURITY.md](../SECURITY.md); unresolved hardening
mechanisms are open work; architectural prose must not report them as delivered.

**Cancellation versus rollback.** Stopping local work does not undo effects
already dispatched. Re-running after a timeout or reporting failure can repeat
effects. A missing result is uncertainty, not a successful cancellation receipt.

**Evidence versus memory.** Saved examples and previous runs accelerate the
next investigation only when their provenance, limitations and applicability
remain visible. They never override the current document, policy or observation.

## Extension discipline

Add one capability at its semantic owner, then project it into affected adapters.
Carry its rejection behavior and evidence through the same path. Preserve
versioned JSON, runtime error codes and internal APIs unless a separately
accepted contract change says otherwise. Keep protocol-specific lifecycle and
privacy rules in their adapter; share runtime facts rather than identical JSON.

Prefer a small complete operator workflow over an inventory of disconnected
features. For example, discovering a workflow, identifying a missing input,
previewing it, observing a controlled run and locating a failed attempt is a
useful slice. A catalog alone cannot prove that loop works.

The draft [agent operability plan](../plans/current/agent-operability.md) applies
these boundaries to progressive discovery, trustworthy action selection,
bounded evidence retrieval and reusable knowledge. It does not change the
current Arazzo grammar, enable new commands, or authorize implementation.
