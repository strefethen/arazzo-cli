---
status: draft
epic: ac-852d1
created: 2026-11-05
---

# MCP security hardening for public v0.9.1

## Authority and disposition

**Disposition — 2026-11-07: superseded as release policy.** Steve adopted the
explicit operator-trust model for re-enabling `arazzo-cli serve`, decomposed in
[epic:ac-664e5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-664e5): stdio
only; the operator selects workflows and file roots at launch; the connected
client may execute them with the process's credentials; one startup notice and
no per-operation approval. That epic implements file-root enforcement on every
platform (replacing the macOS/Windows denial approved below), bounded frames,
backlog, results, bodies and documents with operator-configurable limits,
cancellation on disconnect, failure-text hygiene, a hermetic end-to-end suite,
and the restored `serve` command with documentation of what is and is not
restricted.

The following items in this draft are **no longer release prerequisites** and
remain future work under [epic:ac-852d1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-852d1):
restricted-destination network policy and DNS-to-connection binding
(ac-33962, ac-6f29d), Kafka-discovered socket interception (ac-6a8ca), the
exact decoded-node and intermediate-result accounting, the reviewed address
table, and the ac-950a8 decision gate. ac-fc556 and ac-da2e9 are closed as
superseded by ac-30242 and ac-abdc8. The text below is retained as the record
of the earlier approvals and their engineering gates; it does not describe the
shipped model.

Seed, verbatim from the 2026-11-05 assignment:

> Author grounded MCP hardening plan and implementation-ready ticket decomposition for public v0.9.1, under user explicit approval.
>
> User approved original ac-950a8 policies AND now: pinned root directories; reject dot descendants/.. /symlink or reparse traversal/multiply-linked regular files/unprovable platform aliases; exact normalized host/IP exceptions across ports, reviewed non-global blocking; framing 4KiB line/8KiB total/16 headers, one active plus one pending; safe error <=512B and string IDs<=128B. Resource ceilings 16docs/16MiB source,4MiB HTTP body,16MiB retained execution,depth64/100k decoded nodes,1MiB intermediate results, whole-operation failure (accounted data not RSS). Explicitly IMPLEMENT equivalent AsyncAPI file/broker discovery/connection controls, no disabling MCP message capability. Existing accepted 256KiB request/4MiB document/64KiB serialized response,30s HTTP/300s workflow defaults+max, proxies off, checked DNS/redirect no second lookup, secure absent/empty roots retained.
>
> Plan/decompose only when implementation ownership/contracts actually settled, flag precise unresolved engineering gate instead of pretending.

Historical decisions, including superseded sizes, remain in
[ac-950a8](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-950a8).
No earlier approval is erased. This draft records approved behavior; it does
not claim all engineering contracts are settled. Package selection, broker
connection injection, bounded decoding, and exact address-table review remain
gates below. Acceptance/decomposition waits for those gates and the independent
transfer/design audit the parent will dispatch. No renewed policy approval is
needed for the choices quoted above.

Filesystem approval, verbatim from Steve's 2026-11-05 design assignment:

> Record/refine MCP filesystem implementation contract after explicit Steve approval: v0.9.1 DENIES MCP agent-selected AND transitive file access on macOS/Windows until safe acquisition proven; implement secure Linux route, ordinary CLI unchanged. This capability reduction approved, no general reduction of remaining controls.

> Parent approves technical refinement only where settled. Linux rustix1.1.4 safe openat2 BENEATH|NO_SYMLINKS|NO_MAGICLINKS, no weaker fallback; same-handle metadata/regular/linkcount read bounded, authored traversal/base chain preserved incl OpenAPI AsyncAPI/fileURI.

This supersedes the requirement to prove successful macOS/Windows MCP file
acquisition for v0.9.1. It does not waive their denial tests, Linux containment,
dependency review, other resource controls, or broker/network work. Independent
readiness/design audit remains outstanding; the overall plan stays Draft.

## Outcome and workflows

An MCP client can discover and execute authorized workflows, validate and
describe authorized documents, and generate results, including AsyncAPI message
workflows, without obtaining ambient filesystem or non-public network access.
Oversize work fails as a whole, with a complete safe failure response. Work
already sent to external systems is not rolled back; an output failure explicitly
says that retrying can repeat changes.

Only operator-selected configuration, discovery directories and top-level startup
specifications loaded before protocol dispatch are trusted reads on every platform.
That trust never follows references: tool-triggered source acquisition, including
references authored in a trusted startup spec, is restricted. No tool can mark
input trusted, cause startup reload, or use a startup path as an exemption.
Already-loaded bytes may be used without a new read; lazy loads and cache misses
must use restricted acquisition. Successful list/describe fields, caller file
echoes, document metadata, generated YAML, workflow outputs, and redacted dry-run
requests remain intentional authorized outputs, subject to budgets. Diagnostic
fields inside successful validation results still require safe projection.
Standalone CLI execution remains permissive unless explicitly configured through
the runtime's additive restricted mechanisms. No Arazzo syntax or field meaning
changes, pagination, hidden truncation, broad refactor, release, or dependency
installation is part of this authoring task.

## Evidence that determines the boundaries

- `arazzo-mcp/src/state.rs::check_path_allowed` returns an authorization result
  before handlers reopen paths; `canonicalize_allowed_dirs` treats both absent
  and empty roots as unrestricted. These are behaviors to replace.
- `arazzo-mcp/src/handlers.rs::{validate_spec,generate_workflow,describe_openapi}`
  use path parsers. `run_workflow` clones the startup spec, prechecks transitive
  paths, constructs an engine, collects events, and builds an output value.
- `arazzo-runtime/src/runtime_core/source_document.rs::SourceDocumentLoader`
  currently covers OpenAPI only. `document_set.rs::bind_local_source` reads
  AsyncAPI bytes with `fs::read`; its file-URI resolution disproves the old
  ticket assertion that file URIs are unsupported.
- `runtime_core/client.rs::HttpClient` owns explicit redirect handling.
  `runtime_core/kafka/connection.rs::connect` instead creates an rskafka
  `ClientBuilder` from bootstrap strings; HTTP policy cannot protect it.
  The locally cached rskafka 0.6.0 `ClientBuilder` surface exposes no custom
  resolver/connector parameter. This is an integration gate, not proof that a
  bootstrap preflight can secure metadata refresh or leader connections.
- `arazzo-mcp/src/protocol.rs::{read_message,serve,write_message}` uses unbounded
  line reads, declared-size allocation, an unbounded decoded queue, and complete
  serialization. `handlers.rs::tool_ok` makes a second escaped representation.
- `runtime_core/events.rs::ExecutionHandle::collect` retains every event;
  `engine_impl.rs::execute_with_timeout_and_completed_workflows` starts a sleeper
  that outlives normal completion. `arazzo-spec::parse_unvalidated_bytes` and
  validator raw parsing construct YAML values before an MCP budget can inspect them.

## Owners and contracts

### Configuration and file acquisition

`arazzo-mcp` owns trusted configuration, root handles, tool policy, safe projection,
and wire limits. Extend `McpServerConfig` and `run_mcp_stdio_configured` now
present in the concurrent timeout candidate; preserve its fields and delegation.
Absent explicit roots derive canonical startup-spec parents;
explicit roots replace them, including explicit empty roots meaning deny-all.
No startup specs and no explicit roots also means deny-all. Capture startup cwd
once for relative tool arguments. Invalid roots fail startup, duplicate directory
identities coalesce. Startup stderr reports mode/root count and denied dot names,
without paths; stdout remains protocol-only.

Private `filesystem.rs` owns pinned directory capabilities and returns acquired
bytes plus document identity, never a checked pathname to reopen. Resolve tool
paths lexically against the captured base and a configured root; allow `.` and
ordinary extensions, reject `..`, dot-prefixed descendants, symlink/reparse
traversal, non-regular files, link count greater than one, alternate streams and
aliases whose identity cannot be proven. Explicit roots cannot override denials.
Root path replacement must not redirect the pinned capability. A renamed root
continues to denote the pinned directory object. This does not attest historical
file provenance before startup or prevent writes to already-authorized files.

On macOS/Windows, the acquisition owner denies every direct or transitive request
before inspecting or opening the selected target, regardless of roots, existence,
startup provenance or dry-run mode. No compatibility flag re-enables it. Root
configuration remains validated at trusted startup; valid roots cannot enable
reads on these platforms. Ordinary CLI native loaders remain unchanged.

Linux uses safe rustix 1.1.4 `openat2` relative to a pinned root with
`BENEATH | NO_SYMLINKS | NO_MAGICLINKS`; leaf flags are RDONLY, CLOEXEC,
NOFOLLOW and NONBLOCK. No openat/manual/native fallback is permitted on any
error, including unsupported kernels/seccomp. Containment-race EAGAIN denies
without retries (decided by recommendation). Metadata, regular-file and nlink==1
checks, bounded reads, and a final metadata check all use that owned descriptor.
Missing metadata denies. Link count is observed before/after reads, not a claim
of continuous alias stability or historical provenance. Roots are trusted startup
directory handles; configured path strings only select those handles thereafter.

`arazzo-runtime` owns URI resolution and document indexing. Add a published
`LocalSourceLoader::load_local(LocalSourceRequest)` seam covering both OpenAPI
and AsyncAPI actual local reads, including file URIs. Retain the old
`SourceDocumentLoader::load(name,path)` API for ordinary callers. A configured
local loader takes precedence, and its failure is terminal; it never falls through
to the old loader/native reads. MCP always installs the local loader.
The request preserves kind/name, raw authored reference, ordered authored base
chain (retrieval base and each `$self`/base contribution), resolved URI and path.
These are resolution facts, never authorization. Validate raw and exactly-once
URI-decoded components before relying on normalized paths. Native tool paths are
not percent-decoded. Supplied-byte identity binding remains read-free; this adds
no remote fetching or external-reference capability. Technical API refinements
and migration are retained in ac-94dc8; no trusted-origin bypass field exists.

MCP owns a shared per-call source-read budget (16 attempts, 16 MiB total, 4 MiB
per document), also used by the runtime adapter. Reserve count before acquisition,
debit chunks before retaining, and use a bounded one-byte overflow probe. Repeat
reads charge again; failure returns no partial document. This settles only source
acquisition accounting, not decoded/retained/intermediate construction budgets.
Filesystem access failures project to `MCP_FILE_DENIED` with fixed text
`File access denied by server policy`; source budget failures project to
`MCP_LIMIT_EXCEEDED` / `Operation exceeded a server resource limit`. Neither
response nor stderr includes native errors, paths, or canary content.

### Destinations and lifetime

Runtime `network_policy.rs` is the single normalization/classification owner;
HTTP and broker connectors consume its authorized address sets. MCP supplies
restricted policy; ordinary runtime defaults stay permissive. Normalize hosts
with the URL host parser to lowercase ASCII/IDNA, remove one trailing DNS dot,
canonicalize IPs, and convert IPv4-mapped IPv6 before matching/classification.
Exceptions match only exact normalized host/IP, across ports. No wildcard,
suffix, CIDR, agent override, or environment-proxy bypass. Host exceptions permit
that host's unicast answers; IP exceptions permit only that address. Reject an
entire mixed answer set if any address remains denied; resolution failure denies.
Unspecified, multicast and broadcast remain non-overridable. Non-global and
embedded-address transition/translation forms deny by default. A reviewed,
versioned prefix table with longest-prefix rules is required before transport
implementation; this draft deliberately does not invent that table from recall.

Each HTTP hop resolves/authorizes once and supplies exactly those addresses to
the connector while retaining hostname/SNI/certificate verification. Disable
proxies, including environment proxies. Connection pools must be scoped to the
immutable policy and authorized endpoint identity. Existing redirects, sensitive
header stripping, TLS behavior and whole-chain deadline remain intact.
AsyncAPI bootstrap, metadata discovery, leader changes, reconnects and every
broker socket require equivalent checks, bound addresses and cancellation.
Bootstrap-only checks or replacing hostnames with IP strings are insufficient.
MCP message execution must remain available; dependency limitations block the
release rather than silently disabling it. Dry-run/replay cause no DNS or socket
work; file reads and data construction remain restricted and budgeted.

MCP timeout defaults/maxima are 30 seconds HTTP and 300 seconds workflow.
Only JSON whole-number integers 1..=30 and 1..=300 are accepted; absence defaults,
all other types/ranges reject before engine construction. A completion signal
ends the watchdog on success/failure/drop; only actual expiry sets timeout before
cancellation. DNS, redirects and broker preparation must fit their operation
deadline. Checked deadline arithmetic protects permissive library callers too.

### Accounted representations and charge points

All byte units below are binary. Budgets are per tool invocation, including
transitive work, shared by child workflows/retries; they never reset per step.
Wire frames have separate connection-level bounds. Trusted startup storage is
outside per-call source acquisition, but clones/materializations during a call
are charged. These limits bound accounted data, not RSS or allocator overhead.

| Representation | Ceiling | Charge before |
|---|---:|---|
| Acquired documents | 16 documents, 16 MiB source total; 4 MiB each | Reserve each read attempt before open; debit each chunk before retaining it. Repeated reads count again; a bounded one-byte probe detects oversize without retaining a prefix as success. |
| HTTP response body | 4 MiB per body | Retaining each decoded body chunk; headers/Content-Length are only early rejection hints. Decompression must not precede the bound unchecked. |
| Decoded values | depth 64; 100,000 nodes per call | Creating scalar/container/key nodes, including alias expansion, parser conversion and copied trees. Root depth is one; map keys count as nodes. Charge cumulatively; no refund of construction work. |
| Retained execution data | 16 MiB simultaneously | Retaining/cloning input/state/output trees, source indexes, body buffers, queued events and dry-run records. Count owned string/key/raw-byte lengths plus 16 bytes per value node; copies charge separately, shared immutable storage once per allocation. Release only on drop/removal. |
| Intermediate tool results | 1 MiB simultaneously | Constructing result trees and their text/escaped copies; same node/string accounting plus serialized buffer lengths. Final wire cap is independent. |
| JSON request | 256 KiB | Reading payload; no unchecked declaration allocation. |
| Complete response JSON | 64 KiB | Writing into a bounded staging serializer, including envelope and escaped inner text, before any stdout write. Newline delimiter is additional framing. |

The accounting formulas above are decisions by recommendation within the approved
accounted-data policy. Construction reserves before allocation/clone; failure
cancels the operation and returns no partial success. Ordinary events unnecessary
to MCP may be drained/discarded, while all required successful fields remain.
Parser internals, YAML aliases, generated examples and expression results cannot
be bounded by walking a completed Value. The exact cross-crate budget carrier,
bounded decoding API and expression/generator insertion points remain engineering
gates; no ticket may claim the table implemented merely from post hoc counters.

### Framing and safe failure

Private MCP `framing.rs` owns a scanner capped at 4,096 bytes per header line,
8,192 header/preamble bytes including separators, and 16 nonblank headers.
JSON-line payloads use the 262,144-byte request cap, not the header-line cap.
Exactly one decimal Content-Length is required for that mode; duplicates,
overflow, malformed framing, invalid UTF-8, oversize or partial EOF terminate
without resynchronization. EOF between frames ends normally. Complete malformed
JSON receives a small parse error with null ID and does not execute.

One request executes serially; one bounded raw frame may be pending. A read
permit must be acquired before beginning that frame and returned when dispatch
takes it. A capacity-one channel alone is insufficient: a blocked producer can
otherwise hold a third frame. Decode only at dispatch. Bounded scanner lookahead
is allowed; no third request may be buffered by the application.

IDs are integers or strings of at most 128 UTF-8 bytes. Invalid IDs return
invalid-request with null ID, never echo their bytes. Notifications produce no
response. Private semantic projectors own fixed messages, at most 512 UTF-8 bytes,
with no raw source/OS/parser text. Tool failures retain `isError:true` and text
containing `{code,message}`. Preserve existing runtime codes; new runtime denial
classification requires an explicit compatibility review, not an invented code.
Safe numeric line/column may remain; diagnostics use typed categories and null
unsafe paths. Examples: `MCP_FILE_DENIED` / `File access denied by server policy`;
`MCP_LIMIT_EXCEEDED` / `Operation exceeded a server resource limit`.
Oversized execution output says `Output exceeded the server limit. Operations may
already have completed; retrying may repeat changes.` No raw-error truncation.

## Delivery order, gates and coverage

Existing [epic:ac-852d1](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-852d1)
remains the owner; completed byte-parser/loader foundations are retained.
The decision ticket remains open until dependent refinement and independent
review satisfy its existing criteria. No provisional ticket becomes ready through
this draft alone.

| Carrier | Outcome / remaining gate |
|---|---|
| ac-950a8 | Preserve approvals; settle mechanism contracts; independent design/transfer audit. |
| ac-452ce, ac-94dc8 | Refined direct and transitive filesystem slices; exact rustix direct-dependency review and independent readiness/design audit before dispatch. |
| ac-33962, ac-6f29d | Runtime HTTP policy then MCP config; reviewed address table and exact denial classification first. |
| ac-1aa44 | Timeout parsing and watchdog cleanup; first bounded candidate after gate/audit. |
| ac-fc556 | Framing/backpressure/serialization; second bounded candidate, aggregate construction remains a separate engineering gate carried by ac-950a8 until decomposed. |
| ac-e88a2, ac-62191, ac-79d9f, ac-51f8c | State, validation, generation and runtime safe projectors; exact per-kind field tables must be refined before dispatch. |
| ac-da2e9 | Observed-state disclosure after all controls and integrated proof, including broker and resource coverage. |

Package gate: review-package/add-package must vet rustix 1.1.4 as a direct
Linux-only dependency with the fs feature, including provenance, license,
advisories and the actual MSRV 1.88 target graph. The existing transitive lock
entry and source investigation do not clear that promotion. No package
installation is authorized by this design task. Linux must prove the retained
containment/alias contracts; macOS/Windows must prove unconditional denial.
Their safe enabling-backend proof is deferred to future re-enablement, not
required to implement the approved v0.9.1 denial.
Investigation owner: [ac-268cb](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-268cb).
Broker gate: select a supported connector injection/maintained dependency change
covering every rskafka discovered socket and cancellation. Do not author a ready
broker implementation ticket until that mechanism is demonstrated in source.
Investigation owner: [ac-6a8ca](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-6a8ca).
The decision ticket requires both investigations; the direct-file ticket also
directly requires capability vetting. Existing prerequisite edges are retained.
The current design author is the sole shared-plan writer; timeout and Kafka
agents own their tickets/candidates and do not acquire plan ownership here.
Budget gate: select a bounded decoder and budget ownership API across spec,
validator, generator, expressions and runtime without dependency cycles or God
module expansion. Existing parser APIs do not establish pre-expansion bounds.
Coordinate existing [epic:ac-c2849](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-c2849)
construction/evaluator investigations and
[ac-e8235](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-e8235) retained-execution
assessment. Their broader scope is not imported as a prerequisite or changed by
this packet. MCP budget settlement remains in the decision ticket.

## System success and retained review packet

One immutable assembled candidate must pass Linux direct/transitive file race and
alias rejection, native macOS/Windows direct/transitive denial before target
open, and trusted-startup/ordinary CLI regression on all release platforms;
mixed DNS, redirect/rebinding/proxy,
broker-discovery/reconnect denial with permitted-message success; exact limit
boundaries, YAML expansion and clone/event exhaustion; queue pressure; timeout
cleanup; and canary secret/path absence in responses and stderr. Test normal
outputs separately from failures. Use hermetic servers/fixtures with explicit
test exceptions and preserve ordinary CLI contracts. Existing checks and fresh
candidate review remain required; no builds were run during authoring.

Public v0.9.1 is blocked until those controls and integrated proof exist.
Rollback cannot silently restore insecure MCP behavior; an insecure earlier
version is not a safe compatibility fallback. New ceilings can reject previously
accepted large work, and external side effects can precede output rejection.
Concurrent source work makes this a source-grounded draft, not release evidence.

**Disposition — 2026-11-05:** Draft with approved filesystem platform reduction
and bounded technical refinement captured. Filesystem source-read APIs are
specified in ac-452ce/ac-94dc8; exact dependency promotion and independent audit
remain gates. Broker, cross-crate construction budgets, reviewed address table
and broader per-kind projection contracts remain unresolved. No full MCP
acceptance/decomposition or release claim follows from this refinement.

First filesystem source candidate is ac-452ce, then ac-94dc8. Neither is cleared
to start: ac-268cb review and existing ac-950a8 prerequisites remain open, and
checkout ownership must be checked after the concurrent timeout/Kafka work.
The two filesystem carriers now have explicit operation maps, migrated only for
those selected tickets using tkt. Migration receipts and frozen contract copies
are retained at /private/tmp/mcp-filesystem-contract-20261105/. The earlier epic
legacy-path metadata issue was not changed. Parent owns independent design and
readiness audit and any prerequisite disposition. No ticket was started or
closed; no source/build/commit work was performed by this design pass.

**Disposition — 2026-11-07:** Superseded as release policy by
[epic:ac-664e5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-664e5); see
the note at the top of this document. No v0.9.1 release claim follows from this
draft, and its remaining gates are not what blocks MCP re-enablement.
