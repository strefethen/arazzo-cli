# Rust Code Smell Audit — arazzo-cli

**Date:** 2026-11-07
**Baseline:** `1ff0567` (`feat(release): block releases that leave the public site stale`). The
working tree carries uncommitted edits in `arazzo-expr` and `arazzo-runtime`: the `EvalContext` work,
which wraps fields in `Arc` and adds the untracked `eval_context.rs` (ac-9534f, paused). Findings
that sit in those hunks are marked **[uncommitted]**.
**Method:** `rust-code-smell` skill, run by a scheduled task. Inventory only: no fixes, no source
touched.

This is a full sweep. Five fresh-context Opus reviewers ran at max effort, read-only and without
cargo:
- the runtime engine: HTTP, actions, payload, criteria, client, redaction, Kafka;
- the rest of the runtime, the `debug/` module and `arazzo-debug-adapter`;
- `arazzo-validate` and `arazzo-spec`;
- `arazzo-expr`;
- `arazzo-cli`, `arazzo-mcp` and `arazzo-generate`.

**Nothing was reproduced.** For every High, the lead re-read the cited code at HEAD and searched for
an owning ticket with `tkt query --include-closed --grep`.

**Prior audit:** [`2026-10-24`](rust-code-smell-audit-2026-10-24.md) (M#, L#). This run's findings
are numbered `N#`.

---

## Status (2026-11-09)

Six of the seven Highs are closed, one commit each on `main` after `39c8e08`, every one through a
fresh-context review that passed:

| # | Ticket | Commit |
|---|---|---|
| N1 | ac-28be3 (closed) | `e34345f` warn when request/response expressions resolve to Missing |
| N2 | ac-12c1e (closed) | `06a2037` end the run on debugger gate errors at every checkpoint |
| N3 | ac-1b0d1 (closed) | `10bd751` redact URLs in transport and redirect error messages; ac-8a804 stays open for the broader contract |
| N4 | ac-b3ba5 (closed) | `a94f3d2` redact string request bodies, plus `0c85cb0` fail-closed whole-value masking; ac-b30ba tracks JSON-text and `&`-in-value residue |
| N5 | ac-43b2f (closed) | `cb28a9b` bound YAML alias expansion at every parse entry point; ac-38c60 stays open for the aggregate budget |
| N6 | ac-fd667 (closed) | `d505233` check operationId source routing at validate time. Its prerequisite on ac-d15a3 was removed: the diagnostics consume `classify_operation_id` and today's `SourceType`, independent of whether `type` becomes optional. A qualified id naming an `asyncapi` source is accepted: §5.8.5 says a step may call an "(AysncAPI Operations Object)" and the spec's own example uses `$sourceDescriptions.asyncOrderApi.placeOrder`. |
| N7 | ac-d15a3 (paused) | Open. Gated on Steve's public-output choice in ac-bd367 (recommended: JSON `null` for an omitted source `type`, nullable schema). |

**Carried High M2 (`Step.timeout` ignored for HTTP steps) is withdrawn.** In
`spec/arazzo/v1.1.0.html` the Step Object's `timeout` is defined for message receipt only: "If no
matching message is received within the `timeout` period, the step fails". `engine_message.rs` reads
it for receive steps, which is the whole of the spec's requirement; enforcing it on HTTP steps would
extend the spec. The 10-24 M2 entry and the conformance-audit row stand corrected by this note.

## Summary

- **Highs: 7.** 3 have no owning ticket (N1, N2, N4), and N2 is the 10-24 M1, still untracked two
  weeks on. The other 4 are tracked, but none of those tickets is in progress.
- **Carried High M2 (`Step.timeout` ignored):** still present for HTTP steps and still untracked.
  Only `engine_message.rs:240` reads `step.timeout`, and only for AsyncAPI receive steps.
- **Mediums:** about 35. **Lows:** about 50. Both are listed briefly below.
- **Highest-risk areas:**
  - Redaction is applied separately to each output (trace, dry-run, `--json` error, `--export-run`),
    and the rules differ.
  - Debugger gate errors are routed through `step_attempt.rs::settle`.
  - Validation has no node, time or diagnostic budget.
  - Expression absence is reported by three different signals: Null, warnings, or `Missing`.

## High findings

| # | Finding | Tracked? |
|---|---|---|
| N1 | `$request.*` / `$response.*` / `$url` / `$method` / `$statusCode` resolve to Missing with **no warning**; templates splice the text `null` | **No** |
| N2 | A breakpoint-condition error at a criterion or output checkpoint becomes a step failure, so onFailure retries or gotos (= 10-24 M1) | **No** |
| N3 | Transport and redirect error messages carry the raw request URL (query-string credentials, userinfo) past redaction | Yes: [ac-8a804](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8a804) (open, ready; assessment) |
| N4 | Request bodies sent as plain strings (form text, plain text, XML) are not redacted in `--trace` or `--dry-run` | **No** |
| N5 | A YAML alias bomb gets past serde_yaml_ng's limit, and every entry point parses the document twice (10-17 L29, raised to High) | Partly: [ac-38c60](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-38c60) (ready; assessment that names aliases) |
| N6 | `operationId` targets that name an unknown source, are malformed, or are ambiguous across sources pass `validate --strict` | Yes: [ac-fd667](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fd667) (blocked) |
| N7 | `sourceDescriptions[].type` is required by serde, but the spec leaves it optional, so valid documents fail with a bare `missing field` error | Yes: [ac-d15a3](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d15a3) (paused, gate: Steve picks the output shape in [ac-bd367](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bd367)) |

### N1 — Missing request/response values produce no warning  [expr F1]

- **Category:** silent fallback. **Severity:** High. **Confidence:** High. Code-verified by the lead.
- **Where:**
  - `crates/arazzo-expr/src/resolution.rs:303-366` (`resolve_message_source`)
  - `crates/arazzo-expr/src/interpolation.rs:24-27`
- **Evidence:**
  ```rust
  if value.is_none() && expression.namespace() == Namespace::Message {   // only $message warns
  (Namespace::Response, Form::Query(_) | Form::Path(_)) => None,
  Resolution::from_option(value)
  other => rendered.push_str(&other.to_string()),                          // Missing -> "null"
  ```
- **Why it matters:**
  - `prepare_typed_condition` (`arazzo-runtime/src/runtime_core/criteria.rs:181-189`) fails only
    when warnings are non-empty. So a typed criterion
    `condition: '^{$response.header.X-Expected-Status}$'` with the header absent evaluates the regex
    `^null$` silently.
  - The same shape using `{$inputs.x}` fails properly.
  - The rustdoc on `ExpressionWarning` (`lib.rs:72-74`) promises a warning for a missing key.
- **Decision, not an accident:** `resolution.rs:806-820` pins `(Value::Null, Vec::new())` for these
  forms. It came from `82cc051` as legacy behaviour. Changing it means re-pinning that test, so it
  needs a ticket with an explicit decision.

### N2 — Debugger gate error is routed as a step failure  [runtime-debug F1; 10-24 M1]

- **Category:** error handling. **Severity:** High. **Confidence:** High. The lead re-traced this at
  HEAD.
- **Where:**
  - `crates/arazzo-runtime/src/runtime_core/engine_http.rs:667-668` (`debug_gate_success_criterion(..).await?`)
  - `runtime_core/step_attempt.rs:137` (`settle(run)`) and `:305-312`
  - `debug/breakpoints.rs:167-174`
  - routing at `engine_actions.rs:109`, which special-cases only `UnresolvedRequestValue`
- **Why it matters:**
  - A breakpoint condition is checked only for syntax when it is set. At evaluation time it can
    still fail with `truth()` "boolean or null required" (`simple_condition/evaluation.rs:154-159`).
  - That error becomes an ordinary failed attempt, so a `retry` re-sends a possibly non-idempotent
    POST, and a `goto` takes the failure branch.
  - The step-entry gate (`engine_impl.rs:352`) uses `?` outside the attempt, so the same error there
    aborts the run instead. The behaviour therefore depends on which checkpoint failed.
- **Verify:** see the 10-24 M1 scenario (count the POSTs).

### N3 — Raw URL in transport error messages  [runtime-engine F1]

- **Where:** `crates/arazzo-runtime/src/runtime_core/client.rs:539-546`; also `:478-483` and
  `:575-591`.
- **Evidence:** `let mut message = format!("executing request: {err}");`. reqwest appends
  ` for url (<full url>)` to its error text.
- **Why it matters:**
  - The message goes into `step_result_error`, then into the CLI `run_error` (`--json`) and the trace
    step's `error` field. `redact_trace_file` never touches either.
  - Only `--export-run` scrubs the message.
- **Ticket:** ac-8a804's goal names exactly these sites.

### N4 — String request bodies skip redaction  [runtime-engine F2]

- **Where:**
  - `crates/arazzo-runtime/src/runtime_core/redaction.rs:108-126` (`redact_trace_request`,
    `redact_dry_run_request`)
  - `:241-258` (`redact_json_value`, where `_ => {}`)
- **Evidence:**
  ```rust
  if let Some(body) = &mut request.body {
      redact_json_value(body);   // Value::String falls through unchanged
  }
  ```
- **Why it matters:**
  - Take `payload: "grant_type=password&password={$inputs.pw}"`. Here `form_body::encode` sends the
    string as-is (`form_body.rs:13-15`), and the body is written verbatim to `--trace` and
    `--dry-run`, even when `pw` is declared `format: password`.
  - Response bodies get `redact_text_patterns` (`arazzo-cli/src/trace.rs:206-216`). Request bodies
    do not.
  - Related closed tickets do not cover this: ac-72ba1 (replayable redacted traces) and ac-kkyy
    (centralize dry-run redaction).

### N5 — YAML alias amplification  [validate F1; generate F6]

- **Where:**
  - `crates/arazzo-validate/src/lib.rs:254-255`: raw `Value` parse, then the typed parse.
  - `arazzo-generate/src/lib.rs:52`: the same pattern for OpenAPI.
  - serde_yaml_ng 0.10.0 `de.rs:478-480` limits alias *jumps* to `events.len() * 100`, not the nodes
    they copy.
- **Why it matters:**
  - Nested anchors (`&a` of k items, `&b` = 99×`*a`, then m×`*b`) build about 99·k·m nodes from
    about 5 KB of YAML. That is several GB, and the release profile aborts on allocation failure.
  - This is reachable from `validate`, `run`, `test`, the debug adapter, and MCP `validate_spec` /
    `describe_openapi`. The MCP 4 MiB byte cap does not help.
- **Ticket:** ac-38c60 is an assessment. It tells the implementer to verify parser alias
  protections, but it does not record this finding.

### N6 — operationId targets unvalidated  [validate F2; 09-19 H17]

- **Where:**
  - `crates/arazzo-validate/src/async_applicability.rs:18-35`: returns early for non-qualified ids
    and for unknown source names.
  - `step_targets.rs` has no `OperationId` arm.
- **Why it matters:**
  - The Arazzo 1.1.0 Step Object says: *"If multiple (non arazzo type) sourceDescriptions are
    defined, then the operationId MUST be specified"*.
  - Today the runtime refuses each such case only when it reaches that step, after earlier steps
    have already sent requests.

### N7 — Optional `type` is required by serde  [validate F3]

- **Where:** `crates/arazzo-spec/src/lib.rs:195-203`. `name` and `url` have `#[serde(default)]`;
  `type_` does not.
- **Spec:** in `spec/arazzo/v1.1.0.html` §5.8.3.1, `type` has no REQUIRED marker: *"The type of
  source description. Possible values are "openapi" or "asyncapi" or "arazzo"."*
- **Note:** ac-d15a3 already plans "the optional source type model". It is waiting on Steve's
  public-output choice in ac-bd367.

## Medium findings (brief)

**Runtime engine**
- Cross-host redirects strip only Authorization, Cookie, Proxy-Authorization and WWW-Authenticate.
  `X-API-Key` and `*token*` headers, plus a Referer with the full query string, still reach the new
  host. The cleartext warning checks only the first URL (`client.rs:627-640`, `:208-220`).
- Parallel mode keeps every retried attempt with its full response until the step finishes
  (`engine_parallel.rs:299-311`).
- The response body is copied about 16 times per step for debugger locals and the trace, even with
  no debugger attached (`engine_trace.rs:690-693`, gate early exit at `:303-305`). Line 308 is
  **[uncommitted]**.
- `EvalContext.response_body` is owned and rebuilt 2–5 times per step (`eval_context.rs:10-16`
  **[uncommitted]**, ac-9534f).
- Regex, XPath and data-dependent simple-condition errors are visible only to the debugger. Action
  criteria discard warnings (`criteria.rs:98-104`, `:126-152`; `engine_actions.rs:275-296`).
- `deps.rs` uses its own `STEP_REF_RE`, which misses digit-leading and case-insensitive step ids, so
  `--parallel` can race a step against its reader. `extract_step_refs` and `routing_refs` disagree
  (`deps.rs:3-6`, `:221-229`).
- `run --step` returns success with stale outputs when it hits the iteration cap
  (`engine_impl.rs:137-141`, `:250-253`).
- An onFailure `end` action replaces the real failure message. `settle` also drops the transport
  trace and the error source (`engine_actions.rs:321-331`).
- YAML payload keys that are not strings become `""`, and `.inf`/`.nan` and tagged values become
  null, with no warning (`payload.rs:188-200`).
- Kafka receive re-fetches the same batch once per record (`kafka/receiver.rs:241-254`).
- `Retry-After` has no cap and no diagnostic (`engine_actions.rs:919-922`).
- Replay silently uses the truncated, redacted `body_preview`, and parses JSON differently from a
  live run (`client.rs:804-813`).

**Runtime rest / debugger**
- The YAML source index records checkpoints with an empty workflow or step id when keys are sorted,
  and always for workflow-level outputs. Those breakpoints show as verified but never fire
  (`dap/source_index.rs:545-551`).
- With the default launch config, the debugger runs the alphabetically first workflow that has a
  breakpoint, possibly a sub-workflow, without telling the user (`dap/session.rs:467-473`).
- XPath node sets copy each match's full subtree text, up to about 64× the 10 MiB body
  (`xpath.rs:112-117`).

**Validate / spec**
- `has_reference_error` makes validation O(steps² × params): every parameter scans all diagnostics
  so far and formats a string each time (`lib.rs:1900-1903`, `expressions.rs:499-504`).
- Diagnostic paths repeat the full workflowId and stepId in every diagnostic, so a 1 MiB document
  can produce about 6 GiB of strings (`lib.rs:1302-1338`).
- A parameter `reference:` key switches off required checks, and non-string references go
  unreported (`expressions.rs:460-461`, `lib.rs:2067`).
- Action `stepId`/`workflowId` values starting with `$` skip both the existence and syntax checks
  (`lib.rs:2350-2366`).
- `$steps.<id>` existence is checked only in workflow outputs (`lib.rs:1542`).
  `step_expression_sites` already lists every site but is unused.
- Regex, JSONPath, XPath and JSON Pointer conditions and selectors are never syntax-checked at
  validate time (`expressions.rs:118-126`, `:359-362`).
- `parse()` throws warnings away, and `run`, `test`, the debug adapter and MCP all use it, so a typo
  like `sucessCriteria` is invisible (10-17 L28, `lib.rs:226-229`).
- Input `$ref` resolution drops sibling keywords and leaves chained refs unresolved (10-17 L27,
  `lib.rs:2488-2494`).
- The effective-parameter helper and the runtime's `merge_workflow_params` disagree for workflowId
  steps (`effective_step.rs:34` vs `engine_impl.rs:928`).
- The workflow cycle check is O(W²) and blames every workflow upstream of a cycle
  (`step_dependencies.rs:11-17`).
- operationPath pointer decoding is re-implemented in three places (`operation_path.rs:86-104`,
  `operation_target.rs:218-230`, `asyncapi_target.rs:171-186`).

**Expr**
- A legacy regex interpolator still drives goto/retry targets. With
  `$sourceDescriptions.shared.login-flow` it captures only `…login` and leaves the id `-flow`
  (`lib.rs:88-91`, `:193-244`). Tracked by [ac-cd36b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-cd36b)
  and [ac-bb7ab](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-bb7ab).
- Every body reference deep-clones the value, and the context owns the bodies
  (`resolution.rs:369-375`; ac-9534f).
- The I-Regexp pattern is compiled once per candidate node (`jsonpath/iregexp_adapter.rs:109-114`).

**CLI / MCP / generate**
- MCP `run_workflow` checks source paths and then lets the runtime read them with a plain
  `std::fs::read`. The pinned-root loader is never installed (`arazzo-mcp/src/handlers.rs:257-279`).
- MCP trusts the declared Content-Length for allocation, so `u64::MAX` panics the reader and the
  server exits 0 (`protocol.rs:106-112`). Tracked by ac-fc556.
- Framing errors end the MCP session silently with exit 0 (`protocol.rs:196-203`).
- MCP handles requests one at a time with no cancellation. `run_workflow` blocks `ping` for up to
  300 s, and `notifications/cancelled` is dropped.
- `generate` emits duplicate workflowIds (`/v1/users` and `/v2/users` both become `crud-users`)
  (`crud.rs:360-367`).
- A `generate` source name taken from a non-ASCII or long title fails the identifier check and
  aborts the run (`crud.rs:663-674`).
- `arazzo test` drops the default User-Agent because it replaces `default_headers` wholesale
  (`test_runner.rs:170-175`).
- `-i KEY=$VAR` with the variable unset sends the literal `$VAR`, and coercion turns `zip=02134`
  into 2134 (`handlers.rs:429-458`).
- `--trace` and `generate -o` can overwrite their own inputs. A bad trace path fails only after a
  live run (`run.rs:313-381`).
- `catalog` and `show` swallow spec parse errors, even with `--verbose`
  (`workflow_catalog.rs:47-51`, `:120-122`).

## Low findings (brief)

- **Runtime:**
  - Two step loops have diverged (`engine_impl.rs`).
  - Production dispatch is duplicated under `cfg(test)` and `cfg(not(test))`.
  - Unchecked `Instant + connect_timeout` panics on an absurd `--kafka-connect-timeout`
    (`engine_message.rs:113`).
  - A tiny `requests_per_second` makes `from_secs_f64` overflow, and `channel_capacity(0)` panics.
  - A poisoned lock silently becomes "not completed".
  - Library `eprintln!` warnings bypass the event stream (`engine_impl.rs:853`).
  - Non-ASCII response header values become `""` (`client.rs:646`).
  - AsyncAPI files are re-read with blocking I/O on every receive attempt.
  - The broker registry lock is held across `connect().await`.
  - `RequestPrepared` events carry unredacted headers.
  - A server variable with no string default is left in place, and path params can then be
    substituted into the host.
  - NATS trial tests depend on a 100 ms sleep.
- **Debugger:**
  - DAP Content-Length is trusted for allocation.
  - Every checkpoint deep-copies the context under the controller lock.
  - `variables` and `evaluate` responses have no size bound.
  - Engine-task panics lose their message, and `CompletionGuard` records `Completed` on unwind.
  - The monitor comment states a false cancellation premise (`session.rs:234-237`).
  - `main` holds the stdout lock for the whole session.
  - The coordinator busy-polls every 5 ms.
  - Launch args of the wrong type are coerced, so `"dryRun": "true"` sends live requests.
  - The public `take_stop_events` breaks stepping.
  - One controller has a single `waiting` flag.
  - The bare-`/` XPath extension is implemented three ways.
  - `body_free_error` matches error-message text.
  - `#[non_exhaustive]` on internal enums forces catch-all arms.
  - DAP `collect()` keeps all events.
  - There are stale `allow(dead_code)`/`allow(deprecated)` attributes, and the crate docs say
    "Arazzo 1.0.1".
  - Fixture `Drop` panics.
  - `setBreakpoints` re-reads every file.
- **Validate:**
  - Repeated cloning, O(n·depth).
  - Raw checks skip non-string component keys.
  - Component-action unknown-field warnings repeat at every use.
  - Diagnostic kinds don't match their documentation: `UnsupportedOperationPath` severity,
    `MissingParameterValue` never emitted, `ReceiveTargetRefused` emitted by the CLI only.
  - Type names are trimmed and lowercased, an undocumented leniency.
  - `lib.rs` sits exactly at its 2,849-line cap, and `collect_diagnostics` is 597 lines.
  - Unused `Parameter::value_as_str` / `is_value_empty`.
  - The typed `validate()` enforces less than parsing does.
  - The root document is read with no size or file-type limit (a FIFO blocks).
  - The identifier class and `dependsOn` classifier are duplicated.
  - Component-resolution errors are untyped strings (09-19 H18, a deliberate decision).
- **Expr:**
  - `$components.*` is reported as an "unknown namespace".
  - Dead `PathError` and `is_truthy`.
  - `evaluate_condition_with_diagnostics` drops the syntax error.
  - Pest's raw Display text is used as the public error message.
  - A failed parse runs pest two or three times.
  - String compare lowercases on every compare and is not case folding (documented).
  - The `rt_`/`c_` alias table can drift into a panic.
  - Typed-condition templates splice values in unescaped (the spec requires raw substitution).
  - The runtime `string_comparison` bench uses double quotes, so it times the syntax-error path.
- **CLI / MCP / generate:**
  - MCP stream framing: a BOM or batch line flips the server into header mode, and an invalid
    request with an id gets no reply.
  - `$ref` chains are resolved O(n²) in `describe`.
  - `object_example` uses `Vec::contains`.
  - `find_id_in_response` gives up at the first success response with no JSON body.
  - `-H` parsing splits `Name=https://…` on the colon.
  - `println!` panics on EPIPE (`| head` exits 101).
  - The `generate` positional rewrite fires on any argument equal to "generate".
  - The trace temp file is created with default permissions and leaks on failure.
  - A `.env` open error is swallowed.
  - Three step-projection copies have drifted: `show --json` drops operationId targets.
  - `run_tests` takes 17 positional arguments.

## Patterns / meta-smells

- **Each output redacts separately:** trace, dry-run, `--json` error, export and observer events all
  apply different rules (N3, N4, redirect headers, `RequestPrepared`).
- **Validate and runtime disagree:** runtime refusals with no validate-time equivalent (N6, `$`
  action targets, `$steps` typos, regex/XPath syntax), plus duplicated classifiers (deps regex,
  pointer decoding, effective parameters).
- **Absence is signalled three ways:** Null, warnings, or `Missing`, and each consumer re-derives it
  (N1, debugger compensation, typed-criteria gate).
- **No resource budget past the byte read:** YAML aliases, diagnostics, identifier length, the
  validation O(n²) paths, the MCP single dispatch thread.
- **Work done before checking whether anyone needs it:** debugger locals, trace bodies and
  `EvalContext` clones on every step. ac-9534f is in flight for part of this.

## Non-findings

- No `unsafe` code anywhere; every crate root has `forbid(unsafe_code)`.
- No reachable `unwrap`/`expect` on document data in non-test code.
- No unsafe UTF-8 slicing.
- Condition nesting is capped at 32. Runtime-expression grammar is non-recursive. Recursion in
  serde_yaml_ng is capped at 128.
- The response size cap is enforced both on Content-Length and while streaming. The redirect chain
  has an overflow-checked deadline. https→http downgrades are refused.
- Cancellation wins over transport, rate-limit, retry-sleep and Kafka waits. No std mutex is held
  across an `.await` in the engine.
- The regex cache is bounded at 256 entries and 4 KiB patterns.
- MCP `filesystem.rs` (`openat2` BENEATH/NO_SYMLINKS, single-link regular files, chunked reads) has
  no traversal or symlink escape.
- The `--json` stdout contract holds. No library crate writes to stdout.
- `xml_admission` refuses DOCTYPE internal subsets, and uppsala never fetches external entities.
- operationPath *classification* has a single owner: all six call sites use
  `classify_operation_path`.
