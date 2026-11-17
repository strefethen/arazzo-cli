---
status: accepted
epic: ac-6a548
created: 2026-11-17
revive-when:
---

# HTTP body handling: media types, binary values, multipart

**Seed:** A consuming agent reported: "arazzo-cli sends a non-JSON request body
as the UTF-8 bytes of a string, so a workflow can't send a body starting with
the JPEG signature `FF D8 FF`. … successful photo uploads, the identical-bytes
204 and the different-bytes 409 can't be tested from Arazzo; uploads only run
through the seed script." Steve, 2026-11-17: "let's not worry about backwards
compatibility rather let's focus on building the right approach to this
situation. what we need to focus on is building the best arazzo cli solution
that will handle this case and continue being aligned with the spec. we likely
need to bring multipart/form-data into the picture here as any arazzo tool that
can't support that content type is likely not credible" and "the goal is to
have arazzo cli be world-class in terms of handling http requests in the most
robust and logical way possible".

## Outcome and non-goals

A workflow can send and receive arbitrary bytes and `multipart/form-data`
without any document syntax the specification does not define. Every request
and response body is encoded or decoded by one media-type rule set, chosen from
the declared media type and the value's kind — never by inspecting content.
Bytes are opaque: they pass through whole-value references and reach the wire
unchanged, and every operation that would need to read inside them fails
closed with a diagnostic.

Backward compatibility of body handling, trace files, and output key order is
explicitly not a constraint (Steve, 2026-11-17). Golden baselines,
`docs/schemas/`, and `RUNTIME_*` codes change where this plan says so; each
change is deliberate and lands in the CHANGELOG.

This initiative does **not**:

- add document syntax — no `x-` fields, no `$file()`-style expression forms, no
  new Request Body Object fields;
- read the referenced OpenAPI operation's `requestBody`, `encoding`, or schema
  to serialize a body (the AGENTS.md "Parameter serialization" stance extends
  to bodies unchanged);
- implement positional (unnamed) multipart, per-part custom headers, nested
  multipart, or runtime assembly of any `multipart/*` type other than
  `form-data` (a pre-built body of such a type can still be sent as bytes);
- stream request or response bodies; the byte representation leaves room for
  it later;
- sniff media types from content, anywhere;
- add binary ingress to the MCP server (follow-up below), or binary
  support to AsyncAPI message payloads;
- embed binary body content in traces (a later, additive opt-in);
- change transport behavior (TLS, redirects, retries, timeouts) beyond
  response decompression. Request bodies stay fully buffered, so the existing
  307/308 resend and step retries keep replaying identical bytes.

## User-visible workflows

**Raw binary upload.** The workflow declares an input whose schema has no
`type`, and a step with `contentType: image/jpeg` and `payload: $inputs.photo`.
The user runs `arazzo-cli run … --input-file photo=./cat.jpg`. The request body
is exactly the file's bytes. Identical-bytes and different-bytes re-uploads are
two runs with two files, or two inputs in one run.

**Encoded binary input.** An input declared `type: string` with
`contentEncoding: base64` (or `base64url`) loaded with `--input-file` becomes
the base64 text of the file; the workflow embeds it in a JSON body as it would
any string.

**Multipart upload.** A step with `contentType: multipart/form-data` and a
payload map — `{caption: $inputs.caption, tags: [a, b], photo: $inputs.photo}`
— sends one part per key in the order the document writes them (one part per
array item), the file part carrying a filename and media type, the boundary
owned by the runtime. Order matters to real services (S3 POST policy uploads
require the file part last). Load-time overrides follow curl's `-F` precedent:
`--input-file 'photo=./cat.jpg;type=image/jpeg;filename=c.jpg'`.

**Binary download and re-upload.** A step receiving `image/png` exposes
`$response.body` as bytes; an output maps it; a later step sends that output as
a raw or multipart body. `--output-file <outputName>=<path>` writes a bytes
output to disk when the run succeeds.

**Text and JSON responses.** `text/plain; charset=iso-8859-1` decodes to the
correct string. A `text/plain` body `123` stays the string `"123"`.
`application/problem+json` parses as JSON. A gzip response decodes first. A
response with no Content-Type is bytes; a pointer into it fails with a
diagnostic that names the missing header.

**Failure paths — each a hard error naming the step and reason:** a JSON
pointer, `{}` interpolation, criterion or Selector Object context, operation
parameter value, or `replacements` target that reaches bytes; bytes or a
multipart payload with no explicit `contentType`; bytes under a JSON or
url-encoded `contentType`; an object or array payload under a type that is not
JSON, form, or multipart; a non-map multipart payload; a `boundary` written in
a multipart `contentType`; a `Content-Type` header parameter on a step with a
request body; an `--input-file` whose resolved schema neither permits bytes nor
declares a supported encoding, that names an undeclared input, or that collides
with `--input`/`--input-json` for the same name; a file over the input size
cap; invalid UTF-8 for a `type: string` input; an unknown `contentEncoding`; an
`--output-file` naming an undeclared output or an unusable target directory
(both checked before the run starts), or whose output is not bytes when the
run ends; `$response.body` referenced on a body whose declared JSON, charset,
or content coding fails to decode; replay of a trace written by an older trace
schema.

**`--json`, run exports, MCP results, and traces.** A bytes value prints as a
descriptor object (size, sha256, media type, filename) whose shape is published
in `docs/schemas/`. Bytes are never printed, logged, or embedded in prose.

## Decision-relevant facts

Specification text, verbatim from `spec/`:

- Request Body Object (Arazzo §5.8.14): `payload` is `Any`; "To represent
  examples of media types that cannot be naturally represented in JSON or YAML,
  use a string value to contain the example, escaping where necessary." It
  defines no escaping and no binary or multipart serialization; Arazzo mentions
  multipart only in §9.3, where an Arazzo document arrives inside a
  `multipart/related` response. `contentType`: "If omitted then refer to
  Content-Type specified at the targeted operation to understand serialization
  requirements." Its examples include "A JSON templated example"
  (`contentType: application/json`, a literal string of JSON text with `{}`
  interpolation), "A complete Runtime Expression example" (`payload:
  $inputs.petOrderRequest`, value kind unstated), and "A Form Data String
  example" (a literal url-encoded string).
- Runtime Expressions (Arazzo §5.9): "Runtime expressions preserve the type of
  the referenced value." Embedded `{}` expressions convert scalars "to their
  string representation"; binary values are not mentioned. "Whether a value is
  stored as a string or parsed structure depends on its content type. JSON
  responses and inputs are typically parsed into structures, while XML and
  plain text are typically stored as strings."
- Workflow Object `inputs`: "A JSON Schema 2020-12 object"; for inputs and
  `components.inputs`, "references via $ref MUST support both JSON Pointer
  fragments and plain-name fragments (defined by $anchor)".
- OAS 3.2 §4.24.4.3 Working with Binary Data: "raw binary is used where
  unencoded binary data is allowed, such as when sending a binary payload as
  the entire HTTP message body, or as part of a multipart/* payload that allows
  binary parts"; "encoded binary is used where binary data is embedded in a
  text-only format such as application/json". Its keyword table gives raw
  binary `type` "omit" ("raw binary is outside of type") with an optional
  `contentMediaType`, and encoded binary `type: string` with `contentEncoding`
  "base64 or base64url".
- OAS 3.2 §4.15.1.1 Encoding Object default `contentType` table: `type`
  absent → `application/octet-stream`; string without `contentEncoding` →
  `text/plain`; "number, integer, or boolean" → `text/plain`; object →
  `application/json`; array → `application/json`, a row that under Encoding By
  Name "applies only to array values inside of a top-level array". §4.14.5.1:
  "Array properties MUST be handled by applying the given Encoding Object to
  produce one encoded value per array item, each with the same name".
- OAS 3.2 §4.15.4.1: media type sniffing "MUST NOT be the default behavior".

Repository facts at `5c28bf2`:

- Request serialization (`engine_http.rs`) sends a non-JSON string payload as
  its UTF-8 bytes and everything else via `serde_json::to_vec`; `form_body.rs`
  likewise. All values are `serde_json::Value`, which cannot hold non-UTF-8.
  A `Content-Type` header parameter silently overwrites the body's type.
- An unresolved reference is not an error: `arazzo-expr` turns a missing value
  into `null` plus a warning (`resolution.rs`), and the request is still sent.
- The wire layer already takes `Option<Vec<u8>>` and re-sends it on 307/308
  hops from its own redirect loop (`client.rs`).
- Response classification (`client.rs`) matches substrings `json`, `xml`,
  `rss`, JSON-parses every non-XML body regardless of declared type, and treats
  a missing Content-Type as JSON. `eval_context.rs` exposes a non-JSON body
  only if it is valid UTF-8 (audit F22's open follow-up) and clones the body on
  every context build. XPath rejects non-UTF-8 input (`xpath.rs`), and the
  criterion default context is built eagerly for every criterion type with
  `from_utf8_lossy` (`criteria.rs`); no parser honors an XML encoding
  declaration.
- Traces are redacted evidence. The runtime records each response body as a
  `from_utf8_lossy` string with a `body_lossy` flag; before writing, the CLI
  (`arazzo-cli/src/trace.rs`) redacts that text by JSON key and text pattern
  and truncates it to `--trace-max-body-bytes`. Replay rebuilds responses from
  the stored text, already refuses any trace whose `schemaVersion` is not the
  current one, and matches each request on method, URL, the full header map,
  and the logical projected body (`replay_projection.rs`).
- `ProvenancedValues` (`value_provenance.rs`) carries a parallel per-name map
  beside each value store; `StoredValueReference` distinguishes whole-value
  from pointer references (`arazzo-expr/src/resolution.rs`).
- `PropertyDef` (`arazzo-spec`) models `type`, `format`, `default` only;
  `contentEncoding` and `contentMediaType` land in extension leftovers.
  Validation resolves only a workflow-level `$ref` to
  `#/components/inputs/<name>`. Input validation checks `required` by key
  presence in the JSON input map (`input_validation.rs`); MCP always validates
  strictly.
- `serde_json` is built without `preserve_order`, so maps iterate in sorted
  key order everywhere, including the form encoder.
- reqwest is built with `default-features = false, features = ["json",
  "rustls-tls"]` — no content decoding. The response read loop caps the bytes
  it reads against `--max-response-size` (and the MCP cap).
- No code under `crates/*/src` handles `multipart`.
- MCP `max_request_bytes` defaults to 256 KiB (`arazzo-mcp/src/limits.rs`).
- Related drafts: [`arazzo-json-schema-2020-12-inputs.md`](arazzo-json-schema-2020-12-inputs.md)
  (replaces `PropertyDef` with a lossless input schema and validates the input
  map through a JSON Schema backend) and
  [`arazzo-runtime-http-decomposition.md`](arazzo-runtime-http-decomposition.md)
  (pure-move split of `engine_http.rs` against a pinned behavior baseline).

Ecosystem, as surveyed by the drafting agent on 2026-11-17 (not re-verified
in review; neither is evidence of what the spec allows): Jentic
`arazzo-engine` carries Python `bytes` as ordinary values, turns bytes entries
of a multipart map into file parts, and keeps binary responses as bytes;
Redocly `respect-core` supports multipart and octet-stream through a
`$file('path')` payload form — a grammar extension the compliance gate forbids.
Neither keys binary handling on `contentEncoding`; both treat bytes as a value
and both ship multipart. This plan matches that behavior without the syntax.

## Architecture

### D1. Bytes are an opaque value kind: side tables at rest, a typed body tree in flight

A `Blob` is immutable, reference-counted bytes plus optional `filename` and
`mediaType`, with an internal source that today is always memory and may later
be a file for streaming.

*At rest*, blobs never enter `serde_json::Value`. Every store that holds whole
slots — workflow inputs, workflow outputs, step outputs, `$response.body`,
`$request.body`, resolved sub-workflow inputs, and the `$workflows.<id>`
input and output stores — gains a blob map keyed like its value map. The blob
map lives inside the same struct as its value map, as `ProvenancedValues`
does, so no store can be copied (retry and `goto` inheritance, sub-workflow
calls, replay's recorded inputs) or serialized (`--json`, MCP results, DAP
scopes) without it; a copy that dropped it would make the slot genuinely
missing, which is the `null` fail-open below. `$request.body` is a blob slot
when the payload is a whole blob; a `$request.body` reference to a body tree
containing blob leaves is an error.

*Resolution is the choke point.* Expression resolution gains a third outcome
beside present and missing: a blob, returned only for a whole-value reference
to a blob slot. Because a missing value today degrades to `null` and a
warning, a blob slot must never look missing: no conversion from the blob
outcome to a JSON value exists, so each call site either accepts it or returns
a hard error. The accepting sites are a closed list — a request-body payload
(whole, or a map or array leaf), a workflow or step output, a step's
`$response.body`, and a parameter that passes an input to a `workflowId` step.
Everything else is an error with a dedicated diagnostic: pointers, `{}`
interpolation, criterion and Selector Object contexts (explicit or default),
`simple` condition operands, operation parameters in any location, replacement
targets and values, AsyncAPI `correlationId` and message payloads, and DAP
evaluate, watch, and breakpoint-condition expressions. Inside
`successCriteria` it fails the step through the same path as any other
criterion evaluation error today. The criterion default context is resolved
only by `regex`, `jsonpath`, and `xpath` criteria, so a `$statusCode == 200`
check on a PNG response does not touch the body. A surface not yet made blob-aware is in the
error set, not a pass-through.

*In flight*, payload resolution produces a request-body tree whose nodes are
JSON scalars, maps, arrays, or blob leaves. The tree exists only between
payload resolution and the body encoders; every encoder matches it
exhaustively, so a JSON or form encoder meeting a blob leaf is a compile-time
obligation to error, not a runtime convention. `replacements` operate on the
tree.

Rejected: a typed value enum through every value store (bytes are never
traversed, so the reach buys nothing the side table lacks, and it would
collide with the input-schema plan's own value-tree replacement); handles
embedded in `Value` (fail open — a sink that forgets the check uploads the
handle as JSON, and the handle leaks into traces, DAP, and stdout); and the
first draft's "unaware code sees a missing value" rule (review, 2026-11-17:
missing is `null` plus a warning here, so that rule would have sent `null`
where bytes were meant).

Repository choice: the spec says runtime expressions preserve the referenced
value's type and is silent on binary values. Decided by recommendation.

### D2. One media-type rule set for requests and responses

A single parser yields essence, structured-syntax suffix (`+json`, `+xml`), and
parameters (`charset`, `boundary`), case-insensitively. Every body decision
goes through it; substring matching on Content-Type is retired from
`client.rs` and `engine_http.rs`.

Responses, after transport content decoding; first matching row wins:

| Declared type | `$response.body` |
|---|---|
| empty body (including 204, 304, and `HEAD`) | absent |
| JSON essence or `+json` | parsed structure, read as UTF-8 with a leading BOM ignored and any `charset` parameter disregarded (RFC 8259); a parse failure errors when the body is referenced, so `$statusCode` criteria still run |
| XML essence or `+xml` | string decoded by `charset`, else a BOM, else the XML declaration's encoding, else UTF-8 |
| `text/*`, `application/x-www-form-urlencoded`, or any type carrying a `charset` parameter | string decoded by `charset`, UTF-8 if absent; an unknown label or a decoding failure errors when referenced, never lossy |
| absent or unparseable Content-Type | bytes (RFC 9110 permits assuming `application/octet-stream`; no sniffing) |
| anything else | bytes |

This closes audit F22's open follow-up by making `$response.body`
charset-aware and keeping it fail-closed. Default-context XPath selects over
the same decoded string as `context: $response.body`, so live runs and replay
(which rebuilds from decoded text) see identical input; the lossy default
context is retired. Each row is a repository choice within
§5.9's "typically"; decided by recommendation, including bytes for an absent
Content-Type.

Requests (payload resolved to the D1 tree first). An omitted `contentType`
follows the JSON rows:

| `contentType` | Payload | Wire |
|---|---|---|
| omitted | contains a blob | error: explicit `contentType` required |
| JSON essence, `+json`, or omitted | literal string in the document (with or without `{}` interpolation) | its UTF-8 text, verbatim (the spec's "JSON templated example") |
| JSON essence, `+json`, or omitted | any other value, including a whole runtime expression that resolves to a string | JSON serialization (so a string scalar is sent quoted) |
| JSON, form | contains a blob | error |
| `application/x-www-form-urlencoded` | map or string | existing form encoder, keys now in document order |
| `multipart/form-data` | map | D4 |
| `multipart/form-data` | anything else, or a `boundary` parameter in the document | error |
| any other | string | its UTF-8 bytes; a `charset` other than UTF-8 is an error, not a transcode |
| any other | number or boolean | its JSON text |
| any other | blob | raw bytes |
| any other | object, array, or null | error |

The runtime alone sets `Content-Type` from the step's `contentType`. A
`Content-Type` header parameter on a step with a request body is an error. A
blob's own `mediaType` never sets or overrides the request `Content-Type`; it
is used only for a multipart part.

The literal-versus-expression split for JSON strings is an **interpretation**
under the AGENTS.md ambiguity rule: the only spec example that sends a string
under `application/json` is a literal template, and "preserve the type of the
referenced value" supports serializing a referenced string as a JSON string.
Interpolation inserts the spec's string representation with no JSON escaping,
so a templated body is only as valid as its inputs; `validate` warns when a
literal JSON-typed payload with no interpolation is not JSON text. That check
reads the document, not runtime content. `payload: "{$expr}"` is the explicit
way to send pre-serialized JSON held in a string verbatim. Independent review
(2026-11-17) found this the best-supported reading: §5.9 grants "as-is"
treatment only to embedding ("If the value is already a string … it is
embedded as-is without modification").

`replacements` on a literal JSON-typed string payload parse the interpolated
text as JSON, apply the targets as JSON Pointers ("target MUST be interpreted
as JSON Pointer if the payload is application/json", §5.8.15), and re-serialize;
text that does not parse is an error.

Omitted `contentType` defaulting to JSON is a deviation from "refer to
Content-Type specified at the targeted operation", kept because the
serialization stance forbids reading the operation. It is recorded as a new
finding in the conformance audit; closing it is a stance change and a separate
initiative.

Content coding: enable reqwest gzip and brotli decoding, each new dependency
passing package review. The existing read loop then caps decoded bytes, which
bounds decompression bombs. The transport is expected to drop
`Content-Encoding` and `Content-Length` from a decoded response, hiding them
from `$response.header`; phase 1 confirms that and accepts it. A response
still carrying a `Content-Encoding` the runtime cannot decode is not decoded
by its Content-Type: its body errors when referenced, naming the coding.

### D3. Bytes enter and leave only through the CLI host

Document text never names a file. Bytes enter through `run` and `test`'s
`--input-file '<name>=<path>[;type=<media>][;filename=<name>]'`; the path runs
to the first recognized `;type=` or `;filename=` suffix and is otherwise taken
literally, with no environment expansion. The flag decides that the input is
file content; the input's property schema only converts or rejects:

| Property schema | Result |
|---|---|
| no `type`, and no `$ref`, `enum`, `const`, or applicator keyword (`allOf`, `anyOf`, `oneOf`, `not`, `if`) | bytes; `mediaType` from `;type=`, else the schema's `contentMediaType`, else none; `filename` from `;filename=`, else the path's basename (never a directory) |
| `type: string` + `contentEncoding: base64` or `base64url` | base64 (or base64url) string of the file |
| `type: string`, no `contentEncoding` | the file as strict UTF-8 text, unmodified |
| any other `type`, unknown `contentEncoding`, any `$ref` or applicator this runtime does not resolve | error |
| name not declared in the workflow's `inputs`, or also given by `--input`/`--input-json` | error |

The size cap is configurable and enforced while reading, not from file
metadata alone, so a growing file or a device cannot exceed it. Schema
`default` is never bytes. A blob input satisfies `required`; JSON Schema
validation of the input map runs over JSON inputs only, and blob inputs are
validated solely by this table. The table applies the JSON Schema 2020-12
content vocabulary as OAS 3.2 §4.24.4.3 tabulates it; `format: binary` has no
effect. Arazzo does not say how a host supplies input values, so this is a
repository choice, decided by recommendation.

This plan reads `type`, `contentEncoding`, and `contentMediaType` through the
current input model and does not wait for the input-schema plan. When that
plan lands it owns resolution (`$ref`, `$anchor`) and must preserve this table
and blob exclusion from its validator; until then, an unresolvable property
schema fails closed per the fourth row.

Bytes leave through `run`'s `--output-file <outputName>=<path>`. Before the
run starts the name must be a declared workflow output and the target
directory usable. The file is written only when the workflow succeeds and the
output is bytes — atomically, replacing any existing file (curl `-o`
precedent); a non-bytes or absent output at that point is an error.

Everywhere a value is serialized — `--json`, run exports, MCP results, DAP
variables, traces — a blob is the descriptor `{kind: "bytes", size, sha256,
mediaType?, filename?}`, published in `docs/schemas/`. The MCP server can
therefore run a workflow that downloads and re-uploads bytes; only ingress is
out of scope. Sensitivity is a property of the blob, set when it is loaded
from an input declared sensitive (`writeOnly`) and carried wherever it flows:
every descriptor rendered from it, in a request record, an output, or a trace,
is a fixed redacted marker with no hash, and replay treats two redacted blob
leaves as equal.

### D4. `multipart/form-data` from the Arazzo value alone

The payload must resolve to a map; each key is a part name, in document order.
Document order requires enabling `serde_json`'s `preserve_order` workspace-wide
(payload literals already arrive ordered from the YAML parser). Nested maps in
outputs and responses then keep document and response order; top-level input,
output, and step maps stay sorted because their stores are `BTreeMap`s. Map
equality stays order-insensitive, so replay comparison is unaffected.
Accepted under the no-compatibility direction, with goldens re-baselined in
phase 1. Decided by recommendation.

Part rules apply the OAS Encoding Object default table to the value kind,
because the runtime does not read the operation schema those defaults are
keyed on (repository choice; a known limit beside audit F16):

| Entry value | Part |
|---|---|
| string, number, boolean | text, no part `Content-Type` header (RFC 7578 default `text/plain`) |
| blob | the blob's `mediaType`, else `application/octet-stream`; `filename` from the blob, else the part name |
| object | `application/json` |
| array | one part per item, same name, in array order; an item that is itself an array is `application/json` |
| null, empty array | omitted |

Each request attempt gets a fresh random boundary, added to the wire
`Content-Type` only; the logical header that traces record and replay compares
stays `multipart/form-data`. Part names and filenames are sent as UTF-8 with
`"`, CR, and LF percent-escaped (the WHATWG form-submission convention for
RFC 7578 bodies; repository choice), and no `filename*`. No part headers beyond
`Content-Disposition` and `Content-Type`. The assembled body is buffered, so a
multipart request holds its blobs twice at peak.

### D5. Traces, replay, and redaction

The first draft retained every response body whole as base64 wire bytes.
Reversed in review (2026-11-17): traces are redacted evidence, and base64
would carry response secrets past the JSON-key and text-pattern redaction and
past `--trace-max-body-bytes`.

Traces get one new schema version, introduced in phase 1 with its final shape:

- A body D2 decodes to JSON or text is recorded as its decoded Unicode text and
  goes through the existing redaction and truncation unchanged. `body_lossy`
  is removed, since charset decoding is exact or an error. Replay rebuilds the
  body from that text by the recorded media type, without re-applying
  `charset` — as faithful as today's replay of a redacted, truncated body.
- A bytes body, or one that failed to decode, is recorded as its descriptor
  plus any decode error. Content is never written: redaction cannot inspect
  it. The same holds for request blob leaves and blob inputs.
- The trace marks blob slots out of band. Nothing infers a blob from a value
  that merely has the descriptor's shape.
- The published trace `ContentType` classification is replaced by the parsed
  media type.

Replay never touches the wire, so it restores a recorded blob as a
content-less blob carrying its descriptor: whole-value flow, outputs, and
request matching (by descriptor hash) work, and replay has no sink that needs
content — `replay` takes no `--output-file`. Decided by recommendation.

### D6. Placement and order

New responsibilities are new modules with single owners under `runtime_core/`:
the media-type parser, response body decoding, request body encoding (raw,
form, multipart), and the blob type with its side-table plumbing.
`engine_http.rs`, `client.rs`, and `payload.rs` lose their classification and
serialization branches to calls into those modules and gain no new logic. CLI
flags live in `cli.rs` and `handlers.rs` per AGENTS.md.

This plan runs before the HTTP decomposition plan. That plan's pinned behavior
baseline is taken after this plan completes; the two are never interleaved, so
no decomposition slice mixes a move with a behavior change.

Required order:

1. **Media types and response decoding** — D2 response half, content coding,
   charset, empty bodies, `preserve_order`, the D5 trace schema, and the audit
   finding for omitted `contentType`. Phase 1 also introduces the
   non-present error outcome in resolution (bytes or undecodable body, with no
   accepting sites yet), because without it a reference would degrade to
   `null`. Until phase 3 a bytes-classified body is classified and traced but
   not yet a value: any reference to it is the bytes diagnostic. Re-baselines
   goldens once.
2. **Blob core** — D1 at rest and in flight, D2 request table, D3 input side,
   descriptor output. The first slice is a tracer: one JPEG from
   `--input-file` to the wire, with every non-accepting site proven to error.
3. **Binary responses** — bytes `$response.body`, outputs, `--output-file`,
   blob replay.
4. **`multipart/form-data`** — D4, with validate-time diagnostics for
   statically detectable errors.
5. **Remaining surfaces** — DAP descriptors, sub-workflow blob propagation;
   both error on blobs until then.

## Verification and operational risks

Hermetic `tiny_http` tests assert exact wire bytes and headers for every row of
the D2, D3, and D4 tables, with a negative case for each error row (AGENTS.md
spec-surface rule). Multipart bodies are verified by parsing them back with an
independent multipart parser, not by string comparison. Golden, schema-drift,
and CLI-contract drift guards move in phases 1–3; each re-baseline is reviewed
against the table row that explains it.

Risks:

- **Fail-open blobs.** The central safety property: no sink ever serializes a
  stand-in for bytes, and none sends `null` in their place. Enforced by the
  blob resolution outcome with its closed accepting list and by the exhaustive
  body tree; verified by the phase 2 tracer and a test per non-accepting site.
- **Secrets in traces.** Response text in any charset still passes redaction
  and truncation, and no trace, export, or `--json` output contains blob
  content; a redaction canary test covers a non-UTF-8 text body.
- **Arbitrary file access.** Only CLI host flags read or write files; a test
  asserts no document field reaches the file loader or writer. Filenames sent
  to servers are basenames.
- **Memory.** Blobs are reference-counted, input files and decoded responses
  are capped as they are read, and the per-evaluation response-body clone is
  removed.
- **Behavior change.** Phase 1 changes `$response.body` for mislabeled and
  untyped responses — an API that omits Content-Type on JSON now needs a
  status or header criterion, not a body pointer — and changes JSON key order
  everywhere; both are intended and go in the CHANGELOG.
- **Rollback.** Each phase is an independent commit series on `main`;
  reverting a phase restores the prior behavior without data migration. Traces
  written by newer versions do not replay on older ones.

## System success condition

Against a hermetic test server, workflows run from the CLI:

1. upload a JPEG loaded with `--input-file` as `image/jpeg`, and the server
   receives byte-identical content;
2. re-upload identical and different bytes and observe 204 and 409;
3. upload the JPEG plus a caption and a tag array as `multipart/form-data`,
   file part last, and an independent multipart parser on the server recovers
   every part, name, order, filename, and media type;
4. download a PNG, re-upload it from a step output, and write it with
   `--output-file`, byte-identical to the source;
5. read a `text/plain; charset=iso-8859-1` and a gzip `application/json`
   response correctly, and survive a 307 redirect of the binary upload with
   identical bytes delivered;
6. replay their own traces — the multipart and binary runs included — to the
   same step outcomes and output descriptors;
7. leave traces and `--json` output that contain no body bytes and in which a
   secret planted in an ISO-8859-1 text response is redacted;

and every failure path in the workflows section produces its documented
diagnostic, with no request sent where the failure precedes the request.

## Out-of-scope follow-up

**MCP binary ingress.** `run` accepts only a JSON input map, and the 256 KiB
request limit admits about 190 KB of base64 — too small for real photos. The
options are a separate, larger limit for an `inputFiles` content map, or
accepting file paths under the server's existing document-authorization roots.
Recommendation: authorized-root paths, because the stdio server already reads
documents from those roots under the trust model in epic ac-664e5, and base64
through JSON-RPC doubles memory for no security gain. This is a security
surface, so it is excluded from this plan and belongs to the MCP epic.

---

**Disposition:** Ready for decomposition — two review passes, a revision pass,
and a targeted recheck of D1, D5, and the JSON interpretation (2026-11-17,
code at `5c28bf2`); the recheck's corrections are applied and the JSON
interpretation passed independent review. Steve directed auto-acceptance of
recommendations on 2026-11-17, which covers every decision marked "decided by
recommendation" here. MCP binary ingress is excluded and routed to epic
ac-664e5. Steve waived the `plans/current/` three-plan limit for now and
pre-approved new dependencies that pass the `add-package` skill (2026-11-17).

Accepted 2026-11-17 and decomposed into epic ac-6a548 (19 Tier 2 tickets). Decomposition placed the media-type parser in `crates/arazzo-spec/src/media_type.rs` rather than `runtime_core/` (D6), because `arazzo-validate` needs the same classifier and AGENTS.md's `operation_path.rs` precedent forbids a second parser; decided by recommendation.
