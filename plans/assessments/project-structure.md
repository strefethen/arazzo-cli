# Cumulative project structure assessment

This is the continuing assessment for structural changes in this repository. Future sessions should update the current ownership map and append a dated evidence entry, preserving previous conclusions and distinguishing verified changes from proposed work. The source and executable contracts remain authoritative; this document is an assessment, not implementation authority.

## Current ownership map

| Unit | Responsibility | Dependency boundary |
|---|---|---|
| `arazzo-spec` | Typed Arazzo document and canonical operation/source/dependency reference classifiers | Does not depend on execution or presentation |
| `arazzo-validate` | Parsing and document diagnostics | Consumes model/expression contracts; no run artifact policy |
| `arazzo-expr` | Runtime expression and condition evaluation | No CLI or runtime execution dependency |
| `arazzo-runtime` | HTTP execution, scheduling, attempt facts, debug controller | Consumes model/evaluator; no filesystem export format policy |
| `arazzo-cli` | Command parsing, execution orchestration, user output and persisted artifacts | Consumes published runtime API; owns CLI-only schemas/formats |
| `arazzo-generate` | OpenAPI-to-Arazzo generation | Independent of execution history |
| `arazzo-debug-adapter` | DAP protocol and debugger presentation | Consumes runtime/debugger contracts |
| `arazzo-mcp` | MCP protocol and workflow exposure | Consumes runtime and validation contracts |

## Structural rules for continuation

- Keep runtime execution facts separate from persisted presentation formats. New rendering consumers should consume the versioned run artifact or published capture types, not reach into `VarStore` or parse trace internals.
- Put attempt capture in the shared attempt protocol, never independently in sequential, parallel, and single-step schedulers.
- Keep run orchestration, run export, replay trace, and ordinary stdout formatting distinct. A command dispatch file may delegate; it must not accumulate another feature's grouping, redaction, or persistence policy.
- Add focused integration targets for new concerns. Do not add new export cases to the existing large `engine_execution.rs` or `cli_integration.rs` suites. Reuse existing cohesive test support where it fits.
- New files belong in existing crates unless a demonstrated dependency or encapsulation boundary requires a new unit. Avoid generic `utils`/`common` production buckets and unnecessary dependencies.
- When extending export metadata, apply redaction to every retained copy of a value, not just its source header map. Destination checks must account for directories another requested writer may create, resolve prospective `..` aliases before execution, preserve existing symlink semantics, and use actual filesystem naming equivalence rather than unconditional case folding.
- `god-files.md` owns known legacy-module findings; this assessment refers to that inventory rather than duplicating it. Any claim of resolution needs current source evidence.

## 2026-10-09 — JSON run export

Assessment started at **2026-10-09T20:31:21Z** (**13:31:21 America/Los_Angeles**). Authority: Steve's request to implement `--export-run`, use Tier 2 implementation sub-agents, and leave a cumulative assessment. Initiative: [JSON run export](../completed/json-run-export.md), [epic:ac-bd06d](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd06d).

### Baseline observed before implementation

- Checkout: `main`, source baseline `bd0647bd3a51542b105cba4047a9770ab9fd7595`, aligned with the local `origin/main` tracking ref. No push/fetch was performed; this is not a claim about the live remote.
- Eight workspace crates already provide a useful separation among document contracts, evaluator, execution, CLI, generation and protocols. No new crate is required for run export.
- `runtime_core/step_attempt.rs` owns the shared attempt lifecycle. `events.rs::ExecutionResult` collects events. Those seams allow opt-in execution capture without introducing scheduler-specific collectors.
- `handlers.rs::run_workflow` combines run configuration, execution, trace handling, diagnostics and stdout dispatch. A bounded extraction to a run-specific owner makes the new export integration comprehensible without assigning export policy to that broad file.
- `trace.rs` owns debugging/replay evidence, including body previews and truncation. Reusing its artifact as a run-data contract would couple data inspection to body capture and preview limits. The selected design keeps their contracts separate.
- Measured baseline: CLI `handlers.rs` 1,000 lines, `output.rs` 897, `main.rs` 554; runtime `events.rs` 583, `step_attempt.rs` 481 and `state.rs` 409. These are navigation/size signals, not independent God-module findings.
- The index was empty. Five tracked debugger/expression files had unrelated edits, and `carried-baggage.md` was untracked. A binary diff of those five paths was retained outside the repository for preservation verification. Their presence must be disclosed in test evidence.
- Live reservations were empty and no ticket was in progress. Ticket overlap queries found no existing run-export initiative; the separate URL-userinfo-redaction repair [ac-d95a9](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d95a9) remains outside this initiative.

### Selected placement and review

- [ac-b9a4e](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b9a4e): focused body-free runtime capture owner and a focused runtime integration target.
- [ac-c6856](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c6856): bounded run orchestration extraction, focused export contract/persistence owner, a focused CLI integration target, schema and user documentation.
- Both implementation tickets assign Modify/Create/Must-not-grow responsibilities. Source writes are serialized; independent reviews use fresh frontier contexts. GPT-6 Sol/high implements each settled ticket; GPT-6 Astra/high performs plan, transfer, and candidate reviews.
- Fresh plan review settled runtime outcome versus final command outcome, recursive text redaction in JSON string leaves, emission-order sequence semantics and absent metadata on early transport errors. The fresh transfer audit found complete coverage and coherent ownership; its symbol correction was applied before kickoff.
- During runtime verification, source inspection found that existing failure messages embed body previews. The capture/export contract requires omitting those previews without altering ordinary runtime errors or trace contracts; failure/nested-call regression cases are required. Failed dry-run attempts must remain failures rather than being labeled successful simulations.
- CLI verification confirmed that captured request headers describe the prepared step request. Run-wide defaults (including CLI `-H`) and transport-injected headers are applied later and are absent from that map. Consumers must not treat it as a complete wire-header dump or invent missing headers in the CLI. Response headers come from the received response; `contentType` is the runtime classification, with the received MIME value available in the redacted response headers.
- Preservation checks observed one documentation-comment change in the unrelated, already-dirty expression file during this session. The implementation agent confirmed it did not make that edit. Preserve the latest external WIP rather than restoring the initial snapshot.

### Completion evidence

Runtime capture completed in [5c4de46](https://github.com/strefethen/arazzo-cli/commit/5c4de4624756fc176e399f8d3457a11bb60b162f) (local; not pushed), independently reviewed by a fresh GPT-6 Astra/high context and closed through `tkt`. The new `runtime_core/run_record.rs` owner is 136 lines; focused `tests/run_capture.rs` is 485 lines. Seven capture tests pass in debug and release, as do workspace check/fmt/strict clippy/tests and runtime rustdoc. The public seam is `EngineBuilder::capture_run`, `EngineEvent::RunStep`, the record/metadata/outcome types, and `ExecutionResult::run_steps`. Context initializer edits are the only scheduler changes. Runtime rustdoc reports one pre-existing private-link warning in unchanged `engine_http.rs`.

The CLI extraction gives run execution a named owner instead of widening the command dispatcher. The export module owns this artifact's model, conversion/redaction and atomic persistence; its private `paths.rs` owns destination identity. These responsibilities share one versioned contract; no general filesystem utility or new dependency is introduced. The existing stdout owner and large integration targets remain untouched by the feature. User-facing semantics live in `docs/run-export-v1.md`, the distributed schema has the existing schema drift guard, and `docs/architecture.md` links the implementation boundaries to this continuing assessment.

CLI implementation is committed in [22945ba](https://github.com/strefethen/arazzo-cli/commit/22945ba527f9f1ca1d050d8b557e1aa406239069), followed by review remediation in [1a403c5](https://github.com/strefethen/arazzo-cli/commit/1a403c5eafdceb909cf46633623c1643a474eaf7) and [8dd8732](https://github.com/strefethen/arazzo-cli/commit/8dd873273c4731797113102ca037662400d8cc5f) (local; not pushed). Final workspace check, formatting, strict all-target/all-feature clippy and the full workspace suite passed: 1,268 tests passed, none failed, one existing doctest ignored. Thirteen focused export tests and nine schema drift tests passed. The schema's optional routing fields and review findings were corrected before the final gate run; earlier successful logs are not substituted for final-source proof. Raw evidence and retained ticket/start snapshots are indexed in `/tmp/arazzo-export-evidence/cli-ac-c6856/evidence.md`, with exact-final-source proof in its `*-case-final.log` set. The original successful start packet was recovered directly from this session's tool log, without reconstructing it from later ticket state.

Final source measurements for the two implementation candidates:

| File | Before | After | Assessment |
|---|---:|---:|---|
| CLI `handlers.rs` | 1,000 | 680 | Run execution extracted; delegation and schema dispatch remain |
| CLI `output.rs` | 897 | 897 | Existing stdout owner unchanged |
| CLI `main.rs` | 554 | 558 | Module and option wiring only |
| CLI `run.rs` | — | 419 | Run execution orchestration |
| CLI `run_export.rs` | — | 391 | One versioned artifact contract, conversion and persistence |
| CLI `run_export/paths.rs` | — | 225 | Private destination identity and filesystem naming checks |
| Runtime `events.rs` | 583 | 596 | New variant and filtered accessor |
| Runtime `step_attempt.rs` | 481 | 485 | Shared numbering and capture delegation |
| Runtime `state.rs` | 409 | 411 | Capture flag/counter initialization state |

Focused integration targets group scenarios around their respective published capture and CLI artifact contracts; existing broad integration targets did not grow. The new CLI target is 843 lines across 13 tests plus shared hermetic fixture support. Keep it specific to the export contract; a separate concern should get a separate target. No crate, dependency, general utility bucket or Arazzo expression extension was added. One private CLI feature folder, `run_export/`, gives filesystem identity a separate owner in `paths.rs`. It has no dependency on artifact types or runtime execution. The guarded ticket refinement adds that advisory path under the Design's existing permission to split cohesive writer boundaries; Goal, Design, Acceptance Criteria and Testing Obligations remain unchanged. Original execution snapshots are retained alongside the refinement snapshot. New source files fit the existing crate boundaries and have named owners rather than widening broad dispatch/output modules.

Verification began in the disclosed dirty checkout. Feature paths match the named candidates; unrelated Rust debugger/expression changes were present, and separate VS Code edits appeared during the session. Their owner committed those ten paths separately in `f11fa1623ea11e4c4c0274b828c2a12d641107be` during the final export work. That commit is not part of either export ticket's implementation, even though direct-on-main history places it between CLI candidates. Reviews distinguish the exact export commits/path scope from this interleaved context. Earlier verification is qualified dirty-checkout evidence, not a claim that its entire tested tree equaled the then-committed source. No reset, staging rearrangement, branch, worktree or push was used to conceal the shared state.

The first independent CLI review required two settled corrections: prospective `..` aliases could evade preflight when the trace writer later created a missing parent, and the duplicate `contentType` metadata field bypassed string-pattern redaction. Review also required explicit preservation proof for an existing regular artifact after write failure. A second fresh review confirmed those corrections and identified a distinct case-insensitive prospective-name alias. Its resolution uses the real filesystem's naming equivalence, through a narrowly owned temporary identity probe in an existing shared ancestor; it does not lowercase all paths, create user output parents/destinations, or silently ignore probe/cleanup errors. This filesystem concern motivated the private path-owner split. The third fresh cumulative GPT-6 Astra/high review passed the exact final CLI range and adopted contract, resolving all prior findings. It independently checked Unicode normalization aliases, symlinked-parent aliases, explicit filesystem errors, source preservation and owned-probe cleanup. Report: `/tmp/arazzo-export-evidence/cli-review-round3.md`.

Independent CLI QA repeated **after review at 2026-10-09T22:44:51Z** observed the exact final source candidate in a five-step hermetic workflow with six attempts, including a 503-to-200 retry. It retained all 300 declared forecast rows, preserved preceding output on final-step failure, kept ordinary JSON stdout unchanged, and emitted UTC production timestamps. Three success/failure/metadata artifacts validated against the distributed schema. Undeclared body markers and cookie/content-type fixture secrets were absent. Prospective input and case-varied trace aliases were rejected before HTTP without creating destination files. The host filesystem was case-insensitive; the automated regression also defines the distinct-name behavior for case-sensitive hosts, which was not independently run on a separate volume. Evidence: `/tmp/arazzo-export-evidence/qa-before-close.json`; fixture and inspectable artifacts: `/tmp/arazzo-export-evidence/system/`. These are session-local evidence files; durable source tests, schemas, ticket references and this assessment provide continuation context.

Both child tickets and their epic are closed/completed. After closure, independent system verification repeated the same integrated scenario at **2026-10-09T22:45:43Z** (**15:45:43 America/Los_Angeles**), with all assertions passing on the same source candidate; `/tmp/arazzo-export-evidence/system/evidence.json` records that result. Elapsed time from initial survey to this system result was **2 hours 14 minutes**. Verification dominated the session, including full workspace reruns after reviewed corrections. Final bookkeeping changes only documentation and plan references; no behavior review or workspace rerun is required for that prose-only commit, and the reviewed implementation remains unchanged.

The resulting organization is coherent: execution facts stay in runtime, CLI orchestration has its own owner, artifact representation/persistence stays in the CLI export owner, and filesystem identity is private and independent of the artifact model. The broad handler shrank by 320 lines, while stdout, trace ownership and existing large test targets did not gain export responsibilities. The versioned schema, focused regression targets and documented limitations make future rendering consumers possible without coupling them to execution internals. Review exposed material alias and redaction gaps in earlier candidates; the final candidate has no outstanding review findings. This is an improvement within the existing crate structure, not a claim that inherited module debt is resolved. The feature was committed locally; no push, release or update of the installed CLI binary was performed. Unrelated `carried-baggage.md` remains untracked and preserved.

### Inherited concerns

- Broad existing handler/output modules and large integration suites remain maintenance concerns; this feature must avoid expanding their responsibilities. Existing decomposition work stays separate.
- `plans/current/` already contained eight other documents when this initiative began, exceeding the SDLC's three-plan guidance. This session completed and moved its own plan; disposition of unrelated plans needs a separate authority/state review.
- Export-local URL sanitation must not be mistaken for resolving the existing trace/MCP URL-userinfo debt.
