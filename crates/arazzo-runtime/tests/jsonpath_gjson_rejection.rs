//! Engine-level proof that GJSON-only `#` syntax is not JSONPath on any typed
//! surface (audit finding F20).
//!
//! Arazzo v1.1.0 §5.8.11.4.3 requires a `type: jsonpath` condition to be *"a
//! valid JSONPath expression conforming to [RFC9535]"* and §5.8.12 requires an
//! implementation to *"apply the semantics defined in that version's
//! specification"*. GJSON's `#` length form, its `#(…)` / `#(…)#` dot-form
//! filters and the bracket-wrapped `[#(…)]` variant are syntax in none of the
//! versions §5.8.12.1 allows, so every typed read and write surface must
//! reject them through the shared RFC 9535 query owner instead of resolving
//! them.
//!
//! The five rejected forms are the ones `arazzo-expr`'s
//! `wrappers_share_the_owner_and_report_json_path_errors` pins at the owner;
//! this file proves the production routing on top of them — criteria, Selector
//! Objects and replacement targets/values — together with the controls that
//! keep the rejection honest: quoted and literal `#`, the RFC 9535 feature set
//! the forms superficially resemble, and the separate runtime-expression
//! GJSON dot-path extension, which is unchanged.
//!
//! Companions: `jsonpath_semantics.rs` owns the criterion cardinality rules,
//! `jsonpath_selectors.rs` owns the general selector/replacement contract.

mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{EngineBuilder, ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{
    CriterionType, ExpressionType, OutputValue, Replacement, RequestBody, SelectorObject,
    SelectorType, Step, StepTarget, SuccessCriterion, ValueSource, Workflow,
};
use common::*;
use serde_json::{json, Value};

/// Items whose `sku`/`q` members are exactly what a GJSON `#(…)` filter would
/// address, plus a literal `#` member name, a `#`-bearing string literal and a
/// selected `null`, so the positive controls sit in the same document as the
/// rejected forms.
const BODY: &str = r##"{"items":[{"sku":"A","q":1},{"sku":"A","q":2},{"sku":"B","q":3}],"#":{"q":"hash-key"},"tags":[{"tag":"#x","q":"hash-literal"}],"value":null}"##;

/// The GJSON-only forms: unquoted terminal `#`, `.#.`, `#(…)`, `#(…)#` and the
/// bracket-wrapped variant. None is RFC 9535 syntax.
const GJSON_FORMS: [&str; 5] = [
    "$.items.#",
    "$.items.#.q",
    r#"$.items.#(sku=="A").q"#,
    r#"$.items.#(sku=="A")#.q"#,
    r#"$.items[#(sku=="A")].q"#,
];

const SYNTAX_ERROR: &str = "invalid JSONPath syntax";

fn jsonpath_criterion(context: &str, condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        condition: condition.to_string(),
        context: context.to_string(),
        type_: Some(CriterionType::Name("jsonpath".to_string())),
        ..SuccessCriterion::default()
    }
}

fn criterion_step(step_id: &str, context: &str, condition: &str) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        success_criteria: vec![jsonpath_criterion(context, condition)],
        ..Step::default()
    }
}

fn jsonpath() -> SelectorType {
    SelectorType::Name("jsonpath".to_string())
}

fn selector(context: &str, expression: &str) -> SelectorObject {
    SelectorObject {
        context: context.to_string(),
        selector: expression.to_string(),
        type_: jsonpath(),
        extensions: BTreeMap::new(),
    }
}

/// The mapping form of a JSONPath Selector Object, for nesting inside a
/// literal replacement value.
fn selector_yaml(context: &str, expression: &str) -> Value {
    json!({"context": context, "selector": expression, "type": "jsonpath"})
}

fn literal(value: Value) -> ValueSource {
    to_yaml(value).into()
}

fn selector_output(context: &str, expression: &str) -> OutputValue {
    OutputValue::Selector(selector(context, expression))
}

fn output_step(step_id: &str, outputs: Vec<(&str, OutputValue)>) -> Step {
    Step {
        step_id: step_id.to_string(),
        target: Some(StepTarget::OperationPath("GET /data".to_string())),
        outputs: outputs
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
        ..Step::default()
    }
}

fn jsonpath_replacement(target: &str, value: ValueSource) -> Replacement {
    Replacement {
        target: target.to_string(),
        target_selector_type: Some(SelectorType::ExpressionType(ExpressionType {
            type_: "jsonpath".to_string(),
            ..ExpressionType::default()
        })),
        value,
        ..Replacement::default()
    }
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

fn workflow(steps: Vec<Step>) -> Vec<Workflow> {
    vec![Workflow {
        workflow_id: "wf".to_string(),
        steps,
        ..Workflow::default()
    }]
}

fn parse_json(body: &str) -> Value {
    match serde_json::from_str(body) {
        Ok(value) => value,
        Err(error) => panic!("fixture body is not JSON: {error}"),
    }
}

/// Runs `wf` with tracing against a recording server that always serves
/// [`BODY`]; returns the result and every request the server received.
async fn execute(
    workflows: Vec<Workflow>,
    inputs: BTreeMap<String, Value>,
) -> (ExecutionResult, Vec<RecordedRequest>) {
    let log = new_request_log();
    let recorder = log.clone();
    let server = start_server(move |method, url, headers, body| {
        record_request(&recorder, &method, &url, &headers, &body);
        MockHttpResponse::json(200, BODY)
    });
    let spec = make_spec_with_base(&server.base_url, workflows);
    let engine = match EngineBuilder::new(spec).trace(true).build() {
        Ok(engine) => engine,
        Err(error) => panic!("building engine: {error}"),
    };
    let result = engine.execute_collect("wf", inputs).await;
    (result, logged_requests(&log))
}

/// Resolves `wf` in dry-run mode; nothing is sent and each dry-run request
/// carries the resolved body plus the preparation warnings.
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

fn outputs(result: &ExecutionResult) -> &BTreeMap<String, Value> {
    match &result.outputs {
        Ok(outputs) => outputs,
        Err(error) => panic!("workflow failed: {error}"),
    }
}

/// A rejected JSONPath criterion fails the step with the ordinary criteria
/// code and reports its cause exactly once on the nested criterion record and
/// once, prefixed, on the enclosing step. Returns the nested warning.
fn assert_criterion_rejected(result: &ExecutionResult, condition: &str, needle: &str) -> String {
    let error = match &result.outputs {
        Ok(outputs) => panic!("{condition} must fail the step, got outputs {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.kind,
        RuntimeErrorKind::SuccessCriteriaFailed,
        "{error}"
    );

    let traces = result.trace_steps();
    let trace = match traces.first() {
        Some(trace) => trace,
        None => panic!("{condition}: the failing step must leave a trace record"),
    };
    assert_eq!(trace.criteria.len(), 1, "{:?}", trace.criteria);
    let record = &trace.criteria[0];
    assert!(!record.result, "{condition}: {record:?}");
    assert_eq!(record.type_, "jsonpath");
    assert_eq!(record.condition, condition);
    let nested: Vec<&String> = record
        .warnings
        .iter()
        .filter(|warning| warning.contains("JSONPath"))
        .collect();
    assert_eq!(nested.len(), 1, "{condition}: {:?}", record.warnings);
    assert!(nested[0].contains(needle), "{condition}: {}", nested[0]);
    assert!(
        nested[0].starts_with(&format!("{condition}: ")),
        "{condition}: the diagnostic must name the declared condition: {}",
        nested[0]
    );
    let enclosing: Vec<&String> = trace
        .warnings
        .iter()
        .filter(|warning| warning.contains("JSONPath"))
        .collect();
    assert_eq!(enclosing.len(), 1, "{condition}: {:?}", trace.warnings);
    assert_eq!(*enclosing[0], format!("successCriteria[0]: {}", nested[0]));
    nested[0].clone()
}

fn warnings_for(warnings: &[String], prefix: &str) -> Vec<String> {
    warnings
        .iter()
        .filter(|warning| warning.starts_with(prefix))
        .cloned()
        .collect()
}

/// Every GJSON-only form is rejected as invalid JSONPath by a `type: jsonpath`
/// criterion, with the diagnostic §5.8.11.4.3 expects rather than a silent
/// falsy decision. Each form would have selected something under GJSON, so a
/// passing step here would mean the form was executed.
#[tokio::test]
async fn gjson_forms_are_rejected_by_typed_criteria() {
    for form in GJSON_FORMS {
        let (result, requests) = execute(
            workflow(vec![criterion_step("gjson", "$response.body", form)]),
            BTreeMap::new(),
        )
        .await;
        let message = assert_criterion_rejected(&result, form, SYNTAX_ERROR);
        assert!(
            !message.contains("draft-goessner"),
            "{form}: an unsupported dialect is not an excuse to execute it: {message}"
        );
        assert_eq!(
            requests.len(),
            1,
            "{form}: the step still issues its request"
        );
    }
}

/// Admission and complete parsing precede context resolution and any predicate
/// evaluation, so neither a context that resolves to nothing nor a left-hand
/// operand that would already decide an `||` can conceal GJSON syntax, and a
/// GJSON path compared with `null` is rejected rather than evaluated as an
/// ordinary absent-value comparison.
#[tokio::test]
async fn gjson_criteria_fail_before_missing_context_and_short_circuit() {
    // Missing context: §5.8.11.4.3's null-context failure is silent, so a
    // reported syntax error can only come from admission running first.
    let form = r#"$.items.#(sku=="A").q"#;
    let (result, _) = execute(
        workflow(vec![criterion_step(
            "nullctx",
            "$response.body.absent",
            form,
        )]),
        BTreeMap::new(),
    )
    .await;
    let trace_context = result.trace_steps()[0].criteria[0].context.clone();
    assert_eq!(trace_context, "$response.body.absent");
    assert_criterion_rejected(&result, form, SYNTAX_ERROR);

    // Short-circuit: the left operand alone selects one node, so a parser that
    // stopped at the first satisfied disjunct would pass the step.
    let control = "$.items[?@.q == 1]";
    let (result, _) = execute(
        workflow(vec![criterion_step("control", "$response.body", control)]),
        BTreeMap::new(),
    )
    .await;
    outputs(&result);

    let hidden = "$.items[?@.q == 1 || @.sku.#]";
    let (result, _) = execute(
        workflow(vec![criterion_step("hidden", "$response.body", hidden)]),
        BTreeMap::new(),
    )
    .await;
    assert_criterion_rejected(&result, hidden, SYNTAX_ERROR);

    // Comparison with null: the whole query is rejected, so the criterion
    // carries a diagnostic instead of failing silently on an empty nodelist.
    let compared = "$.items[?@.missing.# == null]";
    let (result, _) = execute(
        workflow(vec![criterion_step("compared", "$response.body", compared)]),
        BTreeMap::new(),
    )
    .await;
    assert_criterion_rejected(&result, compared, SYNTAX_ERROR);
}

/// Every GJSON-only form is rejected by a JSONPath Selector Object too: the
/// output reads as `null` and carries exactly one diagnostic bound to the
/// declared selector. The step has no other warning source, so the total
/// count is the proof that nothing resolved and nothing warned twice.
#[tokio::test]
async fn gjson_forms_are_rejected_by_typed_selector_objects() {
    let names = ["terminal", "interior", "first", "all", "bracketed"];
    let step = output_step(
        "select",
        names
            .iter()
            .zip(GJSON_FORMS)
            .map(|(name, form)| (*name, selector_output("$response.body", form)))
            .collect(),
    );
    let (result, _) = execute(workflow(vec![step]), BTreeMap::new()).await;
    outputs(&result);

    let trace = result.trace_steps()[0];
    for name in names {
        assert_eq!(trace.outputs.get(name), Some(&Value::Null), "{name}");
    }
    assert_eq!(
        trace.warnings.len(),
        GJSON_FORMS.len(),
        "one diagnostic per rejected selector and nothing else: {:?}",
        trace.warnings
    );
    for (name, form) in names.iter().zip(GJSON_FORMS) {
        let expected = format!("output \"{name}\": {form}: ");
        let matched = warnings_for(&trace.warnings, &expected);
        assert_eq!(matched.len(), 1, "{form}: {:?}", trace.warnings);
        assert!(matched[0].contains(SYNTAX_ERROR), "{}", matched[0]);
    }
}

/// The rejection is syntax-specific, not `#`-phobia, and it does not narrow
/// RFC 9535: a literal `#` member name, a `#`-bearing string literal in a
/// filter, and the root/wildcard/index/slice/descent/compound/count queries the
/// GJSON forms superficially resemble all keep their zero/one/many results. A
/// selected `null` is one match and warns not at all; zero matches normalize to
/// `null` with the ordinary no-match warning, which is the only warning here —
/// so a selected null and a query error stay distinguishable.
#[tokio::test]
async fn quoted_hash_literals_and_rfc_9535_queries_keep_their_results() {
    let cases: Vec<(&str, &str, Value)> = vec![
        ("hashKey", "$['#'].q", json!("hash-key")),
        (
            "hashLiteral",
            "$.tags[?@.tag == '#x'].q",
            json!("hash-literal"),
        ),
        ("root", "$", parse_json(BODY)),
        ("wildcard", "$.items[*].q", json!([1, 2, 3])),
        ("index", "$.items[0].q", json!(1)),
        ("slice", "$.items[1:].q", json!([2, 3])),
        ("descent", "$..sku", json!(["A", "A", "B"])),
        ("compound", "$.items[?@.sku == 'A' && @.q > 1].q", json!(2)),
        ("count", "$.items[?count(@.*) == 2].q", json!([1, 2, 3])),
        ("selectedNull", "$.value", Value::Null),
        ("zeroMatch", "$.items[?@.sku == 'Z'].q", Value::Null),
    ];
    let step = output_step(
        "select",
        cases
            .iter()
            .map(|(name, expression, _)| (*name, selector_output("$response.body", expression)))
            .collect(),
    );
    let (result, _) = execute(workflow(vec![step]), BTreeMap::new()).await;
    outputs(&result);

    let trace = result.trace_steps()[0];
    for (name, expression, expected) in &cases {
        assert_eq!(
            trace.outputs.get(*name),
            Some(expected),
            "{name}: {expression}"
        );
    }

    assert_eq!(
        trace.warnings.len(),
        1,
        "only the zero-match selector warns: {:?}",
        trace.warnings
    );
    assert!(
        trace.warnings[0].starts_with("output \"zeroMatch\": ")
            && trace.warnings[0].contains("selector matched no values"),
        "{}",
        trace.warnings[0]
    );
}

/// A GJSON target and a GJSON Selector Object inside a replacement value each
/// skip exactly their own replacement: the neighbouring replacements still
/// apply, and no other body location — including the sibling a GJSON write
/// would have reached — is mutated. Proven on the captured wire body and on
/// the dry-run resolved body, whose warnings carry the same diagnostics.
#[tokio::test]
async fn gjson_replacement_targets_and_values_leave_the_body_unchanged() {
    let target_form = r#"$.items.#(sku=="A").q"#;
    let value_form = r#"$.items.#(sku=="A")#.q"#;
    let payload = json!({
        "keep": "old",
        "items": [{"sku": "A", "q": "old"}, {"sku": "B", "q": "old"}],
        "value": "old",
        "last": "old"
    });
    let replacements = vec![
        jsonpath_replacement("$.keep", literal(json!("new"))),
        jsonpath_replacement(target_form, literal(json!("new"))),
        jsonpath_replacement(
            "$.value",
            ValueSource::Selector(selector("$inputs.document", value_form)),
        ),
        jsonpath_replacement(
            "$.last",
            literal(json!({"nested": selector_yaml("$inputs.document", value_form)})),
        ),
    ];
    // Only the two sound replacements move; every `q` a GJSON write would have
    // reached keeps its original value.
    let expected = json!({
        "keep": "new",
        "items": [{"sku": "A", "q": "old"}, {"sku": "B", "q": "old"}],
        "value": "old",
        "last": "old"
    });
    let inputs = BTreeMap::from([(
        "document".to_string(),
        json!({"items": [{"sku": "A", "q": 1}, {"sku": "A", "q": 2}]}),
    )]);
    let steps = vec![body_step("replace", payload, replacements)];

    let (result, requests) = execute(workflow(steps.clone()), inputs.clone()).await;
    outputs(&result);
    assert_eq!(requests.len(), 1);
    let sent: Value = match serde_json::from_str(&requests[0].body) {
        Ok(value) => value,
        Err(error) => panic!(
            "captured body is not JSON: {error}; body={}",
            requests[0].body
        ),
    };
    assert_eq!(sent, expected);

    let dry = dry_run(workflow(steps), inputs).await;
    outputs(&dry);
    let request = dry.dry_run_requests()[0];
    assert_eq!(request.body, Some(expected));
    let warnings = &request.warnings;

    // [0] and [3] wrote nothing invalid; [3]'s nested selector is the hard
    // failure, so it reports and is skipped like [2].
    assert!(
        warnings_for(warnings, "requestBody.replacements[0]:").is_empty(),
        "{warnings:?}"
    );

    // A rejected target is one warning: the value already resolved.
    let target_warnings = warnings_for(warnings, "requestBody.replacements[1]:");
    assert_eq!(target_warnings.len(), 1, "{warnings:?}");
    assert!(
        target_warnings[0].contains(SYNTAX_ERROR),
        "{}",
        target_warnings[0]
    );

    // A rejected value is two: the selector diagnostic and the skip notice
    // that says the body was left alone.
    for index in [2, 3] {
        let value_warnings = warnings_for(warnings, &format!("requestBody.replacements[{index}]:"));
        assert_eq!(value_warnings.len(), 2, "[{index}] {warnings:?}");
        assert!(
            value_warnings[0].contains(value_form) && value_warnings[0].contains(SYNTAX_ERROR),
            "[{index}] {}",
            value_warnings[0]
        );
        assert!(
            value_warnings[1]
                .contains("value did not resolve; replacement skipped and body unchanged"),
            "[{index}] {}",
            value_warnings[1]
        );
    }
}

/// Control for the scope limit: the separate GJSON dot-path extension on
/// Arazzo runtime expressions (`$response.body.…`) is not a typed JSONPath
/// surface and is unchanged by the RFC 9535 cutover. The same three forms that
/// a `type: jsonpath` selector rejects still resolve here, with no warning.
#[tokio::test]
async fn runtime_expression_gjson_dot_path_extension_is_unchanged() {
    let step = output_step(
        "legacy",
        vec![
            (
                "count",
                OutputValue::RuntimeExpression("$response.body.items.#".to_string()),
            ),
            (
                "first",
                OutputValue::RuntimeExpression(r#"$response.body.items.#(sku=="A").q"#.to_string()),
            ),
            (
                "all",
                OutputValue::RuntimeExpression(
                    r#"$response.body.items.#(sku=="A")#.q"#.to_string(),
                ),
            ),
        ],
    );
    let (result, _) = execute(workflow(vec![step]), BTreeMap::new()).await;
    outputs(&result);

    let trace = result.trace_steps()[0];
    assert_eq!(trace.outputs.get("count"), Some(&json!(3)));
    assert_eq!(trace.outputs.get("first"), Some(&json!(1)));
    assert_eq!(trace.outputs.get("all"), Some(&json!([1, 2])));
    assert!(trace.warnings.is_empty(), "{:?}", trace.warnings);
}
