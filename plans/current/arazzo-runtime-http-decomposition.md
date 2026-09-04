---
status: draft
epic:
created: 2026-08-26
revive-when:
---

# Arazzo Runtime HTTP Step Decomposition

**Seed:** "engine_http.rs is now ~1,000 lines across four concerns (operationId
resolution, URL building, parameter serialization, request prep). Not on
god-files.md yet — the file to watch before this area grows again. create a plan
that will refactor this properly and structure the code according to rust best
practices"

## Outcome and non-goals

`crates/arazzo-runtime/src/runtime_core/engine_http.rs` stops existing. The HTTP
step path becomes an `http/` unit of concern-owned modules, each with a named
owner, an explicit import list, a narrow published surface, and its own
colocated unit tests. Every `RUNTIME_*` code, refusal message, warning string,
URL byte, and header map the runtime produces today is produced identically
after the move.

The measure of success is not that six files replace one. It is that the next
change to operation resolution touches one module that no other module can
reach into, and that a new `ParamLocation` variant is a compile error at exactly
one site instead of a parameter silently dropped at another.

This initiative does **not**:

- change Arazzo, OpenAPI, expression, runtime, CLI, MCP, or DAP semantics — the
  compliance gate in `AGENTS.md` is not engaged, because nothing here touches
  document shape, field meaning, or either grammar;
- change `RUNTIME_*` codes, refusal text, warning text, or golden baselines;
- re-litigate the duplicate-warning defect, which was fixed in `b8cdb8b` before
  this plan was accepted and is now pinned by a characterization test;
- decompose `engine_impl.rs`, `engine_actions.rs`, `payload.rs`, `client.rs`, or
  the integration-test god files, which have their own owners;
- retire `runtime_core/helpers.rs`, the leftover shim from the previous split —
  though no module created here may route through it;
- change the parameter-serialization stance (`AGENTS.md`: parameters serialize
  from the Arazzo value alone), resolve F16, or alter which OpenAPI fields the
  runtime reads.

## Developer-visible workflows

This is internal structure; no CLI, output, or spec-facing behavior changes.
The workflows below are the maintainer's.

1. **Change an operationId refusal.** One module owns all three resolution
   forms and every message they emit. No sibling constructs an
   `OperationIdNotFound`.
2. **Add a `ParamLocation` variant.** `arazzo-spec` grows the variant; exactly
   one exhaustive `match` in the runtime fails to compile. Today two sites read
   `param.in_` and only one of them is exhaustive.
3. **Change how a query component is assembled.** URL assembly is reached
   through one function taking an already-resolved target and already-resolved
   bindings; it cannot re-enter parameter resolution.
4. **Prove a URL rule next to the code it proves.** Each module carries
   `#[cfg(test)] mod tests`. In-crate unit coverage now exists but sits in one
   `engine_http_tests.rs` sibling keyed to two `pub(crate)` entry points, so a
   rule's test is not next to its owner.
5. **Read the HTTP path for the first time.** `http.rs` names its children and
   re-exports only what `engine_impl.rs` and `engine_parallel.rs` consume.

## Decision-relevant facts

- The file is 1,050 physical lines at `b8cdb8b`, entirely production code — it
  carries no `#[cfg(test)]` module of its own. That distinguishes it from every
  entry currently on `god-files.md`, where 60–100% of the bulk is inline tests:
  this decomposition moves production code, not test bodies.
- **`engine_http.rs` is clean and committed at `b8cdb8b`**, which is therefore a
  pinnable baseline. The refusal-message work this plan was drafted against
  landed in `94965cb` and `b8cdb8b` during drafting; line numbers cited here
  describe `b8cdb8b`.
- **In-crate unit coverage now exists, in one module keyed to entry points
  rather than to concerns.** `runtime_core/engine_http_tests.rs` — 498 lines, 20
  test functions, uncommitted at the time of writing — covers branches the
  integration targets cannot reach, because `prepare_http_request` and
  `build_url_from_path` are `pub(crate)`. Two consequences. It is migration
  load: unit tests move with the code they prove, so every slice from S1 on
  redistributes part of this module. And it is corroboration: its own doc
  comment independently names `operation_id_routing.rs`,
  `querystring_parameter.rs`, and `operation_path_forms.rs` as owning distinct
  concerns — the same concern map this plan derived from the production code.
- **The checkout has a second writer.** That module, a `runtime_core.rs`
  declaration, and additions to `tests/operation_id_routing.rs` are uncommitted
  work from another session. No slice may start until it lands or reverts, and
  the counts above are restated at S0 rather than trusted from this draft.
- The seed counts four concerns; there are six. Measured spans: operation
  resolution ~240 lines (`:71–276`, `:1015–1050`), step orchestration and
  response evaluation ~275 (`:462–739`), URL and parameter binding ~263
  (`:740–1002`), request assembly ~158 (`:314–461`, `:1004–1013`), transport
  safety ~63 (`:8–70`). Orchestration is the largest concern and the seed does
  not name it.
- **Parameter handling is already split across two sites, and only one is
  safe.** `build_url_from_path` matches `param.in_` exhaustively with a comment
  stating that a new variant must be a compile error. `prepare_http_request`
  reads the same field through `if param.in_ == Some(Header) … else if …
  Cookie`, which a new variant passes through in silence. One spec concept, two
  implementations, one guard.
- **The duplicate-warning defect is already closed, which makes S3 a pure
  move.** Until `b8cdb8b`, both loops called `resolve_value_source` on all of
  `step.parameters` before dispatching on location, so a header or cookie
  parameter whose value warned emitted the identical string from each site.
  URL assembly now resolves inside each `match` arm through a local `resolve`
  closure, and `a_parameter_value_that_warns_is_reported_once_per_parameter`
  pins the result across all four locations. Consolidating the two sites is
  therefore behavior-preserving; earlier drafts of this plan treated it as a
  behavior change and carried a three-option decision that no longer applies.
- **The current structure is a physical split, not an ownership split.** Five
  files carry `impl Engine` blocks; 17 of 22 modules under `runtime_core/` open
  with `use super::*`. `engine_http.rs` was itself carved out of `engine_impl.rs`
  under exactly that pattern and re-accreted to 1,050 lines. Repeating it is the
  predicted failure.
- Inherent `impl Engine` methods defeat module privacy: `pub(super)` on an
  `Engine` method exposes it to every sibling in `runtime_core`, so no
  concern can hold anything back from another.
- **The tests already have the owners the production code lacks.**
  `operation_id_routing.rs` (825 lines), `querystring_parameter.rs` (431),
  `operation_path_forms.rs` (212), and `transport_tls.rs` (567) are focused
  targets that map nearly one-to-one onto the concerns proposed below. The
  proposed boundaries are therefore already validated by how the suite is
  organized.
- The external surface is small and known: `execute_http_step` (two callers in
  `engine_impl.rs`, `engine_parallel.rs`), `unused_insecure_hosts` (CLI
  `handlers.rs`, `test_runner.rs`, `transport_tls.rs`), `resolve_operation_id`
  (`engine_execution.rs`), and `prepare_http_request` / `build_url_from_path`
  (inline tests in `arazzo-runtime/src/lib.rs`). Nothing else reaches in.
- `evaluate_step_response` carries `#[allow(clippy::too_many_arguments)]` — the
  lint is reporting the missing type, not a false positive.
- Four drift guards bound this work: `golden_spec_baseline`, `schema_drift`,
  `operation_path_agreement`, `cli_contract_snapshots`. Any movement in them
  during a slice means the slice was not isomorphic.
- `plans/current/` holds seven plans against SDLC's limit of three. This plan
  makes eight. Disposition is owed on the older drafts.

## Architecture

### The decision: types enforce the boundaries, not file names

Four new files each opening `impl Engine { … }` over `use super::*` would leave
every concern reachable from every other and would re-accrete — that is the
documented history of this file. Inherent methods on a shared type cannot hold
anything back from a sibling, and a glob import means a new dependency costs
nothing and appears nowhere in review.

So the boundary is not a file. It is a **type that a stage must be handed in
order to run**. Where a stage's input type can only be produced by the previous
stage, the pipeline order is checked by the compiler rather than by a reviewer.

### The pipeline

Five stages exist today, interleaved inside two methods of 148 and 263 lines
that each re-reach into `Engine`. Separated, each stage's output type makes the
previous stage's mistakes unrepresentable:

```text
Step + VarStore
  ─▶ ResolvedOperation   which operation, and whose server
  ─▶ ParameterBindings   every parameter resolved exactly once, keyed by location
  ─▶ RequestUrl          url, plus the parameters as actually sent
  ─▶ PreparedRequest     method, headers, body, trace
  ─▶ StepExecution       sent and evaluated
```

### Published surfaces

The type surfaces that enforce this pipeline — eight of them, each with the
current-state citation it replaces and the defect it removes — are grounded in
[`plans/assessments/arazzo-runtime-http-type-surfaces.md`](../assessments/arazzo-runtime-http-type-surfaces.md).
That document owns the signatures; this plan owns the boundary decision, the
slice order, and the risks, and cites them rather than repeating them.

The architectural claims those signatures carry, which the slices below must
satisfy:

- **One Parameter Object owner.** `ParameterBindings::bind` is the only reader
  of `param.in_`, through one exhaustive `match`, resolving each parameter
  exactly once. This collapses two sites of which only one is exhaustive, and
  is the single largest structural change in the unit.
- **Stage order is checked by the compiler.** `PreparedRequest::assemble` takes
  `RequestUrl` and `ParameterBindings` by value, and only `bind` and `into_url`
  produce those types. A request cannot be assembled before its parameters are
  bound because the types are unavailable, not because a reviewer noticed.
- **Two methods move off `Engine` onto `WorkflowIndex`** — the `OnceLock` index
  init and the eval-context factory, which read only index data. These are the
  precondition for every other surface: they are both reasons the assembly path
  currently holds `&self`, and until they move, no stage can drop the engine.
- **Newtypes carry the rules that comments carry today.** `Querystring` for the
  already-encoded query component that must never be re-serialized; `Method`
  for the POST-with-body default, replacing an empty-string sentinel.
- **Lints are answered with types, not allows.** `StepContext` replaces the
  `#[allow(clippy::too_many_arguments)]` at `:613`, which is deleted rather
  than relocated.
- **`RuntimeError` does not change.** `RUNTIME_*` codes are a wire contract in
  `--json` and the golden baselines. Only the ~15 inline message sites move,
  into private constructors owned by the concern that raises them.

### Module layout

```text
runtime_core/http.rs                 // declares children; re-exports exactly 5 names
runtime_core/http/
    operation.rs        OperationResolver, ResolvedOperation, refusals
    parameters.rs       ParameterBindings, Querystring
    url.rs              RequestTarget, RequestUrl  (absorbs runtime_core/url.rs)
    request.rs          PreparedRequest, Method
    execute.rs          impl Engine — send, dry-run, evaluate response
    transport_policy.rs impl Engine — cleartext warning, unused hosts
    warnings.rs         Warnings, Scope
```

`http.rs` re-exports only `execute_http_step`, `unused_insecure_hosts`,
`resolve_operation_id`, `prepare_http_request`, and `build_url_from_path` — the
five names outside callers already use. Every module declares explicit imports;
none carries `use super::*`. `impl Engine` survives only in the two modules that
need the live engine: one sends requests and drives the debugger, the other
reads client state. The dependency graph is one-directional, and `parameters`
depends on nothing else in `http/`:

```text
execute ──▶ request ──▶ url ──▶ parameters ──▶ warnings
    │           │         │
    │           └─────────┴──▶ operation
    └──▶ transport_policy
```

Every module carries `#[cfg(test)] mod tests` for the rules it owns. The unit is
production-only today, so every rule in it is currently proved at a distance
through integration targets.

### What each move removes

| Move | What it removes |
|---|---|
| `ParameterBindings::bind` | Two `param.in_` read sites, one of them non-exhaustive — the structural cause of the duplicate warning `b8cdb8b` fixed at the symptom |
| `Warnings` + `Scope` | Five copy-pasted `format!` prefixes |
| `Querystring` newtype | "Do not re-encode this" enforced only by a comment |
| `WorkflowIndex::operations` / `::eval_context` | Both reasons the assembly path held `&self` |
| `Method` newtype | Method-defaulting spread through `prepare_http_request` |
| `RequestTarget` consuming `self` | Base selection tangled with string assembly |
| `StepContext` | `#[allow(clippy::too_many_arguments)]` |
| Per-concern error constructors | ~15 inline `format!` messages with no owner |

### Required order

Each slice is an independently revertible head, verified against the same
baseline. Slices land newest-last and revert newest-first.

- **S0 — Precondition.** Land or revert the second writer's uncommitted work
  (`engine_http_tests.rs`, the `runtime_core.rs` declaration, the
  `operation_id_routing.rs` additions). Pin the baseline commit and blob, and
  restate the line counts and test-function count from it rather than from this
  draft. Add `engine_http.rs` to the `god-files.md` watch list with its measured
  split, so the entry exists before the work rather than after.
- **S1 — Scaffold and transport policy.** Create `http.rs` and move the smallest
  concern (~63 lines). Proves the pattern — explicit imports, module root
  re-export, unit tests — on a slice small enough to review as a pattern
  decision rather than a diff.
- **S2 — Operation resolution.** Largest self-contained move, no behavior
  surface beyond message text, already covered by `operation_id_routing.rs`.
- **S3 — Parameter owner.** See the decision below; this is the only slice that
  is not a pure move.
- **S4 — URL assembly.** Consumes S3's bindings; absorbs `runtime_core/url.rs`.
- **S5 — Request assembly.** Consumes S4.
- **S6 — Orchestration.** Moves the remainder; `engine_http.rs` is deleted in
  this slice, not before.
- **S7 — Anti-recurrence guard.** See below.

### No open decisions

Every slice is now a pure move. The one question this plan carried — whether
consolidating the two parameter sites was a behavior change — was answered by
the `b8cdb8b` fix and its characterization test: it is not. S3 consolidates two
read sites into one exhaustive `match` without changing what the runtime emits.

The isomorphism contract therefore applies uniformly. No slice may pair a
structural move with a behavior change; a defect found mid-slice is a separate
ticket, and the slice lands unchanged around it.

### Anti-recurrence guard

A one-time split without a guard reproduces the starting state; that is the
documented history of this exact file. S7 adds a bounded structural check:
every module under `http/` stays under a stated line ceiling, contains no
`use super::*`, and declares no `impl Engine` block outside `execute.rs` and
`transport_policy.rs`. The guard fails the build with the concern name, so the
next accretion is answered at review time by moving the code to its owner.

### Rejected shapes

- **A new `arazzo-http` crate.** The concerns share `EngineInner`, `VarStore`,
  and `RuntimeError`; extracting them across a crate boundary is a larger
  design than the seed asks for and would widen a published surface.
- **A trait per concern.** There is one implementation of each. Traits would add
  indirection without a second implementor or a test seam that narrow inputs do
  not already give.
- **Splitting by line count into `engine_http_1/2/3`.** Physical, not
  ownership — the failure mode already observed.

## Coverage

- **Security engineering:** Two mechanisms move and neither may weaken. The
  cleartext-credential warning must keep firing once per host, on live clients
  only, for non-loopback `http` with an `Authorization` or `Cookie` header from
  either step headers or client defaults — `transport_tls.rs` is the surface
  that proves it, and S1 runs it before and after. Separately, every refusal in
  `http/operation.rs` is a fail-closed control: an unresolvable, ambiguous, or
  unqualified-with-multiple-sources `operationId` is refused *before* a URL is
  built, which is what stops a request reaching the wrong host. S2's ticket
  states that no refusal may become a warning, a fallback, or a default —
  adding one would need the explicit approval `AGENTS.md` requires for
  degraded-mode behavior. Redaction is untouched: `redaction.rs` stays the
  owner and no new module formats a header value into a message.
- **Testing strategy:** Three layers. (1) Existing focused integration targets
  are the isomorphism proof and must pass unchanged at every slice:
  `operation_id_routing`, `querystring_parameter`, `operation_path_forms`,
  `transport_tls`, `engine_execution`, `engine_workflow`, `engine_soap`,
  `transport_characterization`. (2) The four drift guards —
  `golden_spec_baseline`, `schema_drift`, `operation_path_agreement`,
  `cli_contract_snapshots` — are the tripwires; any movement means the slice
  was not isomorphic and the slice is wrong, never the baseline. (3) Colocated
  unit tests per module, which are additive proof and never a substitute for
  (1). Layer (3) is partly written already: `engine_http_tests.rs` is
  redistributed rather than replaced, each test landing with the concern it
  exercises, and a slice that leaves a test behind in the old module has not
  finished. Its 15 tests fall across S3–S6, not S1–S5 — three to parameters,
  four to URL assembly, three to method and body, one to target resolution, and
  two (`a_dry_run_names_the_output_whose_expression_warned`,
  `a_success_criterion_warning_carries_the_index_of_the_criterion`) to
  orchestration. Nothing in it touches transport policy or operation
  resolution, so S1 and S2 inherit no test debt and S6 inherits more than its
  production span suggests. Full `cargo fmt --check`, `cargo clippy --workspace
  --all-targets --all-features -D warnings`, and `cargo test --workspace` gate
  every slice, per `AGENTS.md`.
- **Concurrent writers:** Two kinds. In the code, `WorkflowIndex::op_index` is a
  `OnceLock` whose documented race — two threads compute the same deterministic
  index, one `set()` no-ops — must survive the move to `http/operation.rs`
  unchanged, including its comment; `Engine` stays `Clone` over `Arc<EngineInner>`
  and no module may introduce interior mutability the engine does not already
  have. In the checkout, one agent owns writes: a second session is currently
  adding in-crate tests to this unit, S0 exists to resolve that, and no two
  slices may
  be implemented in parallel because each moves code the next one reads.
- **Rollout / rollback:** Internal refactor, no release artifact, no migration,
  no persisted state. Each slice is one commit, revertible in isolation while it
  is the head and as a contiguous suffix newest-first afterward. `engine_http.rs`
  survives as a shrinking file until S6, so at every intermediate state the
  crate builds and the full suite passes. There is no flag, no phased rollout,
  and no dual-path period — a half-migrated concern is never shipped, because
  each slice moves a whole concern.

## Material risks

- **A second writer is active in this unit, and the plan already went stale
  once.** Between drafting and first revision, the refusal-message work
  committed and 498 lines of in-crate tests appeared, invalidating three claims
  in this document. The lesson is not that S0 is a hard gate — it is that
  counts and citations here are a drafting snapshot with a short half-life, and
  every slice restates them from its own baseline rather than trusting this
  draft.
- **A slice that "tidies while moving."** The most likely failure is a reviewer
  accepting an improved message or a collapsed branch inside a move commit.
  Mitigation: the drift guards, plus tickets that state the diff must be
  reviewable as a move.
- **The published surface widens by accident.** Six modules invite six sets of
  `pub(crate)`. Mitigation: `http.rs` re-exports exactly five names, and S7's
  guard makes an addition visible.
- **S3's behavior question stalls the sequence.** S4–S6 depend on S3's bindings.
  Mitigation: the decision is owed at decomposition, not at implementation.
- **Conflict with in-flight spec work.** The conformance audit's open findings
  (F1, F16) and the AsyncAPI plan both name this file. Mitigation: check
  ticket reservations at decomposition; a spec-surface ticket and a slice must
  not run concurrently against the same concern.

## System success condition

After the slices close: `engine_http.rs` does not exist; no module under
`http/` contains `use super::*`; `impl Engine` appears only in `execute.rs` and
`transport_policy.rs`; every module carries unit tests for the rules it owns;
the four drift guards pass unmoved against the S0 baseline; the full workspace
suite passes with the same test count plus the additive unit tests; and
`god-files.md` records the unit as decomposed rather than watched.

The end-to-end evidence is a single `cargo test --workspace` run at S7 showing
the four guards green against the S0 baseline, paired with the diff-stat showing
1,050 lines redistributed to six named owners with no net behavior diff in the
golden baselines.

---

**Disposition:** Draft — no design questions remain; S3's three-option decision
was answered by the `b8cdb8b` fix. Two process items before acceptance: (1) the
second writer's in-crate tests must land or revert before a baseline can be
pinned; (2) `plans/current/` is at seven of three, so accepting this plan owes
disposition on the older drafts. The `plan-writer` adversarial review round has
not run.
