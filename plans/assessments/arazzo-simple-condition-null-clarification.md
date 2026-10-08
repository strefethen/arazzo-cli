# Simple-condition null comparison clarification (local draft)

Status: local draft; not filed upstream. Date: 2026-10-08.

The [vendored Arazzo 1.1.0 Simple Conditions section](../../spec/arazzo/v1.1.0.html#simple-conditions) is the normative authority. The [accepted simple-condition evaluation contract](../current/arazzo-1.1-conformance-evidence.md#accepted-simple-condition-evaluation-contract-2026-10-07) records the maintainer's local interpretation; it does not resolve the specification upstream.

## Specification contradiction

The specification states:

> null only equals itself (null == null is true). Comparing null with any other value evaluates to false.

The same section gives this example:

```yaml
# Pass if status code is 200 AND body contains data
- condition: $statusCode == 200 && $response.body.data != null
```

With status 200 and `data: {}`, the example advertises passing. Literal application of the quoted prose instead makes `data != null` false because exactly one operand is null. With absent or explicit-null data, that comparison is also false under the accepted local interpretation. The example therefore does not provide the advertised existence test.

Upstream clarification requested: does “Comparing null with any other value” include `!=`? Please align the prose and example so authors can determine the intended comparison and existence semantics. This draft selects no new upstream rule and does not claim that a clarification has been accepted.

## Accepted local interpretation

The [accepted plan](../current/arazzo-1.1-conformance-evidence.md#accepted-simple-condition-evaluation-contract-2026-10-07) defines the following local behavior:

- Exactly one null or missing operand yields false for all six comparisons: `==`, `!=`, `<`, `<=`, `>`, and `>=`.
- Both operands null or missing yield true for `==`, false for `!=`, and an evaluation error for ordering.
- `!(value == null)` is the supported existence spelling, for example `!($response.body.data == null)`.
- Property/index access on a missing value propagates missing; dereferencing present explicit null produces an evaluation error.

Executable evidence is provided by [conformance_simple_condition_evaluation_positive_evidence](../../crates/arazzo-expr/tests/simple_condition_evaluation.rs#conformance_simple_condition_evaluation_positive_evidence) and [conformance_simple_condition_evaluation_negative_evidence](../../crates/arazzo-expr/tests/simple_condition_evaluation.rs#conformance_simple_condition_evaluation_negative_evidence). These are evidence of accepted local behavior, not normative specification text. There is no alternate compatibility mode.

The specification ambiguity remains unresolved. These local interpretations do not establish full Arazzo 1.1 compliance. This is an unfiled local issue draft; no public issue, PR, message or publication has been made, and the vendored specification is unchanged.
