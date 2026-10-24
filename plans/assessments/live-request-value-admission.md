# Live-request value admission

**Ticket:** [ac-704a6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-704a6) ·
**Date:** 2026-10-24 · **Source:** `2726e34` · **Status:** assessment only. No
runtime change, no ticket created.

**Accepted by Steve.** On 2026-10-18: *"stop the step before sending when a required value
can't be resolved. Preserve intentional null and empty values."* On 2026-10-24 he
accepted five sub-decisions: (1) interpolated text is admitted like an exact
expression; (2) selector zero matches stay resolved `null`; (3) a new additive
error code, working name `RUNTIME_UNRESOLVED_REQUEST_VALUE`, separate from
`RUNTIME_INVALID_PARAMETER_VALUE`; (4) admission failure is an ordinary step
failure, so `onFailure` runs and retries are allowed; (5) dry-run reports the
same failure, and dry-run provenance detail stays with
[ac-fda34](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34).

This document turns that into a contract. One behavior fork is still open
(§4), plus five confirmations (§5). One of them comes from the spec check: the
vendored text **does not** say that zero matches produce `null`.

## 1. Evidence: L4 still reproduces at `2726e34`

I built the CLI from a `git archive` snapshot of HEAD. The snapshot excludes
other agents' uncommitted work. I ran a synthetic fixture against a loopback
Python server on `127.0.0.1:48704`. A `GET` returns `{"name":"rex"}`, with no
`id`. The next step sends `DELETE /pets/{petId}?owner=…`, and both values come
from `$steps.find.outputs.id`.

| Run | Exit | stderr | Server received |
|---|---|---|---|
| exact, `--expr-diagnostics warn` | 0 | empty | `DELETE /pets/` (no `owner`) |
| exact, default diagnostics | 0 | empty | `DELETE /pets/` |
| `petId: "pet-{$steps.find.outputs.id}"` | 0 | empty | `DELETE /pets/pet-null` |
| exact, `--dry-run` | 0 | empty | plans `DELETE …/pets/` with no warnings |

The causal chain, with every link in current source:

1. **The producer loses absence without a warning.** A `$response.body#/id`
   miss is `Resolution::from_option(None)`
   (`arazzo-expr/src/resolution.rs:221`). Message-source misses in the
   request/response namespaces return Missing with **no** warning; only
   `$message.*` and named lookups warn. The output path is
   `criteria.rs:228` → `evaluate_with_diagnostics` → `into_public`, which maps
   `Missing => Value::Null` (`resolution.rs:29`).
2. **The stored output is plain JSON.** `VarStore::set_step_output`
   (`state.rs:279`) stores a `Value`. `EvalContext.steps` is
   `BTreeMap<String, Value>` (`arazzo-expr/src/lib.rs:105`). The consumer's
   `$steps.find.outputs.id` therefore resolves as *Present(null)*, which looks
   the same as an authored null.
3. **The request builder serializes null as "nothing".** For a path value,
   `value_to_string` gives `Null => String::new()` (`payload.rs:10`). A null
   query value is dropped (`engine_http.rs:697`), and so is a null
   `querystring` (`:726`). Headers and cookies become empty
   (`engine_http.rs:322`, `:331`). Inside a template, an embedded null renders
   as `"null"` (`interpolation.rs:26`).
4. **The diagnostics policy runs too late.** `--expr-diagnostics` defaults to
   `off` (`cli.rs:91`). It is applied after execution in `run.rs`, by which
   time the request has already been sent.

The real-null controls that must not change are
`dry_run_resolves_explicit_null_and_empty_parameter_and_replacement_values`
(`arazzo-runtime/tests/engine_execution.rs:3967`) and
`run_dry_run_accepts_and_resolves_explicit_null_and_empty_values`
(`arazzo-cli/tests/cli_integration.rs:3836`). The zero-match controls are
`engine_execution.rs:3689`, `jsonpath_selectors.rs:476`, and
`jsonpath_selectors.rs:740`.

**Rejecting on null is ruled out.** Real null and unresolved data are the same
`Value` by step 2, so the predicate has to be provenance.

## 2. What the specification says (verbatim, `spec/arazzo/v1.1.0.html`)

Each quote below was checked against the vendored HTML (tags stripped).

- `#parameterValue` (l. 885): "The value can be a constant, a Runtime
  Expression, or a Selector Object to be evaluated and passed to the
  referenced operation or workflow."
- `#requestBodyPayload` (l. 1652): "The value can be a literal value or can
  contain Runtime Expressions or Selector Objects which MUST be evaluated prior
  to calling the referenced operation." This places admission before dispatch.
- `#stepOnFailure` (l. 714): "An array of failure action objects that specify
  what to do upon step failure." This supports sub-decision 4.
- Regex and JSONPath conditions (l. 1375, 1390): "If the `context` evaluates
  to `null` or `undefined`, the condition MUST evaluate to *fail*." The spec
  itself separates null from undefined.
- Runtime expressions (l. 2006, 2009): "Runtime expressions preserve the type
  of the referenced value." and "Scalar values (string, number, boolean, null)
  are converted to their string representation." So a *real* null embedded in
  a template stays `"null"`.
- `#selectorObjContext` (l. 1597): "A Runtime Expression which MUST evaluate
  to structured data (e.g., $response.body) and set the context for the
  selector to be applied on."

**Where the spec is silent.** These are implementation choices, not
spec-mandated behavior:

- What an unresolvable runtime expression yields. No sentence covers it. The
  accepted policy is our choice in that silence.
- **What a Selector Object yields on zero matches. Open item for Steve.**
  §5.8.13 (`#selector-object`) defines only `context`, `selector`, and `type`.
  It says nothing about cardinality or a no-match result. The only zero-node
  text is in JSONPath *conditions* (l. 1388): "A condition fails (falsy) when
  the JSONPath expression returns an empty nodelist (zero nodes)." That is
  about criterion truthiness, not a Selector value. OAS 3.2.0 does not define
  Selector Objects. So sub-decision 2 stands as a repository choice and cannot
  be cited as spec. Separately, `AGENTS.md` § Expression surface says
  "Zero/one/many matches normalize to `null` / scalar / ordered array" under a
  heading that claims "This list is the spec". That line has no support in
  `spec/` and should be reworded as repository behavior.
- OpenAPI `required`. The repository does not read OpenAPI parameter metadata
  (`AGENTS.md` § Parameter serialization), and this contract does not either.
  Admission depends only on how the Arazzo value resolved.

## 3. The accepted contract

**Rule.** A step fails before dispatch with `RUNTIME_UNRESOLVED_REQUEST_VALUE`
when any value used to build its request has *unresolved* provenance. Real
null and empty values keep their current serialization.

### 3.1 Value classes

| Class | Example | Today | Contract |
|---|---|---|---|
| Authored literal `null` / `""` | `value: null` | serialized as null | **allowed, unchanged** |
| Data that really is null | prior output from `{"id": null}` | serialized as null | **allowed** |
| Missing key / pointer miss | `$response.body#/id` absent, `$steps.s.outputs.x` undeclared, `$inputs.typo` | null | **fail** |
| Unknown step/workflow, `$components.*`, unresolvable `$sourceDescriptions.<n>.<ref>` | — | null + warning | **fail** |
| Not available before the request | `$response.*`, `$statusCode`, `$url`, `$request.*` in the step's own request | null | **fail** |
| Expression syntax error; malformed template | — | null, or the literal text + warning (`interpolation.rs:35`) | **fail**, decided by the syntax error, not the text |
| Embedded unresolved expression in a template | `pet-{$steps.f.outputs.id}` | `"pet-null"` | **fail** (sub-decision 1) |
| Embedded real null in a template | — | `"pet-null"` | **allowed** (spec l. 2009) |
| Selector zero matches | `$.absent` | null + warning | **allowed as null** (sub-decision 2; see §5c) |
| Selector selects a real null | — | null | **allowed** |
| Selector hard failure: JSONPath parse/version/limit/eval error; JSON Pointer version or syntax; XPath version or non-string context; unknown type | — | null + warning, or replacement skipped | **fail** |
| Selector context unresolved | `context: $steps.x.outputs.y` missing | null + warning | **fail** |
| Unresolved upstream value: stored step output, workflow output, `$workflows.*`, subworkflow input or output | L4 | null | **fail**, via provenance (§6) |
| Required input absent (CLI without `--strict-inputs`) | — | null | **fail** at the consuming step |
| Declared-optional input absent | — | null (query omitted) | **§4: A fails, B allows** |

### 3.2 Locations

Each location applies the same rule to direct values, nested literal leaves,
and template fragments. Workflow-level parameters are included because
`merge_workflow_params` runs before every attempt.

| Location | Unresolved today | Real null today, unchanged |
|---|---|---|
| `path` | `""` → `/pets/` | `""` |
| `query` | dropped | dropped |
| `querystring` | dropped | dropped |
| `header` | `""` | `""` |
| `cookie` | `name=` | `name=` |
| `requestBody.payload`, any depth | JSON `null` leaf | `null` leaf |
| `replacements[i].value` | writes `null`; a JSONPath hard failure skips the replacement (`payload.rs:298`) | writes `null` |

The contract replaces every "Unresolved today" cell with **step fails, nothing
sent**. A replacement *target* that matches nothing is a location question, not
a value question. It keeps its current warn-and-skip behavior.

### 3.3 Boundary and paths

**Last pre-transport boundary:** `Engine::prepare_http_request`
(`engine_http.rs:227`), including the `build_url_from_path` call at `:252`. It
returns `Result` before the `RequestPrepared` event, the dry-run branch
(`:384–386`), and `client.request` (`:432`). The structured-`querystring`
`InvalidParameterValue` refusal already fails at this point, and
`tests/path_parameter_safety.rs::assert_refused` is the model test: no request
is logged, no prepared or sent events fire, and `dry_run_requests()` is empty.

| Path | How it reaches the boundary |
|---|---|
| Sequential | `execute_step_with_result` (`engine_impl.rs:673`) → `execute_http_step` → `prepare_http_request` |
| `--parallel` | `execute_parallel_attempt` (`engine_parallel.rs:307`) → `execute_http_step` on the level's `VarStore` snapshot. Outputs merge back at `:127`. |
| `run --step` | `execute_step_inner` (`engine_impl.rs:207`) runs its deps first (or `--no-deps` refuses with `STEP_MISSING_DEPENDENCY`), then the same `execute_step_with_result` |
| Retry | Each attempt calls `prepare_http_request` again. A `retry` with a `stepId`/`workflowId` reference can supply the missing value. |
| Subworkflow | Parent parameters become child inputs (`engine_impl.rs:855`). Child outputs come back as parent step outputs (`:915–918`). A retry's `workflowId` reference registers at `:806`. Admission happens at the child's or parent's next request. |
| Dry-run | Same boundary, because preparation precedes the dry-run branch. See §5a. |

**Routing.** The `Err` passes through `step_attempt::settle` (`:304`) into a
failed `StepResult` with `err_kind` set, which is ordinary `onFailure` routing.
With no matching action, `control::step_result_error` keeps the kind, so the
workflow error carries the new code. `StepResult.response` is `None`, so
failure criteria on `$statusCode` see no response.

## 4. The open fork: absent declared-optional inputs

Today an omitted optional input is the only way to drop an optional query
filter. A schema `default: null` cannot express that, because
`PropertyDef.default` is `Option<Value>` (`arazzo-spec/src/lib.rs:298`) and
serde reads `null` as `None`.

- **A. Strict.** Any absent input fails at the consuming request. This is the
  most fail-closed option, but it ends the omit-an-optional-filter idiom with
  no replacement. It also catches `DELETE /pets/{petId}` when `petId` is
  optional and omitted.
- **B. Omission is intent (recommended).** When building the request context
  only, an input that is declared in `workflow.inputs`, not `required`, and not
  supplied becomes an authored `null`, so current serialization applies.
  Undeclared names, missing required inputs, and inputs that were supplied with
  unresolved provenance (for example from a parent workflow) still fail. Here
  the author declared the input optional and the caller chose to omit it. That
  is caller intent, unlike L4's silent upstream data drift. The remaining risk,
  an optional input used as a path segment, is visible in the document.

The two alternatives are identical in every other cell of §3. Neither changes
criteria, actions, or how stored values are shown.

## 5. Confirmations Steve must give

- **(a) Dry-run synthetic responses.** `execute_dry_run_step` evaluates outputs
  against a fabricated `{}` (`engine_http.rs:452`). Under sub-decision 5 taken
  literally, every response-derived output would be unresolved, and dry-run of
  any chained workflow would fail at its first consumer, including this
  fixture. *Recommended:* treat "unresolved only because transport was
  skipped" as a separate provenance class. It is not an admission failure, and
  ac-fda34 renders it as `<unresolved:…>`. Everything else that would fail a
  live run fails dry-run the same way.
- **(b) Replacement values: fail, not skip.** The F20 closure records that a
  failed replacement value "leaves that replacement unapplied and the body
  untouched" (`payload.rs:298`). Under this contract the step fails instead.
- **(c) Selector zero matches leave a residual L4.** An output
  `{context: $response.body, selector: $.id, type: jsonpath}`, or the
  `jsonpointer` equivalent (whose miss is coded as zero matches,
  `payload.rs:224`), still gives null, and that null still produces
  `DELETE /pets/`. Only the runtime-expression form `$response.body#/id` is
  protected. This follows from sub-decision 2, which the spec does not decide
  (§2).
- **(d) Retry exhaustion masks the code.** An exhausted retry reports
  `RUNTIME_RETRY_LIMIT_EXCEEDED` (`engine_actions.rs:163`), as it already does
  for every kind. The new code appears only in the per-attempt trace error.
- **(e) One provenance owner.** ac-fda34 currently plans its own
  `runtime_core/dry_run.rs` sidecar, limited to exact aliases. Live admission
  needs the same sidecar plus templates and pointer suffixes. *Recommended:*
  one owner for both, with ac-fda34 revised to use it.

## 6. Implementation shape (recommended)

- **One evaluator.** `arazzo-expr` adds one public, provenance-keeping value
  resolution that covers both exact expressions and templates
  (`interpolation.rs` plus `resolution.rs`). It reports which embedded
  expressions were Missing. `evaluate_resolved` (`lib.rs:168`) is the
  precedent, but it handles neither templates nor diagnostics. Do not add a
  second parser.
- **Value walk.** Generalize `payload.rs`'s private `jsonpath_failed`
  (`:43`) into an "unresolved" flag carried per leaf through
  `resolve_value_checked` and every selector type. Zero matches stay sound.
- **Propagation.** `VarStore` gets a crate-private unresolved set for inputs,
  step outputs, and workflow inputs/outputs. Every write records into it:
  `engine_impl.rs:739`, `:806`, `:915–918`, `:975`, `build_outputs` (`:991`),
  and `engine_parallel.rs:127`. `evaluate_output_value_detailed`
  (`criteria.rs:235`) must return provenance. Public outputs, trace, DAP
  scopes, and criteria keep showing `null`.
- **Request context.** A request-only `EvalContext` leaves unresolved entries
  out, so `$steps.x.outputs.y` and pointer suffixes resolve Missing through the
  existing `resolve_named`, and `EvalContext` gains no field. Header and body
  use this context with `method` set (`engine_http.rs:258`, `:312`), but the
  URL context does not (`:659`). Today `$method` in a path or query parameter
  is Missing. The same slice should give every location one context.
- **Subworkflow nesting.** If a parent parameter has any unresolved leaf, the
  whole child input is marked unresolved. This is conservative, and the
  per-leaf version is a non-goal. It sits on the single subworkflow-parameter
  path from [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4).
  Today's legacy branch renders null as `""` through `interpolate_string`
  (`engine_impl.rs:865–880`).
- **Error message.** On `Err`, `settle` returns `StepTraceData::default()`, so
  the preparation warnings are lost. The message must carry the location and
  the source, for example
  `step "remove": parameters[0](path:petId): $steps.find.outputs.id did not resolve; request not sent`.

## 7. Compatibility and surfaces

- **Error code.** `RuntimeErrorKind` is `#[non_exhaustive]` (`error.rs:3`),
  and `docs/internal-api-v1.md` allows backward-compatible additions. The new
  variant and code are additive. `run.schema.json` types `code` as a plain
  string, so `docs/schemas/` does not change.
- **CLI `run`/`test`.** One `{"kind":"error",…,"code":"RUNTIME_UNRESOLVED_REQUEST_VALUE"}`
  with a non-zero exit, under every `--expr-diagnostics` mode, because the run
  error is returned before the policy runs (`run.rs:348`). Under `error` mode
  the code is the new one, not `RUNTIME_EXPRESSION_DIAGNOSTICS`.
- **MCP.** `handlers.rs:321–328` passes `err.code()` through, so no code
  change is needed. With `strict_inputs(true)` (`:280`), a missing required
  input still fails earlier with `RUNTIME_INPUT_VALIDATION`. Alternative B
  behaves the same over MCP and the CLI.
- **DAP.** `session.rs:382` already prints `workflow failed [code]`. Variable
  views keep showing `null`.
- **Library.** Public outputs stay unchanged. `ExecutionResult.outputs` is
  `Err` with the new kind.
- **Drift guards.** Any `golden_spec_baseline` or `cli_contract_snapshots`
  fixture that sends unresolved values will move. Each move must be explained,
  not re-baselined.
- **Behavior change for users.** Workflows that currently send requests with
  silently missing data will fail. Write a CHANGELOG "Upgrade if" line for the
  release.

## 8. Follow-on split (owner-based; draft, not created)

None of these is ready until Steve answers §4 and §5.

1. **`arazzo-expr` provenance resolution.** Exact expressions and templates
   through one evaluator. Tests cover a template with a missing value vs a
   template with a real null, and syntax errors.
2. **`payload.rs` value walk.** Per-leaf unresolved provenance for every
   selector type and nested literal, plus the replacement-value change (§5b).
   Negative tests cover a rejected hard failure, and the zero-match controls
   stay null.
3. **`VarStore` provenance and request context.** All write sites in §6,
   input materialization for alternative B, and subworkflow propagation.
   Depends on ac-681d4.
4. **Admission and error.** `prepare_http_request`/`build_url_from_path`, the
   new `RuntimeErrorKind`, and the message format. Proof is this L4 loopback
   fixture under sequential, `--parallel`, `run --step`, retry with recovery,
   and subworkflow, asserting that no `DELETE` arrives. The authored-null and
   zero-match controls must stay green.
5. **Surfaces and release notes.** Verify only: CLI contract snapshot, MCP, and
   DAP code passthrough. Plus CHANGELOG and the `AGENTS.md` zero-match wording
   (§2).
6. **Revise ac-fda34** onto the shared provenance owner (§5a, §5e). That is a
   planner decision, not part of this ticket.

Order: 1 → 2 → 3 → 4 → 5. Slice 6 can start after 3.
