# Jentic workflow corpus: bounded offline survey

Date: 2026-10-18. Disposition: stop expansion after the first survey and a
three-document independent quality sample. Subsequent authorization produced
one epic, three new Tier 2 tickets and one updated existing ticket, as recorded
below. The survey made no product changes or live API calls; subsequent
authorized implementation and local commits are recorded under Bounded
execution outcome. No public workflow execution or account setup occurred.

## Assessment

This is useful noisy input for parser/validator robustness, but it is not a
trustworthy collection of executable workflows or a semantic test oracle.
The bounded review did not establish one fully correct, useful multi-step
workflow. The sample contains plausible descriptions attached to broken data
wiring and invented output fields. Repairing hundreds of these documents would
turn this into a workflow-curation project rather than focused CLI improvement.

The user requested an epic and at most ten initial Tier 2 tickets, then added a
stop condition if the workflows proved meaningless. After the quality findings,
the user explicitly authorized a bounded epic and Tier 2 tickets for confirmed
CLI bugs and a small negative-fixture set, while ruling out further surveying
or repair of Jentic workflows. The four-ticket slice below implements that
ticket-authoring instruction. The CLI findings remain useful independently of
this corpus.
The user clarified that the large upstream issue backlog is automated and that
low quality is expected from these AI-generated workflows. Issue volume is not
used as quality or maintenance evidence here; no exhaustive issue review was
undertaken. The assessment rests on the observed document and data-wiring defects.

## Snapshot and method

- Upstream: `jentic/jentic-public-apis`, commit
  [`290ca550fd1ca326400712aa2493c6e35fd03b63`](https://github.com/jentic/jentic-public-apis/tree/290ca550fd1ca326400712aa2493c6e35fd03b63/workflows),
  committed 2026-10-18 01:03:24 UTC.
- Complete workflows subtree: `bdcf009c1e2a24d6525a89304c6091187b314ab7`.
  GitHub's whole-repository recursive tree was truncated, so workflow enumeration
  used the separate, non-truncated subtree. No absence conclusion relies on the
  truncated tree.
- CLI: source HEAD `aa58c2d59bc5ed2fb7609837162d73b834ad8a29`, version 0.8.2,
  built using `cargo build --offline --locked -p arazzo-cli`. Existing local
  `Cargo.toml` changes only disable development debuginfo and were preserved.
- Binary SHA-256:
  `8112004e304100a9a37f468b3c9f361bb7876dd7c322c47ed85ea950bfba41ba`.
- All 381 workflow files downloaded at the immutable commit and verified against
  their Git blob SHA-1s; SHA-256s are retained in the accompanying dated record.
- Each original file was passed to `validate <absolute-path> --json`, both
  normally and with `--strict`, from an empty working directory. No workflow ran.
- Source descriptions were audited separately. The validator's
  `arazzo_validate::parse_with_diagnostics` reads the local document and does not
  load its OpenAPI sources. Missing source files are therefore not counted as
  parser failures or an undocumented promise of structural validation.
- 353 candidate OpenAPI documents were downloaded from the same commit; 28
  candidate paths returned HTTP 404. These are acquisition results. All subsequent
  parsing, validation and source/operation analysis was offline.
- Vendored Arazzo 1.0.1, 1.1.0 and OpenAPI 3.2.0 checksums passed
  `./spec/fetch.sh --verify`.

Raw evidence and temporary survey scripts are retained at
`/private/tmp/jentic-arazzo-survey-20261018/`. The compact durable record is
[jentic-corpus-survey-2026-10-18.json](jentic-corpus-survey-2026-10-18.json).
Temporary acquisition scripts are not a supported repository test harness.

## Validation results

| Measure | Result |
|---|---:|
| JSON documents parsed | 381 / 381 |
| Declared Arazzo version | 1.0.1 in all 381 |
| Workflows / steps | 1,999 / 6,085 |
| Default validation pass / fail | 269 / 112 |
| Strict validation pass / fail | 269 / 112 |
| Warnings, crashes, timeouts, non-JSON result envelopes | 0 |

Failure groups overlap; document counts below must not be summed.

| Diagnostic family | Documents | Diagnostics | Classification |
|---|---:|---:|---|
| `unsupportedOperationPath` | 89 | 1,205 | Invalid source syntax |
| `invalidIdentifier` | 22 | 353 | Invalid output keys |
| `invalidExpression` | 31 | 326 | Invalid expression/reference syntax |
| `duplicateIdentifier` | 3 | 9 | Duplicate step IDs |

Representative distinct causes:

- Apacta uses `openapi_source#/paths/...` instead of an expression identifying
  the source description. This is not a 1.1-only restriction: the 1.0.1 Step
  Object says, "A Runtime Expression syntax MUST be used to identify the source
  description document."
- Autotask output keys include `items[0].id`. The 1.0.1 Step Object requires keys
  to match `^[a-zA-Z0-9\.\-_]+$`; array brackets are not allowed in the key.
- Devopness uses `$response.body#access_token` (missing the pointer slash),
  Listen Notes uses `$response.headers.*` (plural), and Discord uses dot-style
  body traversal. The 1.0.1 runtime-expression grammar defines
  `header-reference = "header." token` and
  `body-reference = "body" ["#" json-pointer ]`.
- Ticketmaster Discovery repeats a step ID within a workflow. The 1.0.1 Step
  Object says the step ID "MUST be unique amongst all steps described in the
  workflow."

Normative owners: [Arazzo 1.0.1](../../spec/arazzo/v1.0.1.html), anchors
`step-object`, `runtime-expressions`, and `format`; compare
[Arazzo 1.1.0](../../spec/arazzo/v1.1.0.html). No confirmed CLI false rejection
was identified in this bounded survey. This is not an exhaustive conformance
certification of every diagnostic.

## JSON support: confirmed CLI defects

Arazzo 1.0.1 §4.2 says a description "may be represented either in JSON or YAML
format." JSON already parses through the current YAML deserializer.

Using the unchanged Canada Holidays JSON bytes:

| Surface | Observed result | Owning source |
|---|---|---|
| `validate file.json --json` | Pass | `arazzo-validate/src/lib.rs::parse_bytes_with_diagnostics` |
| `list file.json --json` | Two workflows | `arazzo-cli/src/handlers.rs::list_workflows` |
| `catalog json-only-directory --json` | Empty array | `handlers.rs::is_arazzo_yaml_path` |
| `show <id> --dir json-only-directory --json` | `SHOW_WORKFLOW_NOT_FOUND` | `handlers.rs::find_workflow` |
| Same bytes, filename ending `.arazzo.yaml` | Catalog and show succeed | Same parser, different filename filter |
| `test file.arazzo.json --json` | `TEST_PATH_NOT_ARAZZO` | `test_runner.rs::discover_test_specs` |
| `test json-only-directory --json` | `TEST_NO_SPECS` | `arazzo-mcp/src/state.rs::discover_specs` |

The shared recursive scanner also governs MCP/CLI `serve` directory discovery;
its JSON omission was confirmed in source, not through a live MCP session.
CLI help calls input a YAML spec despite direct JSON support. Generation is a
separate contract and explicitly writes YAML; that is not a finding here.

The initial probe mistakenly included unsupported `test --dry-run`; those
results establish only an argument error and were excluded. Corrected probes
above rejected at discovery before any execution or API request.

Backlog check found an existing owner for explicit-file suffix rejection:
[ac-9b658](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9b658). Its current
directory-only-YAML acceptance wording was reconciled when the user authorized
this bounded slice. No duplicate was created. The existing
[portable execution corpus design](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-14446)
is broader than this validation survey and was not changed.

## Version and source-resolution separation

Changing only the version to 1.1.0 in temporary copies left all 381 exit outcomes
and diagnostic lists identical. Originals were untouched. There were no observed
version-gated failures; this collection does not establish 1.1 feature coverage.

377 documents use `./apis/openapi/...` while living inside `workflows/<vendor>/`.
Under the 1.0.1 relative-reference rule, "relative references are resolved using
the URL of the referring document." They therefore point below the workflow
directory, where the complete subtree proves the referenced files do not exist.
One WhatsApp document uses an absolute `/apis/...` path; three documents use
mutable GitHub `main` URLs, so their original source declarations are not pinned.

For diagnosis only, checking the relative/root-absolute paths from repository
root, and replacing `main` with the snapshot commit in remote candidates, found
353 source files and 28 absent ones. This was an explicitly labeled analysis of
likely intended locations, not a runtime fallback or a correction to the corpus.

In those 353 candidates, direct OpenAPI operation indexing checked 4,523 step
operation-ID references: 4,454 unique, 63 ambiguous across 14 documents, and six
absent across two documents (Loket and Slack). These are additional source/data
defects; structural validation passing does not establish that targets resolve.
Unretrieved sources and `operationPath` targets were not included in those counts.

## Workflow quality and account-free candidates

A focused independent reviewer inspected three documents and their pinned API
schemas. None was established as a fully correct useful multi-step workflow.

| Sample | Meaningful intent | Why the supplied workflow is unreliable |
|---|---|---|
| Canada Holidays | Compare provincial holidays | Calls the same province with identical inputs twice and exports only the second result; no comparison occurs. |
| Color Pizza | Look up a color and related names/colors | Assigns the entire response object to scalar name/hex outputs, then feeds those objects into string query parameters. It also has invalid operation-path syntax. |
| Parliament Commons Votes | Search, select a division ID, retrieve its details | A real chain exists, but output selectors invent fields: `divisionDetails` is absent from `PublishedDivision`; the count endpoint returns an integer, not an object containing `totalCount`. |

Additional bulk signals, not independently adjudicated findings: all 6,085 step
success checks use `$statusCode == 200`; 4,936 of 10,737 step outputs select the
whole response body. A conservative lexical scan flagged local step-output name
mismatches in 200 documents, including 145 of the 269 structural passes. Many
look like dot-based object traversal accidentally embedded in an output name.
These are triage signals, not 1,428 separate CLI bugs or a complete correctness
count. Further reference-validation work overlaps existing expression/dependency
work and was not ticketed from this scan.

**Authentication:** all three sampled API descriptions declare no root/operation
security requirements or security schemes. That makes them candidates for
account-free testing, but does not make the supplied workflows correct. No live
call verified current account requirements or availability.

The providers offer useful public APIs independently of Jentic's workflows:

- [Canada Holidays API](https://canada-holidays.ca/api) documents a free API;
  its pinned description declares no authentication. A small deliberately
  authored two-province fixture would be more useful than this supplied flow.
- [UK Parliament's developer hub](https://developer.parliament.uk/) lists Commons
  Votes as a supported public-data API; the pinned description declares no
  authentication. The search-to-detail idea is worth retaining after correct
  response extraction is independently authored and checked.
- [CIRCL hashlookup](https://new.circl.lu/services/hashlookup/) documents public,
  free lookups with credential-free request examples. Its supplied Jentic
  SHA-1 flow similarly feeds a whole response object into a hash path parameter.

Missing OpenAPI `security` alone is not proof that an arbitrary service needs no
account: descriptions can be incomplete, or credentials can appear as ordinary
parameters. No corpus-wide account-free claim is made.

## Stop decision

Do not start a broad import, repair campaign, or pass-rate target. Keep this
report and the real JSON-discovery reproductions. The authorized tickets cover
four representative negative fixtures and the independent JSON issues. Useful
positive execution examples would need to be independently authored and checked;
that work is outside this slice. Ordinary tests remain hermetic.

## Authorized ticket slice

Epic: [epic:ac-f2957](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-f2957),
**Close confirmed JSON discovery gaps and preserve four negative corpus cases**.

| Ticket | Scope | Disposition |
|---|---|---|
| [ac-f5d91](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-f5d91) | JSON discovery in catalog/show and direct-input help | New Tier 2 bug, P2 |
| [ac-7af90](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-7af90) | Shared recursive `.arazzo.json` discovery for MCP and CLI test/serve | New Tier 2 bug, P2 |
| [ac-9b658](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9b658) | Explicit-file test selection regardless of suffix | Reused and updated Tier 2 bug; existing P3 preserved |
| [ac-3dbca](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3dbca) | Four minimized negative JSON fixtures with passing controls | New Tier 2 task, P2 |

The tracker permits one owning epic per ticket. The reused explicit-file ticket
retains [epic:ac-40dcd](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-40dcd)
as owner and is referenced from the new epic. The new epic explicitly requires
checking its completion before closure; the reference itself is non-blocking.
Its legacy file metadata was selectively migrated through `tkt` for that one
ticket, narrowing the existing broad test scope to a focused new test file.

No additional corpus survey or upstream repair was performed. At the authoring
handoff, negative fixtures were ticketed but not yet implemented. Overlapping
portable-execution-corpus and broader conformance tickets were left unchanged.

Ticket-authoring handoff checks: **Lint PASS** at warning severity for the epic and four tickets;
**implementation readiness PASS** after independent review of the four ticket
designs; **queue eligibility PASS** for all four implementation tickets in
`tkt ready`; **start clearance NOT ASSESSED**, since implementation was not
requested and live checkout/reservation checks belong to kickoff. No ticket had
been started at that handoff; the subsequent authorized execution follows.

## Bounded execution outcome

Steve subsequently authorized executing this epic with sized agents and focused
tests. All four tickets completed, one writer at a time, with a fresh independent
review of each immutable candidate. The referenced explicit-file ticket retained
its original epic ownership and completed before the new epic's final member.

| Ticket | Reviewed local candidate | Result |
|---|---|---|
| [ac-9b658](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9b658) | [56b3234](https://github.com/strefethen/arazzo-cli/commit/56b32345c34daac951f2f061a725f702feca2ee4) | Explicitly named JSON/YAML documents reach the parser regardless of suffix. |
| [ac-f5d91](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-f5d91) | [73c2c40](https://github.com/strefethen/arazzo-cli/commit/73c2c404e37bccd53796368749dd56db6d65a2e2) | Catalog/show discover JSON; direct-input help reflects supported formats. |
| [ac-7af90](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-7af90) | [c124f79](https://github.com/strefethen/arazzo-cli/commit/c124f793e8c9c0f7bba61842836c7d9562cc96c2) | Shared recursive discovery includes `.arazzo.json`, with MCP/CLI regression coverage. |
| [ac-3dbca](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-3dbca) | [6f04e27](https://github.com/strefethen/arazzo-cli/commit/6f04e277d29e9e1a8d23d6057adc3d9d811976e9) | Four minimized negatives have pinned provenance and passing controls. |

These commits are local and were not pushed. On the final source candidate,
combined verification passed all 12 tests in the four changed CLI integration
targets and all four MCP JSON-discovery tests. The negative-fixture test includes
16 rejection and 16 passing-control checks across two versions and two modes.
Per-ticket checks, named compatibility guards, formatting, and final workspace
clippy passed. Tests stayed focused; the full workspace test suite was not run.
Local HTTP fixture tests used approved sandbox escalation solely to bind localhost.

The epic auto-completed after the last member closed. Contract/start snapshots,
initial file and Git baselines, AC/test mappings, raw logs and independent review
results are retained under `/private/tmp/arazzo-epic-f2957/`; durable ticket notes
identify each reviewed contract and candidate. No further survey, upstream repair,
live public-API workflow execution, dependency change, push or release occurred.
Unrelated release/version/debug-profile and assessment work was preserved.
