# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Typed JSONPath — `type: jsonpath` success criteria, Selector Objects, and
`targetSelectorType: jsonpath` payload replacement targets — now executes as
RFC 9535 on one shared engine, replacing two handwritten subsets.

**Upgrade if** you use `type: jsonpath` anywhere: queries that previously
failed as "unsupported JSONPath" now run, filter and comparison semantics
follow the RFC, a criterion is decided by nodelist cardinality rather than
truthiness, and a declared `draft-goessner-dispatch-jsonpath-00` version is
rejected instead of quietly evaluated as something else.

- **One engine** — criteria, Selector Objects, and replacement targets share
  a single RFC 9535 query owner; both handwritten implementations are gone,
  with no engine-selection flag, alias, or fallback.
- **The whole query language** — recursive descent (`$..sku`), slices
  (`$.items[0:2]`), unions, negative indices, and the standard function
  extensions `length()`, `count()`, `value()`, `match()`, and `search()`.
- **RFC 9535 only** — an omitted version or `rfc9535` executes; every other
  declared JSONPath version, including the expired Goessner draft, is a
  permanent capability limit and is rejected before evaluation.
- **Explicit resource budgets** — conservative admission limits on query size,
  query structure, and context nesting, applied before parsing or evaluation.
- **Rust 1.88** — the minimum supported Rust version for the workspace.

### Added

#### Workflow Engine
- `arazzo_expr::JsonPathQuery` is the single owner of typed JSONPath: it
  admits the declared version and the resource budgets, validates the complete
  expression before any context is resolved, and runs one *located* query so a
  selected value and its RFC 6901 pointer always come from the same walk.
- `match()` and `search()` evaluate through an I-Regexp matcher and accept a
  literal pattern or a pattern drawn from the queried document. An invalid
  pattern is logical false; a resource or backend failure invalidates the whole
  query, including under negation, instead of degrading to false.
- Admission limits, checked before the parser and the evaluator run: 16,384
  UTF-8 bytes per query, 128 combined occurrences of the raw bytes `.` `[` `(`
  `!` `&` `|` in a query, and 128 nested containers in the queried context.
  Each rejection names the resource and the limit. These are explicit budgets,
  not a universal CPU, heap, or result-size quota, and this release does not
  claim 100% RFC 9535 Compliance Test Suite conformance.

### Changed

#### Workflow Engine
- Recursive descent and array slices no longer raise an "unsupported JSONPath"
  diagnostic on typed surfaces; they evaluate under RFC 9535 semantics, as do
  filters, unions, negative indices, and the standard function extensions.
- A declared JSONPath version other than `rfc9535` is rejected before
  evaluation: a criterion fails its step with an error, a selector resolves to
  `null` with one warning, and a replacement leaves the body unchanged with a
  warning. `validate` has no JSONPath version advisory, so such a declaration
  still validates as document metadata and is rejected only at run time.
- A JSONPath failure in a replacement value — including a nested Selector
  Object — skips that replacement and leaves the body unchanged, rather than
  writing `null`. Earlier replacements stay applied and later ones continue.
- The legacy GJSON-flavored dot-path traversal used by `$response.body...`
  runtime expressions is unchanged and remains an arazzo-cli extension. It is
  a separate code path; its syntax is rejected on every typed JSONPath surface.

#### Quality
- Raise the workspace minimum supported Rust version to 1.88.
- `serde_json_path` 0.7.2 is pinned with `serde_json_path_core` vendored under
  `vendor/serde_json_path_core` for one eight-line recursive numeric-equality
  repair; `PATCHES.md` records its provenance, checksum, and removal criterion.

### Fixed

#### Workflow Engine
- A `type: jsonpath` criterion is decided by nodelist cardinality, per Arazzo
  v1.1.0 §5.8.11.4.3: one or more selected nodes pass and zero fail, so a
  single node holding `false`, `0`, `""`, or `null` now passes instead of being
  read for truthiness. A null or undefined context still fails.
- A run of JSONPath delimiter characters in a criterion condition no longer
  panics the runtime; malformed expressions are reported as syntax errors.
- A payload replacement pointer with an empty name segment (`//child`) no
  longer collapses to `/child`, so it can no longer overwrite a root sibling.

## [0.6.1] - 2026-09-11

Path parameter substitutions reject dot-segment navigation, and 301/302
redirects preserve non-POST request methods and payloads.

**Upgrade if** your workflows use dynamic path parameters or encounter 301/302
redirects, especially on `PUT`, `PATCH`, or `DELETE` requests that must reach
their destination as writes.

- **Path parameter safety** — refuse substitutions that would navigate through
  a complete dot segment, including values supplied by a previous response.
- **Redirected writes** — non-POST requests retain their method, exact body
  bytes, and payload headers when following a 301 or 302 response.
- **Release notes** — each GitHub release includes its version's CHANGELOG
  section, and tagging refuses a version with missing release notes.

### Fixed

#### Security
- Refuse path-parameter substitutions that form complete `.` or `..` URL path
  segments, including encoded-dot seams and empty or adjacent substitutions.
  Validate the final URL after query assembly, before live dispatch, dry-run
  planning, or replay consumption, using `RUNTIME_INVALID_PARAMETER_VALUE`.
- Encode backslashes in path values as `%5C` to prevent injected path separators.
  Safe dotted names, literal percent escapes, and query/fragment behavior remain
  supported, including unresolved placeholders.

#### Workflow Engine
- A 301 or 302 response no longer converts `PUT`, `PATCH`, or `DELETE` to
  `GET`, which could silently skip a write while reporting workflow success.
  Non-POST requests keep their body and payload headers, including `GET`
  and `HEAD` requests with bodies. This restores the pinned reqwest redirect
  behavior. POST conversion, 303 handling, and 307/308 preservation are
  unchanged.

#### Quality
- Publish the matching CHANGELOG section as the GitHub release body and
  reject tag creation when that section is absent.
- Add hermetic regression coverage for non-POST redirect methods, exact
  body preservation, and 303 payload-header removal when `HEAD` is retained.

## [0.6.0] - 2026-09-06

Header parameters stop duplicating, `$self` becomes the base URI for relative
source references, and workflow inputs enforce `enum` members.

**Upgrade if** you set headers on steps — especially `Authorization` alongside
`-H`, where two field lines meant the wrong credential could reach the server.

- **Headers** — a step's `in: header` parameter replaces the same-named default
  instead of emitting a second field line
- **Credentials** — failure previews sanitize response bodies before showing
  them in traces and errors
- **`$self`** — a relative `sourceDescriptions[].url` resolves against the
  document's identity, per Arazzo 1.1 §5.6.1
- **Inputs** — top-level property `enum` assertions are enforced, fatal under
  `--strict-inputs`
- **Cancellation** — a terminal cancellation can no longer be reinterpreted as
  a retry or a selected action
- **Parallel steps** — a full event channel no longer deadlocks criterion
  emission

### Changed

#### Workflow Engine
- A document that declares `$self` resolves a relative
  `sourceDescriptions[].url` against that identity rather than against the
  directory the document was read from (Arazzo 1.1 §5.6.1). Documents that
  declare no `$self` take a `file://` base from their own directory and bind
  the files they always bound.

### Added

#### Workflow Engine
- Source references bind by identity: a provided document whose `$self` or
  retrieval URI matches the resolved URI wins, then a `file://` URI is read
  from disk, then the reference is refused — naming the source, the authored
  url, the base URI, and the resolved URI. Nothing on this path reaches the
  network; §9.6 presents identity-based referencing as how an implementation
  locates documents in a provided collection without making requests.
- `EngineBuilder::openapi_spec` takes the retrieval path of the document it
  is handed, which is what gives that document an identity to be referenced
  by.
- `examples/self-base-referencing.arazzo.yaml` demonstrates the rule: the
  document is read from `examples/`, its `$self` places it in
  `descriptions/`, and the relative url resolves next to the identity.

#### Validation
- Top-level property `enum` assertions on workflow inputs are enforced. A
  present value, including an injected default, must match a declared member;
  `--strict-inputs` makes the failure fatal.

### Fixed

#### Workflow Engine
- A step's `in: header` parameter replaces a same-named run-wide default
  header instead of adding a second field line. A step declaring
  `User-Agent` sent both `arazzo-cli/0.1` and its own value, which recipients
  fold into `arazzo-cli/0.1, <yours>`. RFC 9110 permits combining repeated
  field lines only where the entire field value is a comma-separated list,
  and `User-Agent` is not such a field.
- The same path duplicated any header a step and a `-H` flag both named. Two
  `Authorization` field lines meant the operator's flag could silently beat
  the document's declared credential, while `--json`, `--dry-run`, and
  `$request.header.*` all reported the step's value alone — the trace pointed
  away from the cause.
- Header names now match case-insensitively, as RFC 9110 requires and Arazzo
  1.1.0 cites for the `header` parameter location, so `-H "user-agent: …"`
  replaces the default agent the way `-H "User-Agent: …"` already did.
  Replacement is per field: a default the step does not name is still sent.
  `in: cookie` parameters were never affected — they are folded into a single
  `Cookie` header before this point.
- A relative `$self` with no retrieval directory to resolve it against is
  reported as a missing directory rather than blamed on `$self`.
- Cancelling an execution terminates routing. Live HTTP sends and
  response-body waits race the execution token, timeout classification is
  centralized, and the action, continuation, nested, recovery, and parallel
  boundaries each recheck it — so a terminal cancellation can no longer be
  reinterpreted as a retry, a wrapper error, or a selected action.
- A parallel step whose bounded event channel filled could deadlock criterion
  emission. The receiver is now polled while the HTTP producer runs, then
  closed and drained before either producer result propagates, preserving
  FIFO event order for source-index replay without detached tasks or a
  larger buffer.

#### Security
- Parsed JSON and text response bodies are sanitized before a
  success-criteria failure previews them, so trace output and CLI error
  consumers no longer receive credentials carried in a failing response.

#### Conformance
- The `operationPath` and source-reference claims name `type: openapi` rather
  than "non-arazzo". An `asyncapi` source is no more resolved or
  identity-bound than an `arazzo` one, and the broader wording asserted
  coverage that does not exist.

#### Documentation
- The README no longer says the `sourceDescriptions` url reading is "None
  implemented yet". An absolute url binds to a provided document whose
  `$self` matches it, and network fetching is a decision rather than a gap.

#### Quality
- 1030 hermetic tests, up from 973.
- Input-validation fixtures no longer share a temp directory when two of
  them are created in the same instant, which could delete one test's spec
  out from under another.
- The parallel event-drain regression runs on conformant Arazzo input: bare
  `operationPath` targets gave way to `operationId`s resolved from a
  temporary OpenAPI document declaring its server, paths, operations, and
  responses, preserving the volume, order, failure, and sequential
  assertions.

## [0.5.0] - 2026-08-26

### Changed

#### Workflow Engine
- **BREAKING** — a bare `operationId` is refused when the document declares
  more than one non-`arazzo` `sourceDescription`, even when the identifier is
  unique. Arazzo 1.1 Step Object: *"If multiple (non arazzo type)
  sourceDescriptions are defined, then the operationId MUST be specified using
  a Runtime Expression."* Such a step previously sent its request to the first
  source's host. Rewrite it as `$sourceDescriptions.<name>.<operationId>` — the
  error names the rewrite. Single-source documents and `--openapi` specs are
  unaffected.

### Added

#### Workflow Engine
- `$sourceDescriptions.<name>.<operationId>` resolves against the named source
  and builds the request URL from that source's `servers` base. Per the
  vendored `source-reference` grammar the source name is `[A-Za-z0-9_-]+` and
  the operation is everything after the first dot, so a dotted `operationId`
  such as `svc.v1.getPet` is addressable.
- Three `RUNTIME_*` codes reach `--json`: `RUNTIME_OPERATION_ID_AMBIGUOUS`,
  `RUNTIME_UNSUPPORTED_OPERATION_ID_FORM`, and
  `RUNTIME_UNSUPPORTED_SOURCE_DESCRIPTION_TYPE`. No existing code changed.

### Fixed

#### Workflow Engine
- A step resolving an `operationId` defined by a second `sourceDescription`
  sent its request to the first source's host, and the source-qualified form
  failed as `RUNTIME_OPERATION_ID_NOT_FOUND`. Dry runs and live runs now both
  route to the source that defines the operation. (#5)
- Duplicate `operationId`s across documents are reported rather than resolved
  by index order. The 0.4.0 rule that an explicitly passed `--openapi` spec
  wins a duplicate still holds; two source documents clashing no longer
  resolve silently.
- A parameter whose value could not be resolved reported its warning twice
  when it was `in: header` or `in: cookie`.

#### Quality
- 973 hermetic tests, up from 931.

## [0.4.0] - 2026-08-25

### Added

#### Validation
- Severity-carrying diagnostics: `validate` distinguishes errors from
  warnings in text and `--json`, and `--strict` promotes warnings to errors
  for CI gates.
- MUST-level identifier and `successCriteria` enforcement, SHOULD-level
  identifier recommendations as warnings, and warnings on unknown
  non-`x-` fields.
- Workflow `dependsOn` enforcement — unknown and cyclic dependencies are
  rejected at validate time instead of surfacing at run time.
- XPath version advisory: every schema-valid XPath declaration the runtime
  will reject — anything but an explicit `version: xpath-10`, including the
  bare `type: xpath` form whose omitted version the specification defaults
  to `xpath-31` — now warns at validate time, naming the declared version
  and the `xpath-10` remedy. This closes the cliff where `validate`
  accepted a document that `run` then rejected. `--strict` promotes these
  like every other warning.
- Golden baselines pinned over every `examples/` and `testdata/` spec, and
  a machine-validated Arazzo 1.1 conformance manifest with a trust gate
  wired into the test suite.

#### Workflow Engine
- Arazzo 1.1 `in: querystring` parameter location: serialized per the
  specification, rejected in documents declaring `arazzo: 1.0.x`, limited
  to one per operation, and a non-string value fails the step instead of
  silently dropping the query.
- `targetSelectorType` honored on Payload Replacement Objects.
- Retry actions referencing a `stepId`/`workflowId` execute call-and-return
  per the specification, and success/failure action `parameters` are passed
  to workflow targets.

### Changed

#### Workflow Engine
- XPath criteria are decided by XPath 1.0 effective boolean value, evaluate
  against the bytes the server actually sent, and fail on a null context
  per specification §5.8.11.4.4.
- One XPath 1.0 version boundary: only an explicit `version: xpath-10`
  reaches the XPath engine; every other declared version and the
  omitted-version form are rejected before evaluation with a diagnostic
  naming the remedy.
- Retry limits are honored exactly at numeric boundaries, decimal retry
  delays are supported, and failure actions fall through per the
  specification.
- The simple-condition parser is constrained to the specification's
  grammar and hardened against malformed input.
- The ` matches ` operator (a tracked non-conformance pending the grammar
  decision) caches compiled patterns in a bounded process-wide cache and
  warns loudly when a pattern does not compile, instead of recompiling on
  every evaluation and silently evaluating to `false`.
- Unresolvable `operationPath` values fail loudly at engine build instead
  of producing a wrong URL, and `describe`/`steps` derive method and target
  from the one canonical classifier.

#### Validation
- Reusable Object handling matches the specification: action `reference`
  shapes are preserved, null references are rejected, typed null action
  overrides are rejected, action fixed fields are enforced, and parameter
  validation carries structural provenance, context, and identity rules.
- Required Arazzo collections are validated, bare `null` success criteria
  are rejected, reusable values enforce the string contract, and required
  `Any` values distinguish presence from absence.

#### Generator
- `generate` emits a document-pointing, relative `sourceDescriptions[].url`
  for generated workflows instead of an absolute base URL.

#### Quality
- JSONPath replacement paths reject GJSON `#` syntax.
- CI runs on current-major action runtimes (off deprecated Node 20).

### Removed
- The `$env.*` expression namespace, an arazzo-cli extension the Arazzo
  specification does not define. `$env` expressions now resolve as an
  unknown namespace — `null`, with a warning — and variable values never
  appear in diagnostics. `.env` files still load into the process
  environment at startup. Migration: declare workflow inputs and pass
  values with `-i`/`--input-json`, referencing `{$inputs.name}`.

### Fixed
- Debug adapter: engine failures surface to the DAP client as errors
  instead of a bare `terminated` event.

## [0.3.0] - 2026-08-03

### Added

#### CLI
- `test` command — CI-native API contract testing. Recursively discovers
  `.arazzo.yaml`/`.arazzo.yml` specs, executes every workflow, and reports in
  TAP (default), JUnit XML (`--format junit`), or JSON (`--json`). Full flag
  parity with `run` (`--input`, `--input-json`, `--header`, `--openapi`,
  `--http-timeout`, `--execution-timeout`, `--expr-diagnostics`, `--parallel`,
  `--strict-inputs`, `--max-response-size`), plus `--fail-fast` and
  `--filter <regex>`. Exits non-zero on any failure or error; parse-error
  suites are tracked separately.
- Agent-facing help text: root and subcommand `--help` now carry "For agents"
  sections covering `--json` envelopes, `schema <command>` discovery, dry-run
  previews, and trace/replay evidence.
- `warnings` array in `run --json` output; new `steps --json` schema; `show
  --json` now includes async step metadata (`action`, `channelPath`,
  `correlationId`, `dependsOn`, `timeout`).
- `--version` flag on the CLI, reporting the crate version.
- Transport trust controls on `run` and `test` (`replay` is offline and takes
  none): `--insecure-host <host[:port]>` (repeatable) disables TLS certificate
  verification for exactly that host while every other host in the same run
  keeps full verification; `--insecure` is the blanket curl `-k` analog;
  `--max-redirects <n>` caps redirect hops (default 10);
  `--allow-downgrade-redirects` permits https→http downgrade hops;
  `--no-transport-warnings` (or `ARAZZO_NO_TRANSPORT_WARNINGS=1`) silences
  transport warning text on stderr.
- Transport warnings with strict channel discipline: live runs emit a startup
  notice listing active insecure exceptions (louder wording for blanket
  `--insecure`), a once-per-host warning when an `Authorization`/`Cookie`
  header would travel over non-loopback cleartext `http://` (loopback exempt),
  and an end-of-run note naming `--insecure-host` entries no request targeted
  (stale-flag detection for Makefiles/CI). Warning text goes to stderr only;
  squelching removes the text while structured `transportWarnings` entries
  persist in `run`/`test` `--json` envelopes and trace run metadata, because
  degraded-trust facts are audit data. Replay runs emit none.

#### Expression Language
- `$self` expression (resolves the current workflow document; no sub-path).
- `$sourceDescriptions.<name>.<reference>` extended beyond `.url` to `.type`
  and named operation references.
- Arazzo 1.1 `$message` expressions: `$message.header.<name>`,
  `$message.payload`, and `$message.payload#/json/pointer`, evaluated without
  assuming a transport.

#### Workflow Engine
- `requestBody.replacements` — JSON Pointer overlays applied to a resolved
  payload before serialization.
- Arazzo 1.1 source/step model: typed AsyncAPI source descriptions, selector
  objects, and async step metadata are parsed, validated, and preserved.
- Preserve Arazzo vendor extensions (`x-*`) through parse and serialize.
- Relative `type: openapi` sourceDescriptions are loaded at engine build:
  their operations are indexed for `operationId` resolution and the request
  base is derived from the document's `servers[0].url` (server variables
  substituted), so `run`/`test`/MCP/debugger workflows no longer need
  `--openapi` for specs their sourceDescriptions already point at. Load or
  parse failures are build errors naming the source and the resolved path —
  never a silent fallback. Explicitly passed `--openapi` specs still win
  duplicate operationIds, with a stderr warning. (#3)

### Changed

#### Workflow Engine
- Arazzo 1.1 async steps fail closed: channel/send/receive execution returns
  `RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT` before any HTTP request preparation.
  1.1 async source and step metadata is typed and preserved, but async
  transport execution is not yet supported.
- Redirect policy is explicit instead of inherited from the HTTP client: the
  engine follows redirects itself up to `--max-redirects` (default 10, the
  previous limit), and https→http downgrade redirects are now refused by
  default with an error naming both URLs
  (`RUNTIME_REDIRECT_DOWNGRADE_REFUSED`); `--allow-downgrade-redirects`
  restores the old following behavior. Exceeding the hop limit reports
  `RUNTIME_REDIRECT_LIMIT_EXCEEDED`, naming the limit and the refused hop.
  Same-scheme redirect semantics are unchanged and pinned by characterization
  tests (301/302/303 method conversion, 307/308 method/body preservation,
  credential-header stripping on cross-host hops, Referer stamping, and the
  whole-chain timeout). Followed hops are recorded on the step's trace request
  as `redirects` (oldest first) and survive into trace files and replay.
- TLS certificate verification failures now name the failing `host:port` and
  the `--insecure-host` remedy instead of surfacing a bare client error.

#### CLI
- `--json` mode no longer writes the human-readable summary to stderr (only
  TAP/JUnit do), keeping structured output clean for programmatic consumers.
- Pre-execution errors (no specs discovered, invalid `--filter`) now exit
  non-zero instead of exiting 0.

#### Quality
- Split the DAP adapter (`dap.rs`, 2,593 → 119-line root coordinator) and the
  runtime core into focused modules; public surfaces and behavior unchanged.

#### Generator
- `generate` now detects OpenAPI 3.1/3.2 input specs and reports an actionable
  message ("generate supports OpenAPI 3.0.x, but this spec declares …") instead
  of a cryptic `invalid type: sequence, expected a string` deserialization
  error. Full 3.1/3.2 ingestion remains future work.

### Fixed

#### Expression Language
- Unsupported JSONPath constructs (recursive descent `..`, wildcards `*`/`[*]`,
  array slices `[a:b]`) now raise an "unsupported JSONPath" diagnostic on the
  criterion instead of silently evaluating to false. Quoted literals are masked
  so filter predicates are not misflagged.
- Fixed JSONPath count-predicate tokenization.

#### Workflow Engine
- Filtered single-step (`--step`) execution: a `goto` whose target is outside
  the filtered set now seeks the first in-scope step at or after the target
  instead of running an earlier step; a `goto` past the filtered tail ends the
  run.
- Preserve an explicit action `type` override (e.g. `type: end`) when merging
  component action references.

#### Generator
- Added reference-cycle guards to the request-body and response resolvers; a
  cyclic `requestBodies`/`responses` `$ref` now returns cleanly instead of
  overflowing the stack.

### Security
- Upgraded `rustls-webpki` 0.103.10 → 0.103.13 (RUSTSEC-2026-0104,
  RUSTSEC-2026-0098, RUSTSEC-2026-0099). This dependency is in the shipped
  binary via `reqwest`/`rustls`.
- Centralized dry-run redaction so all dry-run and trace output share one
  redaction path.
- Cleared dev/optional-dependency advisories not present in the shipped
  binary: `quinn-proto` 0.11.14 → 0.11.16 (RUSTSEC-2026-0185),
  `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204), `anyhow`
  1.0.102 → 1.0.104 (RUSTSEC-2026-0190).

## [0.2.2] - 2026-04-06

### Fixed

#### Workflow Engine
- Preserve multi-valued HTTP response headers instead of last-write-wins
- Special-case `Set-Cookie` handling to avoid comma-join corruption of multi-value headers
- Encode quotes and backslashes in cookie values per RFC 6265
- Return raw body text for non-JSON responses instead of null
- Preserve retry counts across goto cycles and skip retry-count increments on goto self-loops
- Account for per-step retry limits in the workflow iteration cap
- Reject goto actions that specify both `stepId` and `workflowId` at validation time
- Warn on retry fields set on non-retry actions (stderr warning instead of a fatal validation error)

#### Expression Language
- Use approximate (epsilon) equality for f64 comparisons, applied consistently to ordered comparisons
- Handle escaped quotes when splitting list elements in `in` conditions

#### CLI
- Load `.env` before starting the tokio runtime so `$env.*` sees dotenv values in every execution path

#### Security
- Avoid URL normalization during query-parameter redaction so traces record the URL as sent
- Truncate trace body previews at a character boundary to keep valid UTF-8

### Changed

#### Quality
- Reduced debug build size via line-tables-only debug info

## [0.2.1] - 2026-03-29

### Added

#### MCP Server
- `generate_workflow`, `describe_openapi`, and `generate_example` MCP tools

#### CLI
- Improved OpenAPI example value generation in `generate`

### Changed

#### Quality
- Extracted the `arazzo-generate` crate from `arazzo-cli`
- Reduced allocations in the expression evaluator, runtime engine, and validator

### Security
- Upgraded `rustls-webpki` to 0.103.10 (RUSTSEC-2026-0049)

## [0.2.0] - 2026-03-21

### Added

#### MCP Server
- New `arazzo-mcp` crate: Model Context Protocol server exposing workflow tools for AI agent integration
- `serve` CLI subcommand to start the MCP server over stdio

## [0.1.3] - 2026-03-17

### Fixed

#### Workflow Engine
- Honor control-flow decisions (goto, retry, end) in single-step and parallel execution

## [0.1.2] - 2026-03-15

### Fixed

#### Workflow Engine
- Route runtime errors through `onFailure` handlers and preserve original error kinds in step results

## [0.1.1] - 2026-03-15

### Added

#### Expression Language
- `!` (NOT) and parenthesized grouping operators in conditions
- `$workflows.<id>` expressions
- `Retry-After` response header support for retry actions

### Fixed

#### Workflow Engine
- Resolved 8 correctness bugs across expression evaluation, runtime, and the spec model found in cross-agent review

### Changed

#### Quality
- Migrated YAML serialization from `serde_yml` to `serde_yaml_ng` (unmaintained upstream)

## [0.1.0] - 2026-03-13

### Added

#### CLI
- `run` command — execute workflows with inputs, headers, timeout, dry-run, parallel, and trace options
- `validate` command — parse and structurally validate Arazzo YAML specs
- `list` command — list workflows in a spec
- `catalog` command — discover specs across a directory tree
- `show` command — display workflow details with step listing
- `schema` command — print JSON Schema for any command's `--json` output
- `steps` command — list steps within a workflow
- `replay` command — deterministic trace replay with drift detection
- `generate` command — OpenAPI-to-Arazzo CRUD workflow generation
- `--json` flag on all commands for structured output
- `--trace <path>` execution trace output with automatic sensitive value redaction
- `--step` flag for single-step execution with automatic dependency resolution
- `--no-deps` flag for isolated single-step execution (skip dependencies)
- `--strict-inputs` flag for fatal input validation errors
- `--input-json` flag for JSON-typed input values
- `--http-timeout` flag for per-request timeout (default 30s)
- `--execution-timeout` flag for overall workflow timeout (default 300s)
- `--max-response-size` flag for response body size limit (default 10 MiB)
- `--expr-diagnostics` flag for expression evaluation warning surfacing
- Human-readable output for `run` command (structured JSON still available via `--json`)
- Structured JSON error codes with non-zero exit on failure

#### Expression Language
- `$inputs.name` — workflow input parameters
- `$steps.<id>.outputs.<name>` — previous step outputs
- `$env.VAR_NAME` — environment variables (`.env` auto-loaded)
- `$statusCode` — HTTP response status code
- `$method` — HTTP method (GET, POST, etc.)
- `$url` — fully constructed request URL
- `$response.header.Name` — response header (case-insensitive)
- `$response.body.path` — JSON dot-path body access
- `$response.body#/json/pointer` — RFC 6901 JSON Pointer body access
- `$request.header.Name` — request header introspection
- `$request.query.Name` — request query parameter introspection
- `$request.path.Name` — request path parameter introspection
- `$request.body` / `$request.body.path` / `$request.body#/pointer` — request body introspection
- `$outputs.name` — workflow outputs map (within `workflow.outputs`)
- `$sourceDescriptions.<name>.url` — source description URL lookup
- `{$expr}` interpolation in string values
- `//xpath/expression` — XML/HTML body extraction
- Condition operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `contains`, `matches`, `in`
- Expression evaluation diagnostics with warning surfacing

#### Workflow Engine
- HTTP execution with parameter types: header, query, path, cookie, body
- Control flow via `onSuccess` / `onFailure` actions (goto, retry, end)
- Workflow-level default `successActions` and `failureActions`
- Workflow-level `parameters` with step-level override
- Sub-workflow calls via `workflowId` with input/output passing
- Multiple source descriptions with `{sourceName}./path` operationPath routing
- Retry actions with configurable delay and limit
- Parallel step execution via `--parallel`
- Dry-run mode (`--dry-run`) — resolves requests without sending
- Async engine API for non-blocking execution
- `ExecutionObserver` trait for rich event streaming
- Rate limiting via token-bucket algorithm (10 req/sec default)
- Response body size limit (10 MiB default, configurable)
- Goto cross-reference validation at parse time (stepId and workflowId)
- Runtime expression support in goto targets

#### Security
- Trace redaction with stem/substring matching for 14 sensitive key patterns
- Non-JSON body pattern redaction (bearer tokens, key-value secrets)
- Dry-run header redaction
- Consistent `TRACE_REDACTED` constant across all redaction paths
- Output redaction in `--json` structured output

#### VS Code Debugger
- Full Debug Adapter Protocol (DAP) implementation
- Breakpoints on steps, success criteria, actions, and outputs
- Conditional breakpoints using runtime expressions
- Step Over, Step In, Step Out, Continue, Pause controls
- Variable inspection: Locals, Request, Response, Inputs, Steps scopes
- Watch expressions and hover evaluation
- Call stack with sub-workflow depth tracking
- Three-thread coordinator architecture (no deadlocks during slow HTTP)
- YAML parser migrated to yaml-rust2
- Marketplace-ready extension packaging

#### Crate Workspace
- `arazzo-spec` — typed Arazzo 1.0.1 domain model with enum-based types
- `arazzo-validate` — YAML parser with structured validation errors (kind, path, message)
- `arazzo-expr` — expression parser/evaluator with diagnostics and proptest fuzzing
- `arazzo-runtime` — async execution engine with debug controller and rate limiter
- `arazzo-cli` — CLI binary
- `arazzo-debug-adapter` — DAP server with JSON-line debug protocol

#### Performance
- Compiled regex caching via `LazyLock`
- Arc-shared HTTP responses to reduce cloning
- Lazy-init OpenAPI index for faster startup
- Release profile optimization (LTO, `codegen-units=1`, strip, `panic=abort`)
- Benchmark infrastructure with criterion

#### Quality
- `unsafe_code = "forbid"` across all crates
- `unwrap_used = "deny"`, `expect_used = "deny"` via workspace clippy lints
- 337 tests, all hermetic (tiny_http test servers, no external API calls)
- Proptest fuzzing on expression evaluator
- CI: cross-platform build (including aarch64-linux cross-compile), Linux test, MSRV, cargo audit, perf baseline, VS Code extension typecheck + build
- Private-release safeguards: `publish = false` across workspace with CI enforcement script
- Internal release workflow for tagged binaries + `SHA256SUMS.txt`
- Release helper scripts for local preflight, tag cutting, and downloaded-asset verification
- Structured error types with error chain support across runtime and validation crates
- Replaced unmaintained `sxd-document`/`sxd-xpath` with `uppsala`
