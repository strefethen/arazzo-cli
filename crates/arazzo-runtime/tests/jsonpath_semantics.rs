//! Engine-level proof of Arazzo v1.1.0 §5.8.11.4.3 for `type: jsonpath`
//! criteria: the decision is the cardinality of the selected nodelist, never
//! the truthiness of the value the nodelist collapses to. A non-empty nodelist
//! passes even when its single node holds `false`, `0`, `""`, or `null`; an
//! empty nodelist fails. Companion to `xpath_semantics.rs`, which pins the
//! deliberately different §5.8.11.4.4 Effective-Boolean-Value arm.

mod common;

use std::collections::BTreeMap;

use arazzo_spec::{CriterionType, Step, StepTarget, SuccessCriterion, Workflow};
use common::*;

/// The four values where nodelist cardinality and JSON truthiness diverge,
/// each reachable as exactly one node.
const BODY: &str = r#"{"data":{"active":false,"count":0,"name":"","missing":null}}"#;

fn json_body_response() -> MockHttpResponse {
    MockHttpResponse::json(200, BODY)
}

fn jsonpath_step(step_id: &str, context: &str, condition: &str) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        success_criteria: vec![SuccessCriterion {
            condition: condition.to_string(),
            context: context.to_string(),
            type_: Some(CriterionType::Name("jsonpath".to_string())),
            ..SuccessCriterion::default()
        }],
        ..Step::default()
    }
}

fn workflow_with(steps: Vec<Step>) -> Vec<Workflow> {
    vec![Workflow {
        workflow_id: "wf".to_string(),
        steps,
        ..Workflow::default()
    }]
}

/// Runs the steps against a server always serving [`BODY`] and reports whether
/// the workflow — and therefore every criterion in it — succeeded.
async fn run(steps: Vec<Step>) -> Result<(), String> {
    let server = start_server(|_m, _u, _h, _b| json_body_response());
    let engine = new_test_engine(&server.base_url, make_spec(workflow_with(steps)));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    result
        .outputs
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// AC1 — §5.8.11.4.3: *"A condition passes (truthy) when the JSONPath
/// expression returns a non-empty nodelist (one or more nodes)."* Each of these
/// four conditions selects exactly one node, so each passes; every one of them
/// failed while the arm decided on the collapsed value's truthiness.
#[tokio::test]
async fn single_node_nodelist_passes_for_each_falsy_value() {
    let steps = vec![
        jsonpath_step("active", "$response.body", "$.data.active"),
        jsonpath_step("count", "$response.body", "$.data.count"),
        jsonpath_step("name", "$response.body", "$.data.name"),
        jsonpath_step("missing", "$response.body", "$.data.missing"),
    ];
    if let Err(error) = run(steps).await {
        panic!("every single-node criterion must pass: {error}");
    }
}

/// The same four conditions, one workflow per condition, so a failure names the
/// value that regressed instead of only the first step in a chain.
#[tokio::test]
async fn each_falsy_value_passes_in_isolation() {
    for (step_id, condition) in [
        ("active", "$.data.active"),
        ("count", "$.data.count"),
        ("name", "$.data.name"),
        ("missing", "$.data.missing"),
    ] {
        if let Err(error) = run(vec![jsonpath_step(step_id, "$response.body", condition)]).await {
            panic!("{condition} selects one node and must pass: {error}");
        }
    }
}

/// AC2 — the negative half of the same sentence: *"A condition fails (falsy)
/// when the JSONPath expression returns an empty nodelist (zero nodes)."*
/// Without this case a mutation to "always pass" would go undetected.
#[tokio::test]
async fn empty_nodelist_fails_the_step() {
    let result = run(vec![jsonpath_step(
        "absent",
        "$response.body",
        "$.data.absent",
    )])
    .await;
    assert!(
        result.is_err(),
        "an absent key selects zero nodes and must fail the step"
    );
}

/// AC3 — §5.8.11.4.3: *"If the `context` evaluates to `null` or `undefined` …
/// the condition MUST evaluate to fail."* The condition is `$`, which selects
/// the root — one node — against any context, so the failure can only come
/// from the null-context guard ahead of evaluation. The positive control runs
/// the same condition against a resolvable context.
#[tokio::test]
async fn null_context_fails_without_evaluating() {
    if let Err(error) = run(vec![jsonpath_step("control", "$response.body", "$")]).await {
        panic!("the root selector over a non-null context must pass: {error}");
    }

    let result = run(vec![jsonpath_step("nullctx", "$response.body.absent", "$")]).await;
    assert!(
        result.is_err(),
        "a null context must fail the criterion before evaluation"
    );
}

/// AC3 — a condition outside the supported subset fails the step rather than
/// silently passing. The diagnostic naming the offending construct is pinned at
/// unit level (`recursive_descent_reports_unsupported`,
/// `array_slice_reports_unsupported` in `runtime_core/jsonpath.rs`); surfacing
/// that text through the step failure is ac-1946c's scope, not this ticket's.
#[tokio::test]
async fn unsupported_constructs_fail_the_step() {
    for (step_id, condition) in [("descent", "$..data"), ("slice", "$.data[0:2]")] {
        let result = run(vec![jsonpath_step(step_id, "$response.body", condition)]).await;
        assert!(
            result.is_err(),
            "{condition} is outside the supported subset and must fail the step"
        );
    }
}
