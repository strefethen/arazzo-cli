# Form request-body serialization (L3)

Ticket: [ac-9394b](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9394b).
Assessment only. No runtime change is made here. Steve accepted the rules
below on 2026-10-18 (null and empty fields) and 2026-10-24 (rows 1–8). The
implementation is follow-on work (see the last section).

## The defect, reproduced at HEAD 41d85a4

`engine_http.rs::prepare_http_request` sends a non-JSON **string** payload as
raw bytes. Everything else goes through `serde_json::to_vec`. So a mapping
under `application/x-www-form-urlencoded` goes out as JSON.
`engine_workflow.rs::execute_request_body_content_type_and_method_selection`
asserts the form header but never checks the body.

Loopback probe: `git archive 41d85a4`, `cargo build -p arazzo-cli --offline`,
a Python capture server, no network. Run exit 0. The bytes captured for a
mapping payload:

```
Content-Type: application/x-www-form-urlencoded
{"address":{"zip":"99999+1234"},"city":"São Paulo ☕","client_id":"synthetic-client","empty":"","grant_type":"authorization_code","n":42,"nickname":null,"ok":true,"q":"a+b&c=d e","tag":["a","b",null,"c d"]}
```

A string payload `client_id=synthetic-client&scope=read+write` arrived
byte-identical. This matches the 2026-10-18 release evidence (L3,
`observed_request.body` is JSON under the form header). Under the rules below,
the mapping must instead send:

```
address=%7B%22zip%22%3A%2299999%2B1234%22%7D&city=S%C3%A3o+Paulo+%E2%98%95&client_id=synthetic-client&empty=&grant_type=authorization_code&n=42&ok=true&q=a%2Bb%26c%3Dd+e&tag=a&tag=b&tag=c+d
```

## Specification text (vendored, verbatim)

**Arazzo 1.1.0, Request Body Object** (`spec/arazzo/v1.1.0.html#request-body-object`):

> payload — A value representing the request body payload. The value can be a
> literal value or can contain Runtime Expressions or Selector Objects which
> MUST be evaluated prior to calling the referenced operation. To represent
> examples of media types that cannot be naturally represented in JSON or YAML,
> use a string value to contain the example, escaping where necessary.

> contentType — The Content-Type for the request content. If omitted then refer
> to Content-Type specified at the targeted operation to understand
> serialization requirements.

The Form Data example uses a mapping. The Form Data String example uses a
string. Both are quoted verbatim, including the typos in the published string
example (`code{…}`, `scope=$inputs.scope}`):

```yaml
contentType: application/x-www-form-urlencoded
payload:
  client_id: $inputs.clientId
  grant_type: $inputs.grantType
  redirect_uri: $inputs.redirectUri
  client_secret: $inputs.clientSecret
  code: $steps.browser-authorize.outputs.code
  scope: $inputs.scope
```

```yaml
contentType: application/x-www-form-urlencoded
payload: "client_id={$inputs.clientId}&grant_type={$inputs.grantType}&redirect_uri={$inputs.redirectUri}&client_secret={$inputs.clientSecret}&code{$steps.browser-authorize.outputs.code}&scope=$inputs.scope}"
```

**OAS 3.2.0 §4.15.3** (`#encoding-the-x-www-form-urlencoded-media-type`):

> This configuration means that the content MUST be percent-encoded per
> [WHATWG-URL]'s rules for that media type, after any complex objects have been
> serialized to a string representation.

> When there is no encoding field, the serialization strategy is based on the
> Encoding Object’s default values

Its worked example encodes an `address` object as JSON:
`address=%7B%22streetAddress%22%3A%22123+Example+Dr.%22…`, with this note:

> Note that the id keyword is treated as text/plain per the Encoding Object’s
> default behavior, and is serialized as-is.

**OAS 3.2.0 Encoding Object `contentType`** (`#encoding-content-type`). The
default table: string with `contentEncoding` absent → `text/plain`; *number,
integer, or boolean* → `text/plain`; **object → `application/json`**; array →
`application/json`. The text around it:

> Note that in the case of Encoding By Name, this value is the array item for
> properties of type "array", and the entire value for all other types.
> Therefore the array row in this table applies only to array values inside of
> a top-level array when encoding by name.

> Determining how to handle a type value of null depends on how null values are
> being serialized. If null values are entirely omitted, then the contentType is
> irrelevant. See Appendix B for a discussion of data type conversion options.

**OAS 3.2.0 §4.14.5.1 Encoding By Name** (`#encoding-by-name`):

> Array properties MUST be handled by applying the given Encoding Object to
> produce one encoded value per array item, each with the same name, as is
> recommended by [RFC7578] Section 4.3 for supplying multiple values per form
> field.

> The order of these name-value pairs in the target media type is
> implementation-defined.

**RFC6570-style fields** (`#encoding-style`, `#encoding-explode`): `style`
defaults to `"form"` *"only when contentType is not being used due to one or
both of explode or allowReserved being explicitly specified"*. With none of the
three fields present, the defaults come from `contentType`.

**Appendix B** (`#appendix-b-data-type-conversion`):

> However, there is no general-purpose specification for converting
> schema-validated non-UTF-8 primitive data types (or entire arrays or objects)
> to strings.

**Appendix E.2** (`#percent-encoding-and-form-urlencoded`):

> The form-urlencoded media type defines special meanings for = and & as
> delimiters, and + as the replacement for the space character (instead of its
> percent-encoded form of %20).

### Spec check on the two accepted rows that depend on it

- **Object field → `application/json`: supported.** The default table maps
  `object` to `application/json`, and the §4.15.3 example shows those bytes.
- **Array → repeated name: supported, by inference.** The repetition sentence
  sits in Encoding By Name, which opens *"To use the encoding field"*. Three
  passages apply it when no `encoding` field is present: the Media Type
  Object's `encoding` field (*"If no Encoding Object is provided for a
  property, the behavior is determined by the default values documented for
  the Encoding Object."*), the `contentType` note (*"the array item for
  properties of type "array""*), and §4.15.3's *"based on the Encoding
  Object’s default values"*. `explode` is not
  involved, because its `"form"` default only applies when
  `style`/`explode`/`allowReserved` is explicit. Leaving out null array items
  is our convention, not spec text.
- **The defaults are keyed on schema `type`.** This runtime reads no OpenAPI
  metadata, so the resolved JSON value's type stands in for the schema type.
  This is the existing value-only stance (AGENTS.md, "Parameter
  serialization"). Explicit OpenAPI `encoding`, `style`, `explode`,
  `allowReserved`, and `contentEncoding` are ignored, as they are for
  parameters. That is the same posture as F16, and it is tracked
  separately.

## Matrix

**S** = determined by vendored spec text. **C** = accepted convention (Steve,
on the date shown). All bytes come from a WHATWG
`application/x-www-form-urlencoded` serializer: unreserved characters are
`A–Z a–z 0–9 * - . _`, space becomes `+`, and everything else is UTF-8
percent-encoded. S byte rows rest on OAS §4.15.3's normative reference
to [WHATWG-URL] (*"MUST be percent-encoded per [WHATWG-URL]'s rules"*);
WHATWG-URL itself is not vendored in `spec/`. Keys follow the sorted order of `serde_json::Map`, which the
spec permits (*"implementation-defined"*).

### Top-level payload (after expressions, selectors, replacements)

| Resolved payload | Behavior | Basis |
|---|---|---|
| mapping | One pair per field, using the field rules below | S (Arazzo Form Data example; OAS §4.15.3) |
| string | Sent byte-for-byte, never re-encoded | S (Arazzo string guidance and Form Data String example); C row 4 (2026-10-24) |
| empty mapping / all fields null | Empty body, form Content-Type | consequence of the mapping rules |
| array, number, bool, null | Step fails before sending; no JSON-bytes fallback | C row 5 (2026-10-24). OAS defines by-name encoding only for name-value pairs |

### Field values inside a mapping

| Field value | Example in | Bytes out | Basis |
|---|---|---|---|
| string | `{"name":"Alice Smith"}` | `name=Alice+Smith` | S (text/plain, WHATWG) |
| empty string | `{"empty":""}` | `empty=` | C (2026-10-18) |
| null | `{"nickname":null}` | *(pair omitted)* | C (2026-10-18). OAS allows omission but does not require it. Rejected alternatives: `nickname=` (HTTPX/qs default) and `nickname=null` (JSON text) |
| reserved chars | `{"q":"a+b&c=d e"}` | `q=a%2Bb%26c%3Dd+e` | S (E.2, WHATWG) |
| Unicode | `{"city":"São Paulo ☕"}` | `city=S%C3%A3o+Paulo+%E2%98%95` | S (WHATWG UTF-8) |
| key needing encoding | `{"a b":"c~d"}` | `a+b=c%7Ed` | S (WHATWG encodes names too) |
| string that looks like JSON | `{"s":"\"quoted\""}` | `s=%22quoted%22` | S (text/plain, as-is) |
| number / bool | `{"n":42,"ok":true,"pi":1.5}` | `n=42&ok=true&pi=1.5` | C row 3 (2026-10-24). Text/plain is S; the string form is implementation-defined (App. B) |
| array | `{"tag":["a","b",null,"c d"]}` | `tag=a&tag=b&tag=c+d` | S repetition (inferred, above); null item omitted is C row 1 |
| empty array | `{"tag":[]}` | *(no pairs)* | S ("one encoded value per array item") |
| object | `{"meta":{"x":null,"y":[1,"two"]}}` | `meta=%7B%22x%22%3Anull%2C%22y%22%3A%5B1%2C%22two%22%5D%7D` | S (object → application/json); C row 2 |
| array item that is an object or array | `{"pts":[{"x":1},[1,2]]}` | `pts=%7B%22x%22%3A1%7D&pts=%5B1%2C2%5D` | S (per-item default: object/array → application/json) |

A null *inside* an object or array item is part of the JSON text, so it stays
`null`. The null-omission rule applies only to top-level field values and
array items. Inner nulls follow from row 2 and are not a separate rule.

The OAS §4.15.3 example checks out with one caveat. Our output is
`address=%7B%22city%22…%22streetAddress%22…%7D&id=f81d4fae-…`. JSON members
come out in sorted-key order, not the example's order. JSON member order has no
meaning, so this conforms but does not match byte-for-byte.

### Cross-cutting

| Concern | Behavior | Basis |
|---|---|---|
| Media-type match | ASCII case-insensitive on the essence; parameters (`; charset=…`) ignored. Content-Type header sent as authored | C row 6 (2026-10-24). The vendored text does not state this |
| Order of operations | Evaluate expressions and selectors → apply `replacements` to the logical value → encode once at send | Evaluation first is S (*"MUST be evaluated prior to calling"*). Replacements act on the payload, S (*"A list of locations and values to set within a payload."*). Encoding after replacements follows from C row 7 (logical value until transport) |
| Replacement target default for form | Unchanged (`infer_target_kind`). Spec defaults exist only for JSON and XML | not decided here |
| Unresolved required expression | Fails before sending (existing admission decision). Never treated as intentional null | 2026-10-18 note |
| Dry-run, trace, `$request.body` | Logical value (as today) | C row 7 (2026-10-24) |
| Live transport | Encoded bytes | C row 7 |
| `contentType` omitted | Not form-encoded. No OpenAPI lookup to find out | value-only stance |
| JSON and XML/SOAP | Unchanged: JSON via `serde_json`; non-JSON string sent raw | control |
| `querystring` (F16) | Out of scope, unchanged | AGENTS.md |

A consequence of row 4 that is not a new decision: a string template's
`{$expr}` values are interpolated raw, so a value containing `&` or `=` breaks
the form. That is the author's job (*"escaping where necessary"*), just as F16
leaves `querystring` to the author.

## Follow-on implementation and testing boundary

These are scope notes for the follow-on work. No tickets are filed here.

- **Encoder:** a new private `runtime_core/form_body.rs`. It owns the media-type
  test and value-to-bytes encoding, and is reached only from
  `prepare_http_request`'s body serialization. It holds no OpenAPI index and
  exposes no public API. It goes in its own file so `engine_http.rs` does not
  pick up encoding rules. For WHATWG bytes, use `url::form_urlencoded`
  (`url` 2 is already a direct dependency), so nothing new is added.
- **Failure:** a non-mapping, non-string form payload must surface as a step
  failure before sending. Today the serialization closure is infallible
  (`unwrap_or_default`). Any new `RUNTIME_*` code is a contract addition, so
  check `docs/schemas/` and the golden baselines.
- **Proof:** loopback tests that compare captured **body bytes** against each
  matrix row (spaces, `+`, `&`, `=`, Unicode, repeated arrays, JSON object
  fields, null omission, empty string, empty array). Also needed: a negative
  test per rejected top-level shape, showing no request reaches the server; a
  case-and-parameter media-type test; and a dry-run test showing the logical
  value. Tighten
  `execute_request_body_content_type_and_method_selection` to assert its body.
- **Controls that must stay green:** `replacements_overlay_json_payload_before_send`,
  `replacements_dry_run_emits_merged_body`, and the other replacement tests in
  `engine_execution.rs`; `engine_soap.rs`; `xpath_semantics.rs` replacement
  runs; `querystring_parameter.rs`; and the four drift guards.
- **Not in scope:** OpenAPI `encoding`/`style`/`explode` lookup, operation
  Content-Type inference when `contentType` is omitted, `multipart/form-data`,
  and F16.
