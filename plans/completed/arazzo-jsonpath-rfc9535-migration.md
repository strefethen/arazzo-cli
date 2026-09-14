---
status: completed
epic: epic:ac-edd23
created: 2026-09-12
completed: 2026-09-14
evidence: epic:ac-edd23 closing note (2026-09-14 02:32 UTC) — whole-system verification PASS at f467c32; all four members closed, gates green, four drift guards and CLI JSONPath proof passing
---

# Replace handwritten JSONPath with one RFC 9535 engine

**Seed:** Steve: “we need to replace both hand written paths with this”; “we don't
need to go stupid overboard with a million tests for a tiny grammar”; “create an
epic and tier 2 tickets for the migration”. Subsequent decisions: Rust 1.88,
“Use explicit, bounded resource limits”, and “Goessner is and expired draft and
we will not support it period.” Root orchestrates; Sol xhigh implements settled
tickets; fresh Astra reviews candidates.

## Outcome and non-goals

One library-backed JSONPath owner serves typed Selector Objects, replacement
targets, and JSONPath criteria. Both handwritten typed-JSONPath implementations
are retired. RFC 9535 is the sole supported dialect. An explicit Goessner version
produces an unsupported-version diagnostic; there is no compatibility engine,
silent alias, fallback, or future Goessner implementation.
Structural validation still recognizes the version token that Arazzo permits;
runtime capability and document validity remain distinct.

This does not replace Arazzo runtime-expression parsing, its existing legacy
GJSON behavior, simple-condition parsing/equality, ordinary `type: regex`
criteria, XPath, or typed-criterion interpolation. It does not publish a crate,
create another repository, wait for an upstream release, or claim support for
every Arazzo dialect. Broad criterion trace projection retains its existing owner.

## User-visible workflows and authority

A workflow using default JSONPath or explicit `rfc9535` receives the same engine
in all three consumers. Filters, slices, unions, descent, and standard functions
follow RFC semantics instead of the previous subset. Existing subset behavior
that conflicts with RFC 9535 is corrected deliberately.

The vendored [JSONPath conditions](../../spec/arazzo/v1.1.0.html#jsonpath-conditions)
say: “A condition passes (truthy) when the JSONPath expression returns a non-empty
nodelist (one or more nodes).” Selecting false, zero, an empty string, or null
therefore passes; no selected nodes fail. The same section says: “If the context
evaluates to null or undefined, or if the JSONPath expression is syntactically
invalid, the condition MUST evaluate to fail.” A null context is different from
a selected null node within a non-null document.

[Expression Type Object](../../spec/arazzo/v1.1.0.html#expression-type-object)
says: “When used to specify a particular version of JSONPath or XPath,
implementations MUST apply the semantics defined in that version’s
specification.” [Selector Object](../../spec/arazzo/v1.1.0.html#selector-object)
and [Payload Replacement Object](../../spec/arazzo/v1.1.0.html#payload-replacement-object)
establish RFC 9535 for plain JSONPath. Rejecting Goessner is an explicit product
capability limit, not a claim that Arazzo disallows that version.

Selector reads retain zero/one/many normalization to null/scalar/ordered array.
Zero matches produce the existing warning; a selected null is one match.
Replacement targets retain the settled project rule: exactly one location is
applied through the existing JSON Pointer mutator; zero or multiple occurrences
warn and leave the current body unchanged. Duplicate occurrences are not deduped.
Root replacement and sequential replacement order remain supported.
The existing mutator must preserve empty-name pointer segments: removing all
leading slashes currently turns `//child` into `/child` and can overwrite a
sibling. Include the minimal separator-handling repair in this cutover.

A hard JSONPath error in a replacement value, including a nested Selector
Object, leaves that replacement unapplied and reports a warning. Legitimate
selected null and existing zero-match normalization remain distinct from hard
failure. Earlier successful replacements remain applied; later replacements
continue in order. This retains the current warning-based request policy.
Context-resolution failure for a JSONPath Selector also counts as failed
replacement-value resolution; unrelated non-JSONPath warning behavior is unchanged.

Version admission, query limits, and complete syntax validation precede context
resolution or null-context handling on typed paths. Missing data and boolean
short-circuiting cannot conceal invalid GJSON syntax. Quoted `#` remains legal.

## Decision-relevant evidence

Current source at `6ee8cc77df8b55fa507f165384354cf84d7d4723` contains separate
subset/traced walks in `arazzo-expr` and a predicate evaluator in runtime.
Runtime already depends on expr. Several expr traversal/comparison helpers also
serve legacy expressions and simple conditions; removal must follow actual use.

The frozen [qualification report](../../../../.codex/visualizations/2026/09/12/01a09766-f3e8-7df1-9ee1-ef8a7478eda2/iregexp-serde-integration/README.md)
qualifies `serde_json_path` 0.7.2, `serde_json_path_core` 0.2.2 with an eight-line
recursive numeric-equality repair, and the generated-grammar I-Regexp crate.
It records 30 focused tests, 768 match/search combinations, and raw CTS 702/704.
The two retained CTS disagreements treat caret/dollar as anchors contrary to
RFC 9485. Fourteen normalized-path Display defects require typed locations'
`to_json_pointer()` instead. These are recorded limits, not waived tests or a
claim of 100% CTS compliance.

Rust 1.88 is already implemented and reviewed. I-Regexp's source was qualified
at `8bb2186a14f23029f7ec62c8d766d825dc343480`; the public repository's
`e22a35b0d2c25f942f1e4e375525be10d342b7f6` changes README/license metadata only.
The dependency ticket verifies this source identity and pins that immutable
public revision. It does not depend on moving `main` or an unpublished registry
version. The standalone crate keeps its own Rust 1.85 minimum.

## Architecture

**Owner and dependency direction.** Runtime consumes the published expr API;
expr owns a cohesive `jsonpath` module and a private regex callback adapter;
those depend on serde JSONPath and I-Regexp. There is no new workspace crate.
The upstream parsed query, node types, registration, and callback state remain
private. The expr root gains re-exports only and loses its typed-JSONPath walks;
shared legacy helpers and their independently owned tests remain intact.

**Published boundary.** A compiled `JsonPathQuery` validates an expression and
optional version before data is available. Its synchronous query operation
returns ordered matches pairing a borrowed JSON value with its owned RFC 6901
pointer. One located upstream query supplies both; no independent value/count/
location walks or Display parsing. Existing `JsonPathSelection` shape and
selection/pointer convenience names remain, delegating to this owner. Their
error type becomes a dedicated `JsonPathError`, distinguishing invalid syntax,
unsupported version, resource limit, and evaluation failure. The legacy
`PathError` contract is unchanged for non-JSONPath users.

**Dependency maintenance.** Pin `serde_json_path = 0.7.2` with default features
disabled and `functions` enabled. Vendor only the qualified core 0.2.2 repair
under the workspace, retaining upstream licenses and provenance. The precise
patch and checksum are recorded in the vendor README; no broader fork or parser
changes. Remove the override only when an upstream release passes the existing
nested numeric-equality checks. Do not enable global arbitrary precision or
qualification-only competing backend features. All-features builds must work.

**Regex semantics and operational errors.** Registered `match()` and `search()`
delegate to I-Regexp Full and Search modes. Invalid pattern syntax/semantics and
non-string arguments yield logical false. Resource/backend failures invalidate
the entire query, including under negation. Because upstream callbacks return a
boolean, a private scoped thread-local frame records the first operational error
around each synchronous query. An RAII guard restores the previous frame on
every exit, including unwind; nested calls and concurrent threads are isolated.
No frame spans an await and no consumer can query upstream directly. Callback
error collection stores one error, not a growing list. No custom regex parsing,
translation, or pattern rewriting is introduced.

**Admission limits.** Before upstream parsing, allow at most 16,384 UTF-8 query
bytes and 128 occurrences of the raw bytes `.`, `[`, `(`, `!`, `&`, and `|`
combined. This conservative byte count includes quoted/escaped literals; it is
an explicit resource budget, not a lexer or a grammar check. Before evaluation,
iteratively reject context nesting beyond 128 container levels. I-Regexp retains
its existing pattern, repetition, nesting, and compiled-matcher limits. Errors
identify the resource and limit. These limits address known recursion failures;
they do not promise a universal CPU, result-size, or heap bound for every query.
Changing them requires focused boundary evidence, not a new stress-test program.

**Consumer failure projection.** Criteria use raw nodelist cardinality; any
hard error yields false plus a detailed error and existing ExpressionWarning.
This makes HTTP criterion errors visible through current nested/enclosing trace
warnings and debugger detail without new schema fields or RUNTIME codes. Action
criteria and sub-workflow criteria share evaluation semantics; wider ordinary
action-warning projection remains with the existing diagnostics ticket. Payload
resolution carries private hard-failure provenance through nested values so
replacement values cannot turn operational failures into null writes. Ordinary
read callers retain their existing value/warnings public shape.

## Required order and existing ownership

Deliver the dependency/query boundary, then criterion cutover, then selector and
replacement cutover, then integrated proof. The foundation has focused native
tests; it is an additive dependency/API boundary, not a parked extraction.
Criterion cutover precedes facade replacement so the old predicate evaluator
cannot erase the new engine's operational errors during an intermediate state.
No runtime engine-selection flag or fallback is introduced.

Reuse the current cardinality correction as a prerequisite to criterion cutover;
its intentionally narrow old-subset expectations are replaced by the later RFC
cutover. Reconcile the two existing JSONPath decision tickets to this plan. Reuse
the existing F20/GJSON ticket for post-cutover runtime/CLI evidence, replacing
its obsolete proposed handwritten guard. Keep its conformance-epic ownership.
The final migration member depends on that evidence, so completion cannot bypass
it. Serialize the separate simple-extraction ticket after the last expr-root
edit, retaining its frozen helper/test contracts. Typed interpolation, simple
grammar work, and general diagnostics remain separately owned.

## Verification and operational risks

Reuse the frozen qualification fixtures selectively and test the production
composition, not a second qualification harness. Focused cases cover paired
values/locations, RFC features that replace old subset restrictions, recursive
numeric equality, both regex functions with literal/dynamic patterns, callback
failure/isolation, and admission boundaries. Retain the known large-parenthesis
reproducer in a bounded child process to prove rejection without risking the
test runner. No additional I-Regexp grammar corpus is required.

Consumer tests cover criteria across HTTP/actions/sub-workflows; selectors in
parameters, nested payloads and outputs; replacement root/special-key pointers,
zero/many/duplicate locations, empty-name segments with an untouched sibling,
order, and unchanged body on hard value/target
errors. Use captured HTTP bodies and existing dry-run output to prove mutation
behavior. Run the required workspace checks against the final composition and
Rust 1.88; refresh drift baselines only for understood output changes.

The material maintenance burden is the small documented core patch. The material
behavior changes are RFC corrections, explicit Goessner rejection, and documented
resource limits. The library has eager result materialization; admission is not
a hard execution quota. Callback state is private synchronous state, not an
async context. A failed cutover is fixed before further migration; rollback is
a deliberate revert of its atomic candidate, never automatic engine fallback.

## System success condition

An integrated local workflow uses `match()`/`search()` in selectors and criteria,
replaces a payload at a typed JSONPath location, sends the expected captured HTTP
body, and produces the expected result/trace. Negative controls prove unsupported
Goessner, invalid syntax, resource errors, and failed replacement values cannot
produce success or unintended writes. Existing legacy/simple controls remain
unchanged. All typed JSONPath production entry points reach the same library
owner; neither handwritten typed-JSONPath engine remains. Capability docs and
conformance evidence state precisely what this tested composition supports.

---

**Disposition:** Decomposed into [epic:ac-edd23](https://sonos.scapedeck.com/docs/ac-tickets/ac-edd23). Bounded independent plan review and fresh-context transfer audit passed after their scoped corrections. Implementation has not started.
