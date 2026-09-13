//! Engine-level proof that typed JSONPath Selector Objects (Arazzo v1.1.0
//! §5.8.13) and Payload Replacement Object targets (§5.8.15) read and write
//! through the shared RFC 9535 query owner, on the real callers: parameters,
//! nested payloads, replacement values, step and workflow outputs, and
//! sub-workflow/action input parameters. Replacement targets apply at exactly
//! one RFC 6901 location — special keys, empty-name segments, the root — while
//! zero, many and repeated occurrences leave the body unchanged and later
//! replacements see earlier writes. Hard JSONPath failures inside a
//! replacement value or target leave only that replacement unapplied; a
//! selected null and zero matches keep their ordinary normalization. Bodies
//! are proven on the captured wire request and on the dry-run resolved
//! request. Companion to `jsonpath_semantics.rs`, which owns the criterion arm.

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, ExecutionResult};
use arazzo_spec::{
    ActionType, ExpressionType, OnAction, OutputValue, ParamLocation, Parameter, Replacement,
    RequestBody, SelectorObject, SelectorType, Step, StepTarget, SuccessCriterion, ValueSource,
    Workflow,
};
use common::*;
use serde_json::{json, Value};

/// The body every recording server serves: two items in non-sorted order, a
/// null member and a string member for the regex-function control.
const RESPONSE_BODY: &str = r#"{"items":[{"id":2,"enabled":true,"tag":"a"},{"id":1,"enabled":false,"tag":"a"}],"value":null}"#;

const GOESSNER: &str = "draft-goessner-dispatch-jsonpath-00";

fn jsonpath() -> SelectorType {
    SelectorType::Name("jsonpath".to_string())
}

fn jsonpath_version(version: &str) -> SelectorType {
    SelectorType::ExpressionType(ExpressionType {
        type_: "jsonpath".to_string(),
        version: version.to_string(),
        ..ExpressionType::default()
    })
}

fn selector(context: &str, expression: &str, type_: SelectorType) -> SelectorObject {
    SelectorObject {
        context: context.to_string(),
        selector: expression.to_string(),
        type_,
        extensions: BTreeMap::new(),
    }
}

/// A default-version JSONPath Selector Object as a value.
fn select(context: &str, expression: &str) -> ValueSource {
    ValueSource::Selector(selector(context, expression, jsonpath()))
}

fn literal(value: Value) -> ValueSource {
    to_yaml(value).into()
}

/// The mapping form of a Selector Object, for nesting inside literal values;
/// the existing `ValueSource` model recognizes it when the value is resolved.
fn selector_yaml(context: &str, expression: &str) -> Value {
    json!({"context": context, "selector": expression, "type": "jsonpath"})
}

fn replacement(target: &str, type_: SelectorType, value: ValueSource) -> Replacement {
    Replacement {
        target: target.to_string(),
        target_selector_type: Some(type_),
        value,
        ..Replacement::default()
    }
}

fn jsonpath_replacement(target: &str, value: ValueSource) -> Replacement {
    replacement(target, jsonpath(), value)
}

fn body_step(step_id: &str, payload: Value, replacements: Vec<Replacement>) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("POST /items".to_string())),
        request_body: Some(RequestBody {
            content_type: "application/json".to_string(),
            payload: Some(literal(payload)),
            replacements,
            ..RequestBody::default()
        }),
        ..Step::default()
    }
}

fn workflow(steps: Vec<Step>) -> Workflow {
    Workflow {
        workflow_id: "wf".to_string(),
        steps,
        ..Workflow::default()
    }
}

fn parse_json(body: &str) -> Value {
    match serde_json::from_str(body) {
        Ok(value) => value,
        Err(error) => panic!("captured body is not JSON: {error}; body={body}"),
    }
}

fn wire_body(requests: &[RecordedRequest], index: usize) -> Value {
    parse_json(&requests[index].body)
}

fn outputs(result: &ExecutionResult) -> &BTreeMap<String, Value> {
    match &result.outputs {
        Ok(outputs) => outputs,
        Err(error) => panic!("workflow failed: {error}"),
    }
}

/// Runs `wf` with tracing against a recording server that always serves
/// [`RESPONSE_BODY`]; returns the result and every request it received.
async fn execute(
    workflows: Vec<Workflow>,
    inputs: BTreeMap<String, Value>,
) -> (ExecutionResult, Vec<RecordedRequest>) {
    let log = new_request_log();
    let recorder = log.clone();
    let server = start_server(move |method, url, headers, body| {
        record_request(&recorder, &method, &url, &headers, &body);
        MockHttpResponse::json(200, RESPONSE_BODY)
    });
    let spec = make_spec_with_base(&server.base_url, workflows);
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building engine: {error}"),
    };
    let result = engine.execute_collect("wf", inputs).await;
    (result, logged_requests(&log))
}

/// Resolves `wf` in dry-run mode; nothing is sent, and each dry-run request
/// carries the resolved body and the preparation warnings.
async fn dry_run(workflows: Vec<Workflow>, inputs: BTreeMap<String, Value>) -> ExecutionResult {
    let engine = match EngineBuilder::new(make_spec(workflows))
        .dry_run(true)
        .trace(true)
        .build()
    {
        Ok(engine) => engine,
        Err(error) => panic!("building dry-run engine: {error}"),
    };
    engine.execute_collect("wf", inputs).await
}

fn document() -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "document".to_string(),
        json!({"items": [{"id": 2, "enabled": true}, {"id": 1, "enabled": false}], "value": null}),
    )])
}

fn assert_warning(warnings: &[String], needle: &str) {
    assert!(
        warnings.iter().any(|warning| warning.contains(needle)),
        "expected a warning containing {needle:?}, got: {warnings:?}"
    );
}

fn assert_no_warning_for(warnings: &[String], index: usize) {
    let prefix = format!("requestBody.replacements[{index}]:");
    assert!(
        warnings.iter().all(|warning| !warning.starts_with(&prefix)),
        "replacement {index} must not warn: {warnings:?}"
    );
}

/// Parameters, nested payload values, a replacement value, step outputs and
/// workflow outputs all select through the same owner: descent, slices and
/// filters read as RFC 9535, many matches keep query order, and a nested
/// mapping without a `type` stays a literal.
#[tokio::test]
async fn selectors_resolve_through_real_callers() {
    let payload = json!({
        "picked": selector_yaml("$inputs.document", "$.items[?@.enabled == true].id"),
        "nested": {
            "all": [selector_yaml("$inputs.document", "$.items..id"), "literal"],
            "slice": selector_yaml("$inputs.document", "$.items[1:].id")
        },
        "replaced": null,
        "literalMap": {"context": "literal-context", "selector": "literal-selector"}
    });
    let step = Step {
        step_id: "select".to_string(),
        target: Some(StepTarget::OperationPath("POST /items".to_string())),
        parameters: vec![
            Parameter {
                name: "ids".to_string(),
                in_: Some(ParamLocation::Query),
                value: select("$inputs.document", "$.items[*].id"),
                ..Parameter::default()
            },
            Parameter {
                name: "X-First".to_string(),
                in_: Some(ParamLocation::Header),
                value: select("$inputs.document", "$.items[0].id"),
                ..Parameter::default()
            },
        ],
        request_body: Some(RequestBody {
            content_type: "application/json".to_string(),
            payload: Some(literal(payload)),
            replacements: vec![jsonpath_replacement(
                "$.replaced",
                select("$inputs.document", "$.items[-1].id"),
            )],
            ..RequestBody::default()
        }),
        outputs: BTreeMap::from([
            (
                "ids".to_string(),
                OutputValue::Selector(selector(
                    "$response.body",
                    "$.items[*].id",
                    jsonpath_version("rfc9535"),
                )),
            ),
            (
                "disabled".to_string(),
                OutputValue::Selector(selector(
                    "$response.body",
                    "$.items[?@.enabled == false].id",
                    jsonpath(),
                )),
            ),
            (
                "selectedNull".to_string(),
                OutputValue::Selector(selector("$response.body", "$.value", jsonpath())),
            ),
        ]),
        ..Step::default()
    };
    let wf = Workflow {
        outputs: BTreeMap::from([
            (
                "first".to_string(),
                OutputValue::Selector(selector("$steps.select.outputs.ids", "$[0]", jsonpath())),
            ),
            (
                "all".to_string(),
                OutputValue::Selector(selector("$steps.select.outputs.ids", "$[*]", jsonpath())),
            ),
        ]),
        ..workflow(vec![step])
    };

    let (result, requests) = execute(vec![wf], document()).await;

    let outputs = outputs(&result);
    assert_eq!(outputs.get("first"), Some(&json!(2)));
    assert_eq!(outputs.get("all"), Some(&json!([2, 1])));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "/items?ids=2&ids=1");
    assert_eq!(
        header_value(&requests[0].headers, "X-First").as_deref(),
        Some("2")
    );
    assert_eq!(
        wire_body(&requests, 0),
        json!({
            "picked": 2,
            "nested": {"all": [[2, 1], "literal"], "slice": 1},
            "replaced": 1,
            "literalMap": {"context": "literal-context", "selector": "literal-selector"}
        })
    );
    let trace = result.trace_steps()[0];
    assert_eq!(trace.outputs.get("ids"), Some(&json!([2, 1])));
    assert_eq!(trace.outputs.get("disabled"), Some(&json!(1)));
    assert_eq!(trace.outputs.get("selectedNull"), Some(&Value::Null));
    assert!(trace.warnings.is_empty(), "{:?}", trace.warnings);
}

/// Sub-workflow step parameters and success/failure action parameters are
/// resolved by the same value resolver: a JSONPath Selector Object arrives as
/// the callee's `$inputs` with its selected value, a zero-match selector as
/// null.
#[tokio::test]
async fn sub_workflow_and_action_parameters_resolve_jsonpath_selectors() {
    let parent = Workflow {
        outputs: BTreeMap::from([
            (
                "ids".to_string(),
                "$steps.call.outputs.received".to_string().into(),
            ),
            (
                "missing".to_string(),
                "$steps.call.outputs.missing".to_string().into(),
            ),
        ]),
        ..workflow(vec![Step {
            step_id: "call".to_string(),
            target: Some(StepTarget::WorkflowId("child".to_string())),
            parameters: vec![
                Parameter {
                    name: "ids".to_string(),
                    value: select("$inputs.document", "$.items[*].id"),
                    ..Parameter::default()
                },
                Parameter {
                    name: "missing".to_string(),
                    value: select("$inputs.document", "$.absent"),
                    ..Parameter::default()
                },
            ],
            ..Step::default()
        }])
    };
    let child = Workflow {
        workflow_id: "child".to_string(),
        steps: vec![Step {
            step_id: "fetch".to_string(),
            target: Some(StepTarget::OperationPath("GET /data".to_string())),
            ..Step::default()
        }],
        outputs: BTreeMap::from([
            ("received".to_string(), "$inputs.ids".to_string().into()),
            ("missing".to_string(), "$inputs.missing".to_string().into()),
        ]),
        ..Workflow::default()
    };
    let (result, requests) = execute(vec![parent, child], document()).await;
    assert_eq!(outputs(&result).get("ids"), Some(&json!([2, 1])));
    assert_eq!(outputs(&result).get("missing"), Some(&Value::Null));
    assert_eq!(requests.len(), 1, "only the child step sends a request");

    // A failed step's goto-workflow action selects its parameters from the
    // response the step just received.
    let main = workflow(vec![Step {
        step_id: "main".to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        success_criteria: vec![SuccessCriterion {
            condition: "$statusCode == 404".to_string(),
            ..SuccessCriterion::default()
        }],
        on_failure: vec![OnAction {
            type_: Some(ActionType::Goto),
            workflow_id: "fallback".to_string(),
            parameters: vec![
                Parameter {
                    name: "disabled".to_string(),
                    value: select("$response.body", "$.items[?@.enabled == false].id"),
                    ..Parameter::default()
                },
                Parameter {
                    name: "tags".to_string(),
                    value: ValueSource::Selector(selector(
                        "$response.body",
                        "$..tag",
                        jsonpath_version("rfc9535"),
                    )),
                    ..Parameter::default()
                },
            ],
            ..OnAction::default()
        }],
        ..Step::default()
    }]);
    let fallback = Workflow {
        workflow_id: "fallback".to_string(),
        steps: vec![Step {
            step_id: "recover".to_string(),
            target: Some(StepTarget::OperationPath("GET /data".to_string())),
            ..Step::default()
        }],
        outputs: BTreeMap::from([
            (
                "disabled".to_string(),
                "$inputs.disabled".to_string().into(),
            ),
            ("tags".to_string(), "$inputs.tags".to_string().into()),
        ]),
        ..Workflow::default()
    };
    let (result, requests) = execute(vec![main, fallback], BTreeMap::new()).await;
    assert_eq!(outputs(&result).get("disabled"), Some(&json!(1)));
    assert_eq!(outputs(&result).get("tags"), Some(&json!(["a", "a"])));
    assert_eq!(requests.len(), 2, "the failed step and the fallback step");
}

/// Read selectors are admitted — version, syntax, resource budget — before
/// their context is resolved, so a missing context cannot hide the error;
/// evaluation failures surface even under negation; zero matches warn; a
/// selected null is one match and carries no warning.
#[tokio::test]
async fn read_selectors_report_admission_and_evaluation_errors_once() {
    let oversized = format!("${}", ".a".repeat(129));
    let step = Step {
        step_id: "select".to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        outputs: BTreeMap::from([
            (
                "goessner".to_string(),
                OutputValue::Selector(selector(
                    "$steps.nowhere.outputs.body",
                    "$.items",
                    jsonpath_version(GOESSNER),
                )),
            ),
            (
                "syntax".to_string(),
                OutputValue::Selector(selector(
                    "$steps.nowhere.outputs.body",
                    "$.items[",
                    jsonpath(),
                )),
            ),
            (
                "resource".to_string(),
                OutputValue::Selector(selector("$response.body", &oversized, jsonpath())),
            ),
            (
                "regex".to_string(),
                OutputValue::Selector(selector(
                    "$response.body",
                    "$.items[?!match(@.tag, 'a{1000000000}')].id",
                    jsonpath(),
                )),
            ),
            (
                "zero".to_string(),
                OutputValue::Selector(selector(
                    "$response.body",
                    "$.items[?@.id > 5].id",
                    jsonpath(),
                )),
            ),
            (
                "null".to_string(),
                OutputValue::Selector(selector("$response.body", "$.value", jsonpath())),
            ),
        ]),
        ..Step::default()
    };
    let (result, _) = execute(vec![workflow(vec![step])], BTreeMap::new()).await;
    outputs(&result);
    let trace = result.trace_steps()[0];
    for name in ["goessner", "syntax", "resource", "regex", "zero", "null"] {
        assert_eq!(trace.outputs.get(name), Some(&Value::Null), "{name}");
    }
    let warnings = &trace.warnings;
    assert_eq!(warnings.len(), 5, "{warnings:?}");
    assert_warning(
        warnings,
        &format!("unsupported JSONPath version \"{GOESSNER}\""),
    );
    assert_warning(warnings, "invalid JSONPath syntax");
    assert_warning(
        warnings,
        "JSONPath query structural characters limit of 128 exceeded",
    );
    assert_warning(warnings, "JSONPath evaluation failed");
    assert_warning(warnings, "selector matched no values");
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.contains("step \"nowhere\" not found")),
        "admission errors are reported instead of the missing context: {warnings:?}"
    );
}

/// Targets with `#`, `/`, `~`, a quote, Unicode and an empty name each
/// resolve to one RFC 6901 location; `$['']['child']` is `//child`, so the
/// nested child changes and the root-level `child` sibling does not; `$`
/// replaces the whole body. The captured wire body and the dry-run resolved
/// body agree.
#[tokio::test]
async fn replacement_targets_write_special_keys_empty_name_segments_and_root() {
    let payload = json!({
        "#": {"q": "old"},
        "a/b": "old",
        "a~b": "old",
        "O'Reilly": "old",
        "雪": "old",
        "": {"child": "old"},
        "child": "sibling"
    });
    let replacements = vec![
        jsonpath_replacement("$['#'].q", literal(json!("hash"))),
        jsonpath_replacement(r#"$["a/b"]"#, literal(json!("slash"))),
        jsonpath_replacement(r#"$["a~b"]"#, literal(json!("tilde"))),
        jsonpath_replacement(r"$['O\'Reilly']", literal(json!("quote"))),
        jsonpath_replacement(r#"$["雪"]"#, literal(json!("unicode"))),
        jsonpath_replacement("$['']['child']", literal(json!("nested"))),
    ];
    let expected = json!({
        "#": {"q": "hash"},
        "a/b": "slash",
        "a~b": "tilde",
        "O'Reilly": "quote",
        "雪": "unicode",
        "": {"child": "nested"},
        "child": "sibling"
    });
    let steps = vec![
        body_step("special", payload.clone(), replacements),
        body_step(
            "root",
            payload,
            vec![jsonpath_replacement("$", literal(json!({"whole": true})))],
        ),
    ];

    let (result, requests) = execute(vec![workflow(steps.clone())], BTreeMap::new()).await;
    outputs(&result);
    assert_eq!(requests.len(), 2);
    assert_eq!(wire_body(&requests, 0), expected);
    assert_eq!(wire_body(&requests, 1), json!({"whole": true}));
    for trace in result.trace_steps() {
        assert!(trace.warnings.is_empty(), "{:?}", trace.warnings);
    }

    let dry = dry_run(vec![workflow(steps)], BTreeMap::new()).await;
    outputs(&dry);
    let requests = dry.dry_run_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body, Some(expected));
    assert_eq!(requests[1].body, Some(json!({"whole": true})));
    assert!(
        requests.iter().all(|request| request.warnings.is_empty()),
        "{:?}",
        requests
    );
}

/// Zero locations, many locations and repeated occurrences of one location
/// each warn and leave the current body unchanged, and the replacement after
/// them still applies.
#[tokio::test]
async fn replacement_targets_with_zero_many_or_duplicate_locations_leave_body_unchanged() {
    let payload = json!({"items": [{"q": 1}, {"q": 2}], "a": "old"});
    let replacements = vec![
        jsonpath_replacement("$.missing", literal(json!("x"))),
        jsonpath_replacement("$.items[*].q", literal(json!(9))),
        jsonpath_replacement("$.items[0,0].q", literal(json!(9))),
        jsonpath_replacement("$.a", literal(json!("new"))),
    ];
    let expected = json!({"items": [{"q": 1}, {"q": 2}], "a": "new"});
    let steps = vec![body_step("targets", payload, replacements)];

    let (result, requests) = execute(vec![workflow(steps.clone())], BTreeMap::new()).await;
    outputs(&result);
    assert_eq!(wire_body(&requests, 0), expected);

    let dry = dry_run(vec![workflow(steps)], BTreeMap::new()).await;
    outputs(&dry);
    let request = dry.dry_run_requests()[0];
    assert_eq!(request.body, Some(expected));
    assert_eq!(request.warnings.len(), 3, "{:?}", request.warnings);
    assert_warning(
        &request.warnings,
        "requestBody.replacements[0]: JSONPath target matched no locations; body unchanged",
    );
    assert_warning(
        &request.warnings,
        "requestBody.replacements[1]: JSONPath target matched 2 locations",
    );
    assert_warning(
        &request.warnings,
        "requestBody.replacements[2]: JSONPath target matched 2 locations",
    );
    assert_no_warning_for(&request.warnings, 3);
}

/// Replacements are applied in order against the current body: a target that
/// only exists because an earlier replacement created it resolves, and a
/// value selected from `$inputs` lands beside it. The negative control runs
/// the dependent replacement alone against the original body.
#[tokio::test]
async fn sequential_replacements_evaluate_against_the_current_body() {
    let payload = json!({"a": {"b": 1}, "c": "old"});
    let reshape = jsonpath_replacement("$.a", literal(json!({"b": {"d": "x"}})));
    let dependent = jsonpath_replacement("$.a.b.d", literal(json!("y")));
    let from_inputs = jsonpath_replacement("$.c", select("$inputs.document", "$.items[0].id"));
    let steps = vec![
        body_step(
            "ordered",
            payload.clone(),
            vec![reshape, dependent.clone(), from_inputs],
        ),
        body_step("control", payload.clone(), vec![dependent]),
    ];

    let (result, requests) = execute(vec![workflow(steps.clone())], document()).await;
    outputs(&result);
    assert_eq!(requests.len(), 2);
    assert_eq!(
        wire_body(&requests, 0),
        json!({"a": {"b": {"d": "y"}}, "c": 2})
    );
    assert_eq!(wire_body(&requests, 1), payload);

    let dry = dry_run(vec![workflow(steps)], document()).await;
    outputs(&dry);
    let requests = dry.dry_run_requests();
    assert_eq!(
        requests[0].body,
        Some(json!({"a": {"b": {"d": "y"}}, "c": 2}))
    );
    assert!(
        requests[0].warnings.is_empty(),
        "{:?}",
        requests[0].warnings
    );
    assert_eq!(requests[1].body, Some(payload));
    assert_warning(
        &requests[1].warnings,
        "requestBody.replacements[0]: JSONPath target matched no locations; body unchanged",
    );
}

/// A hard JSONPath failure in a replacement value — a resource limit inside a
/// nested array/map, an unsupported version, a missing context — or in the
/// target skips exactly that replacement with its warning; the replacements
/// before and after it apply, a legitimately selected null is written, and a
/// zero-match selection keeps its normalization and warning.
#[tokio::test]
async fn hard_jsonpath_failures_skip_only_the_failed_replacement() {
    let oversized = format!("${}", ".a".repeat(129));
    let payload = json!({
        "first": "old",
        "resource": "old",
        "version": "old",
        "context": "old",
        "target": "old",
        "null": "old",
        "zero": "old",
        "last": "old"
    });
    let replacements = vec![
        // [0] succeeds before any failure.
        jsonpath_replacement("$.first", literal(json!("new"))),
        // [1] resource failure nested in an array inside a map.
        jsonpath_replacement(
            "$.resource",
            literal(json!([{"inner": selector_yaml("$inputs.document", &oversized)}])),
        ),
        // [2] unsupported version whose context is also missing: the version
        // is reported, the context is never resolved.
        jsonpath_replacement(
            "$.version",
            ValueSource::Selector(selector(
                "$inputs.absent",
                "$.n",
                jsonpath_version(GOESSNER),
            )),
        ),
        // [3] valid query over a missing context.
        jsonpath_replacement("$.context", select("$inputs.absent", "$.n")),
        // [4] malformed target syntax.
        jsonpath_replacement("$.target[", literal(json!("new"))),
        // [5] legitimately selected null: one match, written, no warning.
        jsonpath_replacement("$.null", select("$inputs.document", "$.value")),
        // [6] zero matches: null plus the no-match warning, still written.
        jsonpath_replacement("$.zero", select("$inputs.document", "$.absent")),
        // [7] succeeds after the failures.
        jsonpath_replacement("$.last", literal(json!("new"))),
    ];
    let expected = json!({
        "first": "new",
        "resource": "old",
        "version": "old",
        "context": "old",
        "target": "old",
        "null": null,
        "zero": null,
        "last": "new"
    });
    let steps = vec![body_step("failures", payload, replacements)];

    let (result, requests) = execute(vec![workflow(steps.clone())], document()).await;
    outputs(&result);
    assert_eq!(wire_body(&requests, 0), expected);

    let dry = dry_run(vec![workflow(steps)], document()).await;
    outputs(&dry);
    let request = dry.dry_run_requests()[0];
    assert_eq!(request.body, Some(expected));
    let warnings = &request.warnings;

    let skipped: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.contains("replacement skipped"))
        .collect();
    assert_eq!(skipped.len(), 3, "{warnings:?}");
    for (skip, index) in skipped.iter().zip([1, 2, 3]) {
        assert!(
            skip.starts_with(&format!("requestBody.replacements[{index}]:")),
            "{skip}"
        );
    }
    assert_warning(warnings, "requestBody.replacements[1]: ");
    assert_warning(
        warnings,
        "JSONPath query structural characters limit of 128 exceeded",
    );
    assert_warning(
        warnings,
        &format!("requestBody.replacements[2]: $.n: unsupported JSONPath version \"{GOESSNER}\""),
    );
    assert!(
        warnings.iter().all(|warning| {
            !(warning.starts_with("requestBody.replacements[2]:") && warning.contains("not found"))
        }),
        "the version is rejected before the context is resolved: {warnings:?}"
    );
    assert_warning(
        warnings,
        "requestBody.replacements[3]: $inputs.absent: input \"absent\" not found in context",
    );
    assert_warning(
        warnings,
        "requestBody.replacements[4]: invalid JSONPath syntax",
    );
    assert_warning(
        warnings,
        "requestBody.replacements[6]: $.absent: selector matched no values",
    );
    for index in [0, 5, 7] {
        assert_no_warning_for(warnings, index);
    }
}
