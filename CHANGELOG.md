# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`arazzo-cli serve` is back** — the MCP server returns to release binaries
  under an explicit trust model: the connected client can run every loaded
  workflow with the process's credentials, and no approval is asked per
  operation. File tools and referenced documents stay inside the configured
  roots on every platform. New flags set the limits: `--max-request-bytes`,
  `--max-pending-requests`, `--max-result-bytes`, `--max-response-body-bytes`,
  `--max-document-bytes`, `--max-http-timeout` and `--max-execution-timeout`.
  README and SECURITY.md list what is and is not restricted.
- **A workflow can upload a file byte for byte** — `run` and `test` take
  `--input-file '<name>=<path>[;type=<media-type>][;filename=<name>]'`, and the
  input's declared schema decides what the file becomes. A schema with no
  `type` and no `$ref`, `enum`, `const` or applicator keyword loads bytes: a
  step with `payload: $inputs.<name>` and an explicit `contentType` that is
  neither JSON nor form sends exactly those bytes, including across a 307 or
  308 redirect. Their `mediaType` comes from `;type=`, else the schema's
  `contentMediaType`; their `filename` from `;filename=`, else the path's
  basename. A `type: string` schema loads the file's base64 or base64url text
  when `contentEncoding` says so, and otherwise the file as strict UTF-8 text.
  `format: binary` changes nothing, and a schema `default` is never bytes. A
  required input supplied by `--input-file` satisfies `required`.
  - Any other `type`, another `contentEncoding`, `$ref` or an applicator, a
    file that is not UTF-8 for a text input, a suffix on a text input, a
    directory, an undeclared name, a name also given by `--input` or
    `--input-json`, an unreadable file and a file over `--max-input-file-size`
    (default 10 MiB, counted while reading) fail before the run with
    `RUN_INPUT_FILE`. Under `test`, a malformed, unreadable or colliding flag
    is `TEST_INPUT_FILE`; each workflow receives only the files its own
    `inputs` declare, and a refused conversion is a suite error before any
    request.
  - The path runs to the first `;type=` or `;filename=` (case-sensitive) and
    is otherwise taken literally, with no environment expansion. No document
    field names a file: `payload: ./cat.jpg` is the text `./cat.jpg`.
  - Bytes are opaque. The whole value can go to a request body or to a
    workflow or step output, and from there on: `outputs: {photo:
    $inputs.photo}` lets a later step upload `$steps.<id>.outputs.photo`
    byte for byte, through retries and `goto` alike, and a workflow output
    can name a step's bytes output or an earlier bytes output. A JSON Pointer
    into them, `{}` interpolation, a criterion or Selector Object context, a
    `simple` condition operand, a parameter in any location, a replacement
    target or value, a `correlationId`, a `workflowId` step or action
    parameter, a `$workflows` reference to bytes a workflow received or
    returned, and debugger evaluate, watch and breakpoint-condition requests
    fail with `RUNTIME_VALUE_UNAVAILABLE`; a request whose preparation reads
    them is not sent.
  - Bytes with no `contentType`, under a JSON or
    `application/x-www-form-urlencoded` `contentType`, or inside a payload map
    or array fail with `RUNTIME_INVALID_REQUEST_BODY` before sending.
  - After a bytes upload `$request.body` is bytes too: a step output that is
    the whole `$request.body` carries them, and every step criterion, pointer
    or interpolated output, action criterion, action target or parameter, and
    debugger request that reads it fails with `RUNTIME_VALUE_UNAVAILABLE`. An attempt that fails
    in transport records no request, so an `onFailure` action there still reads
    `$request.body` as missing.
  - Bytes are never printed or recorded. A trace lists the input under
    `inputBlobs` and the request body under `request.bodyBlobs`; `--dry-run`
    and `--export-run` show the descriptor (`kind: "bytes"`, `size`, `sha256`,
    `filename`) in place of the body or input. An input declared `writeOnly`
    or `format: password`, or named like a credential, shows
    `{"kind": "bytes", "redacted": true}` with no hash. A bytes output is the
    same descriptor or marker in `run` output with or without `--json`, the
    export's final and per-attempt `outputs` and the MCP `run_workflow`
    result, and is recorded under the trace step's `outputBlobs`; in the export and the
    trace, a bytes output named like a credential is the marker. `replay`
    refuses a trace that recorded bytes with `REPLAY_TRACE_LOAD`.
  - Library callers pass bytes through `WorkflowInputs`, which every
    `Engine::execute*` method accepts in place of the JSON input map. A
    bytes output is in `ExecutionResult::output_blobs`, never in `outputs`;
    `presented_outputs()` renders both with descriptors, and
    `ExecutionHandle::result_only()` now returns `WorkflowOutputs`. Trace and
    run-capture step records gain `output_blobs`.
    **Upgrade note:** a call that relied on type inference for that argument,
    such as `execute(id, serde_json::from_value(v)?)`, must now name the map
    type.

- **A downloaded file can be uploaded byte for byte** — when a response is
  bytes (for example `image/png`, or no `Content-Type` at all), a step output
  that is the whole `$response.body` carries those bytes, and a later step
  with `payload: $steps.<id>.outputs.<name>` and an explicit `contentType`
  sends exactly what the server returned. The bytes' `mediaType` is the
  response's media type without parameters, and they have no `filename`. A
  JSON Pointer into them, `{}` interpolation, a criterion context or a
  `simple` condition operand still fails with `RUNTIME_VALUE_UNAVAILABLE`; for
  a response with no `Content-Type` the reason names the missing header. The
  output shows as a descriptor in `run` output and under the trace step's
  `outputBlobs`, never as content. A response body whose decoding failed stays
  unavailable rather than becoming bytes. `replay` refuses the trace of such a
  run with `REPLAY_TRACE_LOAD`, because it records `outputBlobs`. A replayed
  bytes response body is never restored, so where a trace recorded only
  `response.bodyBlob`, or a library caller uses `replay_trace_steps`, a
  reference to that body fails with `RUNTIME_VALUE_UNAVAILABLE` and the reason
  says the trace holds only a descriptor.

### Changed

- **Request bodies follow one table, and a literal JSON template is sent as
  written** — under `application/json`, any `+json` type or an omitted
  `contentType`, a payload written as a string in the document, with or
  without `{}` interpolation, is sent as its text: the specification's "JSON
  templated example" now reaches the server as JSON instead of one quoted JSON
  string. A whole runtime expression such as `payload: $inputs.s` keeps its
  type, so a string input is still sent quoted; `payload: "{$inputs.s}"` sends
  the string's text as is. Under a type that is neither JSON, form nor
  multipart, a string is sent as UTF-8 and a number or boolean as its JSON
  text; an object, array or null, and a `charset` other than UTF-8, now fail
  with `RUNTIME_INVALID_REQUEST_BODY` before sending instead of being sent as
  JSON or mislabelled. `multipart/form-data` fails the same way until its
  encoder lands, and so does a `boundary` written in its `contentType`.
- **`replacements` work on a literal JSON template** — under a JSON
  `contentType` or an omitted one, a payload written as a string is parsed
  as JSON after interpolation, the replacement targets apply as JSON
  Pointers, and the result is sent as compact JSON. Interpolated text that
  does not parse fails with `RUNTIME_INVALID_REQUEST_BODY` before sending.
  `validate` warns, and `--strict` fails, when such a payload has no `{}`
  interpolation and is not JSON text, under the new `invalidRequestBody`
  kind. Treating a literal string as JSON text is an interpretation of the
  specification's "JSON templated example", not its text.
- **Only `contentType` sets the request `Content-Type`** — a `Content-Type`
  header parameter, in any letter case, on a step with a `requestBody` fails
  with `RUNTIME_INVALID_REQUEST_BODY` before sending; it used to overwrite the
  body's type silently. A step with no `requestBody` may still send one. Bytes
  keep their own `mediaType` out of the header.
- **Maps keep document order** — request payload maps now serialize in the
  order the document writes them, so a form payload `{z: 1, a: 2}` is sent as
  `z=1&a=2` instead of `a=2&z=1`. Nested maps in step outputs and response
  bodies keep document and response order in `--json` output, and the
  generated JSON schemas under `docs/schemas/` list properties in definition
  order. Top-level input, output and step maps stay sorted, and map equality
  stays order-insensitive, so replay matching is unaffected.
- **New MCP startup notice** — `serve` and `arazzo-mcp` print the same three
  `notice:` lines on stderr at startup, stating the trust model and that file
  roots are a path restriction, not a sandbox. They replace the three
  `warning:` lines from earlier builds and never echo a path or value.
- **An unreadable response body is an error, not `null`** — a response body
  that is neither JSON nor valid UTF-8 used to resolve `$response.body` to
  `null` with a warning. Any expression that reads it now fails the step with
  the new `RUNTIME_VALUE_UNAVAILABLE` code, naming the step, the expression and
  the reason: a whole reference or a pointer, `{}` interpolation, a criterion
  or Selector Object context, a `simple` condition operand, an output, an
  action criterion, target or parameter, and debugger evaluate, watch and
  breakpoint-condition requests. A request whose preparation reads such a
  value is not sent. Checks that never read the body, such as
  `$statusCode == 200`, still pass. Missing values are unchanged: `null` plus
  a warning.
- **`$response.body` follows the response's declared media type** — the
  `Content-Type` is parsed once, case-insensitively, and decides how the body
  is read. Nothing is sniffed from the content and nothing is decoded lossily.
  `application/json` and every `+json` type (such as
  `application/problem+json`) parse as JSON, read as UTF-8 with a leading byte
  order mark ignored. XML types and `+xml` are a string decoded by `charset`,
  else a byte order mark, else the XML declaration's `encoding`, else UTF-8.
  `text/*`, `application/x-www-form-urlencoded` and any type carrying a
  `charset` are a string decoded by that charset, UTF-8 if absent: `text/plain`
  `123` stays the string `"123"`, and `text/plain; charset=iso-8859-1` decodes
  correctly. Every other type, a missing `Content-Type` and an unparseable one
  are bytes, which only a whole step output can carry (see "A downloaded file can be
  uploaded byte for byte" under Added).
  Reading a JSON body that does not parse, a body in an unknown charset or
  with bytes invalid for it, or a bytes body any other way fails with
  `RUNTIME_VALUE_UNAVAILABLE` and the reason; checks that
  never read the body, such as `$statusCode == 200`, still pass. An empty body,
  including a 204, is missing (`null` with a warning). Charset labels resolve
  through the WHATWG Encoding Standard table; its `replacement` labels (such as
  `iso-2022-kr`) are rejected, a byte order mark that contradicts the charset
  is an error, and `utf-16` without `le`/`be` needs a byte order mark. Replay
  rebuilds recorded bodies by the recorded media type (see `trace.v2` below).
  **Upgrade note:** an API that sends JSON without a `Content-Type`, or
  labelled `text/plain`, no longer exposes it as JSON. A request `contentType`
  that only contains the text `json` (such as `application/x-ndjson`) no longer
  JSON-serializes a string payload; `application/json` and `+json` types are
  unchanged.
- **gzip and brotli responses are decoded** — requests now offer
  `Accept-Encoding: gzip,br`, and a `gzip` or `br` response body is
  decompressed before its media type is read. `--max-response-size` counts the
  decompressed bytes, so a small body that expands past the limit fails with
  `RUNTIME_RESPONSE_TOO_LARGE`. A decoded response no longer shows its
  `Content-Encoding` or `Content-Length` in `$response.header` or the trace.
  A body still carrying another coding (such as `compress`) is bytes: reading
  it fails with `RUNTIME_VALUE_UNAVAILABLE` naming the coding. Traced request
  headers and replay matching are unchanged; the transport adds
  `Accept-Encoding` below them.
- **Traces move to `trace.v2`** — `run --trace` writes
  `schemaVersion: "trace.v2"`, described in
  [docs/trace-schema-v2.md](docs/trace-schema-v2.md) and
  [docs/schemas/trace-v2.schema.json](docs/schemas/trace-v2.schema.json). A
  response records `mediaType`, the parsed `Content-Type`, in place of the
  `contentType` classification (`json`, `xml` or the raw header). A JSON or
  text body is recorded as its decoded text, in any charset, and is redacted
  and truncated as before; `bodyPreview` and `bodyLossy` are gone. A bytes
  body, or one that failed to decode, is recorded only as a `bodyBlob`
  descriptor (`kind: "bytes"`, `size`, `sha256`, `mediaType`) plus
  `bodyError` when decoding failed: its content is never written. `replay`
  refuses a `trace.v1` file with `REPLAY_TRACE_LOAD`, naming both versions;
  re-record it with this version. **Upgrade note:** tools that read
  `contentType`, `bodyPreview` or `bodyLossy` from trace responses must read
  `mediaType`, `body` and `bodyBlob`.
- **Run exports record the parsed media type** — a `run.v1` response's
  `contentType` is now the parsed `Content-Type` (for example
  `application/json;charset=utf-8`) instead of `json`/`xml`/the raw header,
  and is omitted when the response has no parseable `Content-Type`.
- **Failed-criteria messages never print raw bytes** — the body shown in a
  `success criteria not met` message (stderr, `--json` errors and MCP results)
  is the decoded, redacted text for a JSON or text body and a summary such as
  `<1024 bytes, image/png>` for a bytes or undecodable body, never lossily
  decoded bytes.
- **The default criterion context is `$response.body`** — a `regex`,
  `jsonpath` or `xpath` criterion with no `context` now evaluates over exactly
  what `$response.body` resolves to, so an undecodable body fails the criterion
  with `RUNTIME_VALUE_UNAVAILABLE` instead of matching lossily decoded text.
  A `simple`
  criterion no longer builds a default context: the debugger's
  `criterionContextValue` is `null` for it, and the body stays available as
  `responseBodyRaw`.
- **Missing request and response values now warn** — `$url`, `$method`,
  `$statusCode`, `$request.*` and `$response.*` that resolve to nothing still
  evaluate to `null`, but each absence now reports an expression warning, as
  `$inputs` and `$message` already did. A typed criterion whose template
  references an absent response header therefore fails with a diagnostic
  naming the expression instead of silently matching the text `null`. With
  `--expr-diagnostics error` such runs now exit with
  `RUNTIME_EXPRESSION_DIAGNOSTICS`.
- **`validate` checks `operationId` routing** — a bare `operationId` while two
  or more non-`arazzo` source descriptions are declared, a qualified
  `$sourceDescriptions.<name>.<operationId>` naming an undeclared or `arazzo`
  source, and a malformed qualified form are reported as errors at the step's
  `operationId` path before any request is sent. `run` refuses these documents
  with `RUN_SPEC_VALIDATION`; previously the runtime refused them only on
  reaching the step.
- **MCP `run_workflow` can be cancelled** — closing stdin during `run_workflow`
  now cancels the run and writes no response, as does `notifications/cancelled`
  naming its request id. The server answers other requests while a run is in
  flight; tool calls still run one at a time.
- **MCP failure text is redacted** — `run_workflow` errors, tool errors, and
  `validate_spec` diagnostics pass through the runtime's URL and secret-pattern
  redaction before they reach the client. Successful results are unchanged.
- **MCP `run_workflow` uses Kafka credentials from the environment** —
  `ARAZZO_KAFKA_USERNAME` and `ARAZZO_KAFKA_PASSWORD` set on the server now
  reach Kafka receive steps, as they do for `run` and `test`.

### Fixed

- **Debugger gate errors end the run** — a breakpoint condition that fails to
  evaluate at a success-criterion, output, or sub-workflow checkpoint now ends
  the run with the same `DebugController` error as the step-entry gate,
  instead of being routed through `onFailure`, where a `retry` re-sent the
  request and a `goto` took the failure branch.
- **Transport error messages redact URLs** — connection, redirect-downgrade,
  redirect-limit and invalid-URL errors no longer carry userinfo or sensitive
  query values into `run` errors, `--json`, stderr or the trace step `error`.
- **String request bodies are redacted** — form-encoded, plain-text and XML
  request payloads in `--trace`, `--dry-run` and the MCP dry-run projection
  now have sensitive `key=value` fields and embedded credentials replaced,
  as JSON bodies and response bodies already were. A sensitive field masks its
  whole value, including whitespace and following lines, so passphrases and
  multi-line keys cannot leak. Replay treats recorded and live string bodies
  that differ only in a redacted secret as equal.
- **YAML alias expansion is bounded** — every document parse (`validate`,
  `run`, `test`, `generate`, the debug adapter, MCP `validate_spec` and
  `describe_openapi`, and runtime source loading) now counts expanded nodes
  and refuses a document past 16 nodes per input byte plus 1024, so a few
  kilobytes of nested anchors can no longer exhaust memory. The refusal is an
  ordinary parse error (`VALIDATE_SPEC_PARSE_YAML`, `RUN_SPEC_PARSE_YAML`,
  `RUNTIME_SOURCE_DESCRIPTION_PARSE`) naming the budget. As a consequence
  `parse_unvalidated_bytes` and `generate` now reject duplicate mapping keys,
  which `validate` and runtime source binding already rejected.

## [0.9.0] - 2026-11-06

Run HTTP and Kafka workflows with more faithful Arazzo behavior and clearer automation failures.

**Upgrade if** you chain HTTP requests through workflow values, need Kafka receive
steps, typed success criteria, reliable debugger teardown, or consistent CLI JSON
errors. Review the breaking changes below.

- **Kafka receive workflows** — receive JSON messages, filter correlation IDs, and validate AsyncAPI targets offline.
- **Spec fidelity** — standard operation targets, explicit reusable actions, and typed Criterion interpolation.
- **Reliable execution** — corrected timeout classification, workflow output diagnostics, and cancellation-aware debugger teardown.
- **MCP disabled** — the release excludes the MCP server pending security hardening.
- **Automation contracts** — shared JSON errors and stronger validation with machine-readable diagnostics.

### Added

- **Offline receive validation** — `validate` uses the runtime's AsyncAPI
  receive projection to report refused targets as error diagnostics, without
  connecting to a broker. Select a server with `--asyncapi-server <SOURCE=KEY>`;
  JSON diagnostics retain the runtime code and workflow/step path.
- **Kafka receive correlation filtering** — a step's literal-or-expression
  `correlationId` selects the first matching record using the AsyncAPI message's
  Correlation ID Object. `trace.v1` records `correlationId` and nonzero
  `skippedRecords`; `run --verbose` shows `skipped=<n>`. Missing or malformed
  correlation definitions (including `location`) and non-scalar expected values
  report `RUNTIME_MESSAGE_TRANSPORT_CONFIG`; disagreeing channel-message
  locations report `RUNTIME_UNSUPPORTED_ASYNCAPI_TRANSPORT`. Try the shipped
  `order-followup-correlated` workflow with `--input orderId=42`.
- **AsyncAPI receive on Kafka** — `run` and `test` execute Arazzo 1.1 `receive`
  steps whose source is an AsyncAPI 3.0 document and whose server is a Kafka
  broker: receivers are armed when the run starts, the next JSON record drives
  `$message.payload` / `$message.header.*`, criteria and typed outputs, and
  dependent HTTP steps run as usual. New flags `--kafka-username`,
  `--kafka-tls`, `--kafka-ca-cert`, `--kafka-connect-timeout`,
  `--asyncapi-server <source>=<key>`; environment `ARAZZO_KAFKA_USERNAME` and
  `ARAZZO_KAFKA_PASSWORD` (the password has no flag). New codes
  `RUNTIME_MESSAGE_TRANSPORT_CONFIG`, `RUNTIME_MESSAGE_TRANSPORT`,
  `RUNTIME_MESSAGE_RECEIVE_TIMEOUT`, `RUNTIME_MESSAGE_MALFORMED`,
  `RUN_KAFKA_CREDENTIALS`, `RUN_KAFKA_CA_CERT_READ`, `RUN_ASYNCAPI_SERVER_FLAG`
  (and `TEST_` twins). `--verbose` prints `Kafka receiver armed: …` when a
  topic is ready and a `[msg] RECEIVE …` step line. See `docs/kafka-receive.md`
  and `examples/kafka-order-followup.arazzo.yaml`.

### Changed

- **Breaking: MCP temporarily removed.** The CLI no longer exposes `serve` or links the MCP server. Security hardening remains incomplete; there is no runtime flag to restore MCP in release binaries. Disconnect existing MCP client configurations. Do not downgrade as a security workaround. Ordinary CLI workflow execution remains available.

- `trace.v1` step records gain an optional `message` object for receive steps
  (transport, host, topic, partition, offset, timestamp, headers, redacted
  payload preview, `simulated` for dry runs); HTTP records are unchanged.


- **Breaking:** Operation targets use standard Arazzo syntax exclusively. Both
  operation IDs and canonical JSON Pointers resolve against a bound OpenAPI
  document. Custom request-path syntax, method prefixes, URL-as-base routing,
  and operation lookup in undeclared documents have been removed. Generation
  emits standard targets and rejects duplicate operation IDs.
- **Breaking:** Action `name` fields are always literal. Component reuse requires
  `reference`; name-based lookup and local field merging have been removed.
- **Breaking:** `generate` writes YAML only. `generate --json` now fails with
  `code: "GENERATE_JSON_UNSUPPORTED"`, with or without `--output`, and
  `--output` must end in `.yaml` or `.yml` (`GENERATE_OUTPUT_NOT_YAML`
  otherwise, with no file written). The `--json` metadata object and
  `schema generate` are gone. Without `--json`, output is unchanged.
- With `--json`, a usage error such as an unknown flag or a missing argument
  prints a JSON error object with `code: "CLI_USAGE"` on stdout and still
  exits 2. `replay` and `test` error objects always carry a `code`; `test`
  failures that had none now use `TEST_*` codes, and its `-i`,
  `--input-json`, and `--openapi` errors print the object instead of plain
  text.
- `schema` failures with `--json` print the shared JSON error object, with
  `SCHEMA_UNKNOWN_COMMAND` for an unknown schema name.

### Fixed

- **Example generation** — CRUD generation uses a bounded
  schema walker. Oversized request bodies trim at 1,000 units and warn once per
  step; `password` and `byte` formats produce their intended examples. A
  numeric bound no JSON number can satisfy, such as
  `exclusiveMinimum: 1.7976931348623157e308`, is now an error instead of an
  example that violates it.

- Form request mappings serialize as form bytes rather than JSON.
- Replay compares canonical redacted logical requests, preserving useful mismatch checks without requiring secrets from persisted traces.
- Redaction covers complete bodies before preview truncation, encoded query credential names, and the accepted exact credential vocabulary; export URLs preserve unrelated query pairs and fragments.
- Nested workflow parameters retain container types, and invalid workflow references are rejected before execution.
- Dry-run plans identify transport-dependent skipped values without overwriting ordinary replacement targets.
- Cancellation outcomes are synchronized, and Tokio includes the patched AsyncFd release.

- **Unresolved request values** — HTTP steps refuse unresolved parameters,
  payloads and replacement values, including hard Selector failures, before
  sending a request, with
  `RUNTIME_UNRESOLVED_REQUEST_VALUE` and the source location. This also applies
  through nested workflows and parallel execution; retry actions do not repeat
  the invalid request. Intentional nulls, empty values, omitted optional inputs
  and dry-run transport dependencies retain their distinct behavior. A valid
  Selector with zero matches still resolves to null under the existing policy,
  so this guard does not reject every empty path value.

- Criterion interpolation preserves typed values for regex, JSONPath and XPath 1.0 evaluation.
- Workflow output-resolution warnings are included in CLI results.
- Request timeouts retain their timeout classification; cancellation remains distinct.
- Verbose URL redaction removes URL userinfo credentials.

- **Debugger teardown** — cancelling an invocation releases its paused debug
  checkpoint and prevents later checkpoints from pausing it again. The DAP
  adapter cancels and joins its active runtime on stdin EOF or transport failure,
  preserving the original failure and cancellation-versus-timeout identity.
- Structural validation now rejects `action` and `correlationId` on operation IDs
  qualified to non-AsyncAPI sources, and `channelPath` references to those sources.
  Errors identify the field, source name, and declared source type.

- Update `rustls` from 0.23.45 to 0.23.46 to address RustSec advisories
  RUSTSEC-2026-0346 through RUSTSEC-2026-0349.
- With `--json`, a failing `validate` prints the shared error object, with
  `kind`, `error`, and a `VALIDATE_SPEC_*` `code`, beside the `valid`, `file`,
  `errors`, and `warnings` fields it already had. `--strict` promotions use
  `VALIDATE_SPEC_VALIDATION`. Successful output and human output are
  unchanged; `schema validate` now describes both shapes.
- With `--json`, every `run` failure prints the error object with a `code`.
  A bad `-i` or `--input-json` value (`RUN_INPUT_FORMAT`), an engine-build
  failure such as a missing local source description (the engine's
  `RUNTIME_*` code), and a trace that cannot be written (`RUN_TRACE_WRITE`)
  printed plain text to stderr and nothing on stdout. Human output is
  unchanged.
- With `--json`, a failing `list`, `steps`, `show`, or `catalog` prints the
  shared error object with a `code` (`LIST_SPEC_*`, `STEPS_SPEC_*`,
  `STEPS_WORKFLOW_NOT_FOUND`, `SHOW_DIR_READ`, `SHOW_WORKFLOW_NOT_FOUND`,
  `SHOW_WORKFLOW_AMBIGUOUS`, `CATALOG_DIR_READ`). Stdout was empty. Successful
  and human output are unchanged; their `schema` output now describes both
  shapes.
- `run --verbose` step lines redact sensitive query parameter values in the
  request URL, as trace files do. A value such as `access_token` previously
  reached the terminal and CI logs in clear.
- Check Step and Workflow `dependsOn` cycles without recursion. A chain of a
  few thousand `dependsOn` links overflowed the stack and aborted `validate`,
  `run`, the MCP server, and the debug adapter; the same document now
  validates with the same diagnostics.
- Refuse a JSONPath query nested more than 8 `[`…`]` levels deep before
  parsing it. Parse time doubled with each nested filter, so a criterion,
  selector, or replacement target of about 100 bytes could stall a run for
  minutes, and `--execution-timeout` could not interrupt it. Brackets inside
  string literals and sibling brackets do not count toward the limit.
- Pass constant `workflowId` step parameter values (numbers, strings,
  booleans, mappings, sequences) to the child workflow's `$inputs`. They
  previously arrived as `null`, with no warning.
- The debug adapter answers `continue`, `next`, `stepIn`, or `stepOut` sent
  while the workflow is not paused, and `launch` without `spec`, with an
  error response. It used to report success and then exit, ending the
  debug session.
- Debugger evaluation distinguishes unresolved expressions and invalid
  conditions from ordinary null values, reports breakpoint resolution errors,
  and evaluates watches in the selected stack frame. Session failures retain
  request context instead of appearing as successful completion.
- Update `quinn-proto` from 0.11.16 to 0.11.18 to include its security fix.

### Migration

- Replace custom operation-path forms, method prefixes, and URL-as-base routing
  with an operation ID or canonical operation pointer resolved from a declared
  OpenAPI source. See the updated examples for document bindings and targets.
- Replace action reuse through `name: $components...` with the action's
  `reference` field. Local field merging through action names is removed.
- Remove `generate --json` and `schema generate` from scripts. Generation
  produces YAML, and output filenames must end in `.yaml` or `.yml`.
- Update JSON error consumers to use `kind`, `error`, and `code`; refresh any
  locally copied command schemas from `docs/schemas/`.

## [0.8.2] - 2026-10-09

Add a compact JSON run export for agents and improve debugger hover evaluation.

**Upgrade if** you want durable run artifacts for examples, tests, and agent
handoffs, or if you use the VS Code debugger to inspect response expressions.

- **Run artifacts** — `arazzo-cli run --export-run <path>` writes a redacted
  `run.v1` JSON artifact with typed declared outputs, ordered step attempts,
  request/response metadata, timestamps, and runtime status.
- **Safer export destinations** — run exports refuse paths that alias the trace,
  input spec, explicit `--openapi` files, or local OpenAPI sources loaded from
  `sourceDescriptions`, including filesystem-equivalent paths.
- **Debugger hover drilldown** — VS Code hovers now evaluate `$response`,
  `$response.body`, JSON Pointer prefixes, scalar pointer values, and whole
  simple conditions distinctly.

### Added

- Add `run --export-run <path>` for producing a versioned `run.v1` JSON
  artifact without changing normal stdout. The artifact includes selected run
  metadata, parsed inputs before defaults, final outputs, and per-step execution
  records suitable for `jq`, docs, and agent handoffs.
- Add `schema export-run` and the bundled
  `docs/schemas/export-run.schema.json` contract for consumers that validate or
  generate tooling around exported run artifacts.
- Capture body-free settled step attempts in the runtime so run artifacts can
  describe retries, failures, dry-runs, and HTTP metadata without implicitly
  copying response bodies.

### Fixed

- Refuse unsafe or ambiguous run export destinations, including paths that are
  the same file as the trace output, the input Arazzo spec, explicit OpenAPI
  files, or local `sourceDescriptions` files after filesystem equivalence is
  resolved.
- Preserve both the runtime failure and the export or trace write failure in
  JSON error messages when multiple end-of-run reporting steps fail.
- Improve DAP evaluate handling so `$request` and `$response` hovers are
  expandable, `$response.body` and JSON Pointer prefixes evaluate to their
  actual values, and whole simple-condition hovers evaluate as booleans.

## [0.8.1] - 2026-10-08

Restore non-null guards in simple conditions and prevent startup hangs when
`.env` cannot be read.

**Upgrade if** you use `value != null` conditions or a `.env` path that is a
directory or otherwise fails to read.

- **Non-null guards restored** — `value != null` agrees with `!(value == null)`:
  present non-null values pass, while null or missing values fail.
- **No startup hang** — a hard read error on `.env` is reported once and
  loading stops, instead of looping forever.

### Fixed

- Restore simple-condition `!=` as the complement of `==`, including null
  and missing operands. `value != null` now agrees with `!(value == null)`:
  present non-null data passes, while null or missing data fails. This reverses
  the 0.8.0 interpretation of the specification's ambiguous null prose and
  matches its non-null data example. Evaluation errors still fail the condition.

- `.env` loading in both binaries stops at the first read error other than
  invalid UTF-8 and prints `warning: .env:<line>: stopped reading: <error>`.
  Names read before the error stay set. A directory named `.env` previously
  made every read fail and the loader spin forever, growing memory since
  0.8.0's load report. Invalid-UTF-8 lines are still skipped one at a time.

## [0.8.0] - 2026-10-08

Runtime expressions and simple conditions now use one canonical Pest grammar,
with stricter authoring rules, safer evaluation, and more reliable execution.

**Upgrade if** you use expressions, parallel retries, cancellation, or
operation-specific OpenAPI servers. Review the migration notes below first:
legacy standalone body traversal and non-spec simple-condition operators are
rejected, and existing environment variables now take precedence over `.env`.

- **Canonical expressions** — parsed runtime expressions and simple conditions
  share a grammar; validation provides conservative JSON Pointer migration hints.
- **Predictable conditions** — complete parsing, short-circuit evaluation,
  strict boolean/null truth, and exact comparison of represented integers.
- **Reliable execution** — plain retries stay parallel, step attempts retain
  their own outputs, and cancellation cannot overwrite a completed outcome.
- **Correct endpoints** — operation and path-item servers take precedence for
  operationId targets resolved from source descriptions.
- **Safer inputs** — XML admission limits, a patched TLS dependency, and visible
  `.env` loading that preserves existing environment values.

### Changed

- Required-expression fields, including outputs and Selector/Criterion
  contexts, reject legacy standalone dotted/bracket body traversal. Replace
  `$response.body.status` with `$response.body#/status`; use RFC 6901 escaping
  for literal keys. Wildcards and filters need an appropriate typed JSONPath
  criterion or Selector Object, not a JSON Pointer substitution. Bare XPath
  output strings also require a Selector Object with an explicit `xpath-10`
  version, context, and selector.
- Literal-capable Parameter/payload fields retain ordinary strings, including
  `$USD` and unbraced legacy traversal text. Use a canonical expression for
  selection, or `{$response.body#/status}` to embed it in text. Only `{$`
  starts an embedded expression; other braces remain literal. Missing values
  render as `null` in interpolated text and retain diagnostics.
- Simple conditions reject `contains`, `matches`, `in`, double-quoted string
  literals, and chained comparisons. Use single quotes, double an apostrophe
  to escape it, and use typed regex/JSONPath criteria where appropriate.
  Property/index operators remain valid inside simple conditions.
- Simple conditions parse fully before short-circuit evaluation. Syntax or
  evaluation errors make the owning condition false, while ordinary Step
  failure routing and later eligible Actions remain possible. Detailed
  evaluation results retain diagnostics; this release does not complete
  ordinary CLI/MCP/DAP presentation of every condition error.
- Numbers retain existing i64/u64/finite-f64 storage. Plain integer literals
  and numeric strings without fractions/exponents must fit `[-2^63, 2^64-1]`;
  fraction/exponent conversion rejects overflow and nonzero underflow to zero.
  Represented integers and mixed integer/float comparisons avoid rounding the
  integer to f64. Loader rounding and decimal precision limits remain.
- Numeric-string coercion applies to all six comparisons when exactly one
  operand is numeric; the string must match JSON-number syntax completely.
  Two strings remain case-insensitive text. Exactly one null/missing operand
  makes every comparison false, including `!=`; use `!(value == null)` for
  existence. The specification's null prose/example contradiction remains
  documented locally, and full Arazzo compliance is not claimed.
- Plain retries without a step/workflow target now run within parallel
  dependency levels. Cases that still require sequential execution report
  their reason through events, verbose CLI output, and trace
  `run.sequentialFallbacks`.
- `.env` loading in `arazzo-cli` and `arazzo-mcp` no longer overwrites a
  variable that is already set, including one set to the empty string. To use
  the file's value, unset the variable before running.
- When a `.env` is present, both binaries print one stderr line counting the
  names set, kept from the environment, and ignored, and a `warning:` with the
  line number and reason for each line that has no `=`, an empty name, a NUL
  byte, or cannot be read. Previously such lines were dropped silently. stdout,
  including `--json` output and `arazzo-mcp` framing, is unchanged.

### Fixed

- Route source-description operationId targets through operation-level,
  path-item-level, then document-level OpenAPI servers. Refuse an unusable
  declared server before dispatch instead of selecting a higher-level server.
- Preserve each sequential step attempt's own outputs, retain parallel sibling
  events/traces after a failure, and deliver parallel observer callbacks in
  stream order. Single-step runs announce and complete attempts consistently.
- Keep cancelled invocations from being reported as completed, and keep late
  cancellation from replacing an already reported outcome.
- Generate canonical body JSON Pointers, and migrate bundled examples to the
  enforced expression grammar and typed Selector Objects.
- Preview typed Selector Object outputs correctly in the debugger before the
  step has executed. Validate inherited component-action parameters once at
  their definition while continuing to validate local overrides.
- Reject XML exceeding the nesting limit or containing DTD entity declarations
  before XPath parsing. Update locked rustls to 0.23.45 for RUSTSEC-2026-0285.
- Make conformance evidence scanning handle Rust lifetimes and reject
  unterminated block comments with the source filename.

### Build and release

- Pin Rust 1.98.1 for development, CI, and release builds; MSRV remains 1.88.
- Fetch checksum-verified vendored specifications before compiling tests, and
  write their checksum manifest in portable text-mode form on every platform.

## [0.7.0] - 2026-09-14

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
