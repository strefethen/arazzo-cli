# Arazzo Specification Conformance Audit

**Date:** 2026-08-03
**Scope:** whole workspace — `arazzo-spec`, `arazzo-validate`, `arazzo-expr`, `arazzo-runtime`, `arazzo-generate`, `arazzo-cli` (`arazzo-mcp` and `arazzo-debug-adapter` inherit runtime behavior).
**Grounding:** normative field definitions taken from the published Arazzo Specification v1.1.0 (spec.openapis.org/arazzo/latest.html), not from memory. Every finding below was reproduced against the built CLI; commands and observed output are recorded per finding.

## Summary

Eleven deviations in the original pass, F1–F11 below. Two cause a conformant
document to fail outright or silently misbehave; the rest are unenforced
constraints, ignored fields, non-conformant output, and undocumented
extensions. Later passes added F12–F17 — see the addenda at the end, which also
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
the runtime side is the compatibility rule protecting existing files and is
tracked in GitHub issue #4.

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

**F7 is closed.** `d6dd3ac` rejects `successCriteria: []` at the `parse_bytes`
raw-YAML site (an absent key stays valid; the typed model cannot see the
distinction). Known escape: `successCriteria:` with a null value still passes
— ticketed as ac-d0f42.

**F8 is closed.** `8461693` (ac-85c4a) retains all leftover fields at every
`x-` capture site and warns on unknown non-`x-` keys (kind `unknownField`,
promoted only under `--strict`); serialization still emits `x-` keys only.
Two model completions landed with it so conformant documents do not warn:
workflow-level `dependsOn` (parse/serialize only; semantics ticketed as
ac-51755) and `reference`/`value` allowed at the four reusable-capable action
positions (full Reusable Object modeling ticketed as ac-6131b).

**F4 is closed.** `2f10fe8` (ac-91284) makes `generate` emit a
document-pointing relative `sourceDescriptions[].url` (URI-reference rebased
against the output directory, `./`-guarded when the first segment carries a
colon); the MCP generate tool uses the document file name. Generated documents
now declare `arazzo: 1.1.0`. The runtime's url-as-base-URL reading of absolute
urls is unchanged — that compatibility rule remains part of the F1/F5 decision
and GitHub issue #4.

**F9, F10, and F13 are accepted-as-extension and labeled.** `bc2ae04`
(ac-fb9dc) adds README's "Specification Conformance: Extensions and Gaps"
section: `$env.*`, bare XPath outputs, the `operationPath` extension forms,
url-as-base-URL, name-based `$components` action resolution, and the GJSON
dot-path traversal all carry explicit **arazzo-cli extension** labels, with
the Selector Object documented as the conformant preferred form.

**F11 stays open, now documented.** `$response.query.<name>` /
`$response.path.<name>` remain unimplemented (evaluate to `null`) and are
listed as such in README's not-implemented section, alongside `$message.*`
(evaluator-only context, never populated by the runtime) and the silently
ignored Reusable Object `reference` form on actions (ac-6131b).

The probe document at F8 above now behaves per the epic's acceptance criteria:
its MUST-level violations fail validation, its SHOULD-level violations warn
(errors under `--strict`), and the unknown field warns. Still open after this
epic: F1/F5 (the operationPath / sourceDescriptions idiom product decision),
F12, F14, F15, F16, F18, F19, and F11's implementation.
