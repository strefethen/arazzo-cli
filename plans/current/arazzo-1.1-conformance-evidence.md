# Plan: Arazzo 1.1 Conformance Evidence and Settled Semantics

Status: Accepted

Reconciled: 2026-08-22 after adversarial transfer review. The amendments below
are accepted desired-state authority for Runtime Expression parsing and field
dispatch, action targets, operationPath presentation, XPath boundaries, and
grammar-backed evidence. They replace older ticket prose that treated current
implementation quirks or an unevaluated package candidate as settled design.

## Goal

Make Arazzo 1.1.0 support auditable from the vendored specification to an
executable test. Close the implementation gaps whose required behavior is
already settled, and classify every remaining gap as fail-closed,
unsupported, or non-conformant with a durable owner. A green workspace test
run must never be mistaken for an unqualified claim of full compliance.

## Authority and current evidence

`spec/arazzo/v1.1.0.html` is the normative authority. The current implementation
and tests establish existing behavior; `plans/assessments/arazzo-spec-conformance-audit.md`
records known findings but is not a substitute for the specification.

The clauses that directly constrain this initiative include:

- Section 5.1: *"The Arazzo Specification is versioned using a
  `major.minor.patch` versioning scheme"* and the `major.minor` portion
  designates the feature set; tooling should ignore patch differences.
- Section 5.8: a field is optional only when it is not explicitly REQUIRED and
  is not governed by MUST or SHALL language.
- Section 5.8.3: each source location *"MUST be in the form of a
  URI-reference"*.
- Section 5.8.6: Parameter `value` is REQUIRED; `in` MUST be present for
  operation targets and MUST NOT be used for workflow-input parameters.
- Sections 5.8.7 and 5.8.8: action `name` and `type` are REQUIRED, Success
  Actions allow only `end`/`goto`, and Failure Actions add `retry`.
- Section 5.8.8: omitted `retryLimit` means one retry, and the limit *"MUST be
  exhausted prior to executing subsequent failure actions"*.
- Section 5.8.11: invalid criterion evaluation MUST fail; implementations
  SHOULD report the error; multiple criteria use logical AND.
- Section 5.8.6 says a Parameter value can be a constant, Runtime Expression,
  or Selector Object, and that *"Runtime expressions can be embedded within
  the string value using `{}` notation."*
- Sections 5.8.7 and 5.8.8 require an action `stepId` to reference a Step in
  the current Workflow; an external Arazzo `workflowId` uses a Runtime
  Expression naming its source description.
- Section 5.9 defines the exhaustive `expression` production and states
  `expression-string = *( literal-char / embedded-expression )` and
  `embedded-expression = "{" expression "}"`. Its `literal-char` production
  includes `$` and excludes braces.

At the accepted baseline, the workspace has 802 passing Rust tests, but only
two of the 26 golden-corpus Arazzo documents declare 1.1.0. The golden sweep
records validation diagnostics plus dry-run step ID, method, and URL. It does
not prove request bodies, parameters, outputs, actions, criteria, or live HTTP
semantics. The focused 1.1 guard proves three cross-feature paths only.

## Architectural decisions

### One traceability contract

Add a versioned, machine-readable conformance manifest under
`crates/arazzo-cli/tests/conformance/`. Split entries by owning area so
independent tickets do not contend on one file. Each entry identifies a stable
spec clause/anchor, normative level, implementation status, executable evidence,
and an owning ticket for every non-covered state. A Rust integration test
validates the manifest and is discovered by `cargo test --workspace`; no
parallel CI-only implementation is added.

Statuses are `covered`, `failClosed`, `unsupported`, and `deviation`.
`covered` requires positive and negative executable evidence. Every other
status requires an owner and cannot contribute to a full-compliance claim.
The repository's extension allowlist remains empty; `deviation` records debt,
not approval.

### Behavior remains with its owning crate

- Wire shape and lossless parsing belong to `arazzo-spec`.
- Structural and reference rejection belongs to `arazzo-validate`.
- Condition parsing belongs to `arazzo-expr`; response-aware evaluation,
  scheduling, retry, and tracing belong to `arazzo-runtime`.
- CLI, MCP, and DAP tests prove propagation and integration; they do not
  duplicate normative parsing or runtime enforcement.
- The broad golden sweep remains intentionally narrow. Focused tests own exact
  semantics, and one final integration member composes the corrected paths.

### Effective Step views are target-neutral model ownership

`arazzo-spec/src/effective_step.rs` is the sole syntax-neutral owner for
Workflow-to-Step inheritance. It depends on no validator, evaluator, or runtime
crate and performs no target classification, component resolution, expression
parsing, diagnostics, evaluation, or mutation. `arazzo-spec` re-exports only the
following borrowed surfaces; no generic all-concerns `EffectiveStep` facade is
authorized:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveParameterOrigin {
    Workflow { parameter_index: usize },
    Step { step_index: usize, parameter_index: usize },
}

#[derive(Clone, Copy, Debug)]
pub struct EffectiveParameterRef<'a> {
    pub parameter: &'a Parameter,
    pub origin: EffectiveParameterOrigin,
}

pub fn effective_step_parameters(
    workflow: &Workflow,
    step_index: usize,
) -> Option<Vec<EffectiveParameterRef<'_>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectiveActionOrigin {
    Workflow { action_index: usize },
    Step { step_index: usize, action_index: usize },
}

#[derive(Clone, Copy, Debug)]
pub struct EffectiveActionRef<'a> {
    pub action: &'a OnAction,
    pub origin: EffectiveActionOrigin,
}

pub fn effective_step_success_actions(
    workflow: &Workflow,
    step_index: usize,
) -> Option<Vec<EffectiveActionRef<'_>>>;

pub fn effective_step_failure_actions(
    workflow: &Workflow,
    step_index: usize,
) -> Option<Vec<EffectiveActionRef<'_>>>;
```

Parameter identity is the exact case-sensitive `(name, in_)` pair, including
`None`. Every Step target receives non-overridden Workflow declarations in
Workflow order followed by every Step declaration in Step order. Duplicate
declarations remain visible for validators; an invalid index returns `None`,
and an empty valid view returns `Some(empty)`.

Success and Failure Action lists remain independent and use exact
case-sensitive `name` identity. Their public borrowed/provenance surface is
fixed above, but effective ordering, invalid-duplicate defense, and the mapping
between effective ordinals and declaration indexes remain gated by
[ac-908bc](https://sonos.scapedeck.com/docs/ac-tickets/ac-908bc). Action
implementation tickets must be revisioned to the accepted rows before
lower-cost dispatch. Validators and runtimes consume these canonical references
through local path/context adapters rather than rebuilding inheritance.

### Runtime Expression parsing precedes field dispatch

`arazzo-expr/src/runtime_expression.rs` is the sole lexical owner for Arazzo
1.1 Runtime Expressions. `arazzo-spec` remains independent of expression
parsing. Both `arazzo-validate` and `arazzo-runtime` consume the published
`arazzo-expr` parser; neither adds a regex, prefix heuristic, or second grammar.
Adding the direct `arazzo-validate` dependency on `arazzo-expr` is accepted and
acyclic after the validator decomposition.

The base Runtime Expression parser exposes a borrowed, read-only contract:

```rust
pub fn parse_runtime_expression(
    input: &str,
) -> Result<ParsedRuntimeExpression<'_>, RuntimeExpressionError>;
```

It also owns a crate-private iterator over every valid Runtime Expression prefix
end so syntax consumers can resolve field boundaries without duplicating the
grammar or reparsing every prefix. The parsed expression keeps its AST private
and exposes only the raw input, namespace, local Step output reference,
source-description reference, and component reference needed by evaluator,
validator, and dependency consumers. Names borrow from the input; no returned
value is self-referential.

Parsing consumes the full input in bounded linear time without regex or
backtracking. Dotted `identifier` values are greedy: `$inputs.foo.bar` names the
single input `foo.bar`; nested access uses `#/bar`. Step, Workflow, and source
names use the strict no-dot identifier. A source-description expression splits
once after that strict source name and retains the entire nonempty remaining
reference ID. JSON Pointer escapes are validated, including rejection of
`~2`. Namespace-specific source forms remain exact; the shared-looking ABNF
does not authorize `$message.query` or `$request.payload`.

Expression-string classification is a separate parser that depends on this
base parser and on the accepted result of
[ac-d1e2f](https://sonos.scapedeck.com/docs/ac-tickets/ac-d1e2f). That decision
is accepted and recorded here. On 2026-08-23 Steve selected the
explicit-compatibility literal-brace classification: only the two-byte
sequence `{$` opens an embedded-expression candidate, and every other brace —
a `{` not followed by `$`, and any raw `}` outside an open candidate — is
literal expression-string text. This is a maintainer-owned deviation from the
published `literal-char` production, which excludes both braces from literal
text; it is not specification behavior, and the extension allowlist remains
empty. The recorded motivation is internal to the specification: the Section
5.8.14.2 JSON templated payload example contains literal braces the 1.1.0
ABNF row forbids, and no prose binds any field to the `expression-string`
production by name — but neither fact makes the compatibility row conformant.

The delimiter rule is fixed and is not part of the deviation: the first raw
`}` after `{$` is the sole span delimiter because raw `}` is excluded from
`CHAR`, HTTP `token`, both identifier productions, and JSON Pointer
`unescaped` — no Runtime Expression operand can contain it. Escaped document
text such as the six characters `\u007D` contains no raw delimiter.
`$sourceDescriptions.s.ref}tail` therefore has exactly one valid Runtime
Expression prefix, `$sourceDescriptions.s.ref`, ending before the `}`; as a
required complete expression the full string is rejected because `}tail`
remains unconsumed, and inside a template `{$sourceDescriptions.s.ref}tail`
is that span followed by the literal text `tail`.

Representative classification outcomes under the accepted row:

- `{$inputs.pet_id}` — template span; evaluates.
- `{"petOrder": {"petId": "{$inputs.pet_id}"}}` — template; the JSON braces
  are literal text and only the `{$inputs.pet_id}` span evaluates.
- `{name}`, `a{b`, `a}b`, `}`, `{}` — literal text; no candidate, no error.
- `pay in $USD` — literal; bare `$` outside braces stays literal.
- `{$inputs.pet_id` — reserved candidate with no closing raw `}`:
  classification error; no partial evaluation.
- `{$}`, `{$name}`, `{$inputs.a{b}` — exactly delimited candidates whose
  bodies the canonical Runtime Expression parser rejects: classification
  error; no partial evaluation.
- `$sourceDescriptions.s.ref}tail` in a required-expression field —
  `invalidExpression`.

Conformance disposition for the deviation: the literal-brace row is recorded
in the manifest as `deviation` with
[ac-27ced](https://sonos.scapedeck.com/docs/ac-tickets/ac-27ced) as evidence
owner. No grammar-only `covered` claim may cite `literal-char` while this
deviation stands; that same evidence ticket owns the corresponding
conformance-audit finding and a drafted upstream report of the
grammar/example contradiction for maintainer filing. Embedded-span
recognition, first-raw-`}` delimiting, and full-consumption inner parsing
remain grammar-conformant surfaces claimable through `expression-string` and
`embedded-expression`.

Simple Criterion syntax and evaluation depend on the accepted matrix from
[ac-ec3a1](https://sonos.scapedeck.com/docs/ac-tickets/ac-ec3a1). The vendored
specification fixes part of that behavior but leaves material whitespace,
precedence, Runtime Expression boundary, postfix, numeric, type, truthiness,
short-circuit, and error choices unresolved. The parser and evaluator remain
separate implementation units and neither may infer those choices from current
behavior.

Dispatch is field-specific after those decision gates are accepted:

- Literal-or-expression values — recursive Parameter, Request Body,
  Replacement, and action/subworkflow Parameter values — first accept a valid
  complete expression with type preservation, then valid `{$expression}` spans
  as a string template, otherwise retain the entire string literally. Thus
  `$inputs.x` evaluates, while `$USD`, direct `$env.X`, and `pay $inputs.x` are
  literal.
- Required-expression values — Workflow/Step outputs, Selector `context`,
  Criterion `context`, and Reusable Object `reference` — require one complete
  valid expression. An unknown namespace, template, malformed expression, or
  trailing text is an `invalidExpression` validation error.
- Regex, JSONPath, and XPath conditions substitute only brace-delimited Runtime
  Expressions. Unbraced `$` remains native condition text.
- Simple conditions use the accepted deterministic condition parser and the
  shared Runtime Expression parser for operands. Quoted `$...` remains a string
  literal; no second expression lexer is permitted. Unsupported or ambiguous
  cases fail closed only where the accepted condition matrix says they do.
- Action IDs, dependencies, and operation targets use their specialized field
  grammar and never the generic literal-value resolver.

Malformed embedded-expression behavior and literal braces are governed only by
the accepted brace decision and the fixed first-raw-`}` delimiter rule.
Implementations must not partially evaluate a rejected candidate, consume raw
`}` as Runtime Expression content, or invent an escape syntax.
Validation and direct-evaluator error projection are specified in the dependent
tickets after that decision is recorded.

### Effective Step dependency ownership

Implicit dependency discovery follows effective behavior, not raw Step text.
[ac-89176](https://sonos.scapedeck.com/docs/ac-tickets/ac-89176) is the sole
syntax-neutral model owner for non-action Step expression sites. It consumes the
canonical effective Parameter view, inventories request payload/replacements,
Step success Criteria, and Step outputs with declaration provenance, nested
value path, and field mode, and performs no `$`/brace/condition parsing.
[ac-70691](https://sonos.scapedeck.com/docs/ac-tickets/ac-70691) serially extends
that same owner with effective Success/Failure Action sites after the Action
overlay decision is accepted.

Step `correlationId` is not assigned a field mode by example inference.
[ac-dc3f2](https://sonos.scapedeck.com/docs/ac-tickets/ac-dc3f2) must record
whether it is opaque, executable with an exact field mode/consumer, or
fail-visibly deferred for AsyncAPI execution. The non-action inventory remains
non-dispatchable until its conditional row is replaced by that accepted result.

[ac-2f84e](https://sonos.scapedeck.com/docs/ac-tickets/ac-2f84e) is the only
syntax owner that turns a field-mode/text pair into ordered exact local
`$steps.<stepId>.outputs.<name>[#/pointer]` occurrences. It composes the
canonical Runtime Expression, expression-string, and simple-condition parsers;
neither `arazzo-spec`, validator, nor runtime may recreate those grammars with a
regex, prefix test, brace scan, or evaluator lookup.

[ac-fae45](https://sonos.scapedeck.com/docs/ac-tickets/ac-fae45) combines
explicit local `dependsOn` edges with parsed implicit edges in the landed
validator dependency owner and reports deterministic unknown references and
mixed cycles at their authored sites. [ac-0686b](https://sonos.scapedeck.com/docs/ac-tickets/ac-0686b)
owns runtime plans for whole Workflow, transitive target closure, and target
direct-only scopes, including stable source-index topological order and levels.
The runtime does not scan expressions or rebuild the model inventory.

Action-free full Workflow and run-step scheduling are separately settled by
[ac-c1a40](https://sonos.scapedeck.com/docs/ac-tickets/ac-c1a40) and
[ac-1bb5e](https://sonos.scapedeck.com/docs/ac-tickets/ac-1bb5e). Actionful full
and filtered control flow remain behind
[ac-a73b6](https://sonos.scapedeck.com/docs/ac-tickets/ac-a73b6); the provisional
[ac-c6a65](https://sonos.scapedeck.com/docs/ac-tickets/ac-c6a65) and
[ac-543d9](https://sonos.scapedeck.com/docs/ac-tickets/ac-543d9) bodies are not
implementation authority until that decision is accepted, every alternative or
placeholder is removed, and fresh frontier review passes. Final evidence in
[ac-f4ea8](https://sonos.scapedeck.com/docs/ac-tickets/ac-f4ea8) may call
actionful execution `covered` only with positive scheduler proof; bounded
rejection remains `unsupported` with fail-closed evidence.

This is a pre-1.0 conformance correction, not a compatibility option. The
existing `starts_with('$')` dispatch, nested-dot fallback for dotted input names,
and any secondary Runtime Expression scanners are removed without a feature
flag, fallback, warning-only mode, or extension entry.

### Action targets are identifiers, not templates

Success and Failure Action `stepId` values are literal IDs in the current
Workflow. A `workflowId` is either a literal ID in the current Arazzo document
or an exact `$sourceDescriptions.<strict-name>.<nonempty-reference-id>` form for
an external Arazzo document. Local membership is checked before interpreting a
leading `$`, because identifier regexes are recommendations rather than hard
syntax at these fields.

`step_{$inputs.target}`, `$steps...` action targets, and all other dynamic
target interpolation are non-spec behavior and are removed in separate
validator and runtime units. External Arazzo execution remains unsupported and
must fail visibly before any external target side effect. Action Parameter
values remain literal-or-expression values and keep their normal diagnostics.

### Grammar clauses are first-class evidence

The conformance-manifest contract must represent ABNF-backed claims without
inventing an RFC 2119 level. An `arazzoSpec` claim keeps its specification
anchor and supplies at least one of `normativeLevels` or `grammarRules`; both are
allowed when the claim genuinely uses both. Grammar rule names are checked
against a manifest-level allowlist with the same unknown, duplicate, and unused
guards as specification anchors. A grammar-only claim never fabricates `MUST`,
`SHALL`, or another normative keyword.

### operationPath presentation does not redefine execution

Section 5.8.5 defines `operationPath` as *"A reference to a Source Description
Object combined with a JSON Pointer to reference an operation"* and requires a
runtime expression to identify the source document. That conformant form is
presented unchanged by CLI and MCP surfaces. The repository also accepts a
runtime-only `METHOD target` form; presenting that existing form consistently
does not make it part of the Arazzo specification.

`arazzo_spec::split_operation_method` is the sole presentation classifier. It
recognizes only exact uppercase `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`,
`OPTIONS`, or `TRACE` followed by one literal ASCII space. It returns the entire
remaining target byte-for-byte, including any additional leading space.
Lowercase or mixed-case tokens, unknown tokens, leading whitespace, and values
without that delimiter remain one raw target. CLI human/JSON output and MCP
`describe_workflow` consume this helper; neither adds a second parser.

Execution remains owned by `Engine::prepare_http_request`. For a recognized
runtime form it uses the classified method and target. Otherwise its existing
pre-resolution default is `POST` when the Step has a Request Body Object
(`request_body.is_some()`), even when its payload is absent or null, and `GET`
when the object is absent. A conformant specification-form `operationPath` is
therefore displayed raw and can still fail later through the existing visible
unsupported-resolution path. This presentation work changes no schema, public
Rust API, runtime error code, or execution semantics.

The implementation units are deliberately disjoint: one CLI unit covers list,
inspect, and their JSON contracts; one MCP unit covers `describe_workflow`.
End-to-end evidence uses a shared table containing every supported method plus
lowercase, unknown, leading-whitespace, no-delimiter, double-space, request-body
object with absent/null payload, and specification-form rows. Exact reservations
serialize either unit with contemporaneous writers of the same handler file.

### XPath is a versioned backend boundary, not a preprocessing mode

Section 5.8.11.4.4 says an XPath condition must conform to the selected version,
with XPath 3.1 as the default, and that EBV *"MUST follow the Effective Boolean
Value (EBV) semantics defined by the XPath version being used."* The current
Uppsala 0.3.0 dependency is accepted only as an explicit XPath 1.0 backend. It
must not execute omitted/default-3.1, 2.0, 3.0, 3.1, or unknown versions as 1.0,
and XML namespace text must not be rewritten before evaluation.

`runtime_core/xpath.rs` owns one private parse/version/namespace setup and two
crate-private operations:

1. read-only evaluation over original XML, expression, and declared version,
   returning owned EBV, selector normalization, and cardinality; and
2. replacement over original XML, expression, declared version, and replacement
   value, returning serialized XML.

Uppsala `Document` and `NodeId` values never leave that module. `criteria.rs`
consumes EBV; `payload.rs` retains selector/replacement dispatch, warning, and
unchanged-body policy but no backend-owned mutation state. Bare `//...`
output/debug expressions remain an explicit repository XPath 1.0 extension and
must stay covered through the debugger production caller.

XPath 3.1 adoption is not yet settled. A frontier decision gate must select or
reject a concrete backend and freeze its package/version, public APIs used,
MSRV 1.82 result, dependency and supply-chain delta, sync/owned-result boundary,
criterion/selector/replacement coverage, and unsupported-version behavior.
The implementation ticket remains blocked and is not lower-cost-ready until
that decision is accepted and the ticket is rewritten against it. No candidate
name, fallback, version coercion, or legacy namespace-stripping mode is an
accepted decision merely because it appeared in an older ticket.

### Compatibility and fail-closed policy

New tests that expose a production defect land with the behavior-owning fix,
not as a permanently red test. Unsupported conformant input must fail visibly
before an HTTP side effect. Existing non-spec forms are not treated as
precedent or silently added to an allowlist.

## Execution slices

1. Establish the conformance manifest and its workspace gate.
2. Correct settled model/validator rules for versions, source URI references,
   required Any-value presence, Parameter locations/duplicates, and action
   fixed fields.
3. Correct retry defaults/fall-through and the remaining settled criterion,
   trace, target-reference, and dependency semantics.
4. Promote the existing typed-JSONPath finding instead of duplicating it.
5. Add CLI TAP/JUnit contract tests and extend the committed 1.1 fixture across
   CLI, MCP, and DAP after the behavior members close.
6. Keep OpenAPI source-owned `operationId` routing in the OpenAPI ingestion
   epic, with the final integration member depending on it.
7. Correct the locked XPath 1.0 boundary and prove it through focused runtime
   and debugger coverage without growing the runtime God test.
8. Run the XPath 3.1 backend decision gate; only an accepted concrete dependency
   contract may unlock its implementation unit.
9. Align operationPath presentation in the disjoint CLI and MCP units without
   changing runtime routing.
10. Remove direct `$env` evaluation and add the canonical Runtime Expression
    parser as independent roots; migrate evaluator dispatch after both without
    growing the expression God module.
11. Accept the simple Criterion grammar/evaluation decision, extract the current
    condition owner isomorphically, then implement syntax and evaluation as
    separate units on the canonical Runtime Expression parser. Propagate
    structured failures through runtime and debugger boundaries before recording
    bounded evidence.
12. Accept the expression-string brace decision, then add the dedicated
    expression-string parser and diagnostic-preserving renderer; apply the
    field-mode matrix through the decomposed validator. Keep the legacy
    brace-only `interpolate_string` adapter only while action-target cleanup
    still calls it, then remove that adapter and its regex in the dedicated
    post-cleanup transition ticket.
13. Remove dynamic action targets in separate validator and runtime units before
    adding action-Parameter and subworkflow-Parameter warning transport.
14. Extend the manifest contract for grammar-backed claims, then prove the env
    namespace and embedded-expression boundaries in focused CLI targets rather
    than the CLI God test.

## Dependency policy

The manifest is the root for new members. Parameter-context validation follows
the value-presence work; action validation follows Parameter validation; retry
semantics follows action validation. The existing decimal-`retryAfter` ticket
overlaps the retry implementation and is serialized by the single-writer rule,
but it is not a semantic prerequisite. Typed criterion preprocessing follows
the existing XPath 1.0 dispatcher and typed-JSONPath rejection; trace reporting
follows preprocessing. Local
implicit dependency completion precedes cross-scope fail-closed dependency
handling. The final cross-surface guard depends on every behavior member and
the source-owned `operationId` routing ticket.

2026-08-26, Steve-directed: the source-owned `operationId` routing ticket
([ac-c5105](https://sonos.scapedeck.com/docs/ac-tickets/ac-c5105)) was split.
It now owns the runtime half only — the `arazzo-spec` classifier, the
origin-aware operation index, per-source base selection, and pre-HTTP
ambiguity rejection — and no longer depends on the validator train, making it
ready immediately (driven by GitHub issue #5 and sonos-hub's first-party
multi-source requirement). Its validate-time diagnostics half is
[ac-fd667](https://sonos.scapedeck.com/docs/ac-tickets/ac-fd667), which
carries the former ac-d15a3 dependency and follows the validator
decomposition. When the final cross-surface guard's edges are drawn, both
halves count as the routing prerequisite.

For the Runtime Expression slice, `$env` evaluator removal and the syntax-only
parser are independent roots; the evaluator migration follows both, and
interpolation follows evaluator migration. Validator field enforcement follows
interpolation plus the settled simple-condition boundary. Action validation
follows field enforcement, and runtime action-target removal follows action
validation. Runtime warning transport follows both interpolation and
action-target removal. CLI evidence follows its runtime/validator owner plus the
grammar-manifest contract.

File overlap alone is not represented as a false semantic dependency. The
single-writer repository policy still serializes overlapping implementation
work.

### Intentional ready-root ledger

At the 2026-08-22 graph review, the epic intentionally has these seven ready
members. They are not ordered against one another because no semantic output of
one is an input to another at this stage:

- [ac-80673](https://sonos.scapedeck.com/docs/ac-tickets/ac-80673) owns the
  canonical Runtime Expression parser.
- [ac-9c811](https://sonos.scapedeck.com/docs/ac-tickets/ac-9c811) removes the
  non-specification `$env` namespace.
- [ac-aac49](https://sonos.scapedeck.com/docs/ac-tickets/ac-aac49) performs the
  isomorphic simple-condition ownership extraction.
- [ac-ec3a1](https://sonos.scapedeck.com/docs/ac-tickets/ac-ec3a1) is the human
  simple-condition semantics decision gate, not an implementation dispatch.
- [ac-b8d9a](https://sonos.scapedeck.com/docs/ac-tickets/ac-b8d9a) owns the
  grammar-claim manifest representation.
- [ac-d1e2f](https://sonos.scapedeck.com/docs/ac-tickets/ac-d1e2f) is the human
  expression-string literal-brace decision gate, not an implementation
  dispatch.
- [ac-a983f](https://sonos.scapedeck.com/docs/ac-tickets/ac-a983f) owns the
  already-prerequisite-satisfied typed JSONPath rejection surface.

This ledger must be refreshed before kickoff if `tkt ready` changes. It does not
authorize concurrent writes: [ac-80673](https://sonos.scapedeck.com/docs/ac-tickets/ac-80673),
[ac-aac49](https://sonos.scapedeck.com/docs/ac-tickets/ac-aac49), and
[ac-9c811](https://sonos.scapedeck.com/docs/ac-tickets/ac-9c811) all touch
`crates/arazzo-expr/src/lib.rs` and must be serialized absent exact disjoint live
reservations. [ac-d1e2f](https://sonos.scapedeck.com/docs/ac-tickets/ac-d1e2f)
and [ac-ec3a1](https://sonos.scapedeck.com/docs/ac-tickets/ac-ec3a1) both edit
this plan and must likewise be serialized.

## External owners and prerequisites

- [ac-95df9](https://sonos.scapedeck.com/docs/ac-tickets/ac-95df9) owns the
  document-identity and reference-resolution design;
  [ac-93d90](https://sonos.scapedeck.com/docs/ac-tickets/ac-93d90) follows it
  for lossless JSON Schema 2020-12 input support.
- [ac-68f7d](https://sonos.scapedeck.com/docs/ac-tickets/ac-68f7d) owns the
  unsettled duplicate-Action identity rule. The inline
  fixed-field member does not invent that rule.
- [ac-c5105](https://sonos.scapedeck.com/docs/ac-tickets/ac-c5105) remains in
  the OpenAPI ingestion epic and owns source-qualified `operationId` routing.
  Closed [ac-40602](https://sonos.scapedeck.com/docs/ac-tickets/ac-40602) is its
  source-loading baseline.
- Existing [ac-80a8f](https://sonos.scapedeck.com/docs/ac-tickets/ac-80a8f)
  must settle versioned expression semantics before the simple Criterion
  grammar is narrowed. Existing
  [ac-46638](https://sonos.scapedeck.com/docs/ac-tickets/ac-46638) must establish
  the XPath dispatcher before typed Criterion interpolation is completed.
- [ac-4c05c](https://sonos.scapedeck.com/docs/ac-tickets/ac-4c05c) owns the
  unresolved XPath 3.1 package/backend decision. The blocked implementation
  [ac-d45cd](https://sonos.scapedeck.com/docs/ac-tickets/ac-d45cd) cannot select
  a dependency or start until that decision is accepted and its contract is
  rewritten.
- [epic:ac-aeca8](https://sonos.scapedeck.com/docs/ac-tickets/ac-aeca8) owns the
  complete XPath correction sequence, including the locked 1.0 backend and the
  gated 3.1 implementation.
- [epic:ac-62307](https://sonos.scapedeck.com/docs/ac-tickets/ac-62307) owns the
  two presentation-only operationPath adapters.
- [ac-80673](https://sonos.scapedeck.com/docs/ac-tickets/ac-80673) owns only the
  canonical full-consumption Runtime Expression parser and prefix iterator;
  [ac-9eaf1](https://sonos.scapedeck.com/docs/ac-tickets/ac-9eaf1) migrates the
  evaluator to that parser. [ac-d1e2f](https://sonos.scapedeck.com/docs/ac-tickets/ac-d1e2f)
  settles expression-string brace classification before
  [ac-a48b1](https://sonos.scapedeck.com/docs/ac-tickets/ac-a48b1) supplies
  syntax to the renderer owned by
  [ac-28ba3](https://sonos.scapedeck.com/docs/ac-tickets/ac-28ba3). That
  renderer ticket deliberately preserves the legacy brace-only adapter until
  [ac-cd36b](https://sonos.scapedeck.com/docs/ac-tickets/ac-cd36b) removes its
  action-target caller; only then may
  [ac-bb7ab](https://sonos.scapedeck.com/docs/ac-tickets/ac-bb7ab) remove the
  adapter and regex. This ordering prevents a dependency cycle and keeps the
  temporary compatibility surface explicitly bounded.
- [ac-ec3a1](https://sonos.scapedeck.com/docs/ac-tickets/ac-ec3a1) settles the
  simple Criterion grammar/evaluation matrix. The current condition owner is
  first extracted isomorphically by
  [ac-aac49](https://sonos.scapedeck.com/docs/ac-tickets/ac-aac49);
  [ac-67bf5](https://sonos.scapedeck.com/docs/ac-tickets/ac-67bf5) is then
  limited to syntax,
  [ac-60b0b](https://sonos.scapedeck.com/docs/ac-tickets/ac-60b0b) owns
  evaluation semantics, and
  [ac-d98bd](https://sonos.scapedeck.com/docs/ac-tickets/ac-d98bd) maps the
  resulting structured failures into runtime/debugger decisions. Finally,
  [ac-7b681](https://sonos.scapedeck.com/docs/ac-tickets/ac-7b681) records the
  bounded evidence after all behavior tickets pass.
- [ac-b8d9a](https://sonos.scapedeck.com/docs/ac-tickets/ac-b8d9a) extends the
  finite manifest for grammar-only claims. The env and interpolation surface
  tickets consume that contract; neither fabricates an RFC 2119 level.
- [ac-a7fe9](https://sonos.scapedeck.com/docs/ac-tickets/ac-a7fe9) validates
  Action targets as exact references, and
  [ac-cd36b](https://sonos.scapedeck.com/docs/ac-tickets/ac-cd36b) removes the
  runtime interpolation path before Parameter-warning transport begins.

These tickets are linked to this initiative. Decision owners are not treated as
completed implementation and do not create a false claim that their outcomes
are already settled.

## Explicit non-goals and planning gates

- Full document-set identity, `$self` base precedence, fragments/`$anchor`,
  linked Arazzo loading, and specification-form `operationPath` remain behind
  the existing document-routing design work. Their compatibility and loading
  policy are not silently decided here.
- A full JSON Schema 2020-12 runtime validator and public Success/Failure Rust
  type split require separate compatibility/dependency decisions. This epic
  records those gaps and may produce accepted designs; it does not claim they
  are implemented.
- OpenAPI `style`/`explode`/`content` serialization, AsyncAPI transport,
  `$response.query/path`, and removal of every documented legacy expression
  form are not absorbed into test infrastructure.
- No version bump, release tag, push, publication, or marketplace action.

## System success

The conformance manifest is complete for the defined Arazzo 1.1 claim surface,
validates in the normal workspace test run, and links each covered claim to
focused executable evidence. All new behavior members and the final semantic
guard pass without unrelated golden re-baselining. Remaining unsupported or
non-conformant clauses are visible, fail safely where execution is possible,
and name an owner. Release notes can then state the exact supported subset
without claiming full Arazzo 1.1.0 compliance.
