---
status: accepted
epic: ac-8ba62
created: 2026-10-29
---

# Root document parse and validation benchmark

**Seed:** Steve's “Accept design and planning (Recommended)” decision is recorded in
[ac-0be12](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0be12). The accepted
[workload assessment](../assessments/arazzo-engine-workload-benchmarks.md) owns the
six families and measurement boundaries. This plan settles only its first,
root-loading slice. Acceptance of the umbrella design is not being reopened.

## Outcome and non-goals

Give maintainers a reproducible local Criterion series for Arazzo root bytes →
validated model and diagnostics at 10, 100, and 1,000 steps. A positive result is
reportable only when the actual parser results match the fixture's independently
specified model and diagnostic expectations.

This slice does not time source loading/indexing, workflow execution, CLI startup,
CPU/RSS, cancellation, or CI. It changes no parser behavior, production API, or
resource limit. It selects no regression threshold and does not claim that these
small fixtures characterize arbitrary documents or the other five families.

## User-visible workflows

A maintainer prepares an evidence directory, records the fixture and executable
hashes and host/build provenance, then runs the dedicated `root_load` benchmark
in `arazzo-validate`. Its three positive series identify step count and serialized
bytes. All positive fixtures and the fixed local OpenAPI source pass untimed
preflight before sampling begins. Separately named malformed-root and duplicate-
step controls must produce their expected failure classes; they are correctness
controls, never successful latency samples.

Criterion owns sampling, warmup, estimates, confidence intervals and raw sample
artifacts. A parse error, unexpected warning, incorrect model, bad source fixture,
or incorrect negative-control outcome fails the invocation and invalidates its
positive report. A prior or partial Criterion result cannot be presented as the
failed invocation's successful baseline. Comparisons use distinct evidence
folders, the same host/configuration, and recorded revision order.

## Decision-relevant facts

The existing public `parse_bytes_with_diagnostics` in
[`arazzo-validate/src/lib.rs`](../../crates/arazzo-validate/src/lib.rs) returns
`Result<(ArazzoSpec, Vec<Diagnostic>), Error>`. It already performs raw and typed
parsing, component resolution, structural/raw-document checks and diagnostic
partitioning. Calling `validate` afterward would measure a different pipeline.
The API reads no source files. Its raw and typed parsing passes remain part of
one measured public call; this plan does not bypass either.

The [workspace manifest](../../Cargo.toml) owns Criterion 0.5 with `html_reports`;
[expression](../../crates/arazzo-expr/Cargo.toml) and
[runtime](../../crates/arazzo-runtime/Cargo.toml) inherit it for their existing
benches. The lockfile resolves Criterion 0.5.1.
[`arazzo-validate/Cargo.toml`](../../crates/arazzo-validate/Cargo.toml) currently has
neither a dev-dependency section nor a benchmark target. Recommend inheriting
that exact workspace declaration, without a version/feature change. This is a
new crate-local development dependency edge. Steve explicitly approved this
bounded reuse on 2026-10-29 despite the recorded package-evidence warnings;
existing installation alone was not approval.
[ac-8b448](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8b448) tracks the
decision, with bounded package evidence in the
[dependency proposal](../security/decisions/2026-10-29-criterion-validate-bench.md).

The known validator God module remains contained under
[`god-files.md`](../../god-files.md). Benchmark orchestration and fixture checks
belong outside production `src/`; this plan requires zero edits to that module
and no new exported symbols.

The [conformance audit](../assessments/arazzo-spec-conformance-audit.md), notably
F1/F4/F5 and F6/F7/F8 and their resolution notes, rules out treating old examples
as fixture authority. The following vendored specification text governs the
minimal selected shape:

- [Arazzo root](../../spec/arazzo/v1.1.0.html#arazzo-specification-object):
  `sourceDescriptions` and `workflows` each state “The list MUST have at least
  one entry.” The required `arazzo` is `1.1.0`; required `info.title` and
  `info.version` are supplied under the [Info Object](../../spec/arazzo/v1.1.0.html#info-object).
- [Source Description](../../spec/arazzo/v1.1.0.html#source-description-object):
  `url` is “A URL to a source description to be used by a workflow.” For relative
  references, it “MUST be in the form of a URI-reference”. One named `openapi`
  source points to the actual adjacent local document, never an API base URL.
- [Workflow](../../spec/arazzo/v1.1.0.html#workflow-object): “The id MUST be unique
  amongst all workflows described in the Arazzo Description.” `steps` is an
  “ordered list of steps”.
- [Step](../../spec/arazzo/v1.1.0.html#step-object): “The stepId MUST be unique
  amongst all steps described in the workflow.” `operationId` names “an
  existing, resolvable operation”, and is mutually exclusive with `operationPath`
  and `workflowId`. One source makes unqualified operation IDs unambiguous.
- [OpenAPI root](../../spec/oas/v3.2.0.html#openapi-object): “at least one of the
  components, paths, or webhooks fields MUST be present.” The local source uses
  `openapi: 3.2.0`, required Info metadata, and ten concrete GET paths.
  [Operation](../../spec/oas/v3.2.0.html#operation-object): “The id MUST be unique
  among all operations described in the API.”
  [Responses](../../spec/oas/v3.2.0.html#responses-object): “The Responses Object
  MUST contain at least one response code”. Each operation has a documented
  `200` response. No schema, parameter serialization, reference expansion, or
  expression interpretation is needed by these fixtures.

## Architecture

The new Cargo benchmark target `root_load` belongs to `arazzo-validate` and
consumes only its existing public parser and the existing public model types.
A focused integration-test target verifies the same private fixture/oracle
support. The support is benchmark-local, with no runtime/CLI consumer and no
published interface. Dependency direction remains benchmark/test → validate
and spec; production units never depend on the benchmark.

Use three committed YAML input snapshots, consumed directly by the benchmark,
and one fixed local OpenAPI document with ten uniquely named operations. Each
root contains one `root_load` workflow, a distinct ordered sequence of step IDs,
and operation references cycling through those ten operations. Root bytes are
capped at 2 MiB per case. The fixed source remains well below the assessment's
4 MiB aggregate source cap. These are experimental fixture bounds, not admission
policy. Fixture generation, disk reads, hashing and source checking all precede
measurement. The support checks the complete deliberately small OpenAPI shape
against independent expected values and resolves every fixture operation ID;
it does not introduce a general OpenAPI validator or runtime source index.

For each Criterion iteration, black-box the prepared immutable bytes, start a
monotonic timer immediately before exactly one call to
`arazzo_validate::parse_bytes_with_diagnostics`, and stop it immediately upon
return. Check the result against the independent expected model and zero-warning
oracle after timing, then release it before the next iteration. Sum those
intervals using Criterion's existing `iter_custom` mechanism. Timer overhead is
included; result destruction, correctness checks, fixture work, source indexing
and a second validation call are excluded. Internal temporary allocation and
destruction inside the parser remain included. Memory use does not accumulate
one parsed document per Criterion iteration.

All cases receive untimed preflight before any series is sampled. Independent
malformed YAML and syntactically valid duplicate-step fixtures prove parsing and
semantic validation failures respectively. Further verifier tests perturb a
positive model's identity, operation target, step order/count and diagnostics so
an accepting-but-wrong parser cannot pass merely because it returned `Ok`.
These checks concern the benchmark oracle, not new production validation rules.

The local evidence bundle retains native Criterion outputs, process exit and
logs, fixture byte counts/SHA-256 hashes, semantic-control outcomes, iteration
success/failure accounting, and the assessment's applicable host/build provenance.
A short usage document prescribes this collection with existing Cargo and host
hash tools. No separate statistics engine, general runner, resource collector,
or durable benchmark-report schema is introduced. Raw sample durations and
iteration counts retain Criterion's meaning; they are not the assessment's
separate twenty-invocation workflow protocol or a p95 claim.

Required order is dependency disposition and this unit contract's acceptance,
then one coherent target/fixture/oracle/documentation implementation slice,
independent candidate review, and a local evidence run. Runtime source-load work
can later reuse the fixture concept but acquires no dependency on benchmark-
private Rust code. Its selected-operation HTTP proof remains with that owner.

## Verification and operational risks

Focused tests establish exact fixture semantics, byte caps, source operation
identity, failure classifications and rejection of wrong successful results.
The actual release benchmark must execute all three series with passing
preflight and per-iteration oracles. A fresh reviewer checks the timing boundary
and single public call, including result lifetime. Repository build gates prove
that the development-only addition leaves workspace consumers intact.

The main risks are accidentally timing setup/cleanup, comparing stale outputs,
and confusing a structurally accepted root with source execution. Separate
output folders, explicit timing/oracle boundaries and the fixed-source preflight
address these risks. Same-host repeats establish that the method runs and retain
noise evidence; this slice establishes no performance budget or cross-platform
claim. No deployment, external service, data migration, or production rollback
is involved.

## System success condition

An independently reviewed `root_load` target produces a fresh local evidence
bundle with successful 10/100/1,000-step Criterion series, exact model and zero-
warning checks on every invocation, and both independent negative controls
passing their expected error oracles. The bundle identifies the source commit,
binary, lockfile, fixtures, host/configuration, commands and native sample data;
its interval demonstrably contains exactly one public parse/validation call.
No source-indexing, workflow, CLI-process or resource metric is relabelled as
root parsing performance. The completion record links that bundle and its
review, without claiming a threshold or the six-family program complete.

---

**Disposition:** Accepted — bounded independent review PASS, round 1, for
[d8f502e](https://github.com/strefethen/arazzo-cli/commit/d8f502ee9a13c6dc08b2f190e061fdf166b60da1)
(local; not pushed). Review evidence is recorded in
[ac-9f790](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-9f790).
Steve's explicit approval in
[ac-8b448](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-8b448) clears the
reserved dependency decision. The Chief of Staff enacts acceptance and directs
one-slice decomposition under the standing maintainer mandate in
[the roadmap](../roadmap.md#standing-delegation-and-continuity) and the accepted
unit-by-unit planning direction in
[ac-0be12](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-0be12).
The reviewed unit is within that direction, uses only existing public APIs and
does not enlarge production ownership. Numerical budgets, other workload families
and any dependency resolution beyond the approved edge remain separate.
