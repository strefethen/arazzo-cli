# Arazzo CLI 0.9.0 — morning review

**Date:** 2026-10-18 (America/Los_Angeles)  
**Original preparation baseline:** `aa58c2d59bc5ed2fb7609837162d73b834ad8a29` on `main`  
**Original candidate:** uncommitted version/changelog preparation on that baseline  
**Disposition:** **HOLD — release preparation is complete; release preflight is red.**

The candidate has enough release content. All build, test, formatting, lint,
schema, extension, and release-binary smoke checks passed. The final preflight
stage, `cargo audit`, failed on four advisories for the locked `rustls 0.23.45`.
The eight High findings from the October 17 audit also remain present: seven
were reproduced with bounded synthetic probes; one was rechecked in source.

The original preparation run performed no tag, publication, push, commit,
dependency update, production-code repair, or ticket-state change. The follow-up
below records the subsequently requested ticket work. Steve's review owns the
remaining release-risk decisions. The prepared files remain uncommitted.

## Follow-up: release-blocker routing

**Triage baseline:** `d55bfb8a1f15723988cd67a470bc1aecca76211e` on `main`.
This section updates ownership and next actions after Steve requested Tier 2
tickets. The preparation results above and verification below remain evidence
for `aa58c2d`, not a passing preflight for the newer checkout. Five intervening
commits added explicit test-path handling, JSON discovery, and Jentic evidence.
Final release scope/changelog and preflight must be reconciled on the eventual
candidate. No runtime implementation was performed during this triage.

Created [epic:ac-abcd5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-abcd5),
**Own newly identified work from the 0.9.0 blocker triage**, with these five
Tier 2 children:

| Owner | Deliverable | Priority | Release relevance |
|---|---|---|---|
| [ac-d3602](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d3602) | Targeted rustls 0.23.46 update and fresh canonical preflight | P1 | Clears the currently failing RustSec gate; no advisory suppression |
| [ac-5eb26](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-5eb26) | Cancel-aware private debug gates and guaranteed DAP cleanup | P1 | One implementation outcome for L1/L2; adapter distribution, separate from the CLI asset |
| [ac-704a6](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-704a6) | Decision-ready live-request admission assessment | P1 | L4 needs an accepted missing/explicit-null/selector/error contract before a code-changing ticket |
| [ac-2406c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2406c) | Decision-ready credential-vocabulary assessment | P1 | Owns only the part of L5 beyond the existing normalization/schema-secret contract |
| [ac-9394b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9394b) | Decision-ready form request-body serialization assessment | P2 | L3 has a proven flat-mapping defect; full compound/null encoding rules need a precise contract |

The first two are implementation tickets; the other three authorize an
assessment artifact only. They do not accept new defaults, redaction stems,
null policies, errors, or OpenAPI-metadata coupling. Completing the assessments
requires explicit acceptance before their corresponding behavior can change.

### Existing owners retained

- **L5 normalization/schema-input redaction:**
  [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51).
  Its legacy absolute-path declarations initially failed lint. A reviewed,
  single-ticket `files` migration with a durable recovery journal converted
  only this ticket. Its refined handoff now names `run.rs::run_workflow`, a
  focused CLI integration test, and operation-aware repo-relative paths.
  Goal/Acceptance Criteria and the no-new-stems rule were preserved. The stale
  negative expectation for `apikeyless` was corrected: current substring matching
  already treats it as sensitive, so it is a positive preservation control.
  The ticket now passes warning-level lint. Broader `key`/`X-Auth-Key` policy
  remains with the new assessment, not this repair.
- **L6 MCP file exposure:**
  [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8) remains the
  maintainer policy gate. The existing file access and error-projection children
  remain blocked on that contract. No policy was silently chosen or duplicated.
- **L7 generator resource limits:**
  [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f) already has
  Steve's bounded-walker decisions and remains the implementation owner.
- **L8 sub-workflow parameter resolution:**
  [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4) remains
  blocked on its active prerequisite; no duplicate parameter-resolver ticket.
- **Dry-run provenance:**
  [ac-fda34](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34) retains its
  narrower scope and is referenced by the live-request assessment.

Existing owners are nonblocking references on the new epic; none was moved
between epics or given a duplicate implementation child. Open-backlog theme
searches and an open GitHub rustls-PR check found no existing owner for the two
new repairs or the three bounded assessments.

### Handoff gates and order

| New Tier 2 tickets | Lint | Implementation readiness | Queue eligibility | Start clearance |
|---|---|---|---|---|
| All five | PASS at `--severity warn` | PASS for each stated deliverable; the three assessments do not authorize production fixes | PASS; all five appear in `tkt ready` | NOT ASSESSED for execution; no kickoff or reservation acquisition was requested |

The RustSec update is the first bounded implementation candidate because it
clears a measurable failing preflight gate. Next, resolve the live-request and
credential-classification assessments and the existing MCP policy gate; these
are the material safety decisions. The generator owner already has a settled
size limit and can be considered for a separate authorized implementation start.
The debugger repair is separately actionable for an adapter release.

At triage time there were no active reservations. `tkt ready` also reported
unreserved dirty release-preparation files; queue eligibility is not checkout
clearance. Future execution must preserve/attribute that WIP and recheck leases.
New repair tickets both forecast `CHANGELOG.md`; serialize their source writes
in this checkout. No implementation, dependency update, tag, publication, push,
Git commit, or ticket lifecycle start/close occurred during this triage.

Ticket creation/refinement results and the selective redaction migration's
reviewed manifest/recovery journal are retained under
`/tmp/arazzo-0.9.0-release-20261018/`. The original probe evidence snapshot is
unchanged.

## Prepared release (original preparation run)

- Root workspace version is `0.9.0`; Cargo regenerated the eight workspace
  package versions in `Cargo.lock`. No dependency version changed.
- `CHANGELOG.md` has a dated `0.9.0` section with a lede, Upgrade if line,
  highlights, the existing unreleased changes, and migration guidance for all
  three breaking changes. It also includes the previously omitted `quinn-proto`
  update and latest debugger work.
- Both debug and release CLI binaries report `arazzo 0.9.0`.
- The pre-existing `[profile.dev] debug = false` edit remains in `Cargo.toml`.
  The pre-existing `carried-baggage.md`, October 17 audit, and October 18 Jentic
  survey files were preserved.
- Only version metadata, the generated lockfile versions, release prose, and
  this dated evidence were prepared. These introduce no new runtime behavior;
  an independent implementation review was therefore not dispatched.

The migration prose describes existing changes rather than authorizing a new
specification feature. Its specification authority is the vendored Arazzo 1.1.0:

- [Step Object](../../spec/arazzo/v1.1.0.html#stepOperationId): “The name of an
  existing, resolvable operation” and “existing within one of the
  `sourceDescriptions`.”
- [Step Object](../../spec/arazzo/v1.1.0.html#stepOperationPath): “A reference to
  a Source Description Object combined with a JSON Pointer to reference an
  operation.”
- [Reusable Object](../../spec/arazzo/v1.1.0.html#reusableObjectReference):
  “REQUIRED. A Runtime Expression used to reference the desired object.”

## Verification

Toolchain: Rust/Cargo `1.98.1`; cargo-audit `0.22.1`. This is local macOS
verification, not proof of cross-platform CI or a packaged VSIX acceptance run.

| Check | Result |
|---|---|
| `cargo build --workspace` | Passed; regenerated workspace lockfile versions |
| Private workspace publish guard | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace` | **1,350 passed**, one existing ignored runtime documentation example |
| Explicit `schema_drift` rerun | **9 passed** |
| Extension `npm run lint` / `npm run build` | Passed |
| Release CLI build and validate/list/dry-run smoke checks | Passed |
| Extension `npm test` (additional to preflight) | **28 passed** |
| Changelog/workspace version pairing and `git diff --check` | Passed |
| `cargo audit` | **Failed: four rustls advisories**, plus two warnings |

The first sandboxed preflight attempt stopped at a hermetic test's denied
loopback bind (`Operation not permitted`). The complete rerun outside that
sandbox passed the workspace tests and reached the advisory failure. No tests
were disabled or baselines refreshed to obtain the pass.

### Security gate

The fetched RustSec database reported these against locked **rustls 0.23.45**;
all four list **`>=0.23.46`** as patched:

| Advisory | Audit title | Reported severity |
|---|---|---|
| [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346) | Debug impls for client session values expose TLS secrets | 4.0, Medium |
| [RUSTSEC-2026-0347](https://rustsec.org/advisories/RUSTSEC-2026-0347) | Buffered application data sent to outer server after ECH rejection | 2.3, Low |
| [RUSTSEC-2026-0348](https://rustsec.org/advisories/RUSTSEC-2026-0348) | Brotli certificate decompression can allocate up to 1 GiB before authentication | 3.7, Low |
| [RUSTSEC-2026-0349](https://rustsec.org/advisories/RUSTSEC-2026-0349) | Unauthenticated ServerHello can downgrade later connections to classical key exchange | 3.7, Low |

The RustSec text has configuration-specific preconditions. These are dependency
scanner findings, not four demonstrated exploits in this application. No
reachability exemption or audit suppression was applied. `rustls` is a direct
dependency of the runtime/CLI and also appears through reqwest's TLS stack.

The two non-fatal warnings were unmaintained `unsafe-libyaml 0.2.11`
([RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449)) and yanked
`chacha20 0.10.1` through rand/quinn-proto. They are separate from the four
vulnerabilities that failed preflight. The release is held pending disposition
of the security gate and a fresh successful preflight on the resulting candidate.

## High audit triage

**Total: eight live High findings; zero closed by this preparation.**
Highest-risk owners are HTTP request construction, redaction, MCP file-tool
error projection, generator example expansion, and debugger teardown.
The top five release risks are L4, L6, L5, L7, and L1.

All eight mechanisms were present at `v0.8.2`, based on source comparison;
these probes establish current behavior, not an old-binary regression test.
The latest debugger commit did not resolve L1/L2. The constant child-parameter
fix resolves a different case from L8's templated containers.

| Finding | Current evidence | Recommended release disposition | Tracking checked through `tkt` |
|---|---|---|---|
| L1: DAP disconnect hangs at a later breakpoint in the same step | Reproduced; single-breakpoint control exits | Hold any adapter release; separate decision for a CLI-only binary release | No dedicated owner found |
| L2: EOF while paused leaves DAP alive | Reproduced; resume-before-EOF control exits | Same adapter-release hold as L1 | No dedicated owner found |
| L3: form mapping sent as JSON | Reproduced on loopback | Known form/OAuth correctness defect; explicit deferral required if shipping | No dedicated owner found |
| L4: unresolved values change the live request target | Reproduced `DELETE /pets/`, exit 0 | **Hold CLI/runtime release: destructive-request risk** | No dedicated live-request owner found; dry-run ticket is narrower |
| L5: credential-name redaction gaps | Reproduced with fake canaries | **Hold CLI/runtime release: credential exposure** | Existing ticket covers only part of the finding |
| L6: MCP parse errors disclose file content | Reproduced using a synthetic scalar file | **Hold agent-facing MCP release: arbitrary readable-file disclosure** | Policy ready; implementation tickets blocked |
| L7: generator has no global output budget and quadratic string padding | Source confirmed; bounded 2,000-character control succeeds | **Hold unrestricted MCP example-generation release: resource exhaustion** | Existing owner ready |
| L8: templated child parameter containers become strings | Reproduced; literal-object control passes | Explicit deferral required; do not describe all container parameters as fixed | Existing owner blocked |

### L1 — DAP disconnect can hang on the next checkpoint

- **Category/severity/confidence:** concurrency; High; high, reproduced.
- **Where:** `crates/arazzo-debug-adapter/src/dap/session.rs::cleanup_runtime`
  (line 440), `crates/arazzo-runtime/src/debug/controller.rs::gate_step`
  (line 383), and HTTP response criterion/output gates.
- **Evidence:** teardown calls `token.cancel()`, one `force_resume()`, then
  `monitor.join()`. The gate waits in `while !guard.continue_permit`; it has no
  cancellation test. `force_resume` supplies only one permit.
- **Observed:** install verified breakpoints on both the success criterion and
  output of one loopback HTTP step; disconnect while stopped at the criterion.
  The adapter remains alive beyond the two-second probe deadline and is killed
  by the harness. With only the criterion breakpoint, disconnect exits 0.
- **Impact:** debugger disconnect can strand the adapter/engine. Whether VS Code
  eventually kills the process was not tested. The CLI release workflow packages
  only the CLI binary; the adapter is a separate distribution surface.

### L2 — EOF while paused does not tear down the runtime

- **Category/severity/confidence:** lifecycle; High; high, reproduced.
- **Where:** `crates/arazzo-debug-adapter/src/dap.rs::run_dap_stdio`, lines 75–103.
- **Evidence:** EOF sets `stdin_closed = true`; the loop exits only when
  `runtime.terminated`, otherwise sleeping and polling. The EOF path does not
  call `cleanup_runtime`.
- **Observed:** close stdin after a stop-on-entry event. The process remains
  alive beyond two seconds and is killed by the harness. Continuing before EOF
  lets the same workflow complete and the adapter exit 0.
- **Impact:** client loss while paused leaves the adapter running. This remains
  a debugger-surface release issue alongside L1.

### L3 — Form mappings are serialized as JSON

- **Category/severity/confidence:** correctness; High; high, reproduced.
- **Where:** `crates/arazzo-runtime/src/runtime_core/engine_http.rs`, lines 286–297.
- **Evidence:** only non-JSON **strings** return raw bytes; every mapping reaches
  `serde_json::to_vec(value)`.
- **Observed:** a POST labelled `application/x-www-form-urlencoded` contains
  `{"client_id":"synthetic-client","grant_type":"authorization_code"}`.
  The command exits 0 because the loopback server accepts it. A real form-only
  endpoint can reject that body.
- **Specification:** the vendored [Request Body example](../../spec/arazzo/v1.1.0.html#requestbody-object-examples),
  lines 1705–1715, explicitly presents “A Form Data example:” with that content
  type and a mapping payload. This finding concerns request-body serialization;
  it does not change the repository's OpenAPI parameter-metadata stance.

### L4 — Missing response data can target a collection URL

- **Category/severity/confidence:** request/data safety; High; high, reproduced.
- **Where:** `engine_http.rs::build_url_from_path`, lines 689–694;
  `runtime_core/payload.rs::value_to_string`, line 10;
  `arazzo-expr/src/resolution.rs::Resolution::into_public`.
- **Evidence:** missing projects to `Value::Null`, null becomes `String::new()`
  for a path value, and null query values are omitted.
- **Observed:** the first GET produces an absent-field output. The next step
  uses it for both `petId` and a query filter. The loopback server receives
  `DELETE /pets/`, with no filter; CLI exit is 0 and stderr is empty. Both
  recorded steps omit warnings.
- **Tracking boundary:** [ac-fda34](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-fda34)
  is **blocked** and covers composed **dry-run** provenance. It does not own
  this live-request safety problem. No dedicated live-request owner was found.

### L5 — Redaction still misses common credential names

- **Category/severity/confidence:** security; High; high, reproduced.
- **Where:** `crates/arazzo-runtime/src/runtime_core/redaction.rs::is_sensitive_key`,
  line 28 and its query/header/trace consumers.
- **Evidence:** `name.to_ascii_lowercase()` is compared with four exact names
  and ten `contains` stems; separators are not normalized.
- **Observed:** fake canaries remain visible in `api_key` and `key` query values
  and the `X-Auth-Key` header in dry-run JSON/trace output. `access_token` and
  `X-API-Key` controls are redacted.
- **Tracking:** [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51)
  is **ready** and covers separator normalization and schema-declared input
  secrets. Its explicit “Do not add new stems” boundary means it does not close
  all of L5, including generic `key` and `X-Auth-Key` vocabulary decisions.

### L6 — MCP file-tool errors can return file contents

- **Category/severity/confidence:** security; High; high, reproduced.
- **Where:** `arazzo-mcp/src/state.rs::check_path_allowed`, line 62;
  `arazzo-mcp/src/handlers.rs::describe_openapi`, line 469;
  `arazzo-generate/src/lib.rs::parse_openapi_file`, lines 11–16.
- **Evidence:** no configured roots returns `Ok(())`; typed parse errors are
  formatted with `{err}` and returned in the MCP tool result.
- **Observed:** `describe_openapi` on a synthetic scalar file returns
  `invalid type: string "SYNTHETIC_PARSE_ERROR_CANARY_090"` to the agent. A
  configured allowed-directory control denies the same outside-root file.
  No real credential file, special device, or FIFO was used.
- **Tracking:** [ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8)
  is a **ready maintainer policy decision**, with implementations
  [ac-452ce](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-452ce),
  [ac-79d9f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-79d9f), and
  [ac-62191](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-62191) **blocked**.
  No policy choice was inferred from permission to prepare a release.

### L7 — Example generation remains unbounded in width and string length

- **Category/severity/confidence:** resource exhaustion; High; high for the
  code mechanism, not a fresh large-scale benchmark.
- **Where:** `arazzo-generate/src/examples.rs::apply_length_constraints`,
  lines 325–329, and `generate_object_example`, lines 195–231;
  `arazzo-mcp/src/handlers.rs::generate_example`, line 492.
- **Evidence:** `while s.chars().count() < min { s.push('a'); }` rescans the
  growing string. The recursive walker has a depth guard but a fresh reference
  visited set on each call, no shared output budget, and no bound on required
  property fan-out. A depth bound alone does not bound total expansion.
- **Observed:** a bounded MCP request for a string of length 2,000 succeeds.
  No huge allocation, unbounded wait, or resource-exhaustion reproduction was
  attempted. The October 17 report's timing/memory measurements remain prior
  evidence, not new measurements from this run.
- **Tracking:** [ac-2763f](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2763f)
  is **ready**. The existing code has not shipped the bounded-walker design.

### L8 — Templated child parameter objects lose their type

- **Category/severity/confidence:** correctness; High; high, reproduced.
- **Where:** `arazzo-spec/src/lib.rs::Parameter::value_as_str`, line 533;
  `runtime_core/engine_impl.rs::execute_subworkflow_step`, line 815.
- **Evidence:** `value_as_str` serializes containers; the `contains("{$")`
  branch returns `Value::String(eval.interpolate_string(&value_str))`.
- **Observed:** parent input `id="42"` with parameter
  `config: {id: "{$inputs.id}"}` gives the child the **string**
  `"{\"id\":\"42\"}"`. Ordinary execution exits 0 with an object/string type
  warning. `--strict-inputs` exits 1 with `RUNTIME_SUB_WORKFLOW_FAILED` because
  the child expects an object. A constant `{id: "42"}` control stays an object
  and passes strict validation.
- **Tracking:** [ac-681d4](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-681d4)
  is **blocked**, currently on
  [ac-cd36b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-cd36b).
  The October 17 audit's blocker detail is stale:
  [ac-d3fbd](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-d3fbd) is now
  **closed/completed**. The ticket's current contract still covers the shared
  resolver path needed for this finding.

## Patterns and limits

Null projection and string conversion can erase the distinction between absent
data and a deliberate value at an HTTP boundary (L4/L8). Redaction and file
containment have incomplete security boundaries (L5/L6). Debug teardown resumes
one wait rather than guaranteeing that later waits cannot occur (L1/L2).
Generator recursion depth is not a total work/output limit (L7).

Controls checked clean: constant child object parameters retain type; recognized
credential names are redacted; an explicit MCP allowed-directory rejects the
outside-root synthetic file; single-breakpoint disconnect and resumed EOF exit
normally. None of these controls closes its corresponding broader finding.

This was a targeted recheck of the eight High findings, not another full audit.
Ticket reads and searches were read-only; no dedicated owner was found for
L1–L4 using the searched titles, goals, and mechanisms. That is a search result,
not proof that no broadly scoped ticket mentions related behavior.

## Morning disposition

The concrete candidate and migration notes are ready to review. Recommend
holding the tag until the RustSec gate has a clean result and Steve has resolved
the L4/L5 live-request and credential risks plus the L6/L7 MCP exposure decisions.
L3/L8 need explicit known-defect disposition if deferred. L1/L2 remain a hold for
adapter distribution; they do not imply the CLI release asset contains the
adapter binary.

Preparation did not grant authority to implement the audit's code or policy
changes. A successful preflight after future changes will still not by itself
prove these behavioral findings fixed.

The dated [evidence snapshot](arazzo-0.9.0-release-evidence-2026-10-18.json)
retains the bounded probe results and verification summary. Full logs, fixtures,
and the scratch reproduction harness are currently under
`/tmp/arazzo-0.9.0-release-20261018/` (`preflight-unsandboxed.log`,
`triage_probes.py`, `triage-results.json`, and `extension-tests.log`).
