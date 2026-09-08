# Recording and replay fidelity decision

**Disposition: assessment; pending Steve's decisions, not implementation authority.**
Prepared 2026-09-08 against `1dc4731122c00c83eaed66a16485b4faaa848814` for
[ac-adb32](https://sonos.scapedeck.com/docs/ac-tickets/ac-adb32), contract revision
`1c4631ba88961794d17d3a1f8b15100607c7b3a407c5ca94b2b344626e371c59`.
The ticket's Goal is the retained intent: “Produce a decision-ready contract and
per-unit decomposition that makes replay's evidence limits explicit, while
reusing the existing redacted-comparison work.” Warning-level lint fails solely
with `plan-not-set`; the authorized planning-stage exception permits this
assessment, not a fabricated plan or an implementation-ready handoff.

## Recommendation and alternatives

Make replay success mean demonstrated execution equivalence. Keep `trace.v1`
as diagnostic evidence, introduce a separately versioned authoritative artifact,
and distinguish an inconclusive comparison from a verified replay. A successful
workflow with different outputs must never be a successful equivalence check.

Recommend `trace.v2`, explicitly requested by a new `run --record <path>` option.
Keep `run --trace` and its 2048-byte diagnostic policy on v1. The new recorder
publishes only complete eligible evidence. Classified secret-bearing or
unclassifiable data fails recording with a fixed error and no v2 artifact;
it cannot silently downgrade to a preview or redacted projection. `--record`
and `--trace` are mutually exclusive initially. Numeric retention limits belong to
[ac-e8235](https://sonos.scapedeck.com/docs/ac-tickets/ac-e8235); acceptance of that
contract is a prerequisite to enabling the recorder.

| Viable choice | Consequence |
|---|---|
| Diagnostic-only product | Smallest repair: reject equivalence claims for all existing traces. Gives up useful deterministic regression proof. |
| Add authority metadata to v1 | Additive JSON is allowed, but old readers ignore unknown fields and still replay incomplete evidence as success. A new required mode/profile cannot protect those readers. |
| **Separate v2 artifact and runtime evidence surface** | Old readers reject the version. Keeps diagnostic policy stable and gives new consumers an enforceable completeness contract; requires migration and new API/CLI contracts. |
| Retain private raw payloads or rehydrate secrets | Wider replay coverage, but introduces secret storage/input custody and dependency semantics. Raw-secret artifacts and secret sidecars are excluded; in-memory rehydration would require a separate accepted design. |

The recommended first release is deliberately conservative about secret-dependent
execution and opaque binary content. Full taint tracking, binary secret
classification, transport simulation, and cross-version semantic equivalence are
separate decisions, not implied implementation work.

## Decisive current evidence

These are implementation facts, not claims about what Arazzo permits.

| Current owner / symbol | Observed contract and consequence |
|---|---|
| [trace.rs](../../crates/arazzo-cli/src/trace.rs), `redact_trace_file`, `TraceRun` | Both `response.body` and preview are sanitized and truncated; no truncation field is written. `bodyBytes` remains the original length. JSON reserialization/redaction also changes length, so length inequality alone cannot establish truncation. The run stores a spec path, not a content binding or final workflow outputs. |
| [engine_trace.rs](../../crates/arazzo-runtime/src/runtime_core/engine_trace.rs), `build_trace_response` | Full bytes become a UTF-8 string with replacement characters; `body_lossy` records that transformation. Empty bytes become `body: None`, `bodyBytes: 0`, a valid empty-body case. |
| [engine_http.rs](../../crates/arazzo-runtime/src/runtime_core/engine_http.rs), `execute_http_step`; [engine_impl.rs](../../crates/arazzo-runtime/src/runtime_core/engine_impl.rs), `execute_inner` | A transport error propagates through `await?`; the engine wraps it with `StepTraceData::default()`. The failed attempt can retain its decision/error but lose its prepared request. |
| [replay.rs](../../crates/arazzo-runtime/src/runtime_core/replay.rs), `ReplayState::from_trace_steps`, `validate_replay_request` | Requestless records are skipped; queues are keyed by workflow/step only. Method comparison ignores ASCII case; URL/headers use equality, JSON request bodies use parsed-value equality. `seq`/`attempt` label diagnostics rather than validate invocation identity. |
| [client.rs](../../crates/arazzo-runtime/src/runtime_core/client.rs), `HttpClient::replay_request` | Uses body, then preview, then empty bytes; ignores `body_lossy`. A response wins over a simultaneous error. An error without response becomes generic `HttpRequest`. Replay parses JSON only for `ContentType::Json`; live `request` tries JSON for every non-XML body. |
| [handlers.rs](../../crates/arazzo-cli/src/handlers.rs), `replay_trace` | Rereads the selected spec and executes with persisted inputs. Success checks successful execution plus request count, not recorded criteria, decisions, outputs, or terminal status. |
| [document_set.rs](../../crates/arazzo-runtime/src/runtime_core/document_set.rs), `DocumentSet::{new,bind}`; [builder.rs](../../crates/arazzo-runtime/src/runtime_core/builder.rs), `build` | Runtime owns provided-document identities, local document loading, bindings, and unclaimed OpenAPI inputs. Replay reuses this construction. There is no HTTP document retrieval here; local/provided documents remain external to the trace. |

Hermetic CLI probes used the current checkout after
`cargo build --offline --locked -p arazzo-cli` succeeded. The debug binary's
SHA-256 was `c1c339460223a68e564d57c3c20458c8e8b7eed41bbf09eeaac3590a33806241`.
The throwaway runner was `/private/tmp/arazzo-replay-probe.py`; its final summary
was `/private/tmp/arazzo-replay-fidelity-1mym0yt8/summary.json`. Fixtures used
valid local OpenAPI operations, synthetic credential sentinels, and a loopback
server stopped before replay. No upstream credentials or services were used.

| Probe | Capture → replay observation |
|---|---|
| 3028-byte JSON with a trailing marker | `marker=ok`; body stored as 2051 bytes → exit 0, `kind=success`, `marker=null`. With limit 8192, full body survives and marker remains `ok`. |
| Fixed and input-derived Authorization | Both traces omit the synthetic credential → both fail `RUNTIME_REPLAY_REQUEST_MISMATCH`. |
| Response field `token` | Original output is the fixture value → exit 0 with output `[REDACTED]`. |
| Five binary bytes | Nine UTF-8 bytes persisted, `bodyLossy=true` → exit 0. Invalid UTF-8 inside JSON changes output from null to U+FFFD, also exit 0. |
| Complete JSON labeled `text/plain` | Fifteen intact bytes, not lossy → `marker=ok` becomes null, exit 0. Byte completeness alone is insufficient. |
| Remove body and preview from a copied trace | Nonzero original body length remains → exit 0, marker null. |
| First connection closes without a response; retry succeeds | Two attempts, decisions retry/next, only one recorded request → replay exits 0 after checking one request and skipping the failed attempt. |
| Change recorded step output or decision | Each independently still returns success. Changing the root workflow's final output returns that changed output as success. |
| Change a non-sensitive request URL | Rejected with `RUNTIME_REPLAY_REQUEST_MISMATCH`, a protection to retain. |
| Change referenced OpenAPI version metadata; then remove file | Metadata change is accepted; removal fails `RUNTIME_SOURCE_DESCRIPTION_LOAD`. Content was never bound. |

The nearest existing [engine replay tests](../../crates/arazzo-runtime/tests/engine_replay.rs),
including `replay_uses_full_body_not_truncated_preview`, supply runtime records
directly; they do not prove persisted CLI body fidelity. These observations
justify a contract change, not merely raising the preview limit.

## Proposed authority contract

**Meaning of equivalence.** Re-execute the same bound document set, inputs,
effective execution configuration, and compatible runtime semantics using only
recorded external outcomes. Compare every attempted step's prepared request,
outcome, criterion result, selected decision, and outputs, plus the workflow's
terminal status, structured error kind, and final outputs. This proves equality
of those runtime observations for that recording. It proves neither upstream
truth/authentication nor elapsed-time, TLS/DNS, socket, or wire-byte equivalence.
An artifact is evidence supplied by its holder, not a signed attestation of a
real remote execution; hashes establish binding, not authenticity.

**Payload representation.** v2 needs orthogonal facts, not one overloaded flag:
`presence = absent | empty | present | unavailable`,
`completeness = complete | truncated | unknown`,
independent `redacted` and `lossy` transformation facts, and
`representation = bytesBase64 | diagnosticText | none`.
Record original/retained byte lengths when known and the classification-policy
version. The in-memory evidence model and reader can represent ineligibility;
the MVP writer never publishes an ineligible run or its affected locations.
Absent means no protocol body; empty means known zero bytes; neither means missing evidence.
Both transformation facts must be known false for exact fidelity; they may both
be true for a diagnostic payload. Reject contradictory field combinations.
Only complete exact bytes, known empty, or known absence can reconstruct an
authoritative response. Previews never become execution data. A partial body
followed by a read error is a failed transport outcome, not a complete response.

Base64 supplies a lossless representation, not permission to persist opaque
content. The first release rejects recording binary/invalid-UTF-8 payloads whose
secret classification cannot be established and publishes no v2 artifact.
Classification must inspect complete text/JSON before any serialization.
Do not persist raw classified credentials, reversible encodings of them, or
unkeyed digests of withheld secret values. Use one response-decoding owner for
live and injected bytes; do not repeat today's content-type parsing split.

**Attempt identity.** A run has an ordered invocation tree: root invocation,
parent invocation, calling step attempt, and child invocation ordinal. Each
record identifies invocation, workflow, step, per-invocation attempt ordinal,
and deterministic logical event ordinal. Workflow/step names alone collide
across repeated subworkflow calls. Concurrent completions retain dependency and
invocation identity rather than claiming wall-clock ordering is deterministic.
Record every attempted step, including failures before request preparation;
distinguish `notPrepared`, `preparedNotSent`, and `sent` from missing request
evidence. Transport outcomes identify failure phase and stable runtime kind;
human error text is non-authoritative context subject to the publication gate
below, not an exception to it. Redirect exchanges,
when present, belong to that attempt; first-release authority rejects incomplete
hop evidence instead of pretending a final response proves all exchanges.

Recording captures facts from existing execution; it must not change retry,
cancellation, or action semantics. A publication rejection is separate from the
workflow result; it never retries or reruns the workflow to obtain another
artifact. Replay injects the original failure kind so
the engine recomputes failure actions and retry decisions. Consume each identity
exactly once, reject duplicates/gaps/unconsumed records, and compare decisions
and outputs at their owning boundaries. A transport-failed attempt still counts
even without a response. Explicitly distinguish not-evaluated outputs from a
computed JSON null; neither absent evidence nor an evaluation failure is null
success. Transport nondeterminism that cannot be injected at the same observable
boundary makes the recording ineligible.
A required finalization record binds the observed terminal result and record
counts; a missing finalization record cannot certify a complete run.

**Document/configuration binding.** The manifest binds exact root Arazzo bytes,
every provided and locally loaded referenced document, and all explicit OpenAPI
documents, including unclaimed ones that can affect operation resolution. For
each, retain an opaque document ID, SHA-256 of eligible original bytes, semantic
identity, resolved source binding, and provided ordinal. Include effective
workflow/step selection, inputs and configuration: actual execution mode,
expression/validation policy, base resolution, effective headers, transport
policy, timeouts, retry/limit options, and runtime/decoder compatibility ID.
Initially require the identical recorded tool build ID; a looser compatibility
mapping needs evidence and separate acceptance.
Projected secret locations never count as exact configuration evidence.

CLI supplies file bytes and retrieval context; runtime `DocumentSet` exports
the identities/bindings it actually used. No second CLI resolver. Arazzo's
existing identity rule remains untouched: “references MUST use the target
document’s $self URI if the $self field is present in that document”
([vendored Arazzo §5.5.2](../../spec/arazzo/v1.1.0.html#identity-based-referencing)).
File relocation is permitted only when content and resulting semantic bindings
match; even a metadata-only content change rejects strict equivalence. Eligible
non-sensitive input values are persisted; source documents remain separately
supplied. There is no embedded source copy or secret sidecar. Classified literal
credentials in a source, retrieval context, or manifest field reject the entire
recording before any digest or metadata is published; an unavailable binding
does not license a partial artifact.
Supporting exact secret-bearing source identity requires another accepted
binding mechanism. Missing documents are not fetched. Source/workflow overrides
must satisfy the same manifest; arbitrary changed-spec comparison is a future
mode, not strict replay.

**Secret publication gate.** Without provenance, selective redaction cannot
prove that another field does not alias a detected secret. For example,
`{token: sentinel, label: sentinel}` leaves `label` intact under current
[redaction.rs](../../crates/arazzo-runtime/src/runtime_core/redaction.rs),
`redact_json_value`; `trace.rs::redact_trace_file` independently sanitizes
outputs by key. A later `{display: $steps.login.outputs.token}` has the same
problem. The v1 artifact is not alias-safe, and this assessment does not claim
the existing projection repair will make it so.

The MVP uses one monotonic, whole-run publication gate. Before execution, inspect
the actual input values, root and every referenced/provided document, retrieval
and semantic identities, resolved configuration, and candidate manifest fields.
During execution, inspect each prepared request (including headers, URL and
body), response, criterion/decision data, outputs, errors/diagnostics, and final
result. Classification uses the canonical key, header, text-pattern and URL
rules, including the userinfo prerequisite. The runtime recorder reports
classification facts; the CLI writer cannot independently declare a projection
safe. If classified secret material is present or discovered at **any** point,
permanently reject publication of **all** value-bearing data for that run,
including material collected before discovery. No per-field cleansing, alias
search, hashes of original values, or claim of unused secrets can clear the gate.
Failure to classify required data also rejects recording. Preflight rejection
starts no workflow; discovery during an already running workflow marks recording
failed while that workflow follows its existing control semantics.

Consequently, `--record` must retain candidate evidence only in bounded memory
until the complete run and every proposed serialized field pass the gate. It
must not stream, spool, checkpoint, or serialize value-bearing evidence to a
temporary file first. After validation, atomic publication is the only disk
write path. A late discovery publishes **no v2 artifact**, no partial manifest,
no diagnostic sidecar, and no supposedly safe request/response projection. An
existing destination remains untouched; no fallback v1 file is written. Resource
exhaustion follows the same no-publication rule under the resource contract.
The publication layer must also gate `--record` stdout/stderr/verbose reports:
do not emit buffered workflow results or diagnostics after rejection, even if
the workflow itself succeeded. Network effects already performed by the live
workflow are not undone; publication rejection never initiates re-execution.

The sole rejection report is fixed program text: `kind: error`,
`code: RECORD_SECRET_MATERIAL`, `error: Recording rejected: classified secret material`.
For unclassifiable content use the fixed code `RECORD_DATA_UNCLASSIFIABLE` and
fixed message `Recording rejected: data cannot be classified`; return exit 1.
Include no user-derived path, identifier, key name, URI, count, length, digest,
error detail, output, or manifest field. These fixed enum/literal fields reveal
only the rejection category and cannot contain an alias. This is a CLI failure
report, not an inspection artifact. Projection-only v2 inspection and projected
re-execution are deferred entirely. The guarantee covers classified material and
every alias once it is encountered; it is not a claim that the existing rules
discover every possible secret. Wider classification/provenance requires a
separately accepted security design.

**Legacy comparison ownership.** Reuse the single location-aware projection planned
by [ac-72ba1](https://sonos.scapedeck.com/docs/ac-tickets/ac-72ba1), including the
userinfo rule owned by [ac-d95a9](https://sonos.scapedeck.com/docs/ac-tickets/ac-d95a9).
Classified header/query/body values compare through that projection; retain
exact non-sensitive URL/header values and existing parsed-JSON request equality.
Recommend preserving today's case-insensitive method comparison; rewrite the
existing ticket's ambiguous “exact method” wording accordingly. A literal
`[REDACTED]` outside a classified location never becomes a wildcard. Mismatch
messages report locations/kinds, not actual sensitive URL/header contents.

Projection proves request equality only outside excluded values; it grants no
publication eligibility, alias-safety or faithful-execution guarantee. A removed
token may control a branch, output, or later non-sensitive request. Neither v1
nor a rejected v2 recording may be re-executed with a redaction marker as an
ordinary value. Preserve the projection work as the existing ticket's bounded
legacy comparison responsibility, not a v2 bypass. Broader v1 alias remediation
and general secret provenance remain separately accepted work.

## Compatibility, verdicts, and migration

[The v1 compatibility policy](../../docs/trace-schema-v1.md#compatibility-guarantees)
preserves existing field meanings/types; the
[changelog](../../docs/trace-schema-changelog.md#change-policy) requires a new
version for breaking changes. `TraceFile` deserialization currently tolerates
unknown fields but `read_trace_file` rejects another `schemaVersion`. Therefore
v2 introduces its own manifest, payload, outcome, and final-result shapes; it
does not reinterpret v1 `body`, `error`, or `bodyBytes`.

Likewise [runtime `api_v1`](../../crates/arazzo-runtime/src/lib.rs) explicitly
freezes trace/runtime-facing models. Keep them intact. Publish separately
versioned evidence/replay-session types and explicit adapters; do not add
mandatory fields to v1 structs or reinterpret their errors. A v1 adapter may
classify evidence as unknown in memory, never publish a converted v2 artifact
or promote it to authoritative.
Recapturing an authorized live run is the migration to complete evidence; replay
itself never performs that run. Old binaries reject v2. New readers report v1
as inconclusive without re-execution or displaying its value-bearing contents:
only fixed `kind: inconclusive` and `reason: REPLAY_LEGACY_EVIDENCE` fields.
Legacy `--trace` retains its current format and behavior, including the known
alias gap; format compatibility is not a new safety guarantee. Existing
external consumers are unknown; release notes must advertise the stricter CLI
success/exit contract before rollout. Rollback can still inspect v1 but cannot
make an old reader understand v2.

Before replay, independently apply the publication classifier to all supplied
sources/configuration and the complete artifact, regardless of its eligibility
claim. Classified secret data returns the same fixed rejection shape as
recording, with code `REPLAY_SECRET_MATERIAL` and fixed message
`Replay rejected: classified secret material`, before re-execution or any
value-bearing report. Missing classification evidence is inconclusive without
re-execution. A fabricated completion flag cannot bypass this gate.

Proposed new replay output contract, to accept explicitly:

| Result | Guarantee and CLI behavior |
|---|---|
| `kind: success`, `equivalence: verified`, exit 0 | All bindings and evidence eligible; every observable comparison passes. Include recorded/replayed terminal status separately, so reproducing a recorded workflow failure can be successful verification. `requestsChecked` includes failed sent attempts; add attempts/decisions/outputs checked counts. |
| `kind: inconclusive`, exit 2 | Well-formed legacy, incomplete, lossy, redacted-dependent, or unsupported evidence. Report only `kind` and a fixed reason enum; no user-derived values, checked-value previews or re-execution. The MVP writer does not publish such v2 recordings. |
| `kind: error`, exit 1 | Malformed/inconsistent artifact, actual binding/request/decision/output mismatch, missing required supplied file, or replay infrastructure failure. Include a safe location and stable code. |

Keep existing `RUNTIME_REPLAY_REQUEST_MISMATCH`,
`RUNTIME_REPLAY_TRACE_EXHAUSTED`, `RUNTIME_REPLAY_RESPONSE_MISSING`, and
`REPLAY_REQUEST_COUNT_MISMATCH` meanings. Propose separate codes:
`REPLAY_EVIDENCE_INVALID` for malformed/inconsistent evidence,
`REPLAY_BINDING_MISMATCH` for supplied content/configuration drift,
`REPLAY_ATTEMPT_MISMATCH`, `REPLAY_DECISION_MISMATCH` (including criterion drift),
`REPLAY_OUTPUT_MISMATCH`, and `REPLAY_OUTCOME_MISMATCH` for observed divergence.
Inconclusive reasons are `REPLAY_LEGACY_EVIDENCE`, `REPLAY_PAYLOAD_INCOMPLETE`,
`REPLAY_PAYLOAD_LOSSY`, `REPLAY_SECRET_DEPENDENCY`,
and `REPLAY_SEMANTICS_UNSUPPORTED`. These names and the publication-gate codes
above are proposals, not shipped codes. Missing data and an actual mismatch must remain
different outcomes. No stable existing code is repurposed.

The CLI verdict owner updates `ReplayOutput`,
[replay.schema.json](../../docs/schemas/replay.schema.json), help, snapshots, and
schema-generation tests together. The current
[schema drift test](../../crates/arazzo-cli/tests/schema_drift.rs),
`schema_replay_matches_checked_in_file`, covers replay output; it does not cover
the manually maintained trace-v1 artifact schema. Add explicit v1/v2 artifact
contract validation, including rejection by legacy readers and no silent
authority promotion. CLI persistence retains atomic publication; a serialization,
limit, or write failure cannot publish a complete authoritative artifact or
return recording success. Preserve v1 behavior independently.

## Owners and smallest implementation sequence

After acceptance, reconcile existing tickets through `tkt`; this assessment
creates none and changes no ticket. Each unit below has at most one new
behavior-owning module; focused test targets are verification, not new runtime
owners. Contracts between units must be settled before their tickets dispatch.

| Order / unit | Owned change and consumable boundary |
|---|---|
| Prerequisite: existing URL redaction | Keep [ac-d95a9](https://sonos.scapedeck.com/docs/ac-tickets/ac-d95a9) as the userinfo owner. No duplicate sanitizer. |
| 1. Runtime comparison | Rewrite [ac-72ba1](https://sonos.scapedeck.com/docs/ac-tickets/ac-72ba1) around its one `replay_projection.rs` Create and validator. Retain classified comparison, drift negatives and v1 wire preservation. Move CLI persistence/verdict integration to its owners. |
| 2. Runtime recording | One evidence model/collector module owns versioned payload facts, invocation/attempt IDs, typed outcomes, terminal evidence and monotonic classification facts. No value-bearing spool or early publication. Narrow wiring in existing HTTP/engine/parallel paths retains failed attempts; no replay branch in ordinary execution. Requires the accepted resource-limit policy. |
| 3. Runtime document observations | Existing `document_set.rs`/builder own manifest observations and configuration provenance; zero new resolver modules. Expose a consumable observation surface, including all supplied documents. |
| 4. CLI persistence | One `trace_v2.rs` owner enforces the whole-run publication gate, then serializes eligible evidence atomically. Rejected runs write nothing, including temporary evidence or projections. Owns v2 artifact schema, v1 compatibility fixtures and migration docs; depends on 1–3 and the resource contract. |
| 5. Runtime replay | One versioned replay-session owner accepts evidence in memory, checks eligibility, consumes exact attempts, injects outcomes and returns structured comparison results. Existing client connects that session to the shared response decoder; no filesystem or network fallback in this owner. Depends on 1–3; 4 supplies persisted round-trip verification. |
| 6. CLI verdict | One `replay_verdict.rs` owner integrates loading/binding/results, new flags, exit policy and output schema, including gating all value-bearing `--record`/replay reports. Depends on 4–5 and proves the full matrix below. |

This explicitly changes the two unconditional authenticated-success commitments
in [ac-72ba1](https://sonos.scapedeck.com/docs/ac-tickets/ac-72ba1): passing its
request projection can no longer mean faithful execution with redacted inputs
or permission to publish a v2 artifact. Steve must accept the whole-run rejection
and verified versus inconclusive behavior before those criteria
are rewritten. Its unchanged-v1-schema commitment remains valid for the
projection repair; new authoritative schema work belongs to unit 4. None of
these proposed units is implementation-ready.

Use focused CLI replay/recording regression targets rather than adding concerns
to the logged `cli_integration.rs` God file
([god-files.md](../../god-files.md)). Amend both existing tickets' test placement
when authoring their executable handoffs; do not move their whole test suite or
grow `engine_impl.rs` with a second orchestration system. Existing runtime tests
remain compatibility consumers. The v1 public trace models, CLI trace writer,
runtime builder/replay API, and event consumers need explicit compatibility
checks; absence of an in-repository consumer is not proof of external absence.

## End-to-end acceptance matrix

Capture with the CLI; eligible cases validate the persisted artifact, stop the
fixture server, then replay. Rejected captures assert that no destination,
temporary evidence or sidecar is created, a pre-existing destination is
byte-identical, and stdout/stderr contain only the fixed rejection report.
An in-memory runtime-only test is insufficient.

| Scenario | Required result |
|---|---|
| >2048-byte JSON, including a marker beyond the preview | v2 retains complete eligible bytes and verifies identical marker; v1/default preview is inconclusive. A cap hit is explicit recording failure/ineligibility, never null success. |
| Complete JSON under JSON, text/plain and missing content type | Same live/injected decode and outputs; exact original bytes preserved before parsing. Include whitespace minification and invalid-JSON negatives. |
| Authorization/Cookie, sensitive query and nested request fields, userinfo | `--record` fails with the fixed secret-material report and no v2 artifact. Legacy projection unit tests still pass only classified locations; a non-sensitive `[REDACTED]` literal never permits drift. |
| Same-payload alias: `{token: sentinel, label: sentinel}` | Detecting `token` rejects the whole capture, including `label`, all earlier records and diagnostics. No redacted partial response or manifest survives. |
| Later-request/output alias: token copied into `display`, a request key, criterion, decision, step/final output or error | Detection at the original source permanently rejects publication, regardless of aliases' later keys or encodings. Check every artifact/report sink and late discovery after several otherwise eligible steps. No re-execution follows rejection. |
| Secret in inputs, root/referenced docs, retrieval URI, config or manifest | Reject before publishing any source digest, identifier or manifest field. Exercise each surface independently, including a secret discovered only in the final result. |
| Binary/invalid UTF-8, known empty body, missing body | Unclassifiable binary fails recording with no v2 artifact; known empty is eligible; missing/truncated/lossy reader evidence is inconclusive without re-execution. A future approved binary mode must round-trip every byte. |
| Connection failure then retry; terminal network failure; body-read failure | Every attempted/sent request and original error kind accounted for; retry decision and terminal outcome match. Delete, reorder, duplicate, or renumber evidence and require rejection. |
| Root and referenced docs, provided unclaimed docs, relocation | Matching bytes and bindings pass offline; same-URL changed bytes reject. Missing files reject without retrieval. Relocation preserves semantic bindings or rejects. |
| Request, criteria/decision, step output, final output and terminal drift | Independent mutations each reject with the owning code; equal request counts never excuse drift. Include repeated child invocations and eligible parallel execution identities. |
| v1/new/unknown versions and tampered completion markers | v1 emits only the fixed legacy-inconclusive report; old reader rejects v2; unknown versions and inconsistent v2 reject. Secret-bearing v2 rejects before re-execution even when labeled complete. No v1 conversion is published. |

Future implementation must run repository build/drift gates and focused surface
proof, then independent review of each immutable candidate. This documentation
deliverable needs link/diff/scope checks and independent decision review, not a
full Cargo suite. Proposed verdicts have not been implemented or verified.

## Decisions still required from Steve

1. Accept the separate v2 artifact/runtime surface and `--record` migration,
   instead of additive-v1 authority or a diagnostic-only product.
2. Accept the stricter default success contract, exit-2 inconclusive result,
   failure-reproduction success semantics, proposed codes, and preserving
   case-insensitive method comparison.
3. **Accept or reject the whole-run publication rule:** classified secret-bearing
   or unclassifiable data makes `--record` fail with no v2 artifact and only the
   fixed rejection report; no partial projection, inspection artifact or
   re-execution is offered. Recommendation: accept this bounded MVP. Rejection
   leaves v2 blocked on a separately accepted security/provenance design; it
   does not authorize selective redaction as a substitute.
4. Accept the owner partition and reconcile the two existing redaction tickets,
   resource prerequisites, schemas and public API contracts before rewritten
   tickets receive transfer review and warning-level lint.

Independent review may establish that this decision package is sound; it cannot
accept these choices on Steve's behalf or certify replay fidelity as delivered.
