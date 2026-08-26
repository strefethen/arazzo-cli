# AGENTS.md — arazzo-cli

Runtime executor for the **Arazzo Specification v1.1.0**
(<https://spec.openapis.org/arazzo/latest.html>). Rust workspace, MSRV 1.82,
`unsafe` forbidden. `CLAUDE.md` imports this file; edit this one.

Run the CLI with `--help` to discover commands and usage. None of that belongs
in this file.

## Compliance gate

**The specification is the sole authority on what an Arazzo document may contain
and what it means. Do not extend it.**

Applies to any change touching document shape, field meaning, the `$…`
expression grammar, the `successCriteria` condition grammar, or accepted
`arazzo:` versions. For those:

- Implement what v1.1.0 says, nothing more. Not in the spec → stop and ask.
- Quote the spec in your plan, from [`spec/`](spec/README.md) — verbatim
  vendored copies of the published documents, so there is no network round trip
  and no drift between what you cited and what you implemented. Do not work
  from memory; model recall of Arazzo is unreliable and is how the existing
  deviations got written.
- **`spec/` is the one citable directory.** Never cite this repo's code, docs,
  `examples/`, or `testdata/` as evidence of what the spec allows — they
  contain known non-conformances. `spec/` is exempt because it is the published
  text itself, byte-for-byte, not our description of it.
- Arazzo defers to OpenAPI by reference — "per OpenAPI constraints". The
  constraints themselves live in [`spec/oas/v3.2.0.html`](spec/oas/v3.2.0.html),
  and a rule is only half-implemented if you stop at the Arazzo sentence.
- Existing deviations are debts, not precedent. Never add one because similar
  ones exist.

**Approved extensions: none.** The allowlist is empty and maintainer-owned; an
agent never adds to it.

Known deviations are tracked in
[`plans/assessments/arazzo-spec-conformance-audit.md`](plans/assessments/arazzo-spec-conformance-audit.md)
— read it before touching spec surface. Closing one of its findings, or
implementing a spec field we don't model yet, needs no approval.

Everything else — CLI, output formats, tracing, debugger, MCP, perf, refactors
— is ordinary work, no gate.

## Build

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

All three before committing. CI tracks unpinned `stable`.

## Layout

| Crate | Notes |
|---|---|
| `arazzo-spec` | Typed model. `operation_path.rs` is the one `operationPath` classifier — runtime and validate must not each parse it. |
| `arazzo-validate` | Parse + structural validation; diagnostics carry severity, `--strict` promotes warnings to errors. |
| `arazzo-expr` | Evaluator. Namespace dispatch is the match in `evaluate_with_diagnostics`. |
| `arazzo-runtime` | Engine. `runtime_core/` = execution, `debug/` = DAP backend. |
| `arazzo-generate` | Arazzo from OpenAPI; the `openapiv3` crate models 3.0.x only. |
| `arazzo-cli` | `cli.rs` = clap defs, `handlers.rs` = bodies. **Not `main.rs`.** |
| `arazzo-mcp`, `arazzo-debug-adapter` | MCP stdio server; full DAP. |

[`spec/`](spec/README.md) holds the vendored specification documents: Arazzo
1.1.0 and 1.0.1, OpenAPI 3.2.0. Never hand-edit them — `./spec/fetch.sh`
re-downloads and rewrites `SHA256SUMS`, `--verify` checks the working copies.

`RUNTIME_*` codes (`runtime_core/error.rs`) appear in `--json` and in golden
baselines — changing one is a breaking contract change.

The VSIX in `vscode-arazzo-debug/` bundles a `--release` build of
`arazzo-debug-adapter`; build the binary before packaging.

## Expression surface (v1.1.0)

`$url` `$method` `$statusCode` `$self` ·
`$request.header|query|path.<name>` · `$request.body[#/ptr]` ·
`$response.header.<name>` · `$response.body[#/ptr]` ·
`$response.query|path.<name>` ·
`$message.header.<name>` · `$message.payload[#/ptr]` ·
`$inputs.<name>[#/ptr]` · `$outputs.<name>[#/ptr]` ·
`$steps.<id>.outputs.<name>[#/ptr]` ·
`$workflows.<id>.inputs|outputs.<name>[#/ptr]` ·
`$sourceDescriptions.<name>.<ref-id>` ·
`$components.parameters|successActions|failureActions.<name>` ·
`{$expr}` string interpolation

**This list is the spec, not our implementation.** Some entries aren't
implemented; some implemented forms aren't here. Anything absent is not Arazzo —
check the audit before using it.

`simple` conditions, complete grammar: `< <= > >= == != ! && || () [] .` over
`boolean` / `null` / `number` / `string`.

Selector Object = `context` + `selector` + `type` (`jsonpath` | `jsonpointer` |
`xpath`, string or versioned object). A mapping is a selector only if it
satisfies all three fields; otherwise it stays a literal. Zero/one/many matches
normalize to `null` / scalar / ordered array.

## Parameter serialization

**Parameters serialize from the Arazzo value alone.** The runtime never reads
the referenced OpenAPI operation's parameter definitions — not `content`, not
`style`, not `explode`. From a source description it takes `paths`, `servers`,
`url`, and `variables`, nothing else.

That is a deliberate stance, not an oversight, and it is uniform across every
`in:` location — so don't make one location consult OpenAPI metadata because a
spec sentence mentions it. The spec sentences that do (Parameter Object:
a `querystring` value *"MUST match the media type format as expressed by the
parameter's `content` field"*) address the document author, not our serializer.

The known consequence is F16 in the conformance audit: a `querystring` value
whose operation declares a non-form media type is inserted verbatim and
unencoded. Changing the stance is a workspace-wide feature, not a per-location
fix — new ticket, not an inline decision.

## Conventions

- Generic Arazzo executor — no domain-specific logic. Smithy is out of scope.
- Every command keeps a `--json` contract with structured errors; diagnostics to
  stderr. Changing one means refreshing `docs/schemas/`.
- Hermetic tests, no network (`tiny_http` for servers).
- Spec-surface tests need a negative case rejecting the non-conformant shape,
  not just a positive one.
- Planning docs live in `plans/`, never `docs/` (user-facing).
- **Never** pipe a verification command in a background task — redirect to a file and echo $?; or set set -o pipefail in the harness's background shell.

`tests/` holds four drift guards: `golden_spec_baseline`, `schema_drift`,
`operation_path_agreement`, `cli_contract_snapshots`. A failure is the signal —
don't re-baseline without knowing why output moved.
