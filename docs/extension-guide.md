# Extension guide

Start with the [system map](system-architecture.md) and the
[runtime flow](architecture.md). Add a capability at its semantic owner, then
project its facts into the affected interfaces. A command, tool or view is not
a new execution engine.

## Choose the owner before the entry point

| Concern | Owner |
|---|---|
| Document shape and target classification | `arazzo-spec`, governed by vendored specification text |
| Validation and its diagnostics | `arazzo-validate` |
| Expression evaluation and resolution provenance | `arazzo-expr` |
| Binding, admission, scheduling, transport, capture and cancellation | Focused `arazzo-runtime` owner |
| CLI syntax and process exit/error presentation | `cli.rs`, shell wiring and `cli_error.rs` |
| CLI use-case behavior | Existing command owner; `handlers.rs` delegates substantial responsibilities |
| MCP argument/policy/framing/result adaptation | Focused `arazzo-mcp` owners |
| DAP session/protocol adaptation | `arazzo-debug-adapter`; runtime owns debug control |
| OpenAPI generation | `arazzo-generate` |

Reuse existing public surfaces. Do not copy a parser, classifier or scheduler
into an adapter to save a dependency discussion. A shared responsibility needs
an explicit unit boundary; a helper named `utils` does not establish one.
Repository legacy-module safeguards apply before extending large modules.

## Add a CLI capability

1. Add syntax in `crates/arazzo-cli/src/cli.rs` and thin dispatch in `main.rs`.
2. Put behavior in its focused existing owner. Run, generation, test and workflow
   catalog already have owners outside the generic handler facade.
3. Keep ordinary output models/rendering with the output owner, while trace and
   run-export serialization/persistence retain their dedicated owners.
4. Return errors through `CliError`/`CliFailure`; let the shell print them.
   New command failures use the canonical JSON error object, with optional
   command-specific fields. Preserve established result-reporting behavior.
5. Generate affected schemas from their types and verify the relevant CLI,
   schema and contract snapshots. Understand changed output before refreshing
   a baseline.

Run flags flow through `RunOptions`/`RunContext` to the execution owner. Test
their combinations, effective behavior and failure paths at that boundary.

The [discovery draft](../plans/current/agent-discovery-and-inspection.md) proposes
grammar-derived metadata and shared inspection. Those surfaces do not exist
yet. Once accepted and implemented, every new capability must update its
semantic annotations and prove that advertised effects match actual behavior.
Do not hand-maintain a competing command schema now.

## Propagate meaning, not just fields

For an affected capability, follow its path from authored input through admission
and execution to public output. Check each actual consumer: CLI, MCP, DAP,
capture/export and replay where applicable. A new runtime fact is not delivered
to agents merely because it exists in a private struct.

Keep validation, supported capability, configured permission and observed outcome
distinct. Preserve missing-value provenance through consumers; presentation
redaction must not change execution data. Document unsupported behavior and
unknowns instead of claiming readiness from a successful parse.

Bind diagnostics and evidence to the narrowest identity available. Do not invent
child-invocation identity from older artifacts or turn incomplete replay into an
equivalence claim. The accepted
[replay fidelity decision](../plans/assessments/replay-fidelity-decision.md)
owns that target contract.

## Change execution, capture or debug control

Runtime owns lifecycle events, settled attempts, final results and cancellation.
Adapters project them; they do not retry failed sends independently or treat
lost observations as proof of rollback. Preserve deterministic logical ordering
without claiming deterministic wall-clock completion.

Use the [internal API policy](internal-api-v1.md),
[trace contract](trace-schema-v1.md) and [run export contract](run-export-v1.md)
for compatibility. New fields need end-to-end redaction and serialization proof.
Breaking changes need versioning and migration. A new streaming consumer must
first have a usable public subscription and bounded lifetime contract; private
channels are not an integration API.

Debug behavior belongs under runtime debug control; DAP transcript tests verify
its protocol projection. Update the editor integration after runtime/adapter
contracts are settled.

## Verify the operator outcome

Exercise a complete, bounded workflow through the changed public surface:
discover/configure when relevant, admit, act, observe and explain. Include the
meaningful rejection path and independently observe side effects where that is
the failure risk. Retain compatibility controls for affected consumers.

For an ergonomics or performance claim, compare the same task and candidate
conditions. Measure calls, bytes and valid timing samples without discarding
required evidence. One reviewed regression fixture is more reusable than a
large transcript with no stable expected result.

Repository instructions own mandatory build gates and lifecycle; this guide
does not duplicate or weaken them. Documentation-only changes need link,
authority and current-versus-proposed checks; do not describe those as runtime
verification.
