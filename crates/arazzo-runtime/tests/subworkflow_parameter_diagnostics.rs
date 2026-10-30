mod common;

use std::collections::BTreeMap;

use arazzo_runtime::{Engine, ExecutionResult, RuntimeErrorKind};
use arazzo_spec::{
    ParamLocation, Parameter, SelectorObject, SelectorType, Step, StepTarget, SuccessCriterion,
    ValueSource, Workflow,
};
use common::{fixture_engine_builder, make_spec_with_base, start_server, success_200, to_yaml};
use serde_json::{json, Value};

fn parameter(name: &str, value: Value) -> Parameter {
    Parameter {
        name: name.to_owned(),
        value: to_yaml(value).into(),
        ..Parameter::default()
    }
}

fn selected(name: &str, pointer: &str) -> Parameter {
    Parameter {
        name: name.to_owned(),
        value: ValueSource::Selector(SelectorObject {
            context: "$inputs.document".to_owned(),
            selector: pointer.to_owned(),
            type_: SelectorType::Name("jsonpointer".to_owned()),
            extensions: BTreeMap::new(),
        }),
        ..Parameter::default()
    }
}

fn http_step(id: &str, path: &str) -> Step {
    Step {
        step_id: id.to_owned(),
        target: Some(StepTarget::OperationPath(format!(
            "{{$sourceDescriptions.test.url}}#/paths/~1{path}/get"
        ))),
        success_criteria: success_200(),
        ..Step::default()
    }
}

fn engine(
    base: &str,
    parameters: Vec<Parameter>,
    dependencies: bool,
    reject: bool,
    dry_run: bool,
) -> Engine {
    let child_outputs = parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.clone(),
                format!("$inputs.{}", parameter.name).into(),
            )
        })
        .collect();
    let parent_outputs = parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.clone(),
                format!("$steps.call-child.outputs.{}", parameter.name).into(),
            )
        })
        .collect();
    let mut call = Step {
        step_id: "call-child".to_owned(),
        target: Some(StepTarget::WorkflowId("child".to_owned())),
        parameters,
        ..Step::default()
    };
    if reject {
        call.success_criteria = vec![SuccessCriterion {
            condition: "$steps.call-child.outputs.clean == 'reject'".to_owned(),
            ..SuccessCriterion::default()
        }];
    }
    let mut steps = Vec::new();
    if dependencies {
        let mut producer = http_step("producer", "seed");
        producer.outputs = BTreeMap::from([("x".to_owned(), "$response.body#/x".into())]);
        let mut bridge = http_step("bridge", "mid");
        let mut input = parameter("x", json!("$steps.producer.outputs.x"));
        input.in_ = Some(ParamLocation::Query);
        bridge.parameters = vec![input];
        bridge.outputs = BTreeMap::from([("x".to_owned(), "$response.body#/x".into())]);
        steps.extend([producer, bridge]);
    }
    steps.push(call);
    let spec = make_spec_with_base(
        base,
        vec![
            Workflow {
                workflow_id: "parent".to_owned(),
                steps,
                outputs: parent_outputs,
                ..Workflow::default()
            },
            Workflow {
                workflow_id: "child".to_owned(),
                steps: vec![http_step("call-child", "ok")],
                outputs: child_outputs,
                ..Workflow::default()
            },
        ],
    );
    fixture_engine_builder(spec)
        .trace(true)
        .dry_run(dry_run)
        .build()
        .unwrap_or_else(|error| panic!("build subworkflow fixture: {error}"))
}

fn outputs(result: &ExecutionResult) -> &BTreeMap<String, Value> {
    result
        .outputs
        .as_ref()
        .unwrap_or_else(|error| panic!("parent success: {error}"))
}

fn inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("number".to_owned(), json!(42)),
        ("boolean".to_owned(), json!(true)),
        ("null".to_owned(), Value::Null),
        ("string".to_owned(), json!("héllo\n\"world\"")),
        ("array".to_owned(), json!([1, false, null, {"x": 2}])),
        ("object".to_owned(), json!({"a": [2, 3], "b": true})),
        ("document".to_owned(), json!({"count": 42})),
        ("snapshot".to_owned(), json!(11)),
    ])
}

#[tokio::test]
async fn child_receives_canonical_exact_braced_nested_and_selector_values() {
    let server = start_server(|_, _, _, _| common::MockHttpResponse::json(200, "{}"));
    let mut parameters = Vec::new();
    let mut expected = BTreeMap::new();
    for (name, value) in inputs()
        .into_iter()
        .filter(|(name, _)| name != "document" && name != "snapshot")
    {
        let exact = format!("exact-{name}");
        parameters.push(parameter(&exact, json!(format!("$inputs.{name}"))));
        expected.insert(exact, value.clone());
        let braced = format!("braced-{name}");
        parameters.push(parameter(&braced, json!(format!("{{$inputs.{name}}}"))));
        let rendered = match value {
            Value::String(text) => text,
            other => other.to_string(),
        };
        expected.insert(braced, json!(rendered));
    }
    for (name, source, received) in [
        (
            "mixed",
            json!("count={$inputs.number}; {$inputs.array}"),
            json!("count=42; [1,false,null,{\"x\":2}]"),
        ),
        ("currency", json!("$USD"), json!("$USD")),
        ("environment", json!("$env.X"), json!("$env.X")),
        (
            "nested",
            json!({"sequence": ["$inputs.number", {"rendered": "{$inputs.boolean}"}]}),
            json!({"sequence": [42, {"rendered": "true"}]}),
        ),
    ] {
        parameters.push(parameter(name, source));
        expected.insert(name.to_owned(), received);
    }
    parameters.push(selected("selected", "/count"));
    expected.insert("selected".to_owned(), json!(42));
    parameters.push(parameter("snapshot", json!("$inputs.number")));
    parameters.push(parameter("snapshot", json!("$inputs.snapshot")));
    expected.insert("snapshot".to_owned(), json!(11));
    let result = engine(&server.base_url, parameters, false, false, false)
        .execute_collect("parent", inputs())
        .await;
    assert_eq!(outputs(&result), &expected);
    assert!(result
        .trace_steps()
        .iter()
        .all(|record| record.warnings.is_empty()));
}

fn warning_parameters(dependencies: bool) -> Vec<Parameter> {
    let mut parameters = vec![
        parameter("missing", json!("$inputs.absent")),
        parameter("clean", json!("ok")),
        parameter(
            "repeated",
            json!("{$inputs.first}/{$inputs.second}/{$inputs.first}"),
        ),
        parameter("malformed", json!("bad {$inputs.number")),
        selected("selected", "/absent"),
        parameter("nested", json!(["$inputs.left", {"leaf": "$inputs.right"}])),
    ];
    if dependencies {
        parameters.push(parameter("dependency", json!("$steps.bridge.outputs.x")));
    }
    parameters
}

fn expected_warnings() -> Vec<String> {
    [
        "parameters[0].value: $inputs.absent: input \"absent\" not found in context",
        "parameters[2].value: $inputs.first: input \"first\" not found in context",
        "parameters[2].value: $inputs.second: input \"second\" not found in context",
        "parameters[2].value: $inputs.first: input \"first\" not found in context",
        "parameters[3].value: bad {$inputs.number: invalid expression string at bytes 4..19: embedded Runtime Expression is missing closing '}'",
        "parameters[4].value: /absent: selector matched no values",
        "parameters[5].value: $inputs.left: input \"left\" not found in context",
        "parameters[5].value: $inputs.right: input \"right\" not found in context",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn assert_warning_owner(result: &ExecutionResult) {
    let traces = result.trace_steps();
    let invoking: Vec<_> = traces
        .iter()
        .filter(|record| record.workflow_id == "parent" && record.step_id == "call-child")
        .collect();
    assert_eq!(invoking.len(), 1);
    assert_eq!(invoking[0].attempt, 1);
    assert_eq!(invoking[0].warnings, expected_warnings());
    let child: Vec<_> = traces
        .iter()
        .filter(|record| record.workflow_id == "child")
        .collect();
    assert_eq!(child.len(), 1);
    assert!(child[0].warnings.is_empty());
    assert!(child[0].seq < invoking[0].seq);
}

#[tokio::test]
async fn full_execution_records_ordered_warnings_on_invoking_step_once() {
    let server = start_server(|_, _, _, _| common::MockHttpResponse::json(200, "{}"));
    let result = engine(
        &server.base_url,
        warning_parameters(false),
        false,
        false,
        false,
    )
    .execute_collect("parent", inputs())
    .await;
    let outputs = outputs(&result);
    assert_eq!(outputs["missing"], Value::Null);
    assert_eq!(outputs["repeated"], json!("null/null/null"));
    assert_eq!(outputs["malformed"], json!("bad {$inputs.number"));
    assert_eq!(outputs["selected"], Value::Null);
    assert_eq!(outputs["nested"], json!([null, {"leaf": null}]));
    assert_warning_owner(&result);
}

#[tokio::test]
async fn run_step_executes_transitive_dependencies_and_retains_warning_ownership() {
    let server = start_server(|_, _, _, _| common::MockHttpResponse::json(200, r#"{"x":73}"#));
    let result = engine(
        &server.base_url,
        warning_parameters(true),
        true,
        false,
        false,
    )
    .execute_step("parent", "call-child", inputs(), false)
    .collect()
    .await;
    assert_eq!(outputs(&result)["dependency"], json!(73));
    let steps: Vec<_> = result
        .trace_steps()
        .iter()
        .map(|record| (record.workflow_id.clone(), record.step_id.clone()))
        .collect();
    assert_eq!(
        steps,
        [
            ("parent", "producer"),
            ("parent", "bridge"),
            ("child", "call-child"),
            ("parent", "call-child")
        ]
        .map(|(workflow, step)| (workflow.to_owned(), step.to_owned()))
    );
    assert_warning_owner(&result);
}

#[tokio::test]
async fn rejected_invocation_keeps_warnings_and_rolls_back_published_outputs() {
    let server = start_server(|_, _, _, _| common::MockHttpResponse::json(200, "{}"));
    let result = engine(
        &server.base_url,
        warning_parameters(false),
        false,
        true,
        false,
    )
    .execute_collect("parent", inputs())
    .await;
    let error = match &result.outputs {
        Ok(outputs) => panic!("criterion must reject, got {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(error.kind, RuntimeErrorKind::SuccessCriteriaFailed);
    assert_warning_owner(&result);
    let traces = result.trace_steps();
    let invoking = traces
        .iter()
        .find(|record| record.workflow_id == "parent")
        .unwrap_or_else(|| panic!("missing invoking trace"));
    assert!(invoking.outputs.is_empty());
}

#[tokio::test]
async fn child_failure_preserves_error_wrapping_and_default_invocation_trace() {
    let server = start_server(|_, _, _, _| common::MockHttpResponse::json(500, "{}"));
    let result = engine(
        &server.base_url,
        warning_parameters(false),
        false,
        false,
        false,
    )
    .execute_collect("parent", inputs())
    .await;
    let error = match &result.outputs {
        Ok(outputs) => panic!("child must fail, got {outputs:?}"),
        Err(error) => error,
    };
    assert_eq!(error.kind, RuntimeErrorKind::SubWorkflowFailed);
    assert!(error.message.contains("sub-workflow child:"));
    let traces = result.trace_steps();
    let invoking = traces
        .iter()
        .find(|record| record.workflow_id == "parent")
        .unwrap_or_else(|| panic!("missing invoking trace"));
    assert!(invoking.warnings.is_empty());
    assert!(invoking.outputs.is_empty());
}

#[tokio::test]
async fn dry_run_child_request_does_not_inherit_invocation_parameter_warnings() {
    let server = start_server(|_, _, _, _| panic!("dry run must not dispatch transport"));
    let result = engine(
        &server.base_url,
        warning_parameters(false),
        false,
        false,
        true,
    )
    .execute_collect("parent", inputs())
    .await;
    assert!(result.outputs.is_ok());
    assert_warning_owner(&result);
    let requests = result.dry_run_requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].warnings.is_empty());
}
