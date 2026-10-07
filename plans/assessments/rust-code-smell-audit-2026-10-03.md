# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-10-03
**Baseline:** `59c8ea1` (`fix(generate): emit canonical body JSON Pointers`)
**Method:** `rust-code-smell` skill — find and enumerate only. No fixes applied, no source touched.
Scheduled run, **delta audit**. Since the [2026-09-26](rust-code-smell-audit-2026-09-26.md) baseline
(`0f83071`), four code commits landed: the XML admission scan (`4505178`), the generated Pest Runtime
Expression parser (`fda6a04`), the Pest simple-condition syntax parser (`0e372b2`), and canonical
body pointers in `generate` (`59c8ea1`). Four fresh-context Opus reviewers took one commit each.
The lead then re-read the cited code for every finding and re-ran the CLI repros for K1–K3, K19–K23,
K25 and the I1 closure. Files the delta did not touch were not re-swept.
**Build state at audit time:** `cargo build --workspace --all-targets`, `cargo fmt --all -- --check`
and `cargo clippy --workspace --all-targets --all-features -- -D warnings` exit 0;
`cargo test --workspace` exits 0 (74 result blocks, 1187 passed, 0 failed, 1 ignored).

**Prior audits:** [`2026-08-22`](rust-code-smell-audit-2026-08-22.md) (F#),
[`2026-09-05`](rust-code-smell-audit-2026-09-05.md) (G#),
[`2026-09-12`](rust-code-smell-audit-2026-09-12.md) (H#),
[`2026-09-19`](rust-code-smell-audit-2026-09-19.md) (I#),
[`2026-09-26`](rust-code-smell-audit-2026-09-26.md) (J#). Findings here are numbered `K#`.

**Confidence.** **REPRODUCED** = demonstrated against the HEAD debug build
(`target/debug/arazzo-cli`) or a scratch probe crate with path deps; the input is given.
"(lead)" means the lead re-ran it; otherwise the reviewer ran it and the lead checked the code.
**Code-verified** = confirmed by reading source. Spec quotes are from
[`spec/`](../../spec/README.md). Fixtures lived in the session scratchpad.

**Model note.** The task asked for "Opus 5 Max". Reviewers ran as Opus through the agent tool,
which has no per-agent effort setting, so "max" was a prompt instruction, not a configured level.
Reviewer usage was about 733k tokens in total (201k, 176k, 164k, 190k).

---

## Summary

- **Total new findings:** 26 — **0 Critical, 2 High, 8 Medium, 16 Low**
- **Prior findings:** I1 **closed** (re-run by the lead). Every other open I#/J# finding sits in a
  file this delta did not change; I11 and J23 were re-run and are live.

**Highest-risk areas**

- `crates/arazzo-expr/src/runtime_expression.rs` tests — compile-time dependency on the gitignored
  `spec/` directory (K8)
- `runtime_core/xpath.rs` → `uppsala` 0.3.0 — the admission scan bounds depth and DTD only; flat or
  wide bodies still stall the worker past `--execution-timeout` (K1, K2)
- `runtime_core/criteria.rs` — XPath criterion errors, including the new refusals, reach no
  ordinary output (K3)
- `crates/arazzo-generate/src/crud.rs` — names from the OpenAPI document flow into output keys,
  step ids, inputs and payloads unchecked, and `generate` never validates its result (K19–K23)

**Top 5 most severe**

1. **K8** — `arazzo-expr` unit tests `include_str!` a gitignored file; a clean checkout cannot
   compile the test target, so CI fails on the next push (High, reproduced)
2. **K1** — A flat XML body with many attributes runs for minutes and ignores
   `--execution-timeout` (High, reproduced)
3. **K2** — The admitted 64 levels multiply the cost of an ordinary `//a//b` expression: 4 MB body,
   18.8 s, 1.85 GB (Medium, reproduced)
4. **K3** — XPath criterion errors are dropped on every non-debugger surface (Medium, reproduced)
5. **K21** — OpenAPI example strings are copied into `payload`, where the runtime evaluates them as
   expressions (Medium, reproduced)

**Systemic observation.** Three of the four commits closed exactly what their ticket named and left
the neighbouring case open. The XML scan closes I1's two vectors but not the same denial-of-service
class through other shapes. The pointer fix escapes names that can never appear in a valid
document. The parser's provenance test proves the ABNF matches the spec only on a machine that has
the untracked spec file.

---

## Findings

### A. XML admission and XPath (`4505178`)

### K1 — A flat XML body with many attributes runs for minutes and ignores `--execution-timeout`
- **Category:** Security / resource · **Severity:** High · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `~/.cargo/registry/src/*/uppsala-0.3.0/src/parser.rs:2122` (and the `xmlns:` check at
  `:2108`), reached from `crates/arazzo-runtime/src/runtime_core/xpath.rs:76` after the scan at `:75`
- **Evidence:**
  ```rust
  // Regular attribute — check for duplicates among regular attrs only
  if raw_attrs.iter().any(|(n, _)| *n == *attr_name) {
  ```
- **Repro:** one element `<r a='' b='' … />` with N distinct attributes, criterion `count(/r) > 0`.
  HEAD debug build: 20k attributes 1.49 s, 50k 9.42 s (quadratic). 100k attributes (697 KB) with
  `--execution-timeout 2s` ran 43.8 s before reporting `RUNTIME_EXECUTION_TIMEOUT`. The reviewer
  measured the 09-24 release build at 16.2 s (100k), 40.8 s (200k, 1.46 MB) and 135 s (400k,
  3.06 MB); 50k `xmlns:pN` declarations took 9.5 s in debug.
- **Why it matters:** any server a workflow calls can stall `run`, `test` or the MCP server with a
  well-formed body under the 10 MiB response cap. `admit_xml` checks depth and DOCTYPE only, so the
  body is admitted, and the work is synchronous, so the timeout cannot interrupt it. Same class as
  I1; rated High, not Critical, because it stalls and does not abort, and memory stays small (71 MB).
- **Tracking:** general [ac-7a6ba](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-7a6ba) only.

### K2 — The admitted 64 levels multiply the cost of an ordinary multi-step `//` expression
- **Category:** Resource / performance · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `uppsala-0.3.0/src/xpath.rs:1074-1089` (`apply_step` carries duplicates into the next
  step; dedup happens once, after the whole path)
- **Evidence:**
  ```rust
  for &node in context_nodes {
      let axis_nodes = select_axis(&step.axis, node, ctx.doc);
      for &candidate in &axis_nodes { ... result.push(candidate); }
  }
  ```
- **Repro:** body `<a>`×64 `<b/>`×1,000,000 `</a>`×64 (4.0 MB, admitted), criterion
  `count(//a//b) > 0`, `--execution-timeout 2s`: 18.8 s and 1.85 GB RSS, then
  `RUNTIME_EXECUTION_TIMEOUT`. Control `count(//b) > 0` on the same body: 1.63 s, 305 MB. The
  reviewer also measured a 449-byte `<a>`×64 chain with six chained `//a` steps at 11.1 s and
  1.49 GB (release).
- **Why it matters:** the server chooses the shape and the author's expression is unremarkable.
  Each of the 64 ancestors contributes the full descendant set, so the depth the scan admits is the
  amplification factor. Rated Medium because it needs two or more descendant steps in the
  expression.

### K3 — XPath criterion errors, including the new refusals, are dropped on every non-debugger surface
- **Category:** Error handling · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crates/arazzo-runtime/src/runtime_core/criteria.rs:144-148` (XPath arm) vs `:89-100`
  (JSONPath arm)
- **Evidence:**
  ```rust
  Err(message) => {
      error = Some(message);      // XPath arm: nothing pushed to expr_warnings
      false
  }
  ```
- **Repro:** body `<a>`×65, XPath criterion `/a`,
  `run --expr-diagnostics warn --json`: exit 1 with
  `step s1: success criteria not met (status=200, body=<a><a>…)`. The text `invalid XML` appears in
  neither stdout nor stderr. The reviewer saw the same in `run -v` and the `--trace` file.
- **Why it matters:** `4505178` says a refusal is reported "exactly as they report malformed XML".
  That holds, because neither is reported. A user whose legitimate response is 65 levels deep or
  carries an internal subset sees an unexplained criterion failure. The JSONPath arm does push a
  warning for the same situation.
- **Tracking:** [ac-1946c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-1946c) (blocked)
  covers criterion errors in ordinary traces generally.

### K4 — Deeply nested XPath expressions overflow the stack; `validate` accepts them
- **Category:** Resource · **Severity:** Low · **Confidence:** High — **REPRODUCED** (reviewer)
- **Where:** uppsala `xpath.rs:666-672`, `:729-733`, `:850`, `:861` (recursive descent); no
  expression budget on the arazzo side (JSONPath has one, `README.md:407`)
- **Repro:** criterion `not(`×100 `true()` `)`×100 exits 134 in the debug build; `validate` exits 0.
  Thresholds on a 2 MiB thread: `not(` 77 debug / 866 release; predicates 98 / 604; parentheses
  135 / 866.
- **Why it matters:** this is I2's shape for XPath. The expression comes from the workflow author,
  so the only exposed path is an MCP client supplying the document. The reviewer rated it High; it
  is recorded as Low because release builds need 600+ levels and Steve declined depth testing for
  I2 on 2026-09-29. Not re-proposed.

### K5 — The new limits refuse some legitimate documents and are not documented
- **Category:** API & maintainability · **Severity:** Low · **Confidence:** High — **REPRODUCED** (reviewer)
- **Where:** `runtime_core/xml_admission.rs:19`, `:96-101`
- **Repro:** an SVG with `<!DOCTYPE svg PUBLIC "…" "…" [ <!ENTITY ns_svg "…"> ]>` is refused with
  "DOCTYPE internal subsets … are not accepted".
- **Why it matters:** `README.md` and `CHANGELOG.md` mention neither limit (`CHANGELOG.md` has no
  section after 0.7.0), and K3 hides the refusal text.

### K6 — Every XPath criterion and output re-scans and re-parses the whole body
- **Category:** Performance · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `runtime_core/xpath.rs:66-77`; callers `criteria.rs:135`, `:191`, `payload.rs:233`
- **Why it matters:** no per-response document cache. The reviewer measured one parse of a 10.4 MB
  wide body at 1.6 s and 1.09 GB RSS in debug. A step with k XPath criteria and outputs pays that k
  times. The re-parse predates this commit; the extra scan per call is new.

### K7 — Admission tests check that skipped markup is not over-counted, never that counting resumes
- **Category:** Testing · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `runtime_core/xml_admission.rs:157-170`;
  `crates/arazzo-runtime/tests/xpath_semantics.rs:312-318`
- **Why it matters:** no test puts a comment, CDATA, PI or quoted attribute before 65 real levels
  and asserts refusal, which is the direction a bypass would take. The integration test asserts
  `elapsed < Duration::from_secs(1)` across a server round trip, which is load-sensitive.

### B. Runtime Expression parser (`fda6a04`)

### K8 — Unit tests `include_str!` the gitignored `spec/` file; a clean checkout cannot compile them
- **Category:** Build / testing · **Severity:** High · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-expr/src/runtime_expression.rs:655`, `:676`; `.gitignore:63` (`/spec/`);
  `.github/workflows/ci.yml:46`, `:74`, `:84`, `:153`
- **Evidence:**
  ```rust
  let html = include_str!("../../../spec/arazzo/v1.1.0.html");
  ```
  `git ls-files spec` lists nothing.
- **Repro:** the reviewer exported HEAD with `git archive`: `cargo build -p arazzo-expr` exits 0;
  `cargo test -p arazzo-expr --lib --no-run` exits 101 with `couldn't read …/spec/arazzo/v1.1.0.html`.
  The lead confirmed the ignore rule, the two include sites, and that no workflow step fetches `spec/`.
- **Why it matters:** CI runs `cargo clippy --workspace --all-targets`, which builds the lib test
  target, so the first push of the 14 unpushed commits fails on all three platforms before tests
  start. The suite passes locally only because this checkout has the untracked directory.
  `crates/arazzo-cli/tests/conformance_manifest.rs:1648-1654` already asserts a gate "must not
  depend on a vendored spec directory".

### K9 — Error kinds come from Pest's furthest attempted rule, so valid parts are blamed and known namespaces are "unknown"
- **Category:** Error handling · **Severity:** Medium · **Confidence:** High — **REPRODUCED**
- **Where:** `runtime_expression.rs:262-264`, `:278-297`
- **Evidence:**
  ```rust
  if position == 0 {
      return syntax_error(RuntimeExpressionErrorKind::UnknownNamespace, 1..input.len());
  }
  ```
- **Repro:** `$inputs` → `UnknownNamespace 1..7`; `$steps.st` → `InvalidNamespaceForm 9..9`;
  `$steps.st.` → `InvalidIdentifier 9..10`; `$workflows.wf.inputs` → `InvalidIdentifier 13..20`
  (blames the valid id `wf`).
- **Why it matters:** a failed keyword literal never advances Pest's position, so the same missing
  `.outputs.` yields different kinds depending on what follows. The enum is documented as "Stable
  categories" and the validator will surface it, so fixing it after the cutover is a contract change.

### K10 — A Pest call-limit failure is reported as a syntax error
- **Category:** Error handling · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `runtime_expression.rs:234-237`
- **Evidence:** `ErrorVariant::CustomError { .. } => Vec::new(),`
- **Repro:** with `pest::set_call_limit(10)`, `$inputs.x` → `Err(UnknownNamespace, 1..9)`.
- **Why it matters:** the limit is process-global and shared with `iregexp-rs`. Nothing in
  production sets it today; if anything does, exhaustion reads as an invalid document.

### K11 — Peak heap is 160–320 bytes per input byte
- **Category:** Resource · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** generated grammar (`name = { (CHAR)* }`, `CHAR = { unescape | … }`); `build.rs:15-19`
- **Repro:** counting allocator, 1 MiB names: `$inputs.<1MiB>` peaks at 168 MB,
  `$request.query.<1MiB>` at 336 MB. Time is linear (34–56 ms per MiB).
- **Why it matters:** every character emits two to four token-queue entries and the parser has no
  input-length bound. It matters once `validate` or MCP `validate_spec` parse untrusted documents.

### K12 — The "frozen" public API has no accessor for most of the syntax it computes
- **Category:** API & maintainability · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `runtime_expression.rs:99-117` (private `RuntimeExpressionForm`), `:127-152`
- **Why it matters:** header, query, path, body pointer, payload pointer, named input/output and
  workflow forms are built but unreachable, even from the parent module. An evaluator would have to
  re-lex `raw()`, which the plan's single-lexical-owner rule forbids, or the API must change.

### K13 — The namespace dispatch in use is hand-copied; the generated `expression` and `source` rules are dead
- **Category:** Maintainability · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `crates/arazzo-expr/grammar/arazzo.pest:3-17` vs `expression`, `source`,
  `expression_string`, `embedded_expression`, `literal_char` in the generated `generated.pest`
- **Why it matters:** "generated from the ABNF" holds for leaf productions only. A dropped or
  widened alternative in the hand-written list still compiles; only example tests would notice.

### K14 — No negative test for malformed `CHAR` backslash escapes
- **Category:** Testing · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `runtime_expression.rs:483-484` (positive `{` / `}` cases only)
- **Why it matters:** the parser rejects `\x`, `\u12` and `\u12G4` today, but nothing pins it.
  AGENTS.md requires a negative case for spec-surface tests.

### C. Simple-condition syntax (`0e372b2`)

### K15 — Condition error messages are Pest's raw diagnostic text
- **Category:** Error handling · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `crates/arazzo-expr/src/simple_condition/syntax.rs:93-103`
- **Evidence:** `message: error.to_string(),`
- **Repro:** `$response.header.==1` → `condition at byte 17:  --> 1:18 … = expected c_token`.
  An 800 KB condition produced a 1.6 MB message; an ESC byte in the input is echoed unescaped.
- **Why it matters:** grammar rule names (`unary`, `c_token`, `ALPHA`) become part of the public
  `ConditionError.message`, one string mixes a byte offset with Pest's character column, and
  document-controlled control bytes reach the terminal. Not user-visible until a consumer lands.

### K16 — `5.e5` and `5. 5` parse as a property access on a number
- **Category:** Spec conformance / testing · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `grammar/arazzo.pest:27` (`postfix`), `:31` (`property`), `:39` (`number`)
- **Repro:** `5.e5` → `Postfix { primary: Number("5"), accesses: [Property("e5")] }`; `5. 5` and
  `5 .5` give `Property("5")`. `5.` alone is rejected.
- **Why it matters:** a float typo such as `1.E3` is not a syntax error; it becomes a dereference
  the future evaluator must resolve. It follows from two accepted table rows, and no test pins
  either outcome.

### K17 — The nesting pre-scan is correct only through an untested grammar invariant
- **Category:** Maintainability · **Severity:** Low · **Confidence:** High — Code-verified (2M-input fuzz found no current disagreement)
- **Where:** `syntax.rs:121-175` (comment: "independent of Runtime Expression syntax");
  `arazzo.pest:44` (`c_stop`)
- **Why it matters:** the scan treats every `'` as a string delimiter. That is sound only while
  `c_stop` keeps `'`, `(`, `)` and `!` out of condition terminals. `tchar` includes `'` and `!`; if
  that row were relaxed, `$response.header.a' && ((((…` would hide every `(` from the limit. No
  test ties the two lexers together.

### K18 — The private syntax tree is visible through derived `Debug` and `PartialEq`
- **Category:** API · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `syntax.rs:41-48`
- **Repro:** `{:?}` of `parse_simple_condition("!($inputs.x)")` prints
  `syntax: Not(Group(RuntimeExpression(0)))`.
- **Why it matters:** any log, snapshot or assertion output that prints the public type exposes
  the tree the plan keeps private. Nothing prints it today.

### D. `generate` (`59c8ea1`)

### K19 — The new pointer escaping is reachable only in documents `validate` rejects, and the new test pins them
- **Category:** Spec conformance / testing · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crates/arazzo-generate/src/crud.rs:925-930`;
  `crates/arazzo-generate/tests/body_output_pointers.rs:56-60`, `:77-80`
- **Evidence:**
  ```rust
  let pointer_token = id_field.replace('~', "~0").replace('/', "~1");
  outputs.insert(id_field.to_string(), format!("$response.body#/{pointer_token}").into());
  ```
- **Spec** (Step Object `outputs`): "The name MUST use keys that match the regular expression:
  ^[a-zA-Z0-9\.\-_]+$"
- **Repro:** response property `a/bId`. `generate --strict` exits 0 and writes
  `a/bId: $response.body#/a~1bId` plus `$steps.create-widgets.outputs.a/bId`. `validate` exits 1:
  `outputs.a/bId value "a/bId" must match the regular expression`.
- **Why it matters:** a name containing `/` or `~` can never be a valid output key, so the `~0`/`~1`
  path never runs in a document that validates. The test asserts those keys exist and never
  validates the result. [ac-a9bea](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-a9bea) told
  the implementer to leave output keys alone, so the gap is in the ticket. The reviewer rated this
  High; recorded as Medium because the trigger is rare and the failure is loud at `validate`.

### K20 — `generate` never validates its own output, and `--strict` has no effect on it
- **Category:** Error handling · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crates/arazzo-cli/src/handlers.rs:418-466`; `crates/arazzo-mcp/src/handlers.rs:449-462`
- **Repro:** the K19 input with `--strict` prints "Generated:" and exits 0.
- **Why it matters:** every generator defect in this section reaches the user as a success and
  shows up later at `validate` or `run`.

### K21 — OpenAPI example and default strings are copied into `payload`, where the runtime evaluates them
- **Category:** Correctness / security · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crates/arazzo-generate/src/examples.rs:41-48`, `:68-70`; `crud.rs:901-905`
- **Spec** (Request Body Object `payload`): "The value can be a literal value or can contain
  Runtime Expressions or Selector Objects which MUST be evaluated prior to calling the referenced
  operation."
- **Repro:** example `"$9.99"` generates `note: $9.99`; `validate --strict` passes; a dry run
  plans the body `{"note": null}` with the warning `unknown expression namespace "$9.99"`. Example
  `"Hi {$inputs.token}"` plans `{"note": "Hi SECRET"}` with `-i token=SECRET` and no warning.
- **Why it matters:** sample values change meaning silently, and an untrusted OpenAPI document can
  make a generated workflow place an input such as the auth token in a request body.

### K22 — The ID heuristic takes the first property ending in `Id`
- **Category:** Correctness · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crud.rs:636-645`
- **Evidence:**
  ```rust
  for name in obj.properties.keys() {
      if name == "id" || name.ends_with("Id") || name.ends_with("_id") || name.ends_with("ID") {
          return Some(name.clone());
  ```
- **Repro:** response properties `{ownerId, id}` generate `ownerId: $response.body#/ownerId`, and
  the read step uses `$steps.create-widgets.outputs.ownerId` as the resource id.
- **Why it matters:** a foreign key ahead of `id` is common; generated read, update and delete
  steps then address the wrong resource. A different trigger from carried I67 (early `?` return).

### K23 — Identifiers built from OpenAPI names are not sanitised (extends I39, I40, I74)
- **Category:** Spec conformance · **Severity:** Medium · **Confidence:** High — **REPRODUCED (lead, step ids)**
- **Where:** `crud.rs:351-358`, `:540`, `:687`, `:746-841`
- **Repro:** path `/v1:widgets` generates `stepId: create-v1:widgets` and
  `$steps.create-v1:widgets.outputs.id`; `/my widgets` generates `create-my widgets`. `validate`
  warns (exit 1 under `--strict`). The reviewer also showed path parameters and security scheme
  names with a space, `#` or `~` producing `$inputs.item id`, which passes `validate` with no
  diagnostic and which the new parser rejects.
- **Why it matters:** the evaluator resolves these today. Documents `generate` produces now will
  start failing at the parser cutover
  ([ac-2997b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2997b),
  [ac-e2aaa](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e2aaa)).

### K24 — A property containing `{` or `}` still emits a non-canonical pointer
- **Category:** Spec conformance · **Severity:** Low · **Confidence:** High — **REPRODUCED**
- **Where:** `crud.rs:926-929`
- **Spec** (ABNF): "unescaped = %x00-2E / %x30-7A / %x7C / %x7F-10FFFF ; Excludes / (%x2F),
  { (%x7B), } (%x7D), and ~ (%x7E)"
- **Repro:** property `a{b}Id` emits `$response.body#/a{b}Id`; `parse_runtime_expression` returns
  `TrailingInput 17..22`. The grammar has no way to write this token.

### K25 — The auth input and the path-ID input share one map with no collision check
- **Category:** Correctness / security · **Severity:** Low · **Confidence:** High — **REPRODUCED (lead)**
- **Where:** `crud.rs:694-718`
- **Repro:** bearer auth plus path `/widgets/{token}` and no POST generates one `token` input
  described as "ID of the widgets resource" with `required: [token, token]`. The reviewer's dry
  run with `-i token=abc` sent the value both as `Authorization` and in the URL path.
- **Why it matters:** narrow trigger, but the credential lands in the request path and its logs.

### K26 — The new test has no negative case through the parser or validator
- **Category:** Testing · **Severity:** Low · **Confidence:** High — Code-verified
- **Where:** `tests/body_output_pointers.rs:84`, `:89-98`
- **Why it matters:** the only negative check is `!expression.starts_with("$response.body.")`.
  Pointers are resolved with `serde_json::Value::pointer`, not `parse_runtime_expression` or
  `arazzo_validate`; either would have caught K19 and K24.

---

## Prior findings at `59c8ea1`

| ID | Status | Evidence | Tracking |
|---|---|---|---|
| I1 XML depth/entities | **Closed** | Lead re-run: `<a>`×200 → exit 1 in 0.03 s (was exit 134); 484-byte entity cascade refused in 0.00 s (was 38 s, ~940 MB); ×65 refused, ×64 passes | [ac-d1649](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d1649) closed by `4505178`. Residual class: K1, K2 |
| I2 nested parens in conditions | Live | The live evaluator is unchanged. The new syntax parser has a 32-level pre-scan but nothing consumes it yet | [ac-67bf5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-67bf5) closed (syntax only); evaluator cutover is [ac-60b0b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-60b0b) (blocked). Depth tests declined 09-29 |
| I3–I10 | Live, not re-run | Their code is unchanged since `0f83071` (`git diff --name-status 0f83071..HEAD`); `arazzo-expr/src/lib.rs` gained only 12 lines of re-exports, which shifts I10's line numbers | As listed on 09-26 |
| I11 `generate` blow-up | Live | Reviewer re-run: 3.6 KB → 1.44 MB YAML; 4.4 KB → 7.8 MB, 268 MB RSS | none |
| J1–J22, J24–J26 | Live, not re-run | `deps.rs`, `engine_impl.rs`, `engine_parallel.rs`, `step_attempt.rs`, `builder.rs`, `engine_http.rs`, `document_set.rs` unchanged | none cite the 09-26 audit |
| J23 server-URL rule duplicated in `generate` | Live | `crud.rs:118-150` still uses `starts_with('/')`; `servers: [{url: "."}]` generates, then `run --dry-run` refuses it | none |
| H10, I25–I28 | Live, not re-run | Files unchanged | As listed on 09-26 |

No ticket cites the 09-19 or 09-26 audit file. A search for I#/J# ids in ticket text matched only
[ac-81fab](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-81fab) and
[ac-ec3a1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-ec3a1). The four J-series Highs
(J1–J4) are untracked.

---

## Patterns / meta-smells

- **The fix covers the named vector, not the class.** I1 → K1/K2 (depth and DTD bounded; width,
  attribute count and expression cost are not). The pointer fix → K19/K24.
- **Evidence that depends on the local machine.** K8 passes only where `spec/` exists; the
  `elapsed < 1s` assertion (K7) depends on load.
- **Synchronous evaluation outside the timeout.** K1 and K2 join I3: any CPU-bound evaluator call
  runs to completion before `--execution-timeout` is noticed.
- **`generate` trusts OpenAPI names.** Output keys (K19), step ids and inputs (K23), payload values
  (K21) and the input map (K25) all take source text unchecked, and nothing validates the result (K20).
- **Diagnostics derived from parser internals.** K9 and K15 both turn Pest's furthest-failure
  bookkeeping into a public error contract before any consumer exists.
- **Tests that pin the gap.** `body_output_pointers.rs` asserts invalid output keys (K19), following
  `deps.rs:500` (J6) and the conformance lines noted on 09-26.

## Non-findings (checked, clean)

- **XML scanner:** every bypass tried against 200 real levels was refused — `>` and `</a>` inside
  quoted attributes, attribute values ending in `/`, `< a>`, surplus closing tags, PIs containing
  `?>`, comments containing `--`. No panic paths, linear, no `unwrap`. DOCTYPE false admission is
  not possible; `SYSTEM "a[b"` is correctly admitted.
- **No external entity or DTD loading in uppsala:** `SYSTEM "file:///etc/passwd"` gives "Unknown
  entity reference"; an `http://127.0.0.1` system id made no request to a local canary.
- **Every XML entry point passes the scan:** criteria, Selector Objects, replacements and debugger
  watches all go through `xpath_backend`.
- **ABNF provenance:** the vendored `.abnf` differs from the spec's block only by the recorded `)`
  indentation. `build.rs` uses `?` throughout, declares both inputs, writes only to `OUT_DIR`, and
  is deterministic. `Cargo.lock` gains three edges and no new packages.
- **Runtime Expression parser:** no panic or hang across about 410 probe inputs (empty, `$`, NUL,
  CR/LF, non-ASCII, every namespace truncated at every byte); linear to 1 MiB; no recursion; no
  Pest types in the public API; required error spans (`$response.body#/a~2` → 17..19) hold.
- **Condition parser:** pre-scan and grammar agree at the limit (32 groups accepted, 33 refused;
  flat `!`/`&&` chains do not accumulate); linear to 1.4 MB; every `expect`/`unreachable!` in
  `map_syntax` is unreachable; no wall-clock assertions.
- **`generate`:** `~` is escaped before `/`; YAML round-trips every odd name tried; the golden diff
  is the one intended line; `a.bId` resolves the literal key end to end.
