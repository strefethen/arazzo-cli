# Origin-scoped credentials for workflow execution

**Disposition: recommendation awaiting Steve; assessment only.** Prepared
2026-09-08 for [ac-d0c49](https://sonos.scapedeck.com/docs/ac-tickets/ac-d0c49).
No transport change or implementation acceptance is implied.

## Recommendation and scope

Make credentials belong to an exact origin. Require explicit origin bindings
for host-supplied secret defaults; bind workflow-supplied credentials to each
logical request's resolved initial origin. Retain them on same-origin redirects
and strip them on an origin change. Permit only explicit, directed, per-header
HTTPS transfer grants. Never infer trust from a source name, DNS equivalence,
TLS exception, redirect, or successful destination authorization.
The effective URL also exclusively determines HTTP authority on every hop;
caller or workflow headers cannot select a different virtual host.

The initial-binding guarantee assumes a trusted workflow author for values
supplied through workflow inputs/expressions. A step occurrence identifies the
producer; it does not prove that an input secret belongs to the selected host.
Host-managed secrets require trusted explicit origin bindings and must be
provided as scoped defaults. Giving an untrusted workflow raw secret inputs
falls outside this guarantee, including on the initial request.

This deliberately changes unsafe credential propagation and Referer behavior
while preserving ordinary custom headers, multiple service origins, existing
method/body redirect rules, and CLI/library destination access. It adds host
execution policy, not Arazzo fields, expressions, authentication discovery,
OpenAPI parameter serialization, OAuth, a secret store, or a config language.
A document may be valid Arazzo and be refused by execution policy. No spec
grammar or field meaning changes are proposed.

## Current behavior and decisive evidence

These are source facts at `423b017fefafd9948fc81f578cba1d31ec4cad69`, not a
claim that existing behavior was approved as security policy.

| Owner / anchor | Current contract and consequence |
|---|---|
| [client.rs](../../crates/arazzo-runtime/src/runtime_core/client.rs), `ClientConfig`, `HttpClient::request` | `default_headers` is an unscoped map applied to every logical request. HTTP names merge case-insensitively, defaults first and step headers replacing them. Redirects mutate this merged map; defaults are not re-added within a chain. |
| Same file, `is_cross_host_hop` | Exactly `prev.host_str() != next.host_str() \|\| prev.port_or_known_default() != next.port_or_known_default()`. Scheme is absent. Cross-host/port stripping removes only `Authorization`, `Cookie`, `Proxy-Authorization`, `WWW-Authenticate`, and `cookie2`. `X-API-Key` survives. |
| Same file, `HttpClient::request`, header merge and per-hop request construction | Caller/default/step `Host` is accepted into the mutable header map, which is cloned into each request. The runtime neither reserves authority nor removes Host across redirects, so URL-origin authorization alone cannot establish the effective HTTP virtual-host audience. |
| Same file, `HttpClient::new`, `request`, `make_referer` | Both reqwest clients disable automatic following; the manual loop builds each hop afresh. `Location` is joined to the previous URL. Referer drops userinfo and fragment but keeps the complete query. On HTTPS→HTTP the helper returns `None`; the caller leaves any existing Referer in the map. |
| [engine_http.rs](../../crates/arazzo-runtime/src/runtime_core/engine_http.rs), `prepare_http_request`, `build_url_from_path`, `warn_cleartext_credentials` | Target/source resolution and query assembly precede transport. Header parameters become headers; cookie parameters form `Cookie`. Cleartext warnings inspect initial prepared/default header names for only Authorization/Cookie, exempt URL-text loopback hosts, and deduplicate by host. They do not inspect subsequent hops, query/userinfo, or custom secret headers. |
| [redaction.rs](../../crates/arazzo-runtime/src/runtime_core/redaction.rs), `is_sensitive_key`, `redact_headers`, `redact_url_query` | Evidence classification uses four exact names plus substring stems. It recognizes `X-API-Key` but is not used by transport. URL redaction rewrites matching literal query keys only; it does not remove userinfo. |
| [handlers.rs](../../crates/arazzo-cli/src/handlers.rs) and [MCP handlers.rs](../../crates/arazzo-mcp/src/handlers.rs), `run_workflow` | CLI `-H` populates defaults. MCP builds default `ClientConfig` with timeout and offers no header or credential-policy tool arguments. Both use `EngineBuilder`. |
| [document_set.rs](../../crates/arazzo-runtime/src/runtime_core/document_set.rs), `DocumentSet` | Source-document identity/binding uses provided bytes or local files, with no remote document fetch. Effective operation servers, not source-document identity URLs, determine request origins. |

The local dependency source for reqwest 0.12.28, pinned by
[Cargo.lock](../../Cargo.lock), confirms `RequestBuilder::new` invokes
`extract_authority` and `basic_auth`. Because the manual loop creates a fresh
builder on every hop, userinfo in a redirect target can create new Basic
Authorization independently of the runtime's header-stripping decision.

[transport_characterization.rs](../../crates/arazzo-runtime/tests/transport_characterization.rs)
already proves same-origin Authorization/Cookie retention and cross-port
stripping while preserving `X-Custom`. Its Referer case contains no query.
[transport_tls.rs](../../crates/arazzo-runtime/tests/transport_tls.rs) proves
default downgrade refusal and the explicit downgrade exception, but its
downgrade test changes port too. It does not prove scheme participates in
credential identity. The hermetic evidence packet below adds the disputed
wire cases; its observations are not future-policy tests.

## Exact identity and credential provenance

`Origin` is `(scheme, host, effective_port)` from the same parsed HTTP(S) URL
used for dispatch. Scheme is lowercase; domain host uses the pinned `url`
parser's lowercase ASCII/IDNA result; IP literals use its canonical numeric
representation, with IPv6 brackets only in serialization. Compare the resulting
host value without DNS lookup. Preserve a terminal DNS dot as distinct; do not
equate DNS aliases, localhost names, IPv4 with mapped IPv6, or a resolved IP
with its hostname. Explicit default ports equal omitted ports (HTTP 80, HTTPS
443). Paths, query, fragments, and userinfo never participate. Thus
`https://EXAMPLE.com:443/a` equals `https://example.com/b`, while
`http://example.com:443` does not. Invalid URLs, non-HTTP(S) schemes, missing
hosts, and invalid ports cannot supply an authorized request origin.

Configuration accepts only absolute origin URLs with empty or `/` path and no
userinfo, query, or fragment. No wildcard, suffix, port range, source-name
alias, or scheme-less spelling is allowed. Invalid entries reject the
invocation before network activity; no permissive interpretation is attempted.

**HTTP authority is transport-owned.** Reject caller-, default-, or
workflow-supplied `Host` case-insensitively, even when its value matches the
URL. `:authority` is likewise reserved/invalid as a normal supplied header,
including case variants. Validate configured/default and workflow header-name
sources during invocation preflight, before network activity, and guard each
prepared request before send. This check precedes merging, so a step override
cannot hide a prohibited default. A caller cannot exempt either name through
a secret declaration, origin binding, or transfer grant.

For every initial send and redirect, derive HTTP/1.1 Host (and HTTP/2 authority
if HTTP/2 is supported later) from that hop's effective URL using the transport's normal host/port/IPv6
serialization. No supplied authority enters the active map or survives a hop.
Thus an ordinary A→B redirect contacts A with A's authority, then B with B's
authority. Explicit virtual-host routing is unsupported; any future trusted
feature must settle URL destination, HTTP authority, and TLS/SNI together.
This assessment adds no policy for unrelated control headers.

| Carrier | Classification and initial binding |
|---|---|
| HTTP authority | `Host` and normal-header `:authority` are reserved transport input, not ordinary custom headers or credentials eligible for bindings. Any supplied occurrence rejects before network activity; only the effective URL supplies their wire values. |
| Headers | Case-insensitive exact built-ins: `authorization`, `cookie`, `cookie2`, `x-api-key`, `api-key`, `set-cookie`, `www-authenticate`; the latter legacy protocol fields remain protected on redirects. `proxy-authorization` is classified but rejected in origin requests: its intended audience is a proxy, for which this API provides no authentication contract. |
| Other headers | Trusted configuration declares exact additional secret names, e.g. `X-Vendor-Key`. Do not use the evidence redactor's substring heuristic as the sending rule. `X-Custom`, `X-Request-ID`, and an undeclared non-secret `X-Session-Mode` remain ordinary headers. Unknown secret names need an explicit declaration; this is not automatic secret detection. |
| Query | Built-in ASCII-case-insensitive decoded names: `access_token`, `refresh_token`, `id_token`, `token`, `api_key`, `api-key`, `apikey`, `password`, `passwd`, `secret`, `client_secret`, `credential`, `session`, `sessionid`, `pwd`. Add exact trusted declarations or declare the entire query confidential for opaque/signed query formats. Inspect every occurrence after one form-url-decoding of the key; preserve original query bytes/order for sending. Values and response `Location` parameters do not grant authority. |
| Userinfo | Both username and password, including username-only token syntax, are confidential. Percent-decode each once as UTF-8; invalid decoded text rejects. Initial nonempty userinfo becomes one explicit Basic Authorization value using reqwest's Basic encoding, bound as an initial credential; remove userinfo from the URL before constructing any hop request or resolving relative Location. Reject a simultaneous effective merged Authorization value as ambiguous. Empty username with an explicit password delimiter still supplies a Basic credential; absent/empty userinfo without a password supplies none and is removed. |
| Host defaults | A classified default requires an explicit per-name allowed-origin set. An absent binding rejects configuration even if a step would override it. At each independent initial request, omit a default outside that set; otherwise apply it before step headers. Ordinary non-authority public defaults retain their current run-wide behavior. |
| Workflow/step values | After existing inheritance and resolution, classified explicit headers/query/userinfo bind to the resolved initial origin under the trusted-author assumption. A default Authorization bound to A does not forbid a separately authored step Authorization for B: they are different credentials. An override discards the default identity and cannot inherit its transfer grant. Case-insensitive duplicate names within one input layer reject as ambiguous; cross-layer step-over-default replacement remains. |
| Source descriptions | Neither a document identity URL, its name, nor its server URL is a credential grant. Resolve the effective request URL first and apply the same rules. Credentials in an effective server URL follow query/userinfo rules. A future remote document loader must separately design its own credentials; do not forward execution defaults to it. |

For every userinfo decision below, credential-bearing means a nonempty parsed
username or a present password, including an explicitly empty password.
An empty `@` without a password supplies no credential and is removed.

The runtime retains carrier/name, initial origin, input layer, logical-request
identity, and any matched grant as provenance. A default's identity is
`DefaultHeader(normalized_name)`; step provenance records workflow/step/carrier
and replaces the overridden default's identity. Values are never identity.
Cookies are explicit request credentials, not a browser cookie jar. Host secrets must
be supplied through scoped host configuration, not ambient default headers
or assumed-safe workflow inputs. A workflow already given a secret can copy
it into an ordinary header, body, or path; this policy is not information-flow
tracking and cannot sandbox such a workflow.

## Per-request and redirect decisions

Evaluate destination authorization, navigation policy (downgrade/hop limit),
and credential policy before each send. Every gate must permit the action;
no credential grant overrides destination/DNS/TLS restrictions. Build initial
credentials once. On redirects use the active map and its retained provenance;
never merge defaults again or regenerate Authorization from URL userinfo.

In this table, A and B are distinct exact origins; “secret headers” includes
Authorization, Cookie, X-API-Key, and declared custom secrets. “Strip” follows
the request without that header and emits a safe signal; “reject” means zero
requests to the rejected destination. Method/body rules remain 301/302/303
conversion versus 307/308 preservation.

| Case | Secret headers | URL credentials | Referer |
|---|---|---|---|
| Initial request with any supplied Host or `:authority` | Reject before any send, including matching values, case variants, and an override hidden by header replacement. No default, workflow value, or grant bypasses the guard. | The URL remains the authority source; no virtual-host override is supported. | No request is sent. |
| Initial request A | Send explicit secrets bound to A and defaults authorized for A; omit defaults bound elsewhere; reject unbound defaults. Proxy-Authorization rejects. | Send initial query unchanged; convert initial userinfo once as above. | For an explicit/default Referer, parse as an absolute HTTP(S) URL; reject malformed input, remove userinfo/query/fragment. If its origin equals A, retain its path; otherwise send only its origin plus `/`. HTTPS→HTTP Referer is omitted. |
| A→A | Retain active secrets. | Same-origin `Location` query, including a new secret query, is accepted. Never invent a query that URL joining removed. Any credential-bearing `Location` userinfo rejects, even for A. | Previous URL without userinfo/query/fragment; path retained. |
| A→B by host, port, or scheme | Strip every active secret header unless an eligible directed grant matches. Ordinary custom headers remain. | If resolved `Location` has any confidential query, reject the whole hop; do not rewrite signed URLs. Non-secret query remains. Credential-bearing userinfo always rejects. | Previous origin plus `/` only. |
| HTTP→HTTPS, same numeric port | An origin change: strip secrets. No HTTP-origin transfer grant is eligible. | Same cross-origin rule. | Previous HTTP origin only. |
| HTTPS→HTTP | Reject by existing default. With explicit `allow_downgrade_redirects`, follow only after stripping all secret headers; no transfer grant can permit cleartext transfer. | Reject if next URL has confidential query or credential-bearing userinfo. | Actively remove both supplied and previously generated Referer. |
| A→B→A or A→B→C | An earlier strip is permanent for the logical request. A return to A never resurrects credentials. A grant for A→B does not grant B→A or B→C. | Re-evaluate each resolved target; the current hop cannot bless a cross-origin secret query. | Recompute on every hop; downgrade always removes. |

All followed-redirect rows additionally require fresh URL-derived authority:
A→A uses A, A→B uses B, and B→A recomputes A. A supplied override is rejected
before the chain begins; it is never carried, silently stripped, or replaced
with a warning after an initial request has already been sent.

An allowed initial HTTP request still sends its bound credentials, preserving
current CLI/library behavior. Broaden `CleartextCredentials` detection to all
classified carriers actually sent and inspect every hop; keep existing
loopback exemption and stderr squelch semantics. HTTP origin binding is not
encryption, and `--insecure` remains an explicit loss of certificate assurance.
Neither flag creates credential-transfer permission.

An explicit trusted transfer grant contains exactly `(default_header_identity,
initial_origin, from_origin, to_origin)`. All three origins must use HTTPS;
the active credential must originate at `initial_origin`, still belong to that
host default, and its allowed-origin set must include `to_origin`. A grant
only retains that active header for that directed edge, never injects a new
value. Step overrides and userinfo-derived Authorization cannot inherit a
same-name default's grant. There is no transfer opt-in for workflow-owned
credentials: use a separate request at B. No wildcard, blanket
legacy mode, query/userinfo-in-Location exception, or Proxy-Authorization
exception exists. For A→B→C, two explicit grants with initial origin A are
required; a missing grant strips the header permanently. A destination needing
different credentials should be a new logical workflow request to that origin.

A `Location` can legitimately contain a fresh destination-signed download URL;
that is distinct from copying source query state. The client cannot establish
which occurred from a query key or value alone. URL joining determines whether
the source query was inherited, but a server can also copy it explicitly.
Recommend rejecting all cross-origin confidential target queries for this
first policy, including fresh signed URLs, rather than treating server input
as a grant. This intentionally breaks such redirect-based downloads when the
query is classified; use an explicit trusted workflow request to the target.
Unclassified query formats remain outside detection until declared (including
whole-query mode). A future destination-signed-URL exception would require its
own explicit trust contract, not a silent bypass here.

Referer is a derived navigation hint, never a credential carrier. Drop its
whole query even when keys look harmless, because arbitrary query names and
signed URLs evade name lists. Cross-origin Referer also loses the path.
These bytes intentionally change from current behavior. No opt-in can restore
credential-bearing Referer; pass an intentional application parameter instead.

## API, migration, and ownership

Add `Origin` and validated `CredentialPolicy` in one runtime owner,
`runtime_core/credential_policy.rs`, and an additive
`EngineBuilder::credential_policy(policy)` method. Policy contains declared
secret names, optional whole-query classification, allowed origins for each
`DefaultHeader` identity, and the directed header grants. Keep values in existing request/default
maps; the policy stores no secret values. The pure owner consumes parsed URLs,
provenance, active credentials, and policy and returns send/strip/reject plus
safe audit data. `client.rs` integrates that result into its existing loop;
it does not gain a second policy implementation. No new crate or package is
needed for this proposed boundary.

Keep `ClientConfig`'s existing fields and `EngineBuilder::client_config` source
compatible: adding a public struct field would break complete Rust struct
literals even though the crate is unpublished. The omitted credential policy
uses these secure defaults. This is a deliberate behavior change for unbound
secret defaults, Proxy-Authorization, supplied HTTP authority,
userinfo/Authorization ambiguity, credentialed redirects, and Referer; do not
market it as behavior-compatible.
Dynamic/external library consumers cannot be ruled out by a workspace search.
Keep public `RequestConfig` and evidence struct fields compatible too; carry
provenance through a private internal request envelope. Existing
[`Engine::new(spec)` and `Engine::with_client_config(spec, config)`](../../crates/arazzo-runtime/src/runtime_core/engine_impl.rs) continue
delegating to the builder and therefore receive its secure policy defaults.
The reserved-authority check applies equally to library header maps, CLI
`-H`/test headers, workflow header parameters, and future trusted MCP/DAP
adapters. Existing `-H 'Host: ...'` virtual-host use now fails, even if matching;
remove it when the URL already selects the intended host. No compatibility
switch or Host-setting policy field is proposed. A genuine virtual-host/TLS-SNI
feature requires a separately accepted trusted API.

| Surface | Trusted input and migration |
|---|---|
| Library | Caller constructs a validated policy and attaches it to the builder. Existing ordinary non-secret defaults work unchanged except the reserved authority headers. Secret defaults without bindings return a safe configuration error; callers add explicit bindings. |
| CLI run/test | One CLI adapter maps trusted operator arguments to the runtime policy. Proposed repeatable flags: `--secret-header NAME`, `--secret-query NAME`, `--credential-origin HEADER=ORIGIN`, and `--credential-transfer HEADER,INITIAL,FROM,TO` (origins cannot contain commas); `--secret-whole-query` declares the whole query confidential. Binding/grant flags refer only to `DefaultHeader(HEADER)` identity. Each parser rejects invalid input; repeated bindings/grants form sets, exact duplicates deduplicate, and a binding for a missing or unclassified default is invalid. Preserve `-H` for values; a secret `-H` now needs its binding. These are proposed new flags, not current help. |
| MCP | Only trusted startup/library host configuration may install declarations, bindings, or grants. Agent tool arguments, inputs, documents, and source descriptions cannot widen them. Until the host API is accepted, MCP uses the strict runtime default; no new permissive tool argument. Host policy remains capped by MCP destination restrictions. |
| DAP | Inherits the same runtime defaults. Any future adapter for trusted launch configuration is owned by invocation parity, not a new credential mechanism. |

Choose one coordinated security release with migration examples; no temporary
warn-and-forward default. For example, `-H 'X-API-Key: synthetic'` gains
`--credential-origin X-API-Key=https://api.example`. Independent requests
to another service still work but omit this default. Explicit step credentials
for each service continue to bind to their own initial origins. Release notes
must name each intentional behavior change and the permanent stripping rule.

The three boundaries remain independent:

* Destination/DNS authorization: [ac-950a8](https://sonos.scapedeck.com/docs/ac-tickets/ac-950a8)
  decides MCP restrictions; [ac-33962](https://sonos.scapedeck.com/docs/ac-tickets/ac-33962)
  is its provisional runtime implementation owner. The referenced
  `plans/current/mcp-security-hardening.md` is absent and is not accepted
  authority. Ordinary CLI/library destination access stays permissive.
* Credential delivery: this proposed runtime owner. No DNS lookup or destination
  exception matching belongs in it.
* Evidence sanitization: existing runtime `redaction.rs`, consumed by CLI trace
  and MCP projections. Reuse [ac-d95a9](https://sonos.scapedeck.com/docs/ac-tickets/ac-d95a9)
  for userinfo sanitization without URL normalization drift. It may ship first;
  new clean transport URLs do not erase old traces or dry-run/error inputs.
  [ac-f7790](https://sonos.scapedeck.com/docs/ac-tickets/ac-f7790) owns missing
  effective defaults in trace/dry-run evidence;
  [ac-b4a51](https://sonos.scapedeck.com/docs/ac-tickets/ac-b4a51) owns `api_key`
  and schema-informed redaction gaps. These are existing owners, not new epics.

[ac-5a709](https://sonos.scapedeck.com/docs/ac-tickets/ac-5a709) owns the shared
invocation mapping and surface trust maxima. Reconcile its proposed public
adapter before any CLI/MCP/DAP implementation; this assessment does not create
a second invocation package or persisted profile.

## Safe errors, evidence, and implementation sequence

Recommend one new stable `RUNTIME_CREDENTIAL_POLICY` error code with bounded
reason identifiers: `invalid_policy`, `unbound_default`,
`ambiguous_authorization`, `proxy_authorization_unsupported`,
`redirect_userinfo`, `redirect_secret_query`, `invalid_referer`, and
`ambiguous_header`, plus `invalid_userinfo` and `authority_override`.
For `authority_override`, report only the fixed reason and reserved header
name, never the supplied value; invalid configuration returns the safe error
before an execution event stream exists. Existing downgrade/limit codes stay unchanged. The accepted
follow-on must update schema authorities and affected consumers together;
this proposal does not silently overload an existing error code.

Add a typed runtime credential-policy audit event for `stripped`,
`default_omitted`, `grant_used`, or `rejected`, containing only safe reason,
carrier/name, provenance layer, normalized from/to origins, and hop index.
Do not serialize values, full URLs, query, userinfo, request bodies, or raw
library error chains into these new policy errors/events. Structured signals
survive warning squelch. Diagnostics must not echo malformed policy entries or
header values. Existing error paths that interpolate URLs remain a separate
uncovered sink, recorded below; do not claim that safe policy errors make every
CLI/library error safe. CLI/MCP display the new safe reasons; runtime
sanitization remains independent of whether a send was permitted. Matching a
redaction rule does not prohibit an intentional successful workflow output.

After Steve accepts the policy and fresh security/architecture review passes,
prepare these bounded slices; none is dispatch-ready now:

| Order / unit | One outcome and owner; behavior-owner Create maximum |
|---|---|
| 1 / `arazzo-runtime` | Consumer-free origin/policy/provenance decisions and public builder boundary. Create only `credential_policy.rs`; modify builder/export glue. Pure contract tests own normalization, validation, and positive/negative decisions. |
| 2 / `arazzo-runtime` | Integrate invocation/pre-send authority guards and URL-derived per-hop authority, initial credentials and every redirect, one-time userinfo conversion, Referer, cleartext detection, safe errors/events, and the two-origin wire matrix. Create no behavior owner; modify the existing client/engine warning/error/event seams. New proof uses a focused integration target, never the God `engine_execution.rs`. |
| 3 / existing evidence owners | Reconcile userinfo/default-header/redaction tickets with the accepted policy so traces/dry-run represent effective credentials safely, including policy-declared custom names. No replacement sanitizer. Sequence replay consumers after the shared projection is settled. |
| 4 / `arazzo-cli` | One shared run/test credential argument adapter and migration/help/schema projection. Create at most `credential_options.rs`; handlers only delegate. Focused CLI integration target, never the God `cli_integration.rs`. |
| 5 / `arazzo-mcp` | Trusted host configuration and rejection of tool-selected widening, coordinated with invocation parity and MCP policy. Create at most one host-policy adapter, or reuse its accepted owner; no mirrored runtime decisions. |
| 6 / affected consumers | Final same-policy CLI/library/MCP proof and public schema review; no behavior-owner Create. Resolve DAP parity through its existing owner. |

Dependency direction is front ends → runtime policy/transport, with no runtime
dependency on CLI/MCP, no new cycle, and no network-policy dependency on secret
values. This sequence preserves existing characterization except the explicitly
listed credential/Referer changes. It authorizes no God-module expansion.

## Future hermetic verification contract

Use synthetic sentinels and two local receiving origins A/B, extending to C for
the multi-hop case. Capture raw received headers, target and body plus receiver
counts; a failed workflow alone does not establish non-disclosure. Run each
header row with Authorization, Cookie, X-API-Key, API-Key, a declared vendor key,
mixed-case names and defaults versus step overrides; keep `X-Custom: keep-me`
as a positive control. Use local TLS fixtures and a local resolver, never
external DNS or services.

| Scenario | Required proof |
|---|---|
| Reserved authority, initial and redirect | Caller/default/step `Host`, mixed-case variants, matching and mismatching values, overwritten defaults, and normal `:authority` all fail with `authority_override` before network activity; A and B counts remain zero even when A would redirect. Ordinary requests without overrides observe A's URL authority at A and B's at B after A→B. Wire-test HTTP/1.1 Host with default/non-default ports and IPv6 serialization; test normal `:authority` input rejection now. Any future HTTP/2 enablement must add URL-derived authority wire proof; do not enable HTTP/2 solely for this policy. |
| Initial A and independent B | Bound A default appears only at A; B remains usable, including an independent same-name step credential. Unbound defaults reject before send. Step overrides replace defaults and their provenance once. |
| A→A, relative and absolute Location | Active secrets reach A; query semantics and 301/302/303 versus 307/308 bodies are preserved; Referer has no query/userinfo/fragment. |
| Same host/different port; different host/same port | B receives ordinary custom headers, zero secret headers; target non-secret query remains byte-preserving. |
| Scheme-only change on same numeric port | HTTP→HTTPS strips secrets despite equal host/port; a pure identity test and TLS/plain fixture verify the decision. |
| HTTPS→HTTP, default and opt-in | Default has zero HTTP requests. Opt-in sends no secret headers and no stale supplied/generated Referer. Confidential target query/userinfo still yields zero HTTP requests. |
| A→B→A and A→B→C | No resurrection. Exact HTTPS directed grant retains only its named header; missing/reversed/wrong-initial grant strips; second transfer requires its own grant. |
| Query and Location | Initial and same-origin confidential queries send; cross-origin confidential queries reject before B sees them. Cover repeated, percent-encoded and mixed-case names, username-only userinfo, whole-query declaration, relative query retention, and non-secret query controls. |
| Userinfo and Authorization | Initial Basic behavior matches the pinned dependency for valid escaped username/password; invalid UTF-8 rejects and clean hop URLs cannot regenerate Basic. Explicit Authorization conflict rejects. Every Location with credential-bearing userinfo rejects, same origin included; test empty-password and empty-`@` cases. |
| Referer | Explicit/default and generated Referer cover secret query, arbitrary query key, userinfo, fragment, same/cross origin and multi-hop downgrade; all forbidden bytes absent on the wire. |
| Normalization and policy | Case/IDNA/default port equivalence; terminal dot, IPv4/mapped IPv6 and aliases distinct; malformed origin/flag/duplicate/port rejection. Proxy-Authorization never reaches origin. |
| Source and surface boundaries | Supplied OpenAPI operation servers A/B bind separately; source identity cannot grant transfer. Library and CLI emit equivalent wire requests for equivalent host policy. MCP tool attempts to change bindings/grants fail while trusted host configuration works within its destination ceiling. |
| Evidence and compatibility | Scan complete CLI stdout/stderr, traces, dry-run, policy error/audit events and MCP frames for sentinels in new-policy scenarios, including rejected malformed values. Probe existing reqwest/error sinks separately as the residual below. Effective defaults appear redacted via their existing owner. Intentional successful outputs and trusted debugger request inspection are not broadly censored. Retain hop limits, cancellation, deadline, replay and method/body characterization; review schema changes rather than blindly rebaseline. |

## Residual findings and disposition

Two observed/source-confirmed concerns need a separate epic with bounded
follow-on tickets under Steve's standing rule; this drafting task does not
mutate tracker state or design their policy:

* Cross-origin 307/308 redirects preserve raw request bodies
  (`client.rs::HttpClient::request`, `current_body` handling). A body containing
  credentials or private business data is sent to B independently of header
  stripping. This assessment intentionally preserves current method/body
  semantics and therefore does not establish a whole-request confidentiality
  guarantee. Decide body transfer separately; do not build a taint engine here.
* Existing runtime URL/error sinks embed request or redirect URLs and upstream
  error text (`client.rs::HttpClient::request`, invalid-URL/send/downgrade/limit
  branches). The userinfo evidence ticket does not cover them.
  [ac-51f8c](https://sonos.scapedeck.com/docs/ac-tickets/ac-51f8c) owns MCP-only
  error projection and explicitly excludes runtime production and ordinary
  CLI errors. A broad runtime/plain-CLI sink repair is currently unowned;
  keep it separate from the bounded new-policy error contract.

Steve's remaining disposition is to accept or revise the recommendation,
particularly the breaking default-header migration, header-default-only HTTPS
grants, Proxy-Authorization and authority-override rejection, query-free/origin-reduced Referer, and
rejection of classified cross-origin signed Location URLs. Confirm the scoped
trusted-workflow guarantee and future API/error additions as part of that
acceptance. Fresh security/architecture review must precede implementation
decomposition. Completion of this assessment cannot close those future gates.

## Planning evidence binding

The retained contract is
[/private/tmp/ac-d0c49-contract-f71f0673.json](/private/tmp/ac-d0c49-contract-f71f0673.json),
SHA-256 `8f193720dd662ef4887ef86fe185e41c93c596ad2fa9e77433316bfbbe675c77`.
Its started revision and the live drafting revision are
`f71f06730e1739bbc46867675d793f18dc4b8e1ffc6e9dd55a53c5399a26095c`.
The linked decision owners were read through live `tkt show`:

| Owner | Verified revision |
|---|---|
| [ac-33962](https://sonos.scapedeck.com/docs/ac-tickets/ac-33962) | `9c9625f0ccfa7cbb1ea144016d82b897ab7c9ba9161cb145a9223081d19dbf5d` |
| [ac-950a8](https://sonos.scapedeck.com/docs/ac-tickets/ac-950a8) | `88b24cb07648185a950dd7747bea2345d9042cbb1451aab7e9478a7085f380c9` |
| [ac-d95a9](https://sonos.scapedeck.com/docs/ac-tickets/ac-d95a9) | `023d6fd2489d9b15ae2d8cf5d87e1be844de121eb620c858e0a1c8fb320442e4` |
| [ac-5a709](https://sonos.scapedeck.com/docs/ac-tickets/ac-5a709) | `9427c9dc9139a64b4e9f96d872fbf307ab866f226e1aae95e5eb730523b9888f` |

The [wire report](/private/tmp/ac-d0c49-wire-probe/report.md) and
[redacted observations](/private/tmp/ac-d0c49-wire-probe/redacted.json) retain
15 controlled local HTTP/TLS scenarios against the source commit above.
Actual receiving headers/targets, rather than only client results, establish:

| Observed scenario(s) | Current wire result |
|---|---|
| Same-origin multi-hop, defaults and step headers | All tested credentials and benign headers remain. A new path does not inherit the old query, but generated Referer contains it; later Referer uses the immediately previous URL. |
| Cross-port target query, defaults and step headers | Authorization/Cookie/Proxy-Authorization disappear; X-API-Key, X-Client-Secret and benign custom remain. Location query reaches B. Defaults and explicit headers share the same propagation behavior. |
| Host change at the same port (`127.0.0.1`→`localhost`) | The same standard-header removal/custom-header retention occurs; Referer names the old host. |
| HTTP→HTTPS at different ports | Standard secrets disappear, custom secrets remain, and Location query reaches B with an HTTP-source Referer. |
| HTTPS→HTTP default; HTTPS same-origin then permitted downgrade | Default refusal contacts no HTTP target. With permission, changed-port standard credentials disappear but custom secrets remain; the earlier automatically generated query-bearing HTTPS Referer survives to HTTP. |
| Both scheme-change directions at the same explicit port | All tested credential headers remain. The permitted HTTPS→HTTP case sends Authorization/Cookie/Proxy-Authorization to cleartext; the direct hop has no preexisting Referer. |
| Cross-origin then return with defaults | Removed standard credentials never return; custom secrets survive the whole chain. |
| Location userinfo/query | After source Authorization is stripped, target userinfo generates fresh Basic Authorization. The target query also reaches B. |
| Initial userinfo, with and without explicit Authorization | Userinfo alone generates Basic; explicit Authorization wins on the observed initial request. Userinfo is absent from the HTTP target and generated Referer. Rejecting this ambiguity is a proposed behavior change. |
| Cross-origin 307 | POST, JSON body, Content-Type and Content-Length reach B, along with custom secrets, despite standard-header stripping. This substantiates the separate body-transfer residual. |

Same-origin Location-userinfo precedence when an explicit Authorization header
is already retained remains **unproven**; the future matrix must test it,
including proof that the proposed rejection runs before any builder can mint
or replace Authorization. The initial-request precedence result does not prove
that case. Default-header merge order/provenance is additionally established
by the source owner, not inferred from redacted equality of different values.

Artifact SHA-256 values were independently checked before this candidate:

| Artifact | SHA-256 |
|---|---|
| Wire report | `0804638a8fe3fabb230cd6b3f549db014d037adb4e0cddcb713b9663b1f9cb41` |
| Redacted observations | `1d1571d49aed3b0d48e766554988c8cf5f0c66cbe3cb69b6a000cce2f9706a90` |
| [Raw synthetic-only observations](/private/tmp/ac-d0c49-wire-probe/raw.json) | `200d0827ea484642dbd3775295b5a55eb8d08b09a2181c0380409f8571465d4d` |
| [Driver source](/private/tmp/ac-d0c49-wire-probe/src/main.rs) | `d82484174c62fb57cd8f9b8c3b071668c0cf7af9eba04454b1517c9608d21942` |
| [Probe lock](/private/tmp/ac-d0c49-wire-probe/Cargo.lock) | `b812a75bbb0d09fd7d825659f878a8fa94a0cee7b47655b9382d0f3f1741cdd0` |
| [Built driver](/private/tmp/ac-d0c49-wire-probe/target/debug/ac-d0c49-wire-probe) | `639157f2686d5d0505298f2d03634dcfd8110e23602517b2641e96b224708e86` |

The report also binds the probe manifest/redactor and repository lock/client
hashes, all checked against their retained bytes. Earlier focused existing
transport tests passed in the agent's tool transcript. Later attempts to
retain fresh test logs failed at sandbox loopback binding (`Operation not
permitted`); those retained failure logs are infrastructure evidence, not
passing tests or product failures. The custom driver successfully completed
its 15 scenarios with controlled-loopback permission and offline locked
dependencies. The artifact is synthetic-only; no real credentials or upstream
services were used.

The [authority supplement](/private/tmp/ac-d0c49-host-probe/report.md), bound
to candidate `8ef458f5061778dd98c571f1586f3a325fd037d0` and the unchanged
runtime client hash, contains one additional public-engine HTTP probe. A
request to the URL-selected loopback listener supplied
`Host: override.invalid:4444`; the receiver observed that exact Host,
`/probe`, and a successful workflow. This proves the initial authority
mismatch. Host retention across redirects follows from the mutable-map source
path; it was not a second supplemental wire scenario. The current locked
reqwest/Hyper feature configuration is HTTP/1 only, so the supplement makes
no claim to have exercised HTTP/2 `:authority`.

These supplemental SHA-256 values were checked against the retained files:

| Artifact | SHA-256 |
|---|---|
| Authority report | `1f9478fd225aac4c5a245e9529347e3408394606d5f6ebc7bb054e61a1091d73` |
| [Authority driver](/private/tmp/ac-d0c49-host-probe/src/main.rs) | `921f83ae0e2f54e6d85c92f614de6d1abbf8f89898f972e33da01c9aac1b1690` |
| [Authority probe lock](/private/tmp/ac-d0c49-host-probe/Cargo.lock) | `2a560e824cf2718572bca176a6b6609ce018e0e2ee346928c33ca0239c4260eb` |
| [Authority binary](/private/tmp/ac-d0c49-host-probe/target/debug/ac-d0c49-host-probe) | `18fb2972c336f6bbb31bd850c594cdb16aa0238c1627f5fc5dd681473a2d7d04` |
| [Authority result](/private/tmp/ac-d0c49-host-probe/result.txt) | `e57a17b6e396316f125c65de69533256af9648706eec6c4371a91370fe7357ef` |

Lint is **FAIL**: `tkt lint --severity warn ac-d0c49` returns only
`plan-not-set`. Steve explicitly authorized planning assessments despite this
tooling-policy mismatch; no fabricated plan or passing lint is claimed.
Planning scope/checkout clearance is **PASS**: the started ticket has the
assessment reservation, current checkout is canonical `main`, and the only
authorized repository write is this file. Implementation readiness is **FAIL**
pending policy acceptance and a reviewed partition. The planning ticket is
in progress with no dependencies; source implementation queue eligibility has
not been assessed. The unrelated untracked Rust audit remains untouched.

No Cargo gates, packages, source changes, or external services are required for
this planning-only contract. Candidate review remains a separate required gate.
