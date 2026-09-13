//! Engine-level proof of Arazzo v1.1.0 §5.8.11.4.3 for `type: jsonpath`
//! criteria over the shared RFC 9535 query owner: the decision is the
//! cardinality of the selected nodelist, never the truthiness of the value the
//! nodelist collapses to. A non-empty nodelist passes even when its single
//! node holds `false`, `0`, `""`, or `null`; an empty nodelist fails.
//! Descent, slices, compound and function filters and object-member filtering
//! follow RFC 9535; the omitted and explicit `rfc9535` versions are one
//! engine; Goessner, syntax and resource errors fail the criterion and surface
//! once through the existing trace warning channels; HTTP, action and
//! sub-workflow criteria share the producer. Companion to
//! `xpath_semantics.rs`, which pins the deliberately different §5.8.11.4.4
//! Effective-Boolean-Value arm.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arazzo_runtime::{EngineBuilder, ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{
    ActionType, CriterionExpressionType, CriterionType, OnAction, Step, StepTarget,
    SuccessCriterion, Workflow,
};
use common::*;

/// The four values where nodelist cardinality and JSON truthiness diverge,
/// each reachable as exactly one node.
const BODY: &str = r#"{"data":{"active":false,"count":0,"name":"","missing":null}}"#;

/// Nested arrays and objects for the RFC 9535 features the old subset
/// rejected (descent, slices) or never parsed (nested function filters).
const RFC_BODY: &str = r#"{"items":[{"id":1,"tags":["a","b"],"price":5},{"id":2,"tags":["c"],"price":15},{"id":3,"tags":[],"price":25}],"meta":{"total":3}}"#;

/// One string node for the regex-function controls.
const REGEX_BODY: &str = r#"[{"value":"a"}]"#;

fn jsonpath_type(version: &str) -> CriterionType {
    CriterionType::ExpressionType(CriterionExpressionType {
        type_: "jsonpath".to_string(),
        version: version.to_string(),
        ..CriterionExpressionType::default()
    })
}

fn omitted() -> CriterionType {
    CriterionType::Name("jsonpath".to_string())
}

fn rfc9535() -> CriterionType {
    jsonpath_type("rfc9535")
}

fn goessner() -> CriterionType {
    jsonpath_type("draft-goessner-dispatch-jsonpath-00")
}

fn criterion(type_: CriterionType, context: &str, condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        condition: condition.to_string(),
        context: context.to_string(),
        type_: Some(type_),
        ..SuccessCriterion::default()
    }
}

fn typed_step(step_id: &str, type_: CriterionType, context: &str, condition: &str) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        success_criteria: vec![criterion(type_, context, condition)],
        ..Step::default()
    }
}

fn jsonpath_step(step_id: &str, context: &str, condition: &str) -> Step {
    typed_step(step_id, omitted(), context, condition)
}

fn workflow_with(steps: Vec<Step>) -> Vec<Workflow> {
    vec![Workflow {
        workflow_id: "wf".to_string(),
        steps,
        ..Workflow::default()
    }]
}

/// Runs `workflows` (entry `wf`) with tracing on against a server that always
/// serves `body`, and reports the result plus the number of requests served.
async fn execute(body: &'static str, workflows: Vec<Workflow>) -> (ExecutionResult, usize) {
    let served = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&served);
    let server = start_server(move |_m, _u, _h, _b| {
        counter.fetch_add(1, Ordering::SeqCst);
        MockHttpResponse::json(200, body)
    });
    let spec = make_spec_with_base(&server.base_url, workflows);
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building engine: {error}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    (result, served.load(Ordering::SeqCst))
}

/// Runs the steps against a server always serving `body` and reports whether
/// the workflow — and therefore every criterion in it — succeeded.
async fn run_with(body: &'static str, steps: Vec<Step>) -> Result<(), String> {
    let (result, _) = execute(body, workflow_with(steps)).await;
    result
        .outputs
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// [`run_with`] over [`BODY`].
async fn run(steps: Vec<Step>) -> Result<(), String> {
    run_with(BODY, steps).await
}

/// A failed JSONPath criterion keeps the ordinary criteria failure code and
/// its declared trace text, and its one warning appears exactly once on the
/// nested criterion record and exactly once, prefixed, on the enclosing step.
/// Returns the nested warning so callers can check the actionable text.
fn assert_error_surfaces_once(result: &ExecutionResult, needle: &str) -> String {
    let err = match &result.outputs {
        Ok(_) => panic!("expected the step to fail on {needle}"),
        Err(err) => err,
    };
    assert_eq!(err.kind, RuntimeErrorKind::SuccessCriteriaFailed, "{err}");
    assert_eq!(err.kind.code(), "RUNTIME_SUCCESS_CRITERIA_FAILED");

    let traces = result.trace_steps();
    let trace = match traces.first() {
        Some(trace) => trace,
        None => panic!("the failing step must leave a trace record"),
    };
    assert_eq!(trace.criteria.len(), 1, "{:?}", trace.criteria);
    let record = &trace.criteria[0];
    assert!(!record.result, "{record:?}");
    assert_eq!(record.type_, "jsonpath");
    let nested: Vec<&String> = record
        .warnings
        .iter()
        .filter(|warning| warning.contains("JSONPath"))
        .collect();
    assert_eq!(nested.len(), 1, "nested warnings: {:?}", record.warnings);
    assert!(nested[0].contains(needle), "{}", nested[0]);
    assert!(
        nested[0].starts_with(&format!("{}: ", record.condition)),
        "the warning is bound to the declared condition: {}",
        nested[0]
    );
    let enclosing: Vec<&String> = trace
        .warnings
        .iter()
        .filter(|warning| warning.contains("JSONPath"))
        .collect();
    assert_eq!(
        enclosing.len(),
        1,
        "enclosing warnings: {:?}",
        trace.warnings
    );
    assert_eq!(*enclosing[0], format!("successCriteria[0]: {}", nested[0]));
    nested[0].clone()
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
/// the same condition against a resolvable context. The null-context failure
/// is the spec's own rule, not an error, so it carries no JSONPath warning.
#[tokio::test]
async fn null_context_fails_without_evaluating() {
    if let Err(error) = run(vec![jsonpath_step("control", "$response.body", "$")]).await {
        panic!("the root selector over a non-null context must pass: {error}");
    }

    let (result, _) = execute(
        BODY,
        workflow_with(vec![jsonpath_step("nullctx", "$response.body.absent", "$")]),
    )
    .await;
    assert!(
        result.outputs.is_err(),
        "a null context must fail the criterion before evaluation"
    );
    let trace = result.trace_steps()[0];
    assert!(
        !trace.criteria[0].result && trace.criteria[0].warnings.is_empty(),
        "a valid query over a null context is a plain failure: {:?}",
        trace.criteria[0]
    );
}

/// RFC 9535 features the old subset rejected (descent §2.5.2, slices §2.3.4)
/// or never parsed (nested function filters §2.4) now decide criteria; each
/// positive has a negative that selects nothing under the same feature.
#[tokio::test]
async fn rfc_features_decide_criteria() {
    for (step_id, condition) in [
        ("descent", "$..id"),
        ("slice", "$.items[0:2]"),
        ("negative-slice", "$.items[-1:]"),
        ("compound", "$.items[?@.price > 10 && @.id < 3]"),
        ("paren-filter", "$.items[?(@.price >= 25)]"),
        ("count", "$.items[?count(@.tags[*]) == 2]"),
        ("nested-count", "$[?count(@[*]) == 3]"),
    ] {
        if let Err(error) = run_with(
            RFC_BODY,
            vec![jsonpath_step(step_id, "$response.body", condition)],
        )
        .await
        {
            panic!("{condition} selects at least one node and must pass: {error}");
        }
    }
    for (step_id, condition) in [
        ("descent-miss", "$..absent"),
        ("slice-empty", "$.items[5:]"),
        ("compound-miss", "$.items[?@.price > 10 && @.id > 5]"),
        ("count-miss", "$.items[?count(@.tags[*]) == 4]"),
    ] {
        let result = run_with(
            RFC_BODY,
            vec![jsonpath_step(step_id, "$response.body", condition)],
        )
        .await;
        assert!(
            result.is_err(),
            "{condition} selects zero nodes and must fail the step"
        );
    }
}

/// RFC 9535 §2.3.5: a filter applied to an object iterates its member values,
/// so `$[?…]` over the root object tests `data` itself, never the root. The
/// old evaluator treated a non-array context as its own single candidate,
/// which made `$[?@.data]` pass; under the RFC it selects nothing, while a
/// predicate about the member value does select it.
#[tokio::test]
async fn root_filters_iterate_object_member_values() {
    for (step_id, condition) in [
        ("member-object", "$[?@.active == false]"),
        ("member-scalar", "$.data[?@ == 0]"),
        ("member-null", "$.data[?@ == null]"),
    ] {
        if let Err(error) = run(vec![jsonpath_step(step_id, "$response.body", condition)]).await {
            panic!("{condition} selects a member value and must pass: {error}");
        }
    }
    let result = run(vec![jsonpath_step("self", "$response.body", "$[?@.data]")]).await;
    assert!(
        result.is_err(),
        "the root object is not its own filter candidate under RFC 9535"
    );
}

/// The omitted version and the explicit `rfc9535` version are one engine:
/// identical passes and identical failures over the same conditions.
#[tokio::test]
async fn default_and_explicit_rfc9535_execute_identically() {
    for type_ in [omitted(), rfc9535()] {
        let label = format!("{type_:?}");
        for (step_id, condition) in [
            ("descent", "$..id"),
            ("slice", "$.items[0:2]"),
            ("filter", "$.items[?@.price > 10]"),
        ] {
            let step = typed_step(step_id, type_.clone(), "$response.body", condition);
            if let Err(error) = run_with(RFC_BODY, vec![step]).await {
                panic!("{label}: {condition} must pass: {error}");
            }
        }
        for (step_id, condition) in [
            ("miss", "$.absent"),
            ("empty-filter", "$.items[?@.price > 100]"),
            ("syntax", "$[?("),
        ] {
            let step = typed_step(step_id, type_.clone(), "$response.body", condition);
            let result = run_with(RFC_BODY, vec![step]).await;
            assert!(result.is_err(), "{label}: {condition} must fail the step");
        }
    }
}

/// Goessner is an expired draft the shared owner does not evaluate: the same
/// condition that passes under RFC 9535 fails with a version error before the
/// expression is examined, and the error surfaces once per channel with the
/// declared condition, type and version intact.
#[tokio::test]
async fn explicit_goessner_version_fails_before_evaluation() {
    let (result, _) = execute(
        BODY,
        workflow_with(vec![typed_step(
            "goessner",
            goessner(),
            "$response.body",
            "$.data.active",
        )]),
    )
    .await;
    let message = assert_error_surfaces_once(
        &result,
        "unsupported JSONPath version \"draft-goessner-dispatch-jsonpath-00\"",
    );
    assert!(message.contains("only rfc9535 is supported"), "{message}");
    let record = &result.trace_steps()[0].criteria[0];
    assert_eq!(record.condition, "$.data.active");
    assert_eq!(record.context, "$response.body");
}

/// The whole condition is parsed before any predicate runs, so a malformed
/// remainder behind an `||` whose left side would already be true still fails
/// the criterion with a syntax error. The old evaluator split on `||` first
/// and short-circuited without ever reading the malformed right side.
#[tokio::test]
async fn malformed_predicate_behind_or_fails_with_syntax_error() {
    if let Err(error) = run(vec![jsonpath_step(
        "control",
        "$response.body",
        "$[?@.active == false]",
    )])
    .await
    {
        panic!("the left side alone must pass: {error}");
    }

    let (result, _) = execute(
        BODY,
        workflow_with(vec![jsonpath_step(
            "hidden",
            "$response.body",
            "$[?@.active == false || @.name &&&]",
        )]),
    )
    .await;
    assert_error_surfaces_once(&result, "invalid JSONPath syntax");
}

/// Version admission and parsing precede context resolution, so an invalid
/// query over a context that resolves to null reports its error instead of
/// hiding behind the spec's silent null-context failure.
#[tokio::test]
async fn invalid_query_with_missing_context_reports_the_error() {
    let (result, _) = execute(
        BODY,
        workflow_with(vec![jsonpath_step(
            "nullctx-syntax",
            "$response.body.absent",
            "$[?(",
        )]),
    )
    .await;
    assert_error_surfaces_once(&result, "invalid JSONPath syntax");

    let (result, _) = execute(
        BODY,
        workflow_with(vec![typed_step(
            "nullctx-goessner",
            goessner(),
            "$response.body.absent",
            "$",
        )]),
    )
    .await;
    assert_error_surfaces_once(&result, "unsupported JSONPath version");
}

/// An I-Regexp resource failure inside `match()` is an operational error of
/// the whole query, not a `false` the predicate can negate: the negated form
/// fails exactly like the plain form, the error surfaces once per channel,
/// and the failure code is the ordinary criteria failure.
#[tokio::test]
async fn iregexp_resource_failure_fails_under_negation() {
    for (step_id, condition) in [
        ("control", "$[?match(@.value, 'a')]"),
        ("negated-control", "$[?!match(@.value, 'b')]"),
    ] {
        if let Err(error) = run_with(
            REGEX_BODY,
            vec![jsonpath_step(step_id, "$response.body", condition)],
        )
        .await
        {
            panic!("{condition} is an ordinary regex predicate and must pass: {error}");
        }
    }

    for (step_id, condition) in [
        ("plain", "$[?match(@.value, 'a{1000000000}')]"),
        ("negated", "$[?!match(@.value, 'a{1000000000}')]"),
    ] {
        let (result, _) = execute(
            REGEX_BODY,
            workflow_with(vec![jsonpath_step(step_id, "$response.body", condition)]),
        )
        .await;
        let message = assert_error_surfaces_once(&result, "JSONPath evaluation failed");
        assert!(message.contains("limit"), "{condition}: {message}");
    }
}

/// Action criteria run through the same producer: an `end` action gated by a
/// jsonpath criterion fires on a falsy single node under either version form,
/// and stays quiet on an empty nodelist, an unsupported version or a syntax
/// error, so the next step still runs. The request count is the observable.
#[tokio::test]
async fn action_criteria_share_the_engine() {
    fn gated_end(type_: CriterionType, condition: &str) -> Vec<Workflow> {
        vec![Workflow {
            workflow_id: "wf".to_string(),
            steps: vec![
                Step {
                    step_id: "first".to_string(),
                    target: Some(StepTarget::OperationPath("GET /data".to_string())),
                    on_success: vec![OnAction {
                        type_: Some(ActionType::End),
                        criteria: vec![criterion(type_, "$response.body", condition)],
                        ..OnAction::default()
                    }],
                    ..Step::default()
                },
                Step {
                    step_id: "second".to_string(),
                    target: Some(StepTarget::OperationPath("GET /data".to_string())),
                    ..Step::default()
                },
            ],
            ..Workflow::default()
        }]
    }

    for (type_, condition, expected_requests) in [
        (omitted(), "$.data.active", 1usize),
        (rfc9535(), "$.data.active", 1),
        (omitted(), "$..missing", 1),
        (omitted(), "$.data.absent", 2),
        (goessner(), "$.data.active", 2),
        (omitted(), "$[?(", 2),
    ] {
        let label = format!("{type_:?} {condition}");
        let (result, requests) = execute(BODY, gated_end(type_, condition)).await;
        if let Err(error) = &result.outputs {
            panic!("{label}: the workflow itself must succeed: {error}");
        }
        assert_eq!(requests, expected_requests, "{label}");
    }
}

/// A `workflowId` step's criteria are evaluated after the child completes,
/// against the child's registered outputs and with no HTTP response; they run
/// through the same producer with the same cardinality and version rules.
#[tokio::test]
async fn sub_workflow_step_criteria_share_the_engine() {
    fn parent_and_child(type_: CriterionType, condition: &str) -> Vec<Workflow> {
        vec![
            Workflow {
                workflow_id: "wf".to_string(),
                steps: vec![Step {
                    step_id: "call".to_string(),
                    target: Some(StepTarget::WorkflowId("child".to_string())),
                    success_criteria: vec![criterion(
                        type_,
                        "$steps.call.outputs.result",
                        condition,
                    )],
                    ..Step::default()
                }],
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_string(),
                steps: vec![Step {
                    step_id: "fetch".to_string(),
                    target: Some(StepTarget::OperationPath("GET /data".to_string())),
                    outputs: BTreeMap::from([(
                        "result".to_string(),
                        "$response.body".to_string().into(),
                    )]),
                    ..Step::default()
                }],
                outputs: BTreeMap::from([(
                    "result".to_string(),
                    "$steps.fetch.outputs.result".to_string().into(),
                )]),
                ..Workflow::default()
            },
        ]
    }

    for (type_, condition) in [
        (omitted(), "$.data.active"),
        (rfc9535(), "$.data.count"),
        (omitted(), "$..missing"),
    ] {
        let label = format!("{type_:?} {condition}");
        let (result, requests) = execute(BODY, parent_and_child(type_, condition)).await;
        if let Err(error) = &result.outputs {
            panic!("{label}: a single-node criterion over the child outputs must pass: {error}");
        }
        assert_eq!(requests, 1, "{label}: only the child step sends a request");
    }
    for (type_, condition) in [
        (omitted(), "$.data.absent"),
        (goessner(), "$.data.active"),
        (omitted(), "$[?("),
    ] {
        let label = format!("{type_:?} {condition}");
        let (result, _) = execute(BODY, parent_and_child(type_, condition)).await;
        let err = match &result.outputs {
            Ok(_) => panic!("{label}: must fail the sub-workflow step"),
            Err(err) => err,
        };
        assert_eq!(
            err.kind,
            RuntimeErrorKind::SuccessCriteriaFailed,
            "{label}: {err}"
        );
    }
}
