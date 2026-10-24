# Redaction vocabulary contract — credential names beyond separator normalization

2026-10-24 · ticket [ac-2406c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-2406c) · source baseline `41d85a4`

**Status: accepted interim contract** (Steve, 2026-10-18 and 2026-10-24).
Implementation is a follow-on, sequenced after
[ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51). The likely
long-term answer is a user-supplied redaction list file, which is its own epic
([ac-c08e2](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c08e2)) and is
not designed here.

## What L5 showed, and who owns which part

Release probe (2026-10-18, fake canary, loopback fixture), dry-run JSON:

```
url:     /probe?api_key=SYNTHETIC-RELEASE-TRIAGE-CANARY&key=SYNTHETIC-RELEASE-TRIAGE-CANARY&access_token=[REDACTED]
headers: X-API-Key: [REDACTED]   X-Auth-Key: SYNTHETIC-RELEASE-TRIAGE-CANARY
```

| Exposed name | Owner |
|---|---|
| `api_key` | [ac-b4a51](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-b4a51), which strips `-`, `_`, and space so the name hits the existing `apikey` stem. It also covers top-level schema `writeOnly` / `format: password` inputs. Its AC4 bars any other new positives. |
| `X-Auth-Key` | This contract. |
| `key` | Stays nonsensitive by decision ("do not redact `key`, that's silly"). |

## Classifier at HEAD

All of this is in `crates/arazzo-runtime/src/runtime_core/redaction.rs`.

- `is_sensitive_key`: ASCII-lowercase, then one of two tests. Either an exact
  match against `SENSITIVE_EXACT` (4 names), or a `contains` match against
  `SENSITIVE_STEMS` (10 stems: password, passwd, secret, token, authorization,
  apikey, cookie, session, credential, pwd). It does not strip separators or
  percent-decode.
- `redact_url_query` splits the raw query on `&` and `=`. It classifies the
  name **as written**, still percent-encoded, and replaces only the value. The
  URL is never re-serialized, so trace replay sees no drift.
- `redact_text_patterns` matches free text with its own `RE_KV` regex:
  `(?i)(password|passwd|secret|token|authorization|apikey|api_key|credential|pwd)(\s*[:=]\s*)\S+`.
  That is a substring match with no anchors, separate from `is_sensitive_key`.

ac-b4a51 as specified, which this contract builds on, does three things. It
normalizes `-`, `_`, and space out of both the name and the table entries. It
leaves `RE_KV` with its own list. It threads a set of sensitive input names,
including the schema-declared ones, into `trace.inputs` and the `--verbose`
echo. This document does not depend on any uncommitted ac-b4a51 code.

## Sinks (HEAD)

| Sink | Path | Classification used |
|---|---|---|
| Trace file | `arazzo-cli/src/trace.rs::redact_trace_file` | `inputs` and step `outputs` via `redact_json_object` (recursive). Request and response headers. URL query, including redirect hop `from`/`to`. JSON bodies by key. Non-JSON bodies by `RE_KV`. |
| Run export | `arazzo-cli/src/run_export.rs` | `redact_values` on inputs and outputs: by key, then `RE_KV` on string values. Headers by name, else `RE_KV` on the value. URL query, then userinfo, then `RE_KV`. Error message via `sanitize_runtime_error` → `RE_KV`. |
| `--verbose` | `arazzo-cli/src/run.rs` (Inputs echo), `output.rs::emit_run_steps` (URL) | Inputs echo: **top-level names only, no recursion**. A `privateKey` nested inside an object input prints in clear even when the trace masks it. Step URLs via `redact_url_query`. |
| CLI dry-run | `arazzo-cli/src/output.rs::emit_dry_run_requests` | `redact_dry_run_request`: headers, URL query, JSON body. |
| MCP | `arazzo-mcp/src/handlers.rs::run_workflow` | Dry-run requests via `redacted_dry_run_request`. Success `outputs` are returned to the caller unmasked, by design and unchanged here. Errors carry the runtime message. |
| Free-text errors | `arazzo-runtime/src/runtime_core/control.rs` | Body preview of a failed step: `redact_json_value` for JSON, else `redact_text_patterns`. |

Out of scope, each with its own owner:

- Request-credential origin and redirect stripping uses reqwest's cross-host
  predicate (`client.rs::is_cross_host_hop`), not this classifier.
- Transport-error exposure:
  [ac-8a804](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8a804).
- Replaying redacted traces:
  [ac-72ba1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-72ba1).

## The two contracts considered

**A. Exact credential names (accepted).** Add five exact names, compared after
ac-b4a51 normalization: `privatekey`, `xauthkey`, `authkey`,
`subscriptionkey`, `ocpapimsubscriptionkey`. They are exact names, not stems.
The tradeoff is a near-zero false-positive rate in exchange for missing
prefixed or suffixed variants such as `x-private-key`.

**B. Author or config declaration (rejected for now).** Redact only what the
author marks.

- Zero false positives, but L5 stays open until every author marks every
  credential.
- Schema `writeOnly` / `format: password` (from ac-b4a51) already reaches
  top-level inputs. It cannot mark a header or query name written literally in
  a step.
- A new config surface is the ac-c08e2 epic, not this ticket.

## Classification matrix

Columns: **HEAD** is current code, **b4a51** is HEAD plus normalization, **A**
is b4a51 plus this contract. The names are fake and were checked against a
scratch re-implementation of the HEAD logic and both proposals. Each result
applies to every structured sink: header, JSON field at any depth, input,
output, and query name.

| Name | HEAD | b4a51 | A | Note |
|---|---|---|---|---|
| `key`, `Key` | – | – | – | decision: generic |
| `api_key`, `API_KEY` | – | ✓ | ✓ | ac-b4a51 |
| `apiKey`, `X-API-Key`, `access_token` | ✓ | ✓ | ✓ | controls |
| `private_key`, `privateKey`, `PRIVATE-KEY` | – | – | ✓ | |
| `X-Auth-Key`, `x_auth_key` | – | – | ✓ | L5 |
| `authKey` | – | – | ✓ | |
| `subscription-key` | – | – | ✓ | |
| `Ocp-Apim-Subscription-Key`, `ocp_apim_subscription_key` | – | – | ✓ | |
| `authorization_key` | ✓ | ✓ | ✓ | existing stem |
| `primaryKey`, `partition_key`, `sortKey`, `foreignKey` | – | – | – | database keys |
| `Idempotency-Key`, `publicKey`, `keyboardLayout` | – | – | – | business / non-secret |
| `x-private-key`, `privateKeyId`, `subscriptionKeyName` | – | – | – | exact match, no stem |

**Nesting.** JSON redaction walks every depth. A matching key's whole subtree
becomes `"[REDACTED]"`, and a non-matching key is recursed into. The
`--verbose` inputs echo is the only sink that does not recurse, so a nested
secret prints in clear there today, for existing stems as much as new names.
Closing that gap belongs to ac-b4a51's verbose/trace parity (its AC3), not to
this vocabulary contract.

**Case.** ASCII case-insensitive everywhere. Non-ASCII names are compared
without folding.

## Query-name encoding (accepted)

Percent-decode the query **name** before normalizing and classifying it. On
output, keep the name bytes exactly as written and replace only the value.

| Raw query name | HEAD | A |
|---|---|---|
| `x%2Dauth%2Dkey` | – | ✓ |
| `private%5Fkey`, `Private%20Key` | – | ✓ |
| `api%5Fkey` | – | ✓ |
| `%6Bey` (decodes to `key`) | – | – |
| `access%5Ftoken` | ✓ | ✓ |

Decoding applies to every query classification, so a name that encodes
existing-stem letters (`%74oken`) also becomes a positive. Decoding is the
accepted point 3 of the 2026-10-24 decision, not part of ac-b4a51 AC4. Header
and JSON names are not decoded. Malformed escapes and escapes that decode to
invalid UTF-8 are not covered by the decision; see residual question 3.

## Free text (accepted: mirror)

Add separator-tolerant alternates for the same five names to `RE_KV`, for
example `private[-_ ]?key` and `x[-_ ]?auth[-_ ]?key`. Free text is a
substring match by nature, so `foo_authkey=v` also matches there even though
it is exact-negative in structured sinks. Mirroring was accepted; the
resulting difference between the two paths is not yet confirmed (residual
question 2). At HEAD, `X-Auth-Key: CANARY` and `private_key=CANARY`
in a non-JSON body or an error message pass through in clear.

## Outputs, errors, replay

- **Successful outputs.** Persisted outputs (trace step outputs, run-export
  outputs) gain the new names. Outputs returned live to the caller (`run
  --json` success, MCP success) are still not masked.
- **Errors.** Masking reaches errors only through `RE_KV` and the JSON body
  preview walk. The new names arrive by mirroring.
- **Replay.** Classification happens when a trace is written. Traces recorded
  before the change still hold clear values and replay unchanged. New traces
  with these names join the existing set whose redacted values fail replay's
  literal equality. That is ac-72ba1's problem and is not widened in kind.
- **Compatibility.** Masking only grows, for exactly the five names and for
  encoded query names. There are no JSON shape changes and no new `RUNTIME_*`
  codes. `cli_contract_snapshots` and `golden_spec_baseline` should not move.

## Implementation boundary (follow-on, after ac-b4a51)

One owner: `redaction.rs`. Add the five names as an exact table compared under
ac-b4a51's normalization, decode the name in `redact_url_query`, and mirror the
names in `RE_KV`. Make no changes at the sink call sites. Tests: the matrix above as positive and negative cases,
including `key` and `keyboardLayout` as pinned negatives, the encoded-name
table, and one `RE_KV` free-text case per name. Add no stems, no config, and
no schema changes.

## Residual questions for acceptance

1. Should a `+` in a query name decode to a space, as
   `application/x-www-form-urlencoded` does? If so, `private+key` → ✓. As
   written, only `%XX` is decoded and it stays negative.
2. Confirm that prefixed variants such as `x-private-key` and
   `x-subscription-key` stay negative under exact matching, given that the
   mirrored `RE_KV` alternates would redact them in free text, and would also
   redact the next token after prose such as `private key: expected PEM` in an
   error message.
3. How should a malformed escape, or one that decodes to invalid UTF-8, be
   classified? The proposal is to classify the name as written, undecoded.
