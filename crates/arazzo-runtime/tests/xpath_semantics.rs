//! Focused public XPath behavior for ac-46638: explicit `xpath-10` criteria,
//! selectors, and replacements evaluate; every other declared version — and
//! the omitted form, which §5.8.12 defaults to `xpath-31` — is rejected
//! before evaluation with the caller-specific surface behavior: a failed
//! criterion with an error, a null selector output with exactly one warning,
//! and an unchanged replacement body with exactly one warning.

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::EngineBuilder;
use arazzo_spec::{
    CriterionExpressionType, CriterionType, OutputValue, Replacement, RequestBody, SelectorObject,
    SelectorType, Step, StepTarget, SuccessCriterion, Workflow,
};
use common::*;
use serde_json::{json, Value};

fn xpath_type(version: &str) -> SelectorType {
    SelectorType::ExpressionType(CriterionExpressionType {
        type_: "xpath".to_string(),
        version: version.to_string(),
        ..CriterionExpressionType::default()
    })
}

fn xml_body_response() -> MockHttpResponse {
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "text/xml".to_string());
    MockHttpResponse {
        status: 200,
        headers,
        body: r#"<root><pets><pet>dog</pet><pet>cat</pet></pets></root>"#.to_string(),
    }
}

fn criterion_step(type_: Option<CriterionType>) -> Step {
    Step {
        step_id: "check".to_string(),
        target: Some(StepTarget::OperationPath("GET /pets".to_string())),
        success_criteria: vec![SuccessCriterion {
            condition: "count(//pet) = 2".to_string(),
            context: "$response.body".to_string(),
            type_,
            ..SuccessCriterion::default()
        }],
        ..Step::default()
    }
}

fn one_step_workflow(step: Step) -> Vec<Workflow> {
    vec![Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![step],
        ..Workflow::default()
    }]
}

#[tokio::test]
async fn explicit_xpath_10_criterion_evaluates_and_decides_the_step() {
    let server = start_server(|_m, _u, _h, _b| xml_body_response());
    let engine = new_test_engine(
        &server.base_url,
        make_spec(one_step_workflow(criterion_step(Some(xpath_type(
            "xpath-10",
        ))))),
    );
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);

    // The same explicit version with a false condition fails the step.
    let server = start_server(|_m, _u, _h, _b| xml_body_response());
    let mut step = criterion_step(Some(xpath_type("xpath-10")));
    step.success_criteria[0].condition = "count(//pet) = 3".to_string();
    let engine = new_test_engine(&server.base_url, make_spec(one_step_workflow(step)));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert!(result.outputs.is_err(), "a false criterion must fail");
}

#[tokio::test]
async fn non_10_and_omitted_xpath_criterion_versions_fail_the_step() {
    // A condition that would PASS under XPath 1.0 — proving the failure comes
    // from version rejection, not from evaluation.
    let variants: Vec<Option<CriterionType>> = vec![
        Some(CriterionType::Name("xpath".to_string())),
        Some(xpath_type("xpath-20")),
        Some(xpath_type("xpath-30")),
        Some(xpath_type("xpath-31")),
        Some(xpath_type("2.0")),
    ];
    for type_ in variants {
        let label = format!("{type_:?}");
        let server = start_server(|_m, _u, _h, _b| xml_body_response());
        let engine = new_test_engine(
            &server.base_url,
            make_spec(one_step_workflow(criterion_step(type_))),
        );
        let result = engine.execute_collect("wf", BTreeMap::new()).await;
        assert!(
            result.outputs.is_err(),
            "{label}: a non-xpath-10 version must fail the step"
        );
    }
}

fn selector_step(version: &str) -> Step {
    let type_ = if version.is_empty() {
        SelectorType::Name("xpath".to_string())
    } else {
        xpath_type(version)
    };
    Step {
        step_id: "select".to_string(),
        target: Some(StepTarget::OperationPath("GET /pets".to_string())),
        outputs: BTreeMap::from([(
            "picked".to_string(),
            OutputValue::Selector(SelectorObject {
                context: "$response.body".to_string(),
                selector: "count(//pet)".to_string(),
                type_,
                extensions: BTreeMap::new(),
            }),
        )]),
        ..Step::default()
    }
}

async fn run_selector(version: &str) -> (Value, Vec<String>) {
    let server = start_server(|_m, _u, _h, _b| xml_body_response());
    let spec = make_spec_with_base(&server.base_url, one_step_workflow(selector_step(version)));
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building engine: {error}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(error) = &result.outputs {
        panic!("selector workflow must succeed: {error}");
    }
    let trace = result.trace_steps()[0];
    let value = trace.outputs.get("picked").cloned().unwrap_or(Value::Null);
    (value, trace.warnings.clone())
}

#[tokio::test]
async fn explicit_xpath_10_selector_preserves_native_scalar_types() {
    let (value, warnings) = run_selector("xpath-10").await;
    // count() is a native number now, not the string "2".
    assert_eq!(value, json!(2));
    assert!(
        !warnings.iter().any(|w| w.contains("XPath")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn non_10_selector_versions_return_null_with_exactly_one_warning() {
    for version in ["", "xpath-20", "xpath-30", "xpath-31"] {
        let (value, warnings) = run_selector(version).await;
        assert_eq!(value, Value::Null, "{version:?}");
        let xpath_warnings: Vec<&String> = warnings
            .iter()
            .filter(|w| w.contains("XPath 1.0 engine"))
            .collect();
        assert_eq!(xpath_warnings.len(), 1, "{version:?}: {warnings:?}");
    }
}

fn replacement_step(version: Option<&str>) -> Step {
    Step {
        step_id: "send".to_string(),
        target: Some(StepTarget::OperationPath("POST /soap".to_string())),
        request_body: Some(RequestBody {
            content_type: "text/xml".to_string(),
            payload: Some(
                serde_yaml_ng::Value::String(
                    "<req><CustomerId>old</CustomerId><Keep note=\"yes\">stay</Keep></req>"
                        .to_string(),
                )
                .into(),
            ),
            replacements: vec![Replacement {
                target: "//CustomerId".to_string(),
                target_selector_type: version.map(xpath_type),
                value: serde_yaml_ng::Value::String("C-42".to_string()).into(),
                ..Replacement::default()
            }],
            ..RequestBody::default()
        }),
        ..Step::default()
    }
}

async fn run_replacement(version: Option<&str>) -> (String, Vec<String>) {
    let server = start_server(|_m, _u, _h, body: String| {
        let mut headers = BTreeMap::new();
        headers.insert("Content-Type".to_string(), "text/xml".to_string());
        MockHttpResponse {
            status: 200,
            headers,
            body,
        }
    });
    let spec = make_spec_with_base(
        &server.base_url,
        one_step_workflow(replacement_step(version)),
    );
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building engine: {error}"),
    };
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    if let Err(error) = &result.outputs {
        panic!("replacement workflow must succeed: {error}");
    }
    let trace = result.trace_steps()[0];
    let sent = trace
        .request
        .as_ref()
        .and_then(|request| request.body.as_ref())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (sent, trace.warnings.clone())
}

#[tokio::test]
async fn explicit_xpath_10_replacement_mutates_only_the_selected_node() {
    let (sent, warnings) = run_replacement(Some("xpath-10")).await;
    assert!(sent.contains("C-42"), "{sent}");
    // Unrelated attributes and text survive the round trip.
    assert!(sent.contains("note=\"yes\""), "{sent}");
    assert!(sent.contains(">stay<"), "{sent}");
    assert!(
        !warnings.iter().any(|w| w.contains("XPath")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn non_10_replacement_versions_leave_the_body_unchanged_with_one_warning() {
    for version in [None, Some("xpath-31")] {
        let (sent, warnings) = run_replacement(version).await;
        assert!(sent.contains("<CustomerId>old</CustomerId>"), "{sent}");
        assert!(!sent.contains("C-42"), "{sent}");
        let xpath_warnings: Vec<&String> = warnings
            .iter()
            .filter(|w| w.contains("XPath 1.0 engine"))
            .collect();
        assert_eq!(xpath_warnings.len(), 1, "{version:?}: {warnings:?}");
    }
}

// ac-d1649 (audit I1): hostile XML response structure is refused before the
// recursive parser runs, and the refusal fails the criterion like any other
// invalid XML instead of aborting the process or exhausting memory.

fn xml_response(body: String) -> MockHttpResponse {
    let mut response = xml_body_response();
    response.body = body;
    response
}

/// Every condition used below is TRUE for its body if the body were parsed,
/// so a failure proves the body was refused rather than evaluated. The hit
/// count proves the response actually arrived (a dead proxy or unreachable
/// server must not satisfy a failure assertion).
async fn run_criterion_against(body: String, condition: &str) -> Result<(), String> {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let served = std::sync::Arc::clone(&hits);
    let server = start_server(move |_m, _u, _h, _b| {
        served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        xml_response(body.clone())
    });
    let mut step = criterion_step(Some(xpath_type("xpath-10")));
    step.success_criteria[0].condition = condition.to_string();
    let engine = new_test_engine(&server.base_url, make_spec(one_step_workflow(step)));
    let result = engine.execute_collect("wf", BTreeMap::new()).await;
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the XML response must have been served"
    );
    result.outputs.map(|_| ()).map_err(|err| err.to_string())
}

fn nested_body(depth: usize) -> String {
    format!("{}leaf{}", "<a>".repeat(depth), "</a>".repeat(depth))
}

#[tokio::test]
async fn deeply_nested_xml_response_fails_the_step_without_crashing() {
    // 5000 levels overflowed the worker stack before admission existed.
    let result = run_criterion_against(nested_body(5_000), "string(/) = 'leaf'").await;
    assert!(result.is_err(), "a hostile depth must fail the criterion");

    // The deepest admitted body still evaluates and passes.
    let result = run_criterion_against(nested_body(64), "string(/) = 'leaf'").await;
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn dtd_entity_cascade_response_fails_the_step_promptly() {
    // The audit's 8-level cascade: ~1 GB and 38 s before admission existed.
    let mut dtd = String::from("<!ENTITY e0 \"lol\">");
    for level in 1..=8 {
        let refs = format!("&e{};", level - 1).repeat(10);
        dtd.push_str(&format!("<!ENTITY e{level} \"{refs}\">"));
    }
    let bomb = format!("<!DOCTYPE r [{dtd}]><r>&e8;</r>");

    let started = std::time::Instant::now();
    let result = run_criterion_against(bomb, "string-length(/r) > 0").await;
    let elapsed = started.elapsed();
    assert!(result.is_err(), "an entity cascade must fail the criterion");
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "refusal took {elapsed:?}"
    );

    // A DOCTYPE without an internal subset is still evaluated.
    let plain = "<!DOCTYPE r SYSTEM \"r.dtd\"><r><pet>dog</pet></r>".to_string();
    let result = run_criterion_against(plain, "count(//pet) = 1").await;
    assert!(result.is_ok(), "{result:?}");
}
