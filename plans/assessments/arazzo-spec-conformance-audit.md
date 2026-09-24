# Arazzo Specification Conformance Audit

**Date:** 2026-08-03
**Scope:** whole workspace — `arazzo-spec`, `arazzo-validate`, `arazzo-expr`, `arazzo-runtime`, `arazzo-generate`, `arazzo-cli` (`arazzo-mcp` and `arazzo-debug-adapter` inherit runtime behavior).
**Grounding:** normative field definitions taken from the published Arazzo Specification v1.1.0 (spec.openapis.org/arazzo/latest.html), not from memory. Every finding below was reproduced against the built CLI; commands and observed output are recorded per finding.

## Summary

Eleven deviations in the original pass, F1–F11 below. Two cause a conformant
document to fail outright or silently misbehave; the rest are unenforced
constraints, ignored fields, non-conformant output, and undocumented
extensions. Later passes added F12–F25 — see the addenda at the end, which also
record which findings have since been closed.

The single most important structural fact: **our `operationPath` idiom and our
`sourceDescriptions[].url` meaning are entangled deviations.** Our form
(`"GET {source}./path"` + url-as-base-URL) is internally consistent and
convenient, but it is not the specification's form, and the specification's
form does not work at all — it validates and then silently produces a
nonsense URL (F1). Any decision about one field forces a decision about the
other.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F1 | Spec-form `operationPath` validates, then builds a garbage URL | P1 | runtime |
| F2 | `in: querystring` rejects the entire document | P1 | model |
| F3 | `targetSelectorType` not modeled; selector kind inferred instead | P2 | model/runtime |
| F4 | `generate` emits base URL as `sourceDescriptions[].url` | P2 | generate |
| F5 | `generate` emits non-conformant `operationPath` syntax | P2 | generate |
| F6 | Identifier regexes never enforced (two MUST, three SHOULD) | P2 | validate |
| F7 | `successCriteria: []` accepted | P2 | validate |
| F8 | Unknown non-`x-` fields silently accepted | P2 | model/validate |
| F9 | `$env.*` is a non-spec expression source | P3 | expr |
| F10 | Bare `//xpath` output values are a non-spec form | P3 | expr |
| F11 | `$response.query.<name>` unsupported | P3 | expr |

---

## P1 — Conformant input breaks

### F1. Spec-form `operationPath` validates, then silently builds a garbage URL

**Spec:** *"A reference to a Source Description Object combined with a JSON
Pointer to reference an operation… A Runtime Expression syntax must be used"* —
e.g. `{$sourceDescriptions.petstore.url}#/paths/~1pets/get`.

**We do:** treat `operationPath` as `"[METHOD ]{sourceName}.<path>"`. The
source prefix parser requires a literal `{name}.` prefix
(`crates/arazzo-runtime/src/runtime_core/url.rs`, `parse_source_prefix`), and a
leading `"METHOD "` token is stripped by `parse_method` in the same file. Bare
`/get` paths are joined onto the source's base URL
(`crates/arazzo-runtime/src/runtime_core/engine_http.rs:448-470`). There is no
JSON Pointer resolution for `operationPath` anywhere in the runtime.

**Observed** — a fully conformant step:

```
$ arazzo validate conform2.arazzo.yaml
Valid Arazzo 1.1.0 spec: Conformance probe

$ arazzo run conform2.arazzo.yaml probe --dry-run
GET https://petstore.example.com/v1{$sourceDescriptions.petstore.url}#/paths/~1pets/get
```

The expression text is concatenated literally onto the base URL. No error, no
warning. This is the worst failure class in the audit: the document is declared
valid and a request is then constructed against a nonsense URL, carrying
whatever headers the workflow configures.

**Note:** all 18 files in `examples/` use a non-conformant `operationPath` form,
and 17 of them have no OpenAPI document at all to point a JSON Pointer at — so
adopting the conformant form is not a mechanical migration. The forms in use are
not uniform, which matters for regression coverage: 16 files use bare paths,
`multi-api-orchestration.arazzo.yaml` uses `{source}./path` (`:37`, `:49`), and
`swagger-petstore-crud.arazzo.yaml` uses `METHOD {source}./path` for all 10 of
its steps — 27 `"METHOD "`-prefixed operationPaths across seven files in total.
The absolute-URL form appears in no example or `testdata/` file at all.

**Second-order defect found while ticketing this.** `arazzo-validate` carries its
own copy of the source-prefix parser, `parse_operation_source_name`
(`crates/arazzo-validate/src/lib.rs:526-540`), which — unlike the runtime's
`parse_source_prefix` (`crates/arazzo-runtime/src/runtime_core/url.rs:35-47`) —
never strips a leading `"METHOD "` token. Its `starts_with('{')` test therefore
fails on `POST {swagger-petstore-openapi-3-0}./pet`, and the
unknown-`sourceDescription` check at `:283-291` is silently skipped for every
one of those 27 operationPaths. Folded into ac-c5c17, which consolidates both
parsers into a single classifier in `arazzo-spec`.

### F2. `in: querystring` rejects the entire document

**Spec:** Parameter Object `in` allows `path`, `query`, `querystring`,
`header`, `cookie`.

**We do:** `ParamLocation` (`crates/arazzo-spec/src/lib.rs:452-457`) defines
only `Path`, `Query`, `Header`, `Cookie`. Because it is a plain serde enum, an
unknown variant is a deserialization error, so the failure is not scoped to the
offending parameter — the whole file fails to parse.

**Observed:**

```
$ arazzo validate conform.arazzo.yaml
validation failed: parsing arazzo yaml: workflows[0].steps[0].parameters[0].in:
unknown variant `querystring`, expected one of `path`, `query`, `header`, `cookie`
```

A conformant document is unusable, and the message tells the author their valid
value is invalid.

---

## P2 — Conformant input degraded, or non-conformant output produced

### F3. `targetSelectorType` is not modeled

**Spec:** Payload Replacement Object has `target` (**REQUIRED**),
`targetSelectorType` (`jsonpath` | `xpath` | `jsonpointer`, or an Expression
Type Object), and `value` (**REQUIRED**). *"If omitted, JSON Pointer for
application/json, XPath for application/xml."*

**We do:** `Replacement` (`crates/arazzo-spec/src/lib.rs:547-557`) has only
`target` and `value`. `targetSelectorType` / `target_selector_type` has zero
occurrences in `arazzo-spec` and `arazzo-runtime`; the selector kind is always
inferred from content type. An author who explicitly declares
`targetSelectorType: jsonpath` on a JSON body gets it silently ignored and the
target interpreted as a JSON Pointer instead — the replacement misapplies or
no-ops rather than erroring.

The related **Selector Object** (1.1) *is* modeled and validated
(`crates/arazzo-validate/src/lib.rs:617-640`, incl. `jsonpointer` and version
checks), so this gap is specific to Payload Replacement.

### F4. `generate` writes a base URL into `sourceDescriptions[].url`

**Spec:** *"A URL to a source description to be used by a workflow"* — the
document.

**We do:** `crates/arazzo-generate/src/crud.rs:59-64` writes the API server URL
extracted by `extract_server_url` (`:98-129`). Already ticketed as **ac-91284**;
the runtime side is the compatibility rule protecting existing files. It now
applies only as a fallback: `f0adfeb` resolves an absolute url by document
identity first (§9.6), and network fetching of a `url` was answered rather than
deferred — GitHub issue #4 is closed as completed, identity-based referencing
being the answer.

### F5. `generate` emits non-conformant `operationPath`

**We do:** `crates/arazzo-generate/src/crud.rs:723` builds
`format!("{method} {{{source_name}}}.{path}")`, i.e. the F1 syntax. Every file
the tool generates is non-conformant on this field. Blocked on the same
open decision as F1.

### F6. Identifier regexes are never enforced

**Spec — note the requirement levels differ, which this audit originally got
wrong.** Only two of the five positions are normative:

- `sourceDescriptions[].name`, `workflowId`, `stepId` — **SHOULD**: *"it is
  RECOMMENDED to follow common programming naming conventions. SHOULD conform
  to the regular expression `[A-Za-z0-9_\-]+`."*
- workflow/step `outputs` keys — **MUST**: *"The name MUST use keys that match
  the regular expression: `^[a-zA-Z0-9\.\-_]+$`."*
- Components Object map keys — **MUST**: *"All the fixed fields declared above
  are objects that MUST use keys that match the regular expression:
  `^[a-zA-Z0-9\.\-_]+$`."*

This distinction is load-bearing. Enforcing the SHOULD-level positions as hard
errors would make `arazzo validate` reject specification-conformant documents,
which is the failure class this audit exists to eliminate. Ticketed
accordingly: MUST-level rules are errors (ac-4a71f), SHOULD-level rules are
warnings promoted under a strictness flag (ac-0379b).

**We do:** validation checks emptiness and uniqueness only
(`crates/arazzo-validate/src/lib.rs:173-186` for `sourceDescriptions[].name`,
`:212-224` for `workflowId`, `:259-271` for `stepId`). No character-class check
exists anywhere in the crate.

### F7. `successCriteria: []` is accepted

**Spec:** *"If provided, must contain at least one Criterion Object."*

**We do:** no emptiness check on `success_criteria` in validation.

### F8. Unknown non-`x-` fields are silently accepted

**Spec:** extensions must be `x-` prefixed.

**We do:** no `deny_unknown_fields` anywhere in `arazzo-spec`. A typo such as
`typoField:` or `sucessCriteria:` is dropped silently, so a misspelled field
looks like a valid document with missing behavior.

**Observed (F6+F7+F8 together)** — this document reports as valid:

```yaml
sourceDescriptions:
  - name: "bad name with spaces!"      # violates [A-Za-z0-9_\-]+
workflows:
  - workflowId: "has spaces & bang!"   # violates [A-Za-z0-9_\-]+
    typoField: not-an-x-extension      # unknown non-x- field
    steps:
      - stepId: "step with spaces!"    # violates [A-Za-z0-9_\-]+
        successCriteria: []            # must contain >= 1 criterion
        outputs:
          "bad key!": $response.body   # violates ^[a-zA-Z0-9\.\-_]+$
```

```
$ arazzo validate gaps.arazzo.yaml
Valid Arazzo 1.1.0 spec: Gap probe
```

Six violations, reported clean. This matters more than any single rule: users
run `arazzo validate` precisely to be told their document is conformant.

---

## P3 — Undocumented extensions and minor gaps

These are defensible as extensions. The problem is that nothing labels them as
extensions, so users cannot tell which parts of their workflow are portable.

### F9. `$env.VAR_NAME`

Not among the specification's runtime expression sources. Implemented in
`arazzo-expr` and documented in `CLAUDE.md`'s expression surface as if it were
standard.

### F10. Bare `//xpath` output values

e.g. `outputs: { title: //item[1]/title }`. Spec output values are a runtime
expression or a Selector Object; a bare XPath is neither. The conformant 1.1
form (Selector Object with `type: xpath`) *is* supported, so this is a legacy
alternative worth marking as an extension.

### F11. `$response.query.<name>` unsupported

Listed by the specification; not implemented. Low practical impact.

### Correctly implemented (checked, no deviation)

Verified present and conformant, to bound the audit: `$workflows.*`,
`$message.*`, `$self`, `$sourceDescriptions.*`, `$components.*` reusable
reference resolution (`arazzo-validate` :890-947), Selector Object with type and
version validation, Success/Failure action types (`end`/`goto`/`retry`), and
Step fields `timeout`, `correlationId`, `action`, `dependsOn`.

**Correction — `x-` vendor extension preservation is *not* complete.** An
earlier revision of this audit listed it as verified across the model. Six
types carry no `#[serde(flatten)]` extension capture and therefore drop `x-`
fields silently: `Replacement` (`crates/arazzo-spec/src/lib.rs:547`),
`OutputValue` (`:630`), `StepTarget` (`:265`), `StepAction` (`:275`),
`SelectorType` (`:582`), and `ValueSource` (`:690`). `Replacement` is the one
that matters — it is a first-class spec object whose extensions are lost on
round-trip. Folded into ac-bd441, which is already editing that struct.

---

## Recommendation

Three tracks, in dependency order:

1. **Fix what breaks conformant input** — F2 (add `querystring`) and F3 (model
   `targetSelectorType`) are small, self-contained, and have no design
   dependency. F1 needs at minimum a *loud failure*: an unresolvable
   `operationPath` must error instead of concatenating expression text into a
   URL. That is worth doing immediately even while the idiom question is open.
2. **Close the validator gaps** — F6, F7, F8. These make `arazzo validate`
   mean what users think it means. Severity follows the specification's own
   requirement level, not the implementer's confidence: MUST-level rules ship
   as errors (F7, and the outputs/Components half of F6); everything else ships
   as a warning promoted under a single strictness flag (the identifier half of
   F6, and F8, which will surface existing typos in user files). All three
   need a warning channel that `arazzo-validate` does not have today — it
   returns `Result<(), Error>` and its only warnings are `eprintln!` calls from
   inside the library, invisible to `--json`, MCP, and the debug adapter. That
   primitive is the prerequisite, not an implementation detail of whichever
   check lands first.
3. **Decide the idiom question** — F1/F4/F5 are one decision, not three:
   do we adopt spec-conformant `operationPath` + document-pointing `url` (and
   what replaces the base-URL idiom for the 17 document-less examples), or do we
   keep our form and document it explicitly as an arazzo-cli extension? This is
   a product decision with breaking-change scope and belongs in a plan, not a
   ticket.

---

## Addendum — 2026-08-05

Four further deviations found while reworking `AGENTS.md`, grounded the same
way (published v1.1.0 text, not memory). Not triaged into the recommendation
tracks above.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F12 | Condition operators `contains`, `matches`, `in [...]` | P3 | expr |
| F13 | `$response.body` wildcard/filter traversal | P3 | expr |
| F14 | `RequestBody.reference` is not a spec field | P2 | model |
| F15 | Success and Failure Action Objects share one `OnAction` struct | P2 | model |

### F12. `contains` / `matches` / `in [...]`

The Criterion Object's `simple` grammar defines `<`, `<=`, `>`, `>=`, `==`,
`!=`, `!`, `&&`, `||`, `()`, `[]`, `.` over `boolean`/`null`/`number`/`string`
literals. These three word operators are not in it. Implemented in
`arazzo-expr` (`for word_op in [" contains ", " matches ", " in "]`).

Blast radius is small: one fixture uses them —
`examples/httpbin-conditions.arazzo.yaml`, three occurrences. `matches` has a
conformant replacement in a `regex`-type criterion.

### F13. `$response.body` wildcard and filter traversal

Beyond the spec's `.` de-reference, the evaluator accepts `[*]`, GJSON-style
`.#`, `#(k==v)`, `#(k==v)#`, and JSONPath filters `[?(@.k=="v")]` inside a dot
path. Overlaps the conformant Selector Object with `type: jsonpath`, which is
already implemented — so this has a like-for-like replacement.

### F14. `RequestBody.reference`

The Request Body Object has exactly `contentType`, `payload`, `replacements`.
Arazzo has no reusable request bodies and `$components` has no `requestBodies`
map. `crates/arazzo-spec/src/lib.rs` carries a `reference` field on
`RequestBody` regardless.

Distinct from `Parameter.reference`, which is the legitimate Reusable Object
(`reference` + `value`) modeled inline.

### F15. `OnAction` merges Success and Failure Action Objects

One struct backs both `successActions` and `onSuccess`/`onFailure`. It carries
`retryAfter` and `retryLimit`, which the spec defines only on the Failure
Action Object — so a `successActions` entry with `retryAfter` parses and
validates. Neither variant models `parameters`, which the spec defines on both.

---

## Addendum — 2026-08-05 (ac-dd828 compliance review)

One further deviation, found while evaluating `ac-dd828` (`in: querystring`)
against the compliance gate. Grounded in the now-vendored published texts —
`spec/arazzo/v1.1.0.html` and `spec/oas/v3.2.0.html` — and reproduced against
the CLI at `cabda6c`.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F16 | Parameter serialization ignores OpenAPI `content` / `style` / `explode` | P2 | runtime |

**F2 is closed.** `314bd4e` modeled `ParamLocation::Querystring` and added
verbatim query-component assembly; `cabda6c` added the at-most-one rule. Two
follow-ups are open against the decisions `ac-dd828` recorded rather than
against F2 itself: `ac-8c8a9` (accepting `querystring` in a `1.0.x` document)
and `ac-7befc` (a non-string value dropping the query instead of failing).

### F16. Parameter serialization ignores OpenAPI `content`, `style`, `explode`

The runtime never reads the referenced operation's parameter definitions.
Grepping `crates/arazzo-runtime/src/` for `parameters`, `content`, `style`, and
`explode` returns nothing; what it takes from a source description is `paths`,
`servers`, `url`, and `variables`. Every `in:` location is serialized from the
Arazzo value alone.

Two consequences, both grounded in the vendored texts:

**1. `querystring` assumes a form-urlencoded, already-encoded value.** The
Arazzo Parameter Object says the value *"MUST match the media type format as
expressed by the parameter's `content` field (e.g.,
`application/x-www-form-urlencoded`)"*, and OpenAPI 3.2 requires `content` for
this location — *"most often"* form-urlencoded, so not always. A value matching
a non-form media type is inserted verbatim and unencoded:

    $ arazzo-cli run nonstring.arazzo.yaml wf1 --dry-run -i 'filters={"q":"red"}'
    GET https://search.example.com/v1/index?{"q":"red"}

That is not a valid query component. The `#` rejection added by `ac-dd828`
catches one illegal character; it does not make the value conformant.

**2. `in: query` hardcodes one serialization, and gets the object case wrong.**
OpenAPI 3.2 gives `in: query` a default of `style: form` (*"Default values
(based on value of `in`): for `"query"` - `"form"`"*) with `explode` defaulting
to true (*"When style is `form` or `cookie`, the default value is true"*),
under which *"parameter values of type array or object generate separate
parameters for each value of the array or key-value pair of the map."*

`build_url_from_path` emits arrays as repeated pairs, which matches that
default — but emits objects as a JSON string
(`serde_json::to_string`, `crates/arazzo-runtime/src/runtime_core/engine_http.rs`),
where the default calls for `a=1&b=2`. Non-default styles
(`pipeDelimited`, `spaceDelimited`, `deepObject`, `explode: false`) are
unreachable for any location.

Unlike the object case, this is not an exotic-input problem: it is the
documented OpenAPI default.

**Decision, recorded rather than fixed.** Serializing from the Arazzo value
alone is now a stated stance in `AGENTS.md` ("Parameter serialization"), not an
oversight to be patched per-location. Fixing `querystring` alone would make it
the only location that consults OpenAPI metadata — a sharper inconsistency than
the uniform gap. Becoming OpenAPI-parameter-aware is a workspace-wide feature
touching every location, with real interop value (`style`/`explode` mismatches
are a common complaint) and its own ticket when it is wanted. The object-case
deviation in consequence 2 is the strongest argument for doing so, and is the
narrowest place to start.

---

## Addendum — 2026-08-13 (ac-3a7f4 follow-up)

One further deviation, surfaced by the ac-3a7f4 fresh review and confirmed
here. Grounded in the vendored texts (`spec/arazzo/v1.1.0.html` §5.8.8; the
same sentence appears verbatim in `spec/arazzo/v1.0.1.html`, so this is not
1.1-only vocabulary) and reproduced against the CLI at `af8cb14`.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F17 | `retry` action `stepId`/`workflowId` reference is never executed | P2 | runtime |

This partially corrects the original pass's "Correctly implemented" list, which
recorded Success/Failure action types (`end`/`goto`/`retry`) as verified: the
constrained-retry half of `retry` (`retryAfter`/`retryLimit`) is implemented;
the reference half is not.

### F17. `retry` action `stepId`/`workflowId` reference is never executed

**Spec** (Failure Action Object, both 1.1.0 and 1.0.1): *"retry - The current
step will be retried. The retry will be constrained by the `retryAfter` and
`retryLimit` fields. If a `stepId` or `workflowId` are specified, then the
reference is executed and the context is returned, after which the current step
is retried."* The field table agrees: `stepId` is *"only relevant when the
`type` field value is `"goto"` or `"retry"`"*, and likewise `workflowId`.

**We do:** the `ActionType::Retry` arm of `execute_action`
(`crates/arazzo-runtime/src/runtime_core/engine_actions.rs:307-382`) reads only
`retry_limit` and `retry_after`, then returns
`FlowDecision::Retry(ctx.current_idx)`. It never reads `action.step_id`,
`action.workflow_id`, or `action.parameters` — a retry action carrying a
reference silently skips it and just retries. The model is not the gap:
`OnAction` carries both fields (`crates/arazzo-spec/src/lib.rs:827-829`), and
the sibling `Goto` arm resolves the same fields for its one-way transfer.

The asymmetry sharpened with ac-3a7f4: `validate_action_parameters`
(`crates/arazzo-validate/src/lib.rs:1037`) now accepts `parameters` whenever
the action declares a `workflowId` — including on a retry action — so a
retry + `workflowId` + `parameters` document validates clean while the runtime
has no reference execution to map the parameters into. That ticket's fresh
review flagged exactly this and deliberately left it open as a follow-up.

**Observed** — a validating document whose retry action references a
`recovery` workflow, run against a local server logging every request:

```yaml
onFailure:
  - type: retry
    retryAfter: 0
    retryLimit: 1
    workflowId: recovery
    parameters:
      - name: reason
        value: token-refresh
```

```
$ arazzo-cli validate retryref.arazzo.yaml
Valid Arazzo 1.1.0 spec: Retry reference probe

$ arazzo-cli run retryref.arazzo.yaml main
step probe: max retries (1) exceeded
```

Server log: `GET /missing` twice (initial attempt + one retry); the recovery
workflow's `GET /ok.txt` never appears. The reference the spec says *"is
executed"* before the retry is silently dropped — the exact failure class this
matters for is a recovery reference that would make the retry succeed (token
refresh, resource reset), which instead retries to exhaustion.

**Precedent for the fix:** call-and-return semantics already exist —
`execute_subworkflow_step`
(`crates/arazzo-runtime/src/runtime_core/engine_impl.rs:631`) executes a child
workflow, registers its state, and resumes the caller. Action-parameter input
mapping for a `workflowId` reference should reuse the
`resolve_value_source`-based construction the `Goto` arm gained in ac-3a7f4.
Ticketed as ac-9b999.

**F17 is closed.** `41e0c6b` (ac-9b999) executes the reference call-and-return
at both serial retry sites, after the retry delay and immediately before each
retried attempt: a `workflowId` reference runs at `depth + 1` with action
`parameters` (or forwarded caller inputs) as callee inputs and registers for
`$workflows.<id>.*`; a `stepId` reference runs the referenced step once,
outputs persisting, without following its routing. A failed reference aborts
the workflow with the additive `RUNTIME_RETRY_REFERENCE_FAILED` rather than
retrying anyway, and validation now extends the goto both-ids /
unknown-target rejections to retry actions.

## Addendum — 2026-08-13 (ac-bd441: `targetSelectorType`)

ac-bd441 closes **F3**: `Replacement` now models `targetSelectorType` (plain
name and Expression Type Object forms) plus the `x-` extension capture, the
runtime honors the declared type (with a pointer-producing JSONPath resolver
feeding the existing JSON Pointer applier), omitted-type routing is keyed on
the declared media type per §5.8.15.1, and one `validate_expression_type`
helper enforces the §5.8.12.1 version table at all three sites (Selector
Object, criterion type, `targetSelectorType`).

Two deviations touched by that work are recorded here as named debt rather
than silently inherited. Both are grounded in `spec/arazzo/v1.1.0.html`
§5.8.12: *"When used to specify a particular version of JSONPath or XPath,
implementations MUST apply the semantics defined in that version's
specification."*

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F18 | JSONPath version tokens accepted without differentiated semantics | P3 | expr/runtime |
| F19 | XPath engine is 1.0 while the spec's default is `xpath-31` | P3 | runtime |
| F20 | GJSON `#` forms accepted by typed JSONPath read surfaces | P2 | expr/runtime |

### F18. Undifferentiated JSONPath version semantics

**Spec** (§5.8.12): version tokens carry semantics — `rfc9535` and
`draft-goessner-dispatch-jsonpath-00` name different JSONPath dialects, and an
implementation *MUST* apply the named version's semantics.

**We do:** both tokens are accepted and routed to the same documented subset
evaluator (`select_json_path` and, since ac-bd441, the pointer-producing
`resolve_json_path_pointers` in `crates/arazzo-expr/src/lib.rs`). The subset
(root, dot/bracket fields, indexes, wildcards, simple filter predicates) is
neither full RFC 9535 nor the Goessner draft, and nothing differentiates the
two tokens. Pre-existing for criterion/selector reads; ac-bd441 added the
replacement write path as a second entry point without changing the stance.

### F19. XPath 1.0 runtime against the specification's `xpath-31` default

**Spec** (§5.8.12/§5.8.12.1): an omitted Expression Type Object means the
default version — `xpath-31` for `xpath` — and a plain `xpath` type *"MUST
conform to XML Path Language 3.1"*.

**We do:** the engine is XPath 1.0 (`uppsala`). Decision 3 of ac-bd441 keeps
documents using 3.1-defaulted forms valid (validation accepts every table
token) and has the runtime execute them under XPath 1.0 semantics with exactly
one capability diagnostic naming the gap per selector/replacement;
`version: xpath-10` executes silently. Closing this finding means shipping an
XPath 3.1 engine, which is a workspace feature, not a per-site fix.

One surface is not yet covered by the capability diagnostic: the criterion
XPath path (`crates/arazzo-runtime/src/runtime_core/criteria.rs`) executes any
declared version silently, with no version gate at all — pre-existing, and now
the only XPath surface without the Decision 3 diagnostic.

**Update 2026-08-23 (ac-46638):** Decision 3's warn-and-run stance is
superseded. Explicit `version: xpath-10` alone evaluates; the omitted form
and every other §5.8.12.1 token are rejected before evaluation on all three
surfaces — a criterion fails with an error, a selector yields null with one
warning, a replacement leaves the body unchanged with one warning. Validation
still accepts every table token as document metadata. The runtime no longer
executes 3.1-defaulted documents under wrong semantics; the finding remains
open only in the sense that an XPath 3.1 engine does not exist, which stays a
workspace feature.

### F20. GJSON forms accepted by typed JSONPath read surfaces

**Spec** (§5.8.11.4.3 and §5.8.12): a criterion with `type: jsonpath` *"MUST
be a valid JSONPath expression conforming to [RFC9535]"*, and implementations
*"MUST apply the semantics defined in that version's specification"*. Selector
Objects use the same versioned JSONPath contract. GJSON's dot-form `#` filters
are not syntax in either allowed JSONPath dialect.

**We do:** the read-side `select_json_path` path used by typed JSONPath
criteria and Selector Objects still accepts GJSON `#` forms. A single-match
`#(...)` form therefore returns the first match instead of reporting invalid
JSONPath syntax. The write-side pointer resolver now rejects these forms for
`targetSelectorType: jsonpath`; the read-side behavior remains unchanged in
that ticket so existing expression semantics are not silently moved. Follow-up
work should make typed JSONPath reads fail closed without changing the separate
GJSON runtime-expression extension.

## Addendum — 2026-08-13 (epic ac-37fe6 close)

The conformance epic is closed. Findings resolved by its final four members,
each behind an independent fresh-context review (verdicts recorded on the
tickets):

**F6 is closed, both halves.** `d6dd3ac` (ac-4a71f) enforces the MUST-level
positions as errors — workflow/step `outputs` keys and all four Components map
keys against `^[a-zA-Z0-9\.\-_]+$`, via an anchored byte-class predicate (no
`regex` dependency). `4191cac` (ac-0379b) reports the SHOULD-level positions —
`workflowId`, `stepId`, `sourceDescriptions[].name` against `[A-Za-z0-9_\-]+`
— as warnings, promoted to errors only under the single `--strict` flag.

**F7 is closed.** The `parse_bytes` raw-YAML site rejects both
`successCriteria: []` and the bare `successCriteria:` null spelling (an absent
key stays valid; the typed model cannot see the distinction). Explicit `null`
and `~` spellings remain rejected during typed parsing. The bare-null escape is
closed under ac-d0f42.

**F8 is closed.** `8461693` (ac-85c4a) retains all leftover fields at every
`x-` capture site and warns on unknown non-`x-` keys (kind `unknownField`,
promoted only under `--strict`); serialization still emits `x-` keys only.
Two model completions landed with it so conformant documents do not warn:
workflow-level `dependsOn` (parse/serialize only; semantics ticketed as
ac-51755) and the inline `reference`/`value` fields at the four
reusable-capable action positions (full Reusable Object modeling completed by
ac-6131b).

**F4 is closed.** `2f10fe8` (ac-91284) makes `generate` emit a
document-pointing relative `sourceDescriptions[].url` (URI-reference rebased
against the output directory, `./`-guarded when the first segment carries a
colon); the MCP generate tool uses the document file name. Generated documents
now declare `arazzo: 1.1.0`. The runtime's url-as-base-URL reading of absolute
urls survives as a fallback only: since `f0adfeb` an absolute url binds by
document identity when a provided document answers to it (§9.6), and that
compatibility rule remains part of the F1/F5 decision. Network fetching of a
`url` was answered rather than deferred — GitHub issue #4 is closed as completed,
identity-based referencing being the answer.

**F9, F10, and F13 are accepted-as-extension and labeled.** `bc2ae04`
(ac-fb9dc) adds README's "Specification Conformance: Extensions and Gaps"
section: `$env.*`, bare XPath outputs, the `operationPath` extension forms,
url-as-base-URL, name-based `$components` action resolution, and the GJSON
dot-path traversal all carry explicit **arazzo-cli extension** labels, with
the Selector Object documented as the conformant preferred form.

**F11 stays open, now documented.** `$response.query.<name>` /
`$response.path.<name>` remain unimplemented (evaluate to `null`) and are
listed as such in README's not-implemented section, alongside `$message.*`
(evaluator-only context, never populated by the runtime).

The probe document at F8 above now behaves per the epic's acceptance criteria:
its MUST-level violations fail validation, its SHOULD-level violations warn
(errors under `--strict`), and the unknown field warns. Still open after this
epic: F1/F5 (the operationPath / sourceDescriptions idiom product decision),
F12, F14, F15, F16, F18, F19, and F11's implementation.

## Addendum — 2026-08-15 (ac-6131b Reusable Object action references)

**The action Reusable Object gap is closed.** `OnAction` now models the
specification's `reference` and optional `value` fields. At workflow
`successActions`/`failureActions` and step `onSuccess`/`onFailure`, a reference
must use the matching `$components.successActions.<name>` or
`$components.failureActions.<name>` namespace and resolves by wholesale
replacement. The resolved action clears the reusable fields; action `value`
has no effect because the specification limits it to parameter references.
Missing component maps and targets now fail through `componentResolution`
instead of silently ending. The existing name-based component idiom remains an
explicit arazzo-cli extension for compatibility and is used only when
`reference` is absent.

## Addendum — 2026-08-15 (ac-51755 workflow dependencies)

Workflow-level `dependsOn` now follows the Arazzo 1.1.0 and 1.0.1 Workflow
Object rule: local references are checked against case-sensitive workflow IDs,
and local cycles are rejected as `dependencyCycle` errors. The shared
`arazzo-spec` classifier accepts only local IDs or the exact
`$sourceDescriptions.<name>.<workflowId>` form. Unknown sources and
non-Arazzo sources are `invalidReference` errors; known Arazzo sources produce
an unsupported-scope warning that global `--strict` promotes.

Runtime execution is fail-closed. Every direct, step, replay, debugger, MCP,
goto, retry-reference, and sub-workflow entry reaches one guard before HTTP.
The guard requires local completion evidence in the current invocation and
rejects external dependencies because this runtime does not load external
Arazzo documents. The stable error code is
`RUNTIME_WORKFLOW_DEPENDENCY_UNSATISFIED`. Completion evidence is an explicit,
per-invocation set; the Engine does not retain completion history.

The `test` command computes a stable document-order-preserving topological
order, rejects filtered selections that omit local transitive prerequisites,
rejects external dependencies before execution, and records failed cases as
completed when `--fail-fast` is disabled. No workflow is auto-added or
auto-run by a filter.

## Addendum — 2026-08-15 (ac-a10c2 required collections)

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F21 | Empty or absent `sourceDescriptions`/`workflows` accepted; absent workflow `steps` accepted (`steps: []` remains valid) | P2 | validate |

### F21. Required source/workflow collections and absent workflow steps were accepted

**Spec:** §4.1 requires an Arazzo Description to contain a
`sourceDescriptions` field with at least one Source Description and at least
one Workflow in `workflows`. The §5.8.1.1 fixed-field rows repeat that each
list MUST have at least one entry. §5.8.4.1 marks the Workflow Object's
`steps` field REQUIRED, but does not say that the list must contain at least
one entry.

**We did:** the typed `Vec` fields used `#[serde(default)]`, so both absent
and empty `sourceDescriptions`/`workflows` lists reached validation as empty
vectors without diagnostics. The same collapse made an absent workflow
`steps` key indistinguishable from `steps: []`; an absent key was accepted,
while the explicit empty list is specification-valid and remains accepted.
This also allowed the CLI `test` command to treat a zero-workflow document as
an empty successful suite.

**Closure:** `ac-a10c2` adds `MissingRequiredField` errors for empty or absent
`sourceDescriptions` and `workflows`, and extends the existing raw-YAML
validation walk to reject an absent `steps` key. `steps: []` remains valid,
because the specification requires the field but does not impose an
at-least-one constraint. Typed `null` values remain parse errors. The shared
parse boundary applies the diagnostics to CLI, test-runner, MCP, DAP, and
runtime consumers without surface-specific changes.

## Addendum — 2026-08-23 (criterion null-context handling)

One further deviation plus a sibling in the same clause, found while
re-examining the typed criterion arms after `301174a` (`fix: decide xpath
criteria by effective boolean value`). Grounded in the vendored
`spec/arazzo/v1.1.0.html` §5.8.11.4.2–§5.8.11.4.4 and reproduced against the
CLI at `301174a` (local server serving
`<root><pets><pet>dog</pet></pets></root>` as `pets.xml`).

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F22 | XPath criterion with null context falls back to the raw response body | P2 | runtime |
| F23 | Regex criterion null context coerced to `""`; empty-matching patterns pass | P2 | runtime |

### F22. XPath criterion null context falls back to the raw response body

**Spec** (§5.8.11.4.4 XPath Conditions): *"If the `context` evaluates to
`null` or `undefined`, or if the XPath expression is syntactically invalid,
the condition MUST evaluate to *fail*."* The JSONPath section carries the
identical rule (§5.8.11.4.3): *"If the `context` evaluates to `null` or
`undefined`, or if the JSONPath expression is syntactically invalid, the
condition MUST evaluate to fail."*

**We did:** the xpath arm of `evaluate_criterion_detailed`
(`crates/arazzo-runtime/src/runtime_core/criteria.rs`) treated a
`Value::Null` context as a signal to evaluate the XPath against the raw
response body (`String::from_utf8_lossy(&resp.body)`), so a criterion whose
explicitly provided `context` resolved to nothing could still pass. The
jsonpath arm already failed on a null context. The fallback predates the
module split (`3cb8a48`) and predates `eval_context` exposing non-JSON bodies
to `$response.body` as raw text
(`crates/arazzo-runtime/src/runtime_core/state.rs:149-155`) — once that
landed, the fallback was load-bearing only for the exact case the
specification says must fail.

**Observed** at `301174a` — an xpath criterion with
`context: $response.body.missing` (a dot path into a string body, resolving
to null) and `condition: count(//pet) > 0`, which the raw body satisfies:

```
$ arazzo-cli run nullctx.arazzo.yaml probe
Workflow completed (no outputs)
$ echo $?
0
```

The criterion passed on a null context. After the fix, the same document and
server:

```
$ arazzo-cli run nullctx.arazzo.yaml probe
step fetch: success criteria not met (status=200, body=<root><pets><pet>dog</pet></pets></root>)
$ echo $?
1
```

**Scope note — default context.** The empty-context lenience path
(`default_criterion_context`) is a distinct, deliberate code path and is not
governed by the quoted sentence: the sentence constrains the provided
`context` expression, and an xpath/jsonpath/regex criterion without `context`
is already non-conforming input (*"and `context` MUST be provided"*, all
three sections). The null check is nonetheless applied uniformly — matching
the jsonpath arm, whose null check has always covered both paths — because a
null default context implies an absent response, an empty body, or a literal
JSON `null` body, for which the old fallback produced `""` or `"null"` and
failed XML parsing anyway. No default-path pass/fail outcome changes.

**F22 is closed** by the change this addendum ships with: the xpath arm now
mirrors the jsonpath arm — a null context fails the criterion with no body
fallback, and `context_value` stays `null` in trace/debug output instead of
being rewritten to the fallback text. The negative test
`xpath_null_context_fails_even_when_raw_body_would_match` pins the
spec-surface case (valid matching XML body, null context, must fail);
`jsonpath_null_context_fails` pins the model arm. One pre-existing test
(`evaluate_criterion_xpath_uses_context_and_condition`) had accidentally
pinned the fallback by building an `EvalContext` without `response_body` —
corrected to mirror engine wiring, which always populates it from the
response.

**Recorded consequence — non-UTF-8 bodies now fail closed.** `eval_context`
decodes the raw body **strictly** (`String::from_utf8(...).ok()`,
`state.rs:152-155`), so a body that is not valid UTF-8 — e.g. ISO-8859-1 XML
with accented characters — never reaches `$response.body`, and an explicit
`context: $response.body` xpath criterion over such a response resolves to
null and fails. The removed fallback used to lossy-decode those bytes, so a
structural condition (`count(//pet)`) could previously pass with `U+FFFD`
corruption in text content. Fail-closed is the deliberate stance (the fresh
review flagged the silent change; it is now pinned by
`xpath_explicit_context_over_non_utf8_body_fails_closed`). The open
follow-up is a product decision, not a patch: whether `$response.body`
should become charset-aware (honor the XML declaration / `Content-Type`
charset) or lossy for undecodable bodies — lossy would extend degraded-mode
decoding to every expression position, which is barred without explicit
approval.

### F23. Regex criterion null context coerced to `""`

**Spec** (§5.8.11.4.2 Regex Conditions): *"If the `context` evaluates to
`null` or `undefined`, the condition MUST evaluate to *fail*."*

**We do:** the regex arm stringifies the context through `value_to_string`
(`crates/arazzo-runtime/src/runtime_core/payload.rs:5-13`), which maps
`Value::Null` to the empty string, then matches the pattern against `""`. Any
empty-matching pattern — `^$`, `.*`, `\d*` — therefore passes on a null
context.

**Observed** — the same probe with `type: regex`, `condition: '^$'`, and
`context: $response.body.missing`, run after F22 was closed (so this is the
regex arm alone):

```
$ arazzo-cli run nullctx-regex.arazzo.yaml probe
Workflow completed (no outputs)
$ echo $?
0
```

Recorded rather than fixed: the closure is the same one-guard shape as F22 (a
null check ahead of the stringification), left as a follow-up to keep the F22
change atomic.

## Addendum — 2026-09-13 (ac-58896 JSONPath criterion cardinality)

One deviation in the sibling of the clause F22 covered, found while auditing
the XPath criterion arm after `301174a` (`fix: decide xpath criteria by
effective boolean value`). Same defect class — a typed result collapsed to a
JSON value before the truth decision — surviving in the JSONPath arm that
change did not touch. Grounded in the vendored `spec/arazzo/v1.1.0.html`
§5.8.11.4.3.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F24 | JSONPath criterion decided by value truthiness, not nodelist cardinality | P1 | runtime |

### F24. JSONPath criterion decided by value truthiness

**Spec** (§5.8.11.4.3 JSONPath Conditions): *"JSONPath expressions return a
NodesType (a nodelist, which is a sequence of zero or more nodes). The
condition evaluates to:"* — *"A condition passes (truthy) when the JSONPath
expression returns a non-empty nodelist (one or more nodes)."* / *"A condition
fails (falsy) when the JSONPath expression returns an empty nodelist (zero
nodes)."* The section inspects no values anywhere. That is deliberate: the
XPath section one clause later (§5.8.11.4.4) does carry a four-way type truth
table and delegates to Effective Boolean Value. JSONPath has no EBV step.

**We did:** the bare-path arm of `evaluate_jsonpath_condition`
(`crates/arazzo-runtime/src/runtime_core/jsonpath.rs`) discarded the
cardinality it was handed and asked the collapsed value whether it was truthy:

```rust
Ok(selection) => JsonPathOutcome::Matched(is_truthy(&selection.value)),
```

`arazzo_expr::select_json_path` returns both halves — `JsonPathSelection`
carries `match_count` ("Number of nodes selected before cardinality collapse",
computed by `count_resolved_path_nodes`) alongside the collapsed `value`. The
correct signal was present at the call site and unused.

The two diverge because the `PathToken::Field` arm of `apply_path_token`
(`crates/arazzo-expr/src/lib.rs`) pushes a node on key *presence*
(`obj.get(name)`), independent of what the key holds. So a one-node nodelist
whose node holds `false`, `0`, `""`, or `null` was reported as a failed
criterion. Those four are the whole divergence: every other JSON value is
already truthy and already passed, which is why this went unnoticed. The
unsupported-syntax arm, the filter-predicate arm, and the null-context guard
in `criteria.rs` were correct and are unchanged.

**Observed** — a step whose only criterion is
`{context: $response.body, condition: $.data.active, type: jsonpath}` against a
body of `{"data":{"active":false,"count":0,"name":"","missing":null}}`, driven
through the engine against a local `tiny_http` server. The specification says
the criterion passes: the nodelist holds one node. Against the pre-fix arm the
workflow failed with

```
step active: success criteria not met (status=200, body={"data":{"active":false,"count":0,"missing":null,"name":""}})
```

and the same failure appeared for `$.data.count`, `$.data.name`, and
`$.data.missing`. After the fix all four steps succeed. That regression is
pinned by `single_node_nodelist_passes_for_each_falsy_value` and
`each_falsy_value_passes_in_isolation` in
`crates/arazzo-runtime/tests/jsonpath_semantics.rs`, both verified to fail
against the pre-fix arm and pass against the fixed one.

**F24 is closed** by the change this addendum ships with: the bare-path arm
decides on `selection.match_count > 0`. `is_truthy` is unchanged and remains
correct for the `simple` and `regex` arms and for the values inside JSONPath
filter predicates; only this criterion call site stopped consulting it.

**Scope note.** The negative half of the rule is pinned separately
(`empty_nodelist_fails_the_step`), as is the null-context guard
(`null_context_fails_without_evaluating`, using the root selector `$` so the
failure can only come from the guard). The diagnostic naming an unsupported
construct stays pinned at unit level; surfacing that text through the step
failure rather than a generic "success criteria not met" is a separate
deviation, not this one. Nothing here narrows or widens the supported JSONPath
subset — F18 (undifferentiated JSONPath version semantics) and F20 (GJSON
forms accepted by typed JSONPath read surfaces) are untouched.

## Addendum — 2026-09-13 (ac-a983f: F18 and F20 under the RFC 9535 engine)

The accepted JSONPath migration
([plans/current/arazzo-jsonpath-rfc9535-migration.md](../current/arazzo-jsonpath-rfc9535-migration.md),
epic [ac-edd23](https://sonos.scapedeck.com/docs/ac-tickets/ac-edd23)) replaced
both handwritten typed-JSONPath implementations with one library-backed
RFC 9535 owner in `crates/arazzo-expr/src/jsonpath.rs`. Every typed JSONPath
entry point — `type: jsonpath` criteria, JSONPath Selector Objects, and
`targetSelectorType: jsonpath` replacement targets — now admits a declared
version, applies the byte and structural budgets, and parses the complete
expression through that owner *before* a context is resolved or a predicate
runs. This addendum records the disposition of the two JSONPath findings that
survived the earlier conformance epic.

**F20 is closed.** GJSON's `#` forms are not RFC 9535 syntax, so the shared
parser rejects them where the old subset evaluator resolved them. The five
forms the debt named — unquoted terminal `#`, `.#.`, `#(…)`, `#(…)#` and the
bracket-wrapped `[#(…)]` — now fail at every typed surface: a criterion is
false with a detailed error plus exactly one warning on both the criterion
record and the enclosing step; a Selector Object reads as `null` with one
diagnostic; a replacement target or a replacement value leaves that
replacement unapplied and the body untouched, while the replacements around it
still apply. Because admission and parsing precede context resolution, neither
a context that resolves to nothing nor a satisfied left-hand `||` operand can
conceal the syntax, and a GJSON path compared with `null` is rejected rather
than evaluated as an absent-value comparison.

The rejection is syntax-specific and narrows nothing: a literal `#` member
name (`$['#']`), a `#`-bearing string literal inside a filter, and the
root/wildcard/index/slice/descent/compound/`count()` queries the GJSON forms
resemble all keep their zero/one/many results, with a selected `null` (one
match, no warning) still distinguishable from a zero-match normalization (one
no-match warning) and from a query error. The separate GJSON dot-path
extension on Arazzo *runtime expressions* (`$response.body.items.#(sku=="A")`)
is a different surface and is unchanged.

Engine-level proof is
`crates/arazzo-runtime/tests/jsonpath_gjson_rejection.rs`; operator-visible
proof — dry-run warnings with an unchanged resolved body, and a live
`tiny_http` run whose `--json` error and `--trace` criterion record both carry
the diagnostic — is `crates/arazzo-cli/tests/cli_jsonpath_gjson.rs`. The
`expr.typed-jsonpath-rejects-gjson` conformance claim is narrowed to RFC 9535
and marked covered against those tests. The
[ac-cf3e2](https://sonos.scapedeck.com/docs/ac-tickets/ac-cf3e2) write-path
regression baseline still holds: the unchanged-body assertions are made on the
captured wire request, not only on a warning.

**F18 is closed, and is recorded here only.** It was closed by `974bacb`
([ac-43177](https://sonos.scapedeck.com/docs/ac-tickets/ac-43177)), which
routed JSONPath criteria through the shared owner, and `a0f7170`
([ac-7a0ce](https://sonos.scapedeck.com/docs/ac-tickets/ac-7a0ce)), which cut
selectors and replacements over to it. The two version tokens are no longer
undifferentiated: an omitted version and an explicit `rfc9535` are one engine,
and `draft-goessner-dispatch-jsonpath-00` is refused by name before the
expression is examined. No conformance-manifest row, status or inventory entry
changes for F18; this entry is its record.

**Permanent capability limit (user-approved, no owning ticket).** Rejecting
`draft-goessner-dispatch-jsonpath-00` is a deliberate product decision — *"Goessner
is an expired draft and we will not support it period"* — not an implementation
gap awaiting a ticket. §5.8.12 says that when a particular JSONPath version is
specified, implementations *"MUST apply the semantics defined in that version's
specification"*, and §5.8.12.1 lists that token as allowed. This runtime will
never apply those semantics, so that version-semantics MUST is permanently
unmet and no future work will close it. Structural validation still accepts the
token as document metadata, so documents declaring it remain valid; runtime
capability and document validity stay distinct. This paragraph, rather than a
conformance-manifest row, is the record of that unmet MUST: the manifest tracks
claims that have or await an owner, and this one has neither.

## Addendum — 2026-09-23 (ac-13ecf operation servers)

One deviation, found in a live workflow: an operation declared its own server
and was sent to its document's `servers[0]` instead. Grounded in the vendored
`spec/arazzo/v1.1.0.html` §5.7 and `spec/oas/v3.2.0.html` §4.1.1, §4.5, §4.9.1
and §4.10.1.

| # | Deviation | Severity | Surface |
|---|---|---|---|
| F25 | Path Item and Operation Object `servers` ignored; every operation sent to the document's `servers[0]` | P1 | runtime |

### F25. Path Item and Operation `servers` ignored

**Spec** (§5.7 Relative References in API URLs): *"When Step Objects reference
API operations via operationId or operationPath, the actual API endpoint URL is
determined by the OpenAPI description’s Server Object, not by the Arazzo
Description’s base URI."* OpenAPI 3.2 declares Server Objects at three levels.
The Path Item Object's `servers` (§4.9.1): *"An alternative servers array to
service all operations in this path. If a servers array is specified at the
OpenAPI Object level, it will be overridden by this value."* The Operation
Object's (§4.10.1): *"An alternative servers array to service this operation.
If a servers array is specified at the Path Item Object or OpenAPI Object
level, it will be overridden by this value."*

**We did:** `index_operations`
(`crates/arazzo-runtime/src/runtime_core/builder.rs`) recorded only an
operation's method, path and origin, and `derive_servers_base` derived one
request base per source from the OpenAPI Object's `servers[0].url`.
`resolve_in_source` and `resolve_bare` (`engine_http.rs`) sent every operation
of a source to that base, so no Path Item or Operation Object `servers` was
ever read.

**Observed** — one source whose document declares `https://doc.example.test`
on the OpenAPI Object, `https://path.example.test/{ver}` (default `v9`) on the
`/path-level` path item, and `https://op.example.test` on the `/op-level`
operation, whose path item declares `https://path.example.test`.
`arazzo-cli run --dry-run --json` at `38b6088` planned

```
https://doc.example.test/doc-level
https://doc.example.test/path-level
https://doc.example.test/op-level
```

and a live run sends where it plans; the live workflow got a 403 from the
document's host for an operation served elsewhere. After the fix the plan is

```
https://doc.example.test/doc-level
https://path.example.test/v9/path-level
https://op.example.test/op-level
```

**F25 is closed for `operationId` targets** by the change this addendum ships
with. `operation_server` (`builder.rs`) is the one place the precedence lives:
the Operation Object's `servers`, else the Path Item Object's, else the
document's. Each operation's result is recorded on its `OperationEntry` when a
source-bound document is indexed, and both resolvers apply it before the
source's document base. Every level reads its `servers` through one rule,
`read_servers`: the first Server Object's `url`, its variables' defaults
substituted, a trailing `/` trimmed. An explicitly provided spec belongs to no
source description — an `operationId` names an operation *"existing within one
of the sourceDescriptions"* — so it keeps the engine-wide base and its
`servers` are read at no level. Engine proof, live and dry-run with bare and
source-qualified targets, is `crates/arazzo-runtime/tests/openapi_servers.rs`;
the operator-visible plan and `--json` refusal code are pinned in
`crates/arazzo-cli/tests/cli_operation_id_routing.rs`.

**Interpretations, recorded as such.** Three choices the specification leaves
open:

- *An empty `servers: []` below the root declares nothing*, so the level above
  applies. §4.1.1 gives an absent and an empty array the same default only on
  the OpenAPI Object and is silent below it; OAI/OpenAPI-Specification#3427
  asked exactly this and was closed without an answer. The reading matches
  swagger-client (`isNonEmptyServerList`) with ApiDOM's `servers`
  normalization, and openapi-generator; Redocly Respect falls back to the
  document.
- *A relative server url is refused, as a runtime limit rather than a
  deviation.* A server url *"MAY be relative, to indicate that the host
  location is relative to the location where the document containing the
  Server Object is being served"* (§4.5.1), and *"For API URLs the $self
  field, which identifies the OpenAPI document, is ignored and the retrieval
  URI is used instead"* (§4.5.2.1). A document this runtime reads from disk has
  no HTTP retrieval location, so no request base follows from it. "Relative"
  now means an RFC 3986 relative reference at every level — the test
  `DocumentSet::bind` already applied to a Source Description url — rather
  than a leading `/`. At the document level that widens the existing
  build-time refusal to `.`, `./x` and scheme-less urls, which previously
  built, dry-ran to a URL with no scheme, and failed only when sent
  (`RUNTIME_HTTP_REQUEST`).
- *A declared level never falls back.* A path-item or operation `servers` that
  yields no usable server — a relative url, a value that is not an array
  (`null` included), or a Server Object without its REQUIRED `url` — refuses
  that operation when a step resolves it (`RUNTIME_SOURCE_DESCRIPTION_PARSE`),
  before anything is sent. The engine still builds, the document's other
  operations still route, and the message says whose limit it is.

**Remaining debt.**

- §5.7 covers `operationPath` too. The specification's JSON-Pointer form is
  unresolved (F1); when it is, its resolver must call `operation_server`.
  Overrides are recorded on operationId-keyed entries, operations without an
  `operationId` are not indexed, and bound document bytes are dropped after
  build, so they will not arrive through the index. The `{name}./path`
  extension keeps the source's document base by design.
- A document whose OpenAPI Object declares no absolute server still fails the
  build even when every operation declares its own. Such a document is
  conformant; Respect and swagger-client accept it.
- Three edges the new levels inherit from the document level: a non-string
  variable `default` leaves `{var}` in the url; `replace_path_params`
  substitutes over the whole target URL, so a same-named `in: path` parameter
  can fill a `{var}` left in a server url; and operations beside a path-item
  `$ref` ignore the referenced item's `servers`, because the indexer does not
  follow that `$ref`.
