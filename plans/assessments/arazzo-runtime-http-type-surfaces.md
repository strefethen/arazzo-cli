# Arazzo Runtime HTTP Step — Type Surfaces

**Captured:** 2026-08-26 against the working copy of
`crates/arazzo-runtime/src/runtime_core/engine_http.rs` (1,050 lines)
**Purpose:** Exact type-level grounding for
[`plans/current/arazzo-runtime-http-decomposition.md`](../current/arazzo-runtime-http-decomposition.md)
and its future Tier 2 movement tickets. The plan owns the boundary decision,
the slice order, and the risks; this assessment owns the signatures those
slices must land.

**Status:** Design surface, not accepted authority. Nothing here is
implemented. A signature changes here first and in the plan never — the plan
cites this document rather than repeating it.

> **Baseline caveat.** Line citations describe `engine_http.rs` at `b8cdb8b`,
> where the file is clean and committed. Two things moved during capture and
> may move again: the refusal-message work landed in `94965cb` and `b8cdb8b`,
> and a second session added `runtime_core/engine_http_tests.rs` (498 lines, 20
> test functions, uncommitted). Re-verify every cite at the plan's S0
> precondition rather than trusting the numbers here.

Each surface below records what exists today, the signature that replaces it,
and the specific defect or smell the change removes. Bodies, line maps, and
per-slice acceptance criteria belong to the tickets, not here.

---

## 1. `Warnings` and `Scope`

**Today.** Warnings are `Vec<String>`, and each producer attaches its own scope
with an inline `format!`. `format!("parameter {:?}: {warning}", param.name)`
appears at three sites (`:329`, `:383`, `:770`); the output-scope equivalent
`format!("output \"{name}\": {warning}")` at two more (`:539`, `:655`). The
copy-paste is what lets the same warning be emitted from two loops.

**Target.**

```rust
pub(crate) struct Warnings(Vec<String>);

impl Warnings {
    /// The only way a warning enters the runtime: with its scope attached.
    pub(crate) fn scoped<W: Display>(
        &mut self,
        scope: Scope<'_>,
        warnings: impl IntoIterator<Item = W>,
    );
}

pub(crate) enum Scope<'a> {
    Parameter(&'a str),
    Output(&'a str),
    RequestBodyPayload,
    SuccessCriteria(usize),
}
```

**Removes.** Five copy-pasted format strings; the possibility of two spellings
of one scope.

---

## 2. `ParameterBindings` and `Querystring`

**Today.** `param.in_` is read at two sites. `build_url_from_path` (`:820`)
matches it exhaustively, with a comment stating that a new variant must be a
compile error rather than a silently dropped parameter. `prepare_http_request`
(`:417`) reads the same field through `if param.in_ == Some(Header) … else if
… Cookie`, which a new variant passes through in silence.

The double resolution these two sites used to cause is fixed. Until `b8cdb8b`,
both loops called `resolve_value_source` on all of `step.parameters` before
dispatching on location, and a header or cookie parameter whose value warned
emitted the identical string from each. URL assembly now resolves inside each
`match` arm through a local `resolve` closure, and
`a_parameter_value_that_warns_is_reported_once_per_parameter` pins that across
all four locations. What remains is the structural cause: two read sites, of
which only one is exhaustive.

**Target.** One owner, one loop, one exhaustive `match`, each parameter
resolved exactly once.

```rust
/// Every parameter of a step, resolved once, grouped by `in:`.
pub(crate) struct ParameterBindings {
    pub path: BTreeMap<String, String>,
    /// Ordered — an array parameter explodes to repeated keys.
    pub query: Vec<(String, String)>,
    pub querystring: Option<Querystring>,
    pub headers: BTreeMap<String, String>,
    pub cookies: Vec<(String, String)>,
}

/// The whole query component, supplied by `in: querystring`. A newtype because
/// the value is already encoded — the one string in the runtime that must
/// never reach `form_urlencoded::Serializer`.
pub(crate) struct Querystring { /* param_name, encoded */ }

impl ParameterBindings {
    pub(crate) fn bind(
        params: &[Parameter],
        eval: &ExpressionEvaluator,
        step_id: &str,
        warnings: &mut Warnings,
    ) -> Result<Self, RuntimeError>;
}
```

**Removes.** The non-exhaustive second read site, which is what let the two
loops drift apart in the first place. The `Querystring` newtype replaces a
comment with a type: today nothing but that comment stops a later edit from
running an already-encoded query component through the serializer and turning
`a=1&b=2` into `a%3D1%26b%3D2`.

**Note.** Behavior-preserving, now that `b8cdb8b` has closed the duplicate
warning. Earlier drafts treated this surface as the one non-isomorphic move;
that no longer holds, and the plan carries no open decision.

---

## 3. `OperationResolver`

**Today.** Resolution is four methods on `Engine` (`:87`, `:114`, `:138`,
`:238`) reading three things: the spec's `source_descriptions`, `source_bases`,
and the lazily built `OperationIndex`.

**Target.**

```rust
pub(crate) struct OperationResolver<'a> {
    sources: &'a [SourceDescription],
    source_bases: &'a BTreeMap<String, String>,
    operations: &'a OperationIndex,
}

impl<'a> OperationResolver<'a> {
    pub(crate) fn resolve(&self, operation_id: &str)
        -> Result<ResolvedOperation, RuntimeError>;
}
```

`ResolvedOperation` keeps its current shape — `method`, `path`, and the
`Option<String>` base of the owning source description.

**Removes.** A concern that reads three fields taking the whole engine, with
the HTTP client, debug controller, observer, regex cache, and dry-run flag in
reach.

---

## 4. Two methods move off `Engine` onto `WorkflowIndex`

**Today.** `Engine::operation_index` (`:87`) drives the `OnceLock` lazy-init of
an index that is a property of `WorkflowIndex`, not of the engine.
`Engine::make_eval_context` (`engine_impl.rs:1013`) reads only
`spec.self_uri` and `source_descriptions_map`.

**Target.**

```rust
impl WorkflowIndex {
    pub(super) fn operations(&self) -> Result<&OperationIndex, RuntimeError>;
    pub(super) fn eval_context(&self, vars: &VarStore, response: Option<&Response>)
        -> EvalContext;
}
```

**Removes.** Both reasons the assembly path holds `&self`. These two moves are
the precondition for every other surface here: without them, no assembly stage
can drop the engine.

The `OnceLock` race semantics must survive verbatim — two threads compute the
same deterministic index and one `set()` no-ops — including the comment that
records why that is safe.

---

## 5. `RequestTarget` and `RequestUrl`

**Today.** `build_url_from_path` (`:740–1002`, 263 lines) does two unrelated
jobs: decide which base the path belongs to (classification, source-base
lookup, absolute-URL detection), then assemble the string (path substitution,
query serialization, `querystring` override rules).

**Target.**

```rust
/// A base and a path that are known to belong together.
pub(crate) struct RequestTarget<'a> { /* base, path */ }

impl<'a> RequestTarget<'a> {
    /// The one place `operationPath` classification and source-base lookup
    /// happen. Refuses before any string is joined.
    pub(crate) fn resolve(
        step: &'a Step,
        operation: Option<&'a ResolvedOperation>,
        index: &'a WorkflowIndex,
    ) -> Result<Self, RuntimeError>;

    pub(crate) fn into_url(
        self,
        bindings: &ParameterBindings,
        step_id: &str,
        warnings: &mut Warnings,
    ) -> Result<RequestUrl, RuntimeError>;
}

pub(crate) struct RequestUrl {
    pub url: String,
    pub path_params: BTreeMap<String, String>,
    /// What `$request.query.<name>` must report — the query as sent.
    pub query_params: BTreeMap<String, String>,
}
```

**Removes.** Base selection tangled with string assembly. `into_url` consumes
`self`, so a target produces exactly one URL and no later edit can build two
and leave which was sent in doubt. `RequestUrl` also drops `warnings` from
today's `UrlBuildResult`, because warnings now accumulate in one `Warnings`
rather than being cloned out of an intermediate result.

---

## 6. `Method` and `PreparedRequest::assemble`

**Today.** The method rule is spread through `prepare_http_request`: an
`explicit_method` string extracted at `:314`, then an `if
explicit_method.is_empty()` block at `:316` applying the "POST when the step
declares a request body, else GET" default. The empty string is the sentinel.

**Target.**

```rust
pub(crate) struct Method(String);

impl Method {
    fn explicit(token: &str) -> Option<Self>;
    /// POST when the step declares a request body, else GET.
    fn defaulted_for(step: &Step) -> Self;
}

impl PreparedRequest {
    pub(crate) fn assemble(
        step: &Step,
        method: Method,
        url: RequestUrl,
        bindings: ParameterBindings,
        eval: &ExpressionEvaluator,
        warnings: Warnings,
    ) -> Result<Self, RuntimeError>;
}
```

**Removes.** An empty-string sentinel for "no method token"; the defaulting
rule having no owner. `assemble` taking `RequestUrl` and `ParameterBindings` by
value is what enforces stage order — a request cannot be assembled before its
parameters are bound, because only `bind` produces the type `assemble` demands.

---

## 7. `StepContext`

**Today.** `evaluate_step_response` (`:614`) takes seven arguments and carries
`#[allow(clippy::too_many_arguments)]` at `:613`. The lint is reporting a
missing type, not a false positive.

**Target.**

```rust
pub(super) struct StepContext<'a> {
    pub workflow_id: &'a str,
    pub step: &'a Step,
    pub vars: &'a VarStore,
    pub depth: usize,
}
```

**Removes.** The `allow`, which is deleted rather than relocated. Three further
`too_many_arguments` allows in `engine_trace.rs` (`:343`, `:406`, `:451`) are
the same shape but out of this unit's scope.

---

## 8. Error constructors

**Today.** `RuntimeError::new(kind, format!(…))` appears at roughly fifteen
sites in `engine_http.rs` with inline message text, so no concern owns the
wording of its own refusals.

**Target.** `RuntimeError` itself does not change — `RUNTIME_*` codes are a
wire contract in `--json` and the golden baselines. The messages become private
constructors in the module that owns the concern:

```rust
// http/operation.rs — the only file that can produce these
fn not_found(id: &str, source: Option<&str>) -> RuntimeError;
fn ambiguous(target: &str, entries: &[&OperationEntry]) -> RuntimeError;
fn unqualified(target: &str, sources: &[&str], has_explicit: bool) -> RuntimeError;
```

**Removes.** Fifteen inline messages with no owner. This is also what keeps the
refusal text isomorphic through the move: one constructor per message is easier
to diff against the baseline than fifteen `format!` calls scattered through
relocated code.

---

## Surface-to-module map

| Surface | Module | Plan slice |
|---|---|---|
| `Warnings`, `Scope` | `http/warnings.rs` | S1 |
| `OperationResolver`, error constructors | `http/operation.rs` | S2 |
| `ParameterBindings`, `Querystring` | `http/parameters.rs` | S3 |
| `RequestTarget`, `RequestUrl` | `http/url.rs` | S4 |
| `Method`, `PreparedRequest::assemble` | `http/request.rs` | S5 |
| `StepContext` | `http/execute.rs` | S6 |
| `WorkflowIndex::operations`, `::eval_context` | `runtime_core/state.rs` | S2 |

Slice order, dependencies, and the S3 decision live in
[the plan](../current/arazzo-runtime-http-decomposition.md).
